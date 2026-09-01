//! A unix socket the daemon answers on, so the GUI can show what is actually
//! happening rather than inferring it.
//!
//! The wire format is one line of text per message, for the same reason the
//! config file is not TOML: a hand-readable protocol needs no serialisation
//! dependency, and `socat` or `nc` is enough to audit what this exposes.
//!
//! Recent transcriptions are held in memory by the daemon and served here.
//! They are deliberately never written to disk -- a file of everything the
//! user has dictated would be a worse thing to leak than the keystroke access
//! itself.

use crate::APP_NAME;
use anyhow::{Context, Result};
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    time::Duration,
};

/// `$XDG_RUNTIME_DIR/<app>.sock`, which the kernel clears at logout.
pub fn socket_path() -> Result<PathBuf> {
    let base = match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => PathBuf::from("/tmp"),
    };
    Ok(base.join(format!("{APP_NAME}.sock")))
}

/// What the daemon reports about itself.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    pub running: bool,
    pub hotkey: String,
    pub model: String,
    pub microphone: String,
    /// Whether the hotkey is held and audio is being captured right now.
    pub recording: bool,
    pub utterances: u64,
    /// Most recent first. Memory only; lost when the daemon stops.
    pub recent: Vec<String>,
}

impl Status {
    /// Renders as `key: value` lines, with `recent` repeated per entry.
    pub fn encode(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("running: {}\n", self.running));
        s.push_str(&format!("hotkey: {}\n", self.hotkey));
        s.push_str(&format!("model: {}\n", self.model));
        s.push_str(&format!("microphone: {}\n", self.microphone));
        s.push_str(&format!("recording: {}\n", self.recording));
        s.push_str(&format!("utterances: {}\n", self.utterances));
        for line in &self.recent {
            // Newlines would break the line-per-field framing.
            s.push_str(&format!("recent: {}\n", line.replace('\n', " ")));
        }
        s
    }

    pub fn decode(text: &str) -> Self {
        let mut st = Status::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once(": ") else {
                continue;
            };
            match k {
                "running" => st.running = v == "true",
                "hotkey" => st.hotkey = v.to_string(),
                "model" => st.model = v.to_string(),
                "microphone" => st.microphone = v.to_string(),
                "recording" => st.recording = v == "true",
                "utterances" => st.utterances = v.parse().unwrap_or(0),
                "recent" => st.recent.push(v.to_string()),
                _ => {}
            }
        }
        st
    }
}

/// Asks the running daemon for its status. `Ok(None)` means nothing is
/// listening, which is the ordinary "daemon stopped" case rather than an error.
pub fn query_status() -> Result<Option<Status>> {
    let path = socket_path()?;
    let mut stream = match UnixStream::connect(&path) {
        Ok(s) => s,
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(None)
        }
        Err(e) => return Err(e).context("connecting to the daemon socket"),
    };
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(b"status\n").context("sending request")?;
    stream.flush()?;

    let mut reply = String::new();
    BufReader::new(stream)
        .read_to_string(&mut reply)
        .context("reading the daemon's reply")?;
    Ok(Some(Status::decode(&reply)))
}

/// Binds the socket, removing a stale one left by a crash.
pub fn listen() -> Result<UnixListener> {
    let path = socket_path()?;
    if path.exists() {
        // Only remove it if nothing is actually listening, so two daemons
        // cannot silently steal the socket from one another.
        if UnixStream::connect(&path).is_ok() {
            anyhow::bail!("another {APP_NAME} daemon is already running");
        }
        let _ = std::fs::remove_file(&path);
    }
    let listener =
        UnixListener::bind(&path).with_context(|| format!("binding {}", path.display()))?;

    // The socket exposes what was dictated, so it is owner-only.
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .context("restricting socket permissions")?;
    Ok(listener)
}

/// Reads one request line from a connected client.
pub fn read_request(stream: &UnixStream) -> Result<String> {
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    Ok(line.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_survives_a_round_trip() {
        let st = Status {
            running: true,
            hotkey: "KEY_RIGHTALT".into(),
            model: "tiny.en".into(),
            microphone: "System default".into(),
            recording: false,
            utterances: 7,
            recent: vec!["hello there".into(), "second one".into()],
        };
        assert_eq!(Status::decode(&st.encode()), st);
    }

    #[test]
    fn newlines_in_transcripts_do_not_break_framing() {
        let st = Status {
            recent: vec!["two\nlines".into()],
            ..Default::default()
        };
        let decoded = Status::decode(&st.encode());
        assert_eq!(decoded.recent, vec!["two lines".to_string()]);
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let st = Status::decode("running: true\nfuture_field: 3\nutterances: 2\n");
        assert!(st.running);
        assert_eq!(st.utterances, 2);
    }
}
