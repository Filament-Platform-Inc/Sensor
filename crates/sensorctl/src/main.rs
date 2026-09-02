//! The CLI: inspect and change settings without hand-editing config.
//!
//! `keys` exists because no hotkey is safe on every keyboard. Laptops fold the
//! function row into an Fn layer, and 2024+ models replaced right Ctrl with a
//! Copilot key, so the reliable way to pick a hotkey is to watch what the
//! hardware actually reports.

use anyhow::{Context, Result};
use evdev::{Device, InputEventKind, Key};
use sensor_core::{
    config::{self, Config, Hotkey},
    APP_NAME,
};
use std::{
    io::Write,
    process::Command,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const USAGE: &str = "\
usage: sensorctl <command>

  setup             download the speech model and enable the daemon
  keys              press a key to see its name, then save it as the hotkey
  config            show the current settings and where they live
  doctor            check permissions, model, and daemon state
  help              this text";

/// Upstream ggml weights. Pinned by name; whisper.cpp keeps these stable.
const MODEL_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin";

fn main() -> Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("setup") => setup(),
        Some("keys") => keys(),
        Some("config") => show_config(),
        Some("doctor") => doctor(),
        Some("help") | Some("--help") | Some("-h") | None => {
            println!("{USAGE}");
            Ok(())
        }
        Some(other) => {
            eprintln!("{APP_NAME}ctl: unknown command {other:?}\n\n{USAGE}");
            std::process::exit(2);
        }
    }
}

/// First-run: fetch the model, then enable the user service.
fn setup() -> Result<()> {
    let model = config::default_model_path()?;
    if model.exists() {
        println!("model already present at {}", model.display());
    } else {
        let dir = model.parent().context("model path has no parent")?;
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;

        println!("Downloading the speech model (~75MB, once)...");
        // Download beside the target and rename, so an interrupted fetch
        // never leaves a truncated file that looks valid.
        let tmp = model.with_extension("partial");
        let status = Command::new("curl")
            .args(["-fL", "--progress-bar", "-o"])
            .arg(&tmp)
            .arg(MODEL_URL)
            .status()
            .context("running curl — is it installed?")?;
        if !status.success() {
            let _ = std::fs::remove_file(&tmp);
            anyhow::bail!("model download failed");
        }
        std::fs::rename(&tmp, &model)
            .with_context(|| format!("moving model into {}", model.display()))?;
        println!("Model saved to {}", model.display());
    }

    match enable_service() {
        Ok(true) => println!("\nDaemon enabled. It will start automatically at login."),
        Ok(false) => println!("\nCould not reach systemd --user; start manually with:\n  sensord"),
        Err(e) => eprintln!("\nwarning: enabling the service failed: {e:#}"),
    }

    if !in_groups() {
        println!(
            "\nOne step remains: log out and back in.\n\
             \n\
             Your group list is fixed when a session starts, so this session cannot\n\
             see the 'input' and 'uinput' groups the installer added you to. After\n\
             logging back in, hold the hotkey and speak."
        );
    } else {
        println!("\nReady. Hold the hotkey and speak.");
    }
    Ok(())
}

fn enable_service() -> Result<bool> {
    let out = Command::new("systemctl")
        .args(["--user", "enable", "--now", "sensord.service"])
        .output();
    Ok(match out {
        Ok(o) => o.status.success(),
        Err(_) => false,
    })
}

/// Whether this *session* has the groups, which is not the same as whether
/// the user is a member -- membership only reaches a session at next login.
fn in_groups() -> bool {
    let Ok(out) = Command::new("id").arg("-nG").output() else {
        return false;
    };
    let groups = String::from_utf8_lossy(&out.stdout);
    let have: Vec<_> = groups.split_whitespace().collect();
    have.contains(&"input") && have.contains(&"uinput")
}

