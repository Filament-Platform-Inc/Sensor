use anyhow::Result;
use std::{path::PathBuf, time::Instant};

fn main() -> Result<()> {
    let model: PathBuf = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "models/ggml-base.en.bin".into())
        .into();
    let wav: PathBuf = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "/tmp/mictest.wav".into())
        .into();

    let t0 = Instant::now();
    let mut stt = sensor_core::stt::Transcriber::load(&model)?;
    println!(
        "model loaded in {:.2}s (once, at startup)",
        t0.elapsed().as_secs_f32()
    );

    let mut reader = hound::WavReader::open(&wav)?;
    let samples: Vec<f32> = reader
        .samples::<i16>()
        .map(|s| s.map(|v| v as f32 / i16::MAX as f32))
        .collect::<Result<_, _>>()?;
    let audio_secs = samples.len() as f32 / 16000.0;

    println!("audio: {audio_secs:.2}s");
    // Run several times: the first includes any lazy init, later ones show the
    // steady-state cost that actually matters for the latency budget.
    let mut times = Vec::new();
    for i in 1..=10 {
        let t1 = Instant::now();
        let text = stt.transcribe(&samples)?;
        let took = t1.elapsed();
        times.push(took.as_millis());
        if i == 1 {
            println!("text: {text:?}");
        }
    }
    times.sort_unstable();
    println!(
        "min {}ms  p50 {}ms  max {}ms",
        times[0],
        times[times.len() / 2],
        times[times.len() - 1]
    );
    Ok(())
}
