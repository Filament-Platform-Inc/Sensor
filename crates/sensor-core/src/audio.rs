//! Microphone capture, resampled to what whisper expects.
//!
//! Whisper wants 16 kHz mono f32. Hardware rarely offers that directly — this
//! machine's mic is 48 kHz stereo — so we take the device's native format and
//! convert: downmix channels to mono, then decimate to 16 kHz.

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};

pub const TARGET_RATE: u32 = 16_000;

/// An input the user could plausibly choose.
pub struct InputDevice {
    /// What cpal calls it; what gets saved to config.
    pub id: String,
    /// What to show a person.
    pub label: String,
    pub is_default: bool,
}

/// Input devices worth offering, with the default marked.
///
/// The wrong microphone is the most likely cause of "it recorded silence",
/// and it is invisible without a way to see what was chosen. ALSA exposes the
/// same hardware many times over -- raw `hw:`, `plughw:`, `dsnoop:` and so on
/// are plumbing, not choices, so they are hidden; PipeWire or PulseAudio sits
/// in front of them on any modern desktop and handles routing.
pub fn input_devices() -> Result<Vec<InputDevice>> {
    let host = cpal::default_host();
    let default = host.default_input_device().and_then(|d| d.name().ok());
    let mut out = Vec::new();
    for dev in host.input_devices().context("listing input devices")? {
        let Ok(id) = dev.name() else { continue };
        if id.contains(':') {
            continue;
        }
        let label = match id.as_str() {
            "default" => "System default".to_string(),
            "pipewire" => "PipeWire".to_string(),
            "pulse" => "PulseAudio".to_string(),
            other => other.to_string(),
        };
        let is_default = Some(&id) == default.as_ref();
        out.push(InputDevice {
            id,
            label,
            is_default,
        });
    }
    if out.is_empty() {
        anyhow::bail!("no usable microphone found");
    }
    Ok(out)
}

/// Audio accumulated by the capture callback.
///
/// The callback runs on a realtime audio thread owned by the driver, while the
/// rest of the program reads the result from another thread, so the buffer is
/// shared as `Arc<Mutex<..>>`: `Arc` to give both threads ownership of the same
/// allocation, `Mutex` so only one touches it at a time.
type Shared = Arc<Mutex<Vec<f32>>>;

pub struct Recorder {
    stream: cpal::Stream,
    buffer: Shared,
    source_rate: u32,
    channels: u16,
}

impl Recorder {
    /// Open the default input device and begin capturing immediately.
    ///
    /// Capture starts at hotkey *press* rather than release, so the audio is
    /// already buffered by the time the user stops speaking. That is most of
    /// the latency budget.
    pub fn start() -> Result<Self> {
        Self::start_on(None)
    }

    /// Capture from a named device, or the system default when `None`.
    ///
    /// Named rather than indexed: device order is not stable across reboots or
    /// hotplug, so a saved index would silently point at the wrong microphone.
    pub fn start_on(preferred: Option<&str>) -> Result<Self> {
        let host = cpal::default_host();
        let device = match preferred {
            Some(want) => host
                .input_devices()
                .context("listing input devices")?
                .find(|d| d.name().map(|n| n == want).unwrap_or(false))
                .ok_or_else(|| anyhow!("microphone {want:?} not found — it may be unplugged"))?,
            None => host
                .default_input_device()
                .ok_or_else(|| anyhow!("no input device — is a microphone connected?"))?,
        };
        let config = device
            .default_input_config()
            .context("querying default input config")?;

        let source_rate = config.sample_rate().0;
        let channels = config.channels();
        let buffer: Shared = Arc::new(Mutex::new(Vec::new()));

        let sink = Arc::clone(&buffer);
        let err_fn = |e| eprintln!("audio stream error: {e}");

        // The driver hands us whatever sample format the device speaks; convert
        // each to f32 so the rest of the pipeline only deals with one type.
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &config.into(),
                move |data: &[f32], _: &_| extend(&sink, data.iter().copied()),
                err_fn,
                None,
            ),
            cpal::SampleFormat::I16 => device.build_input_stream(
                &config.into(),
                move |data: &[i16], _: &_| {
                    extend(&sink, data.iter().map(|&s| s as f32 / i16::MAX as f32))
                },
                err_fn,
                None,
            ),
            cpal::SampleFormat::U16 => device.build_input_stream(
                &config.into(),
                move |data: &[u16], _: &_| {
                    extend(&sink, data.iter().map(|&s| (s as f32 - 32768.0) / 32768.0))
                },
                err_fn,
                None,
            ),
            other => return Err(anyhow!("unsupported sample format: {other:?}")),
        }
        .context("building input stream")?;

        stream.play().context("starting capture")?;

        Ok(Self {
            stream,
            buffer,
            source_rate,
            channels,
        })
    }

    /// Stop capturing and return 16 kHz mono samples.
    pub fn finish(self) -> Result<Vec<f32>> {
        drop(self.stream); // stops the callback before we read the buffer
        let raw = self
            .buffer
            .lock()
            .map_err(|_| anyhow!("audio buffer poisoned by a panicking callback"))?
            .clone();

        let mono = downmix(&raw, self.channels);
        Ok(resample(&mono, self.source_rate, TARGET_RATE))
    }
}

fn extend(sink: &Shared, samples: impl Iterator<Item = f32>) {
    if let Ok(mut buf) = sink.lock() {
        buf.extend(samples);
    }
}

/// Average interleaved channels down to one.
fn downmix(samples: &[f32], channels: u16) -> Vec<f32> {
    if channels <= 1 {
        return samples.to_vec();
    }
    let n = channels as usize;
    samples
        .chunks_exact(n)
        .map(|frame| frame.iter().sum::<f32>() / n as f32)
        .collect()
}

/// Linear-interpolating resample. Adequate for speech at these rates; if
/// transcription quality ever looks rate-related, revisit with a windowed sinc.
fn resample(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let out_len = (samples.len() as f64 / ratio).floor() as usize;
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let idx = pos.floor() as usize;
            let frac = (pos - idx as f64) as f32;
            let a = samples[idx];
            let b = *samples.get(idx + 1).unwrap_or(&a);
            a + (b - a) * frac
        })
        .collect()
}

/// Write samples as a 16-bit mono WAV. Debug aid for M1a and input for M1b.
pub fn write_wav(path: &std::path::Path, samples: &[f32]) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: TARGET_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec)?;
    for &s in samples {
        w.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
    }
    w.finalize()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downmix_averages_stereo_pairs() {
        assert_eq!(downmix(&[1.0, 0.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
    }

    #[test]
    fn downmix_passes_mono_through() {
        assert_eq!(downmix(&[0.1, 0.2], 1), vec![0.1, 0.2]);
    }

    #[test]
    fn resample_thirds_the_length_from_48k_to_16k() {
        let input: Vec<f32> = (0..48_000).map(|i| i as f32).collect();
        assert_eq!(resample(&input, 48_000, 16_000).len(), 16_000);
    }

    #[test]
    fn resample_is_identity_at_same_rate() {
        let input = vec![0.1, 0.2, 0.3];
        assert_eq!(resample(&input, 16_000, 16_000), input);
    }
}
