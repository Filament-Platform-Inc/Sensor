//! The CLI: inspect and change settings without hand-editing config.
//!
//! `keys` exists because no hotkey is safe on every keyboard. Laptops fold the
//! function row into an Fn layer, and 2024+ models replaced right Ctrl with a
//! Copilot key, so the reliable way to pick a hotkey is to watch what the
//! hardware actually reports.

use anyhow::{Context, Result};
use evdev::{Device, InputEventKind, Key};
use sensor_core::{
    config::{self, Config},
    APP_NAME,
};
use std::{
    io::Write,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const USAGE: &str = "\
usage: sensorctl <command>

  keys              press a key to see its name, then save it as the hotkey
  config            show the current settings and where they live
  help              this text";

fn main() -> Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("keys") => keys(),
        Some("config") => show_config(),
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

fn show_config() -> Result<()> {
    let path = config::config_path()?;
    let cfg = Config::load()?;
    println!("config file: {}", path.display());
    if !path.exists() {
        println!("  (does not exist yet; these are the defaults)");
    }
    println!();
    println!("hotkey      = {:?}", cfg.hotkey);
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

    let name = format!("{key:?}");
    let short = name.strip_prefix("KEY_").unwrap_or(&name);
    println!("You pressed: {name}  (code {})", key.code());

    if seen.len() > 1 {
        let mods: Vec<_> = seen
            .iter()
            .filter(|k| is_modifier(**k))
            .map(|k| format!("{k:?}"))
            .collect();
        if !mods.is_empty() {
            println!(
                "note: held alongside {}. Only the single key is used;\n\
                 chords are not supported yet.",
                mods.join(" + ")
            );
        }
    }

    print!("\nSave `hotkey = {short}` to your config? [y/N] ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    if !answer.trim().eq_ignore_ascii_case("y") {
        println!("Not saved.");
        return Ok(());
    }

    let path = save_hotkey(short)?;
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
