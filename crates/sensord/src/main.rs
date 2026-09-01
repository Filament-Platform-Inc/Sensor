//! M1c: the dictation loop.
//!
//! Hold the hotkey, speak, release: the text lands in the focused window.
//! Model and whisper state stay resident between utterances -- that is the
//! whole reason this runs as a daemon rather than a script.

use anyhow::{Context, Result};
use evdev::Key;
use sensor_core::{
    audio::{self, Recorder},
    hotkey::{self, HotkeyEvent},
    output::{Injector, PasteChord},
    stt::Transcriber,
    APP_NAME,
};
use std::{path::PathBuf, time::Instant};

fn main() -> Result<()> {
    let model = model_path()?;
    let key = hotkey_key()?;

    eprintln!("{APP_NAME}: loading model from {}", model.display());
    let mut stt = Transcriber::load(&model)?;
    let mut injector = Injector::new(&format!("{APP_NAME}-virtual-keyboard"))?;
    let events = hotkey::watch(key)?;

    eprintln!("{APP_NAME}: ready — hold {key:?} and speak");

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
                if let Err(e) = handle_utterance(recorder, &mut stt, &mut injector) {
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

    injector.paste(&text, PasteChord::CtrlV)?;

    eprintln!(
        "  {audio_secs:.1}s audio | transcribe {}ms | paste {}ms | {text:?}",
        (transcribed - released).as_millis(),
        transcribed.elapsed().as_millis(),
    );
    Ok(())
}

/// Hotkey: `SENSOR_HOTKEY` as an evdev name (`KEY_F12`, `F12`), else F12.
///
/// F12 over a modifier: holding a real modifier turns every other keypress
/// during dictation into a chord the focused app will act on.
fn hotkey_key() -> Result<Key> {
    let Ok(name) = std::env::var("SENSOR_HOTKEY") else {
        return Ok(Key::KEY_F12);
    };
    key_by_name(&name).with_context(|| format!("SENSOR_HOTKEY: no such key {name:?}"))
}

/// evdev exposes no name lookup, so scan the keycode space for a match.
fn key_by_name(name: &str) -> Option<Key> {
    let name = name.trim().to_uppercase();
    let name = if name.starts_with("KEY_") {
        name
    } else {
        format!("KEY_{name}")
    };
    (0..0x2ffu16)
        .map(Key::new)
        .find(|k| format!("{k:?}") == name)
}

/// Model location: argument, then env var, then the repo-local models dir.
fn model_path() -> Result<PathBuf> {
    if let Some(p) = std::env::args().nth(1) {
        return Ok(p.into());
    }
    if let Ok(p) = std::env::var("SENSOR_MODEL") {
        return Ok(p.into());
    }
    let local = PathBuf::from("models/ggml-tiny.en.bin");
    local
        .exists()
        .then_some(local)
        .context("no model found — pass a path, set SENSOR_MODEL, or place one in models/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_keys_with_or_without_prefix() {
        assert_eq!(key_by_name("F12"), Some(Key::KEY_F12));
        assert_eq!(key_by_name("key_f12"), Some(Key::KEY_F12));
        assert_eq!(key_by_name("SCROLLLOCK"), Some(Key::KEY_SCROLLLOCK));
        assert_eq!(key_by_name("RIGHTCTRL"), Some(Key::KEY_RIGHTCTRL));
    }

    #[test]
    fn rejects_unknown_keys() {
        assert_eq!(key_by_name("NOPE"), None);
    }
}
