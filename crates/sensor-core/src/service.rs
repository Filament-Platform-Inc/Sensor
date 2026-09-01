//! Starting and stopping the daemon, so nobody has to learn systemctl.
//!
//! Wraps `systemctl --user` rather than reimplementing it: systemd already
//! handles restart-on-failure and start-at-login, and a user who inspects
//! what this does should find the ordinary commands they expect.

use anyhow::{Context, Result};
use std::process::Command;

/// What the service is doing, independent of whether the daemon answers IPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Running now, and set to start at login.
    Running,
    /// Not running, but will start at next login.
    Stopped,
    /// Will not start on its own.
    Disabled,
    /// systemd is not managing it -- a manually launched binary, or no
    /// systemd user session at all.
    Unmanaged,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            State::Running => "Running",
            State::Stopped => "Stopped",
            State::Disabled => "Disabled",
            State::Unmanaged => "Not installed as a service",
        }
    }

    /// The small-grey-text explanation shown under the label.
    pub fn description(self) -> &'static str {
        match self {
            State::Running => "Listening for your hotkey. Starts automatically when you log in.",
            State::Stopped => "Not listening. It will start again next time you log in.",
            State::Disabled => "Turned off, and will stay off until you start it again.",
            State::Unmanaged => {
                "No systemd service found. Run `sensorctl setup`, or start sensord yourself."
            }
        }
    }
}

const UNIT: &str = "sensord.service";

fn systemctl(args: &[&str]) -> Result<std::process::Output> {
    Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .context("running systemctl — is systemd available?")
}

pub fn state() -> State {
    let Ok(active) = systemctl(&["is-active", UNIT]) else {
        return State::Unmanaged;
    };
    let active = String::from_utf8_lossy(&active.stdout).trim().to_string();

    let enabled = systemctl(&["is-enabled", UNIT])
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();

    // An unknown unit reports neither; that is the manual-launch case.
    if enabled.is_empty() && active.is_empty() {
        return State::Unmanaged;
    }
    match (active.as_str(), enabled.as_str()) {
        ("active", _) => State::Running,
        (_, "enabled") => State::Stopped,
        _ => State::Disabled,
    }
}

/// Start now and at every login.
pub fn start() -> Result<()> {
    let out = systemctl(&["enable", "--now", UNIT])?;
    fail_with_stderr(out, "starting")
}

/// Stop now, but leave it enabled for next login.
pub fn stop() -> Result<()> {
    let out = systemctl(&["stop", UNIT])?;
    fail_with_stderr(out, "stopping")
}

/// Stop now and do not start at login.
pub fn disable() -> Result<()> {
    let out = systemctl(&["disable", "--now", UNIT])?;
    fail_with_stderr(out, "disabling")
}

/// Pick up a changed config: the daemon reads it once at startup.
pub fn restart() -> Result<()> {
    let out = systemctl(&["restart", UNIT])?;
    fail_with_stderr(out, "restarting")
}

fn fail_with_stderr(out: std::process::Output, doing: &str) -> Result<()> {
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr);
    let err = err.trim();
    anyhow::bail!(
        "{doing} the service failed{}",
        if err.is_empty() {
            String::new()
        } else {
            format!(": {err}")
        }
    )
}
