//! Watching every keyboard for the push-to-talk key.
//!
//! A machine usually has more than one key-capable device — this one has the
//! builtin keyboard plus a gaming mouse that presents a keyboard interface for
//! its extra buttons. Watching only the first would make the hotkey silently
//! dead on an external keyboard, so every capable device gets a thread.

use anyhow::{anyhow, Result};
use evdev::{Device, InputEventKind, Key};
use std::{
    sync::mpsc::{channel, Receiver},
    thread,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    Pressed,
    Released,
}

/// Watch all keyboards for `key`, reporting press and release.
pub fn watch(key: Key) -> Result<Receiver<HotkeyEvent>> {
    let devices: Vec<(String, Device)> = evdev::enumerate()
        .filter(|(_, d)| d.supported_keys().is_some_and(|k| k.contains(key)))
        .map(|(p, d)| (p.to_string_lossy().into_owned(), d))
        .collect();

    if devices.is_empty() {
        return Err(anyhow!(
            "no readable keyboard exposes {key:?} — is the user in the 'input' group? \
             (group changes need a logout to take effect)"
        ));
    }

    let (tx, rx) = channel();
    for (path, mut dev) in devices {
        let tx = tx.clone();
        thread::spawn(move || loop {
            let events = match dev.fetch_events() {
                Ok(e) => e,
                Err(e) => {
                    // A device disappearing is normal — USB keyboards get
                    // unplugged. Drop this watcher, leave the others running.
                    eprintln!("input device {path} closed: {e}");
                    return;
                }
            };
            for ev in events {
                if ev.kind() != InputEventKind::Key(key) {
                    continue;
                }
                // 0 = release, 1 = press, 2 = autorepeat (ignored: holding the
                // key must not re-trigger).
                let signal = match ev.value() {
                    0 => Some(HotkeyEvent::Released),
                    1 => Some(HotkeyEvent::Pressed),
                    _ => None,
                };
                if let Some(s) = signal {
                    if tx.send(s).is_err() {
                        return; // receiver went away; we are done
                    }
                }
            }
        });
    }
    Ok(rx)
}
