//! Getting text into the focused window.
//!
//! uinput emits scancodes, which the compositor maps through whichever keyboard
//! layout is currently active. Typing per-key therefore produces garbage for
//! anyone not on the layout we assumed, and cannot express characters absent
//! from the active layout at all. So the text travels via the clipboard and we
//! inject only the paste chord, whose scancodes are layout-stable.

use anyhow::{anyhow, Context, Result};
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
    ///
    /// Restoring the clipboard is a race: put the old contents back before the
    /// target app has read ours and the user gets their previous clipboard
    /// pasted instead. Sleeping a guessed interval is the usual fix and is why
    /// this bug recurs across the category (OpenWhispr#240). Instead we serve
    /// the text with `wl-copy --paste-once`, which exits after exactly one
    /// paste request -- so process exit *is* the signal that the app has taken
    /// the text, and there is nothing to guess.
    pub fn paste(&mut self, text: &str, chord: PasteChord) -> Result<()> {
        let saved = clipboard_get();

        let mut server = spawn_paste_once(text)?;
        self.send_chord(chord)?;

        // Bounded wait: if the focused app never pastes (wrong chord for a
        // terminal, say) we must not hang, and must still restore.
        let consumed = wait_with_timeout(&mut server, PASTE_TIMEOUT)?;
        if !consumed {
            let _ = server.kill();
            let _ = server.wait();
        }

        if let Some(prev) = saved {
            clipboard_set(&prev)?;
        }

        if consumed {
            Ok(())
        } else {
            Err(anyhow!(
                "the focused window did not accept a paste within {PASTE_TIMEOUT:?} \
                 (a terminal may need Ctrl+Shift+V)"
            ))
        }
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

/// How long to wait for the focused app to take the clipboard before giving up.
const PASTE_TIMEOUT: Duration = Duration::from_millis(600);

/// Serve `text` on the clipboard for exactly one paste, then exit.
fn spawn_paste_once(text: &str) -> Result<std::process::Child> {
    let mut child = Command::new("wl-copy")
        .args(["--paste-once", "--foreground"])
        .stdin(Stdio::piped())
        .spawn()
        .context("running wl-copy — is wl-clipboard installed?")?;
    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(text.as_bytes())?;
    Ok(child)
}

/// Poll for the child to exit. Returns whether it exited within `limit`.
fn wait_with_timeout(child: &mut std::process::Child, limit: Duration) -> Result<bool> {
    let deadline = std::time::Instant::now() + limit;
    while std::time::Instant::now() < deadline {
        if child.try_wait()?.is_some() {
            return Ok(true);
        }
        thread::sleep(Duration::from_millis(5));
    }
    Ok(false)
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
