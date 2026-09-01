//! M1c: the dictation loop.
//!
//! Hold the hotkey, speak, release: the text lands in the focused window.
//! Model and whisper state stay resident between utterances -- that is the
//! whole reason this runs as a daemon rather than a script.

use anyhow::{Context, Result};
use evdev::Key;
use sensor_core::{
    audio::{self, Recorder},
    config::{self, Config},
    hotkey::{self, HotkeyEvent},
    output::{Injector, PasteChord},
    stt::Transcriber,
    APP_NAME,
};
use std::{path::PathBuf, time::Instant};

const USAGE: &str = "\
usage: sensord [MODEL_PATH]

Hold the hotkey, speak, release: the text appears in the focused window.

  MODEL_PATH        whisper model to load; overrides config and SENSOR_MODEL

environment:
  SENSOR_HOTKEY     evdev key name (e.g. RIGHTALT, F12), overrides config
  SENSOR_MODEL      model path

Settings live in the config file; run `sensorctl keys` to find a key name.";

fn main() -> Result<()> {
    if std::env::args().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        println!("\nconfig: {}", config::config_path()?.display());
        return Ok(());
    }
    let cfg = Config::load()?;
    let key = hotkey(&cfg)?;
    let model = model_path(&cfg)?;
    let chord = if cfg.paste_shift {
        PasteChord::CtrlShiftV
    } else {
        PasteChord::CtrlV
    };

    eprintln!("{APP_NAME}: loading model from {}", model.display());
    let mut stt = Transcriber::load(&model)?;
    let mut injector = Injector::new(&format!("{APP_NAME}-virtual-keyboard"))?;
    let events = hotkey::watch(key)?;

    eprintln!("{APP_NAME}: ready — hold {key:?} and speak");
    eprintln!("{APP_NAME}: config at {}", config::config_path()?.display());

    let mut recording: Option<Recorder> = None;
    for event in events {
        match event {
            HotkeyEvent::Pressed => {
                if recording.is_none() {
                    match Recorder::start() {
                        // Capture begins on press, so audio is already buffered
                        // by the time the user stops speaking.
                        Ok(r) => recording = Some(r),
                        Err(e) => eprintln!("  capture failed to start: {e:#}"),
                    }
                }
            }
            HotkeyEvent::Released => {
                let Some(recorder) = recording.take() else {
                    continue;
                };
                if let Err(e) = handle_utterance(recorder, &mut stt, &mut injector, chord) {
                    eprintln!("  {e:#}");
                }
            }
        }
    }
    Ok(())
}

fn handle_utterance(
    recorder: Recorder,
    stt: &mut Transcriber,
    injector: &mut Injector,
    chord: PasteChord,
) -> Result<()> {
    let released = Instant::now();
    let samples = recorder.finish()?;
    let audio_secs = samples.len() as f32 / audio::TARGET_RATE as f32;

    // Whisper emits confident nonsense from near-silence; a tapped hotkey
    // should produce nothing rather than a hallucinated word.
    if audio_secs < 0.3 {
        eprintln!("  too short ({audio_secs:.2}s), ignoring");
        return Ok(());
    }

    let text = stt.transcribe(&samples)?;
    let transcribed = Instant::now();
    if text.is_empty() {
        eprintln!("  no speech detected");
        return Ok(());
    }

    injector.paste(&text, chord)?;

    eprintln!(
        "  {audio_secs:.1}s audio | transcribe {}ms | paste {}ms | {text:?}",
        (transcribed - released).as_millis(),
        transcribed.elapsed().as_millis(),
    );
    Ok(())
}

/// Hotkey: `SENSOR_HOTKEY` overrides the config file, which defaults to
/// right Alt. The env var exists so a key can be tried without editing config.
fn hotkey(cfg: &Config) -> Result<Key> {
    let Some(name) = std::env::var_os("SENSOR_HOTKEY") else {
        return Ok(cfg.hotkey);
    };
    let name = name.to_string_lossy().into_owned();
    config::key_by_name(&name).with_context(|| format!("SENSOR_HOTKEY: no such key {name:?}"))
}

/// Model location: argument, then env var, then config, then the repo-local dir.
fn model_path(cfg: &Config) -> Result<PathBuf> {
    if let Some(p) = std::env::args().nth(1) {
        return Ok(p.into());
    }
    if let Some(p) = std::env::var_os("SENSOR_MODEL") {
        return Ok(p.into());
    }
    if let Some(p) = &cfg.model {
        return Ok(p.clone());
    }
    let local = PathBuf::from("models").join(config::DEFAULT_MODEL);
    local
        .exists()
        .then_some(local)
        .context("no model found — pass a path, set SENSOR_MODEL, or set `model =` in the config")
}