/// Reports what is and is not working, so a broken install is diagnosable
/// without reading source.
fn doctor() -> Result<()> {
    let mut problems = 0;

    let session_groups = in_groups();
    report(
        session_groups,
        "session has 'input' and 'uinput' groups",
        "log out and back in to pick up group membership",
        &mut problems,
    );

    let uinput = std::path::Path::new("/dev/uinput").exists();
    report(
        uinput,
        "/dev/uinput exists",
        "run: sudo modprobe uinput",
        &mut problems,
    );

    let readable =
        evdev::enumerate().any(|(_, d)| d.supported_keys().is_some_and(|k| k.contains(Key::KEY_A)));
    report(
        readable,
        "can read a keyboard",
        "check /dev/input permissions and 'input' group membership",
        &mut problems,
    );

    let model = config::default_model_path()?;
    let has_model = model.exists()
        || std::path::Path::new("models")
            .join(config::DEFAULT_MODEL)
            .exists();
    report(
        has_model,
        "speech model present",
        "run: sensorctl setup",
        &mut problems,
    );

    let wl = Command::new("sh")
        .args(["-c", "command -v wl-copy"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    report(
        wl,
        "wl-copy available",
        "install it: sudo apt install wl-clipboard",
        &mut problems,
    );

    println!();
    if problems == 0 {
        println!("All checks passed.");
    } else {
        println!("{problems} problem(s) found.");
        std::process::exit(1);
    }
    Ok(())
}

fn report(ok: bool, label: &str, fix: &str, problems: &mut u32) {
    if ok {
        println!("  ok    {label}");
    } else {
        println!("  FAIL  {label}\n        -> {fix}");
        *problems += 1;
    }
}

fn show_config() -> Result<()> {
    let path = config::config_path()?;
    let cfg = Config::load()?;
    println!("config file: {}", path.display());
    if !path.exists() {
        println!("  (does not exist yet; these are the defaults)");
    }
    println!();
    println!("hotkey      = {}", cfg.hotkey.describe());
    println!(
        "model       = {}",
        cfg.model
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| format!("(default: {})", config::DEFAULT_MODEL))
    );
    println!("paste_shift = {}", cfg.paste_shift);
    Ok(())
}

/// Watches every key-capable device and reports the first key pressed.
fn keys() -> Result<()> {
    let (tx, rx) = mpsc::channel();
    let mut watched = 0;

    for (_, dev) in evdev::enumerate() {
        // A device that reports KEY_A is a keyboard; mice expose key events
        // too, and on this hardware one of them also exposes a full keymap.
        if !dev.supported_keys().is_some_and(|k| k.contains(Key::KEY_A)) {
            continue;
        }
        watched += 1;
        let tx = tx.clone();
        thread::spawn(move || forward_presses(dev, tx));
    }
    drop(tx);

    if watched == 0 {
        anyhow::bail!(
            "no readable keyboards — you are probably not in the 'input' group yet.\n\
             If you just installed, log out and back in."
        );
    }

    println!("Watching {watched} keyboard(s). Press the key you want to dictate with.");
    println!("(Ctrl+C to cancel.)\n");

    // Modifiers arrive before the key they modify, so take a moment's worth of
    // presses and report the most specific one rather than the first.
    let first = rx.recv().context("no key was pressed")?;
    let deadline = Instant::now() + Duration::from_millis(300);
    let mut seen = vec![first];
    while let Ok(k) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        seen.push(k);
    }
    // Prefer a non-modifier when one arrives, since holding Shift to reach a
    // key should not bind Shift. A modifier pressed alone is a valid choice,
    // so fall back to what was actually pressed first.
    let key = seen
        .iter()
        .copied()
        .find(|k| !is_modifier(*k))
        .unwrap_or(first);

    // A chord is a modifier plus the key that followed it, so holding Right
    // Alt and then pressing '.' binds the pair rather than just the modifier.
    let modifier = seen.iter().copied().find(|k| is_modifier(*k));
    let hotkey = match (modifier, key) {
        (Some(m), t) if m != t => Hotkey::chord(m, t),
        (_, t) => Hotkey::single(t),
    };

    println!("You pressed: {}", hotkey.describe());
    if hotkey.modifier.is_none() {
        println!(
            "note: a single key can collide with normal typing. A chord such as\n\
             Right Alt + . is usually safer — hold the modifier, then the key."
        );
    }

    let short = hotkey.encode();
    print!("\nSave `hotkey = {short}` to your config? [y/N] ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    if !answer.trim().eq_ignore_ascii_case("y") {
        println!("Not saved.");
        return Ok(());
    }

    let path = save_hotkey(short.as_str())?;
    println!(
        "Saved to {}. Restart {APP_NAME}d to pick it up.",
        path.display()
    );
    Ok(())
}

/// Rewrites the `hotkey` line in place, preserving everything else the user
/// has written -- including their comments.
fn save_hotkey(short: &str) -> Result<std::path::PathBuf> {
    let path = config::config_path()?;
    let dir = path.parent().context("config path has no parent")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;

    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut out = String::new();
    let mut replaced = false;
    for line in existing.lines() {
        let is_hotkey = line
            .split('#')
            .next()
            .unwrap_or("")
            .split_once('=')
            .is_some_and(|(k, _)| k.trim() == "hotkey");
        if is_hotkey {
            if !replaced {
                out.push_str(&format!("hotkey = {short}\n"));
                replaced = true;
            }
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !replaced {
        out.push_str(&format!("hotkey = {short}\n"));
    }

    std::fs::write(&path, out).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

fn is_modifier(k: Key) -> bool {
    matches!(
        k,
        Key::KEY_LEFTSHIFT
            | Key::KEY_RIGHTSHIFT
            | Key::KEY_LEFTCTRL
            | Key::KEY_RIGHTCTRL
            | Key::KEY_LEFTALT
            | Key::KEY_RIGHTALT
            | Key::KEY_LEFTMETA
            | Key::KEY_RIGHTMETA
    )
}

fn forward_presses(mut dev: Device, tx: mpsc::Sender<Key>) {
    loop {
        let Ok(events) = dev.fetch_events() else {
            return;
        };
        for ev in events {
            // value 1 is press; 0 is release and 2 is autorepeat.
            if let (InputEventKind::Key(key), 1) = (ev.kind(), ev.value()) {
                if tx.send(key).is_err() {
                    return;
                }
            }
        }
    }
}
