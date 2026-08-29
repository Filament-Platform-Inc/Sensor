//! M1a: hold the hotkey, speak, get a WAV.
//!
//! Not yet the daemon — no model, no injection. This exists to prove capture
//! and hotkey handling work together before whisper enters the picture.

use anyhow::Result;
use evdev::Key;
use sensor_core::{
    audio::{self, Recorder},
    hotkey::{self, HotkeyEvent},
};
use std::{path::PathBuf, time::Instant};

fn main() -> Result<()> {
    let out: PathBuf = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/sensor-capture.wav".into())
        .into();

    let key = Key::KEY_RIGHTCTRL;
    let events = hotkey::watch(key)?;
    println!("Hold {key:?} and speak. Ctrl+C to quit.");

    let mut recorder: Option<(Recorder, Instant)> = None;
    for event in events {
        match event {
            HotkeyEvent::Pressed if recorder.is_none() => match Recorder::start() {
                Ok(r) => {
                    println!("  recording...");
                    recorder = Some((r, Instant::now()));
                }
                Err(e) => eprintln!("  could not start capture: {e:#}"),
            },
            HotkeyEvent::Released => {
                let Some((r, started)) = recorder.take() else {
                    continue;
                };
                let held = started.elapsed();
                let samples = r.finish()?;
                let secs = samples.len() as f32 / audio::TARGET_RATE as f32;
                audio::write_wav(&out, &samples)?;
                println!(
                    "  held {:.2}s, captured {:.2}s of audio -> {}",
                    held.as_secs_f32(),
                    secs,
                    out.display()
                );
            }
            _ => {}
        }
    }
    Ok(())
}
