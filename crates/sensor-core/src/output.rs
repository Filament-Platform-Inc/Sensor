//! Getting text into the focused window.
//!
//! uinput emits scancodes, which the compositor maps through whichever keyboard
//! layout is currently active. Typing per-key therefore produces garbage for
//! anyone not on the layout we assumed, and cannot express characters absent
//! from the active layout at all. So the text travels via the clipboard and we
//! inject only the paste chord, whose scancodes are layout-stable.

use anyhow::{Context, Result};
use evdev::{
    uinput::VirtualDevice, uinput::VirtualDeviceBuilder, AttributeSet, EventType, InputEvent, Key,
};
use std::{
    io::Write,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

/// Which paste chord to send. Terminals bind Ctrl+V to SIGINT-adjacent things
/// and use Ctrl+Shift+V for paste instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteChord {
    CtrlV,
    CtrlShiftV,
}

pub struct Injector {
    device: VirtualDevice,
}

impl Injector {
    pub fn new(device_name: &str) -> Result<Self> {
        let mut keys = AttributeSet::<Key>::new();
        for k in [Key::KEY_LEFTCTRL, Key::KEY_LEFTSHIFT, Key::KEY_V] {
            keys.insert(k);
        }
        let device = VirtualDeviceBuilder::new()
            .context("opening /dev/uinput — is the user in the 'uinput' group?")?
            .name(device_name)
            .with_keys(&keys)?
            .build()?;
        Ok(Self { device })
    }

    /// Put `text` in the focused window: stash the user's clipboard, paste, restore.
    pub fn paste(&mut self, text: &str, chord: PasteChord) -> Result<()> {
        let saved = clipboard_get();
        clipboard_set(text)?;
        self.send_chord(chord)?;

        // The restore race: put the old clipboard back too soon and the target
        // app reads it instead of our text. See OpenWhispr#240. A fixed sleep is
        // a poor fix; tracked as a real problem to solve before v1 ships.
        thread::sleep(Duration::from_millis(400));
        if let Some(prev) = saved {
            clipboard_set(&prev)?;
        }
        Ok(())
    }

    fn send_chord(&mut self, chord: PasteChord) -> Result<()> {
        let shift = chord == PasteChord::CtrlShiftV;
        let press = |k: Key, v: i32| InputEvent::new(EventType::KEY, k.code(), v);

        let mut evs = vec![press(Key::KEY_LEFTCTRL, 1)];
        if shift {
            evs.push(press(Key::KEY_LEFTSHIFT, 1));
        }
        evs.push(press(Key::KEY_V, 1));
        evs.push(press(Key::KEY_V, 0));
        if shift {
            evs.push(press(Key::KEY_LEFTSHIFT, 0));
        }
        evs.push(press(Key::KEY_LEFTCTRL, 0));

        self.device.emit(&evs).context("emitting paste chord")?;
        Ok(())
    }
}

fn clipboard_get() -> Option<String> {
    let out = Command::new("wl-paste").arg("--no-newline").output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn clipboard_set(text: &str) -> Result<()> {
    let mut child = Command::new("wl-copy")
        .stdin(Stdio::piped())
        .spawn()
        .context("running wl-copy — is wl-clipboard installed?")?;
    child
        .stdin
        .as_mut()
        .expect("stdin was piped")
        .write_all(text.as_bytes())?;
    child.wait()?;
    Ok(())
}
