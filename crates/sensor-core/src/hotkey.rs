//! Watching every keyboard for the push-to-talk key or chord.
//!
//! A machine usually has more than one key-capable device — this one has the
//! builtin keyboard plus a gaming mouse that presents a keyboard interface for
//! its extra buttons. Watching only the first would make the hotkey silently
//! dead on an external keyboard, so every capable device gets a thread.
//!
//! Chords are tracked per-device: keys held on two different keyboards are not
//! a chord, and treating them as one would fire on unrelated keypresses.

use crate::config::Hotkey;
use anyhow::{anyhow, Result};
use evdev::{Device, InputEventKind, Key};
use std::{
    collections::HashSet,
    sync::mpsc::{channel, Receiver},
    thread,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    Pressed,
    Released,
}

/// Watch all keyboards for `hotkey`, reporting when it becomes held and released.
pub fn watch(hotkey: Hotkey) -> Result<Receiver<HotkeyEvent>> {
    let trigger = hotkey.trigger;
    let devices: Vec<(String, Device)> = evdev::enumerate()
        .filter(|(_, d)| d.supported_keys().is_some_and(|k| k.contains(trigger)))
        .map(|(p, d)| (p.to_string_lossy().into_owned(), d))
        .collect();

    if devices.is_empty() {
        return Err(anyhow!(
            "no readable keyboard exposes {} — is the user in the 'input' group? \
             (group changes need a logout to take effect)",
            hotkey.describe()
        ));
    }

    let (tx, rx) = channel();
    for (path, dev) in devices {
        let tx = tx.clone();
        thread::spawn(move || watch_device(dev, path, hotkey, tx));
    }
    Ok(rx)
}

fn watch_device(
    mut dev: Device,
    path: String,
    hotkey: Hotkey,
    tx: std::sync::mpsc::Sender<HotkeyEvent>,
) {
    // Which keys are currently held on *this* device.
    let mut down: HashSet<Key> = HashSet::new();
    let mut engaged = false;

    loop {
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
            let InputEventKind::Key(key) = ev.kind() else {
                continue;
            };
            // 0 = release, 1 = press, 2 = autorepeat (ignored: holding the
            // key must not re-trigger).
            match ev.value() {
                0 => {
                    down.remove(&key);
                }
                1 => {
                    down.insert(key);
                }
                _ => continue,
            }

            let held = hotkey.is_held(&down);
            if held == engaged {
                continue;
            }
            engaged = held;
            let signal = if held {
                HotkeyEvent::Pressed
            } else {
                HotkeyEvent::Released
            };
            if tx.send(signal).is_err() {
                return; // receiver went away; we are done
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(hotkey: Hotkey, keys: &[Key]) -> bool {
        hotkey.is_held(&keys.iter().copied().collect())
    }

    #[test]
    fn single_key_matches_itself() {
        let h = Hotkey::single(Key::KEY_F12);
        assert!(held(h, &[Key::KEY_F12]));
        assert!(!held(h, &[Key::KEY_A]));
    }

    #[test]
    fn single_key_still_matches_with_other_keys_down() {
        // Typing while the hotkey is held must not stop the recording.
        let h = Hotkey::single(Key::KEY_F12);
        assert!(held(h, &[Key::KEY_F12, Key::KEY_A]));
    }

    #[test]
    fn chord_needs_both_keys() {
        let h = Hotkey::chord(Key::KEY_RIGHTALT, Key::KEY_DOT);
        assert!(!held(h, &[Key::KEY_RIGHTALT]));
        assert!(!held(h, &[Key::KEY_DOT]));
        assert!(held(h, &[Key::KEY_RIGHTALT, Key::KEY_DOT]));
    }

    #[test]
    fn chord_releases_when_either_key_lifts() {
        let h = Hotkey::chord(Key::KEY_RIGHTALT, Key::KEY_DOT);
        // Whichever the user lifts first, the chord is no longer held.
        assert!(!held(h, &[Key::KEY_DOT]));
        assert!(!held(h, &[Key::KEY_RIGHTALT]));
    }
}
