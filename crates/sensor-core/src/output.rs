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
    /// Both ends of this are races, and getting either wrong pastes the user's
    /// previous clipboard instead of their words:
    ///
    /// * Sending the chord before `wl-copy` has claimed the selection makes the
    ///   app paste whatever was there before. So we wait for the clipboard to
    ///   actually read back as our text rather than assuming the spawn took.
    /// * Restoring the old contents the instant `wl-copy` exits is too early:
    ///   it exits when the app *requests* the data, which is before the app has
    ///   finished reading it. So we settle briefly after that.
    ///
    /// Sleeping a guessed interval instead is why this bug recurs across the
    /// category (OpenWhispr#240).
    pub fn paste(&mut self, text: &str, chord: PasteChord) -> Result<()> {
        let saved = clipboard_get();

        let mut server = spawn_paste_once(text)?;

        // Do not paste until the clipboard actually holds our text, or we race
        // the compositor and the app pastes the previous contents.
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

        // Bounded wait: if the focused app never pastes (wrong chord for a
        // terminal, say) we must not hang, and must still restore.
        let consumed = wait_with_timeout(&mut server, PASTE_TIMEOUT)?;
        if !consumed {
            let _ = server.kill();
            let _ = server.wait();
            // The app never took it, so keep the words reachable rather than
            // restoring over them. Same reasoning as the claim timeout above.
            let _ = clipboard_set(text);
            return Err(anyhow!(
                "the focused window did not accept a paste within {PASTE_TIMEOUT:?}, \
                 so the text was left on your clipboard — press Ctrl+V to insert it \
                 (a terminal may need Ctrl+Shift+V)"
            ));
        }

        // wl-copy exits on the data *request*; the app still has to read it.
        // Restoring immediately can truncate that read.
        thread::sleep(PASTE_SETTLE);

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

/// How long to wait for the focused app to take the clipboard before giving up.
const PASTE_TIMEOUT: Duration = Duration::from_millis(600);

/// How long to wait for `wl-copy` to actually claim the selection.
///
/// Claiming takes ~60ms typically, but contention pushes it well past that,
/// and timing out here loses the transcription entirely -- a far worse outcome
/// than a slow paste. So the budget is generous.
const CLIPBOARD_CLAIM_TIMEOUT: Duration = Duration::from_millis(1500);

/// Grace period between the app requesting the data and having read it.
/// Short enough to stay inside the latency budget, long enough for a local
/// pipe read to complete.
const PASTE_SETTLE: Duration = Duration::from_millis(40);

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
