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
    ipc,
    output::{Injector, PasteChord},
    stt::Transcriber,
    APP_NAME,
};
use std::{
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::Instant,
};

/// How many past transcriptions the GUI can show. Memory only -- never
/// written to disk, and gone when the daemon stops.
const RECENT_LIMIT: usize = 5;

/// State the socket serves. Shared between the dictation loop and the thread
/// answering the GUI, hence Arc<Mutex<..>>.
#[derive(Default)]
struct Shared {
    recording: bool,
    utterances: u64,
    recent: Vec<String>,
}

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

    let mic_label = cfg.microphone.clone().unwrap_or_else(|| "default".into());
    let shared = Arc::new(Mutex::new(Shared::default()));
    serve_status(Arc::clone(&shared), key, &model, &mic_label);

    eprintln!("{APP_NAME}: ready — hold {key:?} and speak");
    eprintln!("{APP_NAME}: config at {}", config::config_path()?.display());

    let mut recording: Option<Recorder> = None;
    for event in events {
        match event {
            HotkeyEvent::Pressed => {
                if recording.is_none() {
                    match Recorder::start_on(cfg.microphone.as_deref()) {
                        // Capture begins on press, so audio is already buffered
                        // by the time the user stops speaking.
                        Ok(r) => {
                            recording = Some(r);
                            shared.lock().unwrap().recording = true;
                        }
                        Err(e) => eprintln!("  capture failed to start: {e:#}"),
                    }
                }
            }
            HotkeyEvent::Released => {
                let Some(recorder) = recording.take() else {
                    continue;
                };
                shared.lock().unwrap().recording = false;
                match handle_utterance(recorder, &mut stt, &mut injector, chord) {
                    Ok(Some(text)) => {
                        let mut st = shared.lock().unwrap();
                        st.utterances += 1;
                        st.recent.insert(0, text);
                        st.recent.truncate(RECENT_LIMIT);
                    }
                    Ok(None) => {}
                    Err(e) => eprintln!("  {e:#}"),
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
) -> Result<Option<String>> {
    let released = Instant::now();
    let samples = recorder.finish()?;
    let audio_secs = samples.len() as f32 / audio::TARGET_RATE as f32;

    // Whisper emits confident nonsense from near-silence; a tapped hotkey
    // should produce nothing rather than a hallucinated word.
    if audio_secs < 0.3 {
        eprintln!("  too short ({audio_secs:.2}s), ignoring");
        return Ok(None);
    }

    let text = stt.transcribe(&samples)?;
    let transcribed = Instant::now();
    if text.is_empty() {
        eprintln!("  no speech detected");
        return Ok(None);
    }

    injector.paste(&text, chord)?;

    eprintln!(
        "  {audio_secs:.1}s audio | transcribe {}ms | paste {}ms | {text:?}",
        (transcribed - released).as_millis(),
        transcribed.elapsed().as_millis(),
    );
    Ok(Some(text))
}

/// Answers status queries on the unix socket, one connection at a time.
/// Failure to bind is not fatal: dictation must keep working even if the GUI
/// cannot attach.
fn serve_status(shared: Arc<Mutex<Shared>>, key: Key, model: &std::path::Path, mic: &str) {
    let listener = match ipc::listen() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{APP_NAME}: status socket unavailable: {e:#}");
            return;
        }
    };
    let hotkey = format!("{key:?}");
    let model = model
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mic = mic.to_string();

    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            if ipc::read_request(&stream).unwrap_or_default() != "status" {
                continue;
            }
            let status = {
                let st = shared.lock().unwrap();
                ipc::Status {
                    running: true,
                    hotkey: hotkey.clone(),
                    model: model.clone(),
                    microphone: mic.clone(),
                    recording: st.recording,
                    utterances: st.utterances,
                    recent: st.recent.clone(),
                }
            };
            let mut stream = stream;
            let _ = stream.write_all(status.encode().as_bytes());
        }
    });
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
    // Repo-local models take precedence when present, so a checkout runs
    // without touching the user's data dir.
    let local = PathBuf::from("models").join(config::DEFAULT_MODEL);
    if local.exists() {
        return Ok(local);
    }
    let installed = config::default_model_path()?;
    if installed.exists() {
        return Ok(installed);
    }
    anyhow::bail!(
        "no speech model found. Run `sensorctl setup` to download it,\n\
         or pass a path / set SENSOR_MODEL / set `model =` in {}",
        config::config_path()?.display()
    )
}
