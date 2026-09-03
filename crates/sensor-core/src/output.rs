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
    /// The clipboard is served for the whole attempt rather than for a single
    /// paste. `--paste-once` looks right -- it stops the transcription lingering
    /// -- but the check that it had been claimed was itself a `wl-paste`, and
    /// that read *was* the one paste. The server exited before the focused app
    /// ever saw the chord, and the wait below then found an already-dead process
    /// and called it a success, so every failure looked like a win. Verifying a
    /// one-shot serve by reading it always spends it.
    ///
    /// Restoring is still a race at the far end: `wl-copy` learns the data was
    /// requested, not that the app finished reading it, so we settle before
    /// putting the old contents back.
    pub fn paste(&mut self, text: &str, chord: PasteChord) -> Result<()> {
        let saved = clipboard_get();

        let mut server = spawn_clipboard_server(text)?;

        // Do not paste until the clipboard actually holds our text, or we race
        // the compositor and the app pastes the previous contents. Reading it
        // back is safe now that the server is not one-shot.
        if !wait_for_clipboard(text, CLIPBOARD_CLAIM_TIMEOUT) {
            let _ = server.kill();
            let _ = server.wait();
            // Leave the transcription on the clipboard rather than restoring:
            // the user just spoke those words, and losing them outright is
            // worse than a clipboard they can paste themselves.
            let _ = clipboard_set(text);
            return Err(anyhow!(
                "could not paste within {CLIPBOARD_CLAIM_TIMEOUT:?}, so the text was \
                 left on your clipboard — press Ctrl+V to insert it"
            ));
        }

        self.send_chord(chord)?;

        // The app reads the selection asynchronously, and nothing tells us when
        // it is done. Hold the text there long enough for that read to land
        // before restoring over it.
        thread::sleep(PASTE_SETTLE);

        let _ = server.kill();
        let _ = server.wait();

        match saved {
            Some(prev) => clipboard_set(&prev)?,
            // There was nothing to restore, and killing the server leaves the
            // selection unowned. Put the words back so they are still reachable.
            None => clipboard_set(text)?,
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

/// How long to wait for `wl-copy` to actually claim the selection.
///
/// Claiming takes ~60ms typically, but contention pushes it well past that,
/// and timing out here loses the transcription entirely -- a far worse outcome
/// than a slow paste. So the budget is generous.
const CLIPBOARD_CLAIM_TIMEOUT: Duration = Duration::from_millis(1500);

/// How long the text stays on the clipboard after the chord is sent.
///
/// This is the whole window the focused app has to notice the keystroke and
/// read the selection, so it is far longer than the old 40ms grace period,
/// which only had to cover a pipe read. Terminals and Electron apps are well
/// past 40ms. It costs nothing perceptible: the text is already on screen.
const PASTE_SETTLE: Duration = Duration::from_millis(400);

/// Polls the clipboard until it reads back as `text`.
///
/// Positive confirmation, rather than assuming the spawn succeeded: reading it
/// back is the only way to know the compositor has handed the selection over.
///
/// The interval backs off rather than polling flat out. Each check spawns a
/// `wl-paste`, and hammering the compositor's clipboard dozens of times a
/// second is both wasteful and liable to trip a desktop permission prompt.
fn wait_for_clipboard(text: &str, limit: Duration) -> bool {
    // wl-paste --no-newline strips one trailing newline, so compare trimmed
    // ends rather than requiring an exact match we might never see.
    let want = text.trim_end();
    let deadline = std::time::Instant::now() + limit;
    let mut interval = Duration::from_millis(10);
    while std::time::Instant::now() < deadline {
        if clipboard_get().as_deref().map(str::trim_end) == Some(want) {
            return true;
        }
        thread::sleep(interval);
        interval = (interval * 2).min(Duration::from_millis(60));
    }
    false
}

/// Serve `text` on the clipboard until we kill the server.
fn spawn_clipboard_server(text: &str) -> Result<std::process::Child> {
    let mut child = Command::new("wl-copy")
        .args(["--foreground"])
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

#[cfg(test)]
mod tests {
    /// `--paste-once` serves the selection exactly once, and the claim check is
    /// itself a read -- so it spent the one paste and the focused app got
    /// nothing, while the dead server read as success. It logged no errors for
    /// weeks. Nothing else in the file would fail if it came back, so assert on
    /// the source directly.
    #[test]
    fn clipboard_is_not_served_one_shot() {
        let src = include_str!("output.rs");
        // Built at runtime so this test's own text is not a match.
        let flag = format!("--{}-once", "paste");
        let uses = src
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                l.contains(&flag) && !t.starts_with("///") && !t.starts_with("//")
            })
            .count();
        assert_eq!(uses, 0, "one-shot serve is consumed by our own claim check");
    }
}
