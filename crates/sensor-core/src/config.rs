//! User configuration: a flat `key = value` file under XDG config.
//!
//! Deliberately not TOML. Three settings do not justify serde plus a parser
//! plus a derive macro in a binary that reads every keystroke; the dependency
//! tree is part of what makes this auditable. Revisit if config grows shapes
//! that this cannot express.

use crate::APP_NAME;
use anyhow::{bail, Context, Result};
use evdev::Key;
use std::path::PathBuf;

/// Right Alt: present on effectively every keyboard, rarely bound by
/// applications, and not part of the Ctrl+V chord used to paste. No key is
/// safe on every machine -- laptops fold the function row into an Fn layer and
/// 2024+ models replaced right Ctrl with a Copilot key -- so this is a
/// starting point that the config overrides, not an assumption.
pub const DEFAULT_HOTKEY: Key = Key::KEY_RIGHTALT;

pub const DEFAULT_MODEL: &str = "ggml-tiny.en.bin";

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub hotkey: Key,
    pub model: Option<PathBuf>,
    /// Terminals need Ctrl+Shift+V; there is no reliable way to detect the
    /// focused app under Wayland, so this stays a user choice.
    pub paste_shift: bool,
    /// Microphone by cpal name; `None` means the system default. Stored by
    /// name because device order changes across reboots and hotplug.
    pub microphone: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: DEFAULT_HOTKEY,
            model: None,
            paste_shift: false,
            microphone: None,
        }
    }
}

impl Config {
    /// Loads the config file if present; a missing file is not an error.
    pub fn load() -> Result<Self> {
        let path = config_path()?;
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                Self::parse(&text).with_context(|| format!("reading config {}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading config {}", path.display())),
        }
    }

    pub fn parse(text: &str) -> Result<Self> {
        let mut cfg = Self::default();
        for (n, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                bail!("line {}: expected `key = value`, got {raw:?}", n + 1);
            };
            let (key, value) = (key.trim(), value.trim());
            match key {
                "hotkey" => {
                    cfg.hotkey = key_by_name(value)
                        .with_context(|| format!("line {}: no such key {value:?}", n + 1))?
                }
                "model" => cfg.model = Some(PathBuf::from(value)),
                "microphone" => {
                    cfg.microphone = (value != "default").then(|| value.to_string());
                }
                "paste_shift" => {
                    cfg.paste_shift = match value {
                        "true" => true,
                        "false" => false,
                        _ => bail!("line {}: paste_shift must be true or false", n + 1),
                    }
                }
                _ => bail!("line {}: unknown setting {key:?}", n + 1),
            }
        }
        Ok(cfg)
    }
}

/// Resolves an evdev key from its name, with or without the `KEY_` prefix.
/// evdev exposes no reverse lookup, so scan the keycode space for a match.
pub fn key_by_name(name: &str) -> Option<Key> {
    let name = name.trim().to_uppercase();
    let name = if name.starts_with("KEY_") {
        name
    } else {
        format!("KEY_{name}")
    };
    (0..0x2ffu16)
        .map(Key::new)
        .find(|k| format!("{k:?}") == name)
}

/// `$XDG_DATA_HOME/<app>`, falling back to `~/.local/share`. Holds the
/// downloaded model; `apt purge` removes it.
pub fn data_dir() -> Result<PathBuf> {
    let base = match std::env::var_os("XDG_DATA_HOME") {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => {
            let home = std::env::var_os("HOME").context("neither XDG_DATA_HOME nor HOME is set")?;
            PathBuf::from(home).join(".local").join("share")
        }
    };
    Ok(base.join(APP_NAME))
}

/// Where the model is expected to live once fetched.
pub fn default_model_path() -> Result<PathBuf> {
    Ok(data_dir()?.join(DEFAULT_MODEL))
}

/// `$XDG_CONFIG_HOME/<app>/config`, falling back to `~/.config`.
pub fn config_path() -> Result<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => {
            let home =
                std::env::var_os("HOME").context("neither XDG_CONFIG_HOME nor HOME is set")?;
            PathBuf::from(home).join(".config")
        }
    };
    Ok(base.join(APP_NAME).join("config"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_is_the_default() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
        assert_eq!(Config::parse("\n\n   \n").unwrap(), Config::default());
    }

    #[test]
    fn parses_each_setting() {
        let cfg = Config::parse(
            "hotkey = F12\nmodel = /tmp/m.bin\npaste_shift = true\nmicrophone = pipewire",
        )
        .unwrap();
        assert_eq!(cfg.hotkey, Key::KEY_F12);
        assert_eq!(cfg.model, Some(PathBuf::from("/tmp/m.bin")));
        assert!(cfg.paste_shift);
        assert_eq!(cfg.microphone.as_deref(), Some("pipewire"));
        // "default" is spelled out in the file but means "no preference".
        assert_eq!(
            Config::parse("microphone = default").unwrap().microphone,
            None
        );
    }

    #[test]
    fn ignores_comments_and_whitespace() {
        let cfg = Config::parse("# a comment\n  hotkey=RIGHTALT   # trailing\n").unwrap();
        assert_eq!(cfg.hotkey, Key::KEY_RIGHTALT);
    }

    #[test]
    fn rejects_bad_input_with_line_numbers() {
        for bad in [
            "hotkey = NOSUCHKEY",
            "nonsense",
            "unknown = 1",
            "paste_shift = yes",
        ] {
            assert!(Config::parse(bad).is_err(), "{bad:?} should not parse");
        }
        let err = Config::parse("hotkey = F12\nbroken")
            .unwrap_err()
            .to_string();
        assert!(err.contains("line 2"), "{err}");
    }

    #[test]
    fn resolves_keys_with_or_without_prefix() {
        assert_eq!(key_by_name("F12"), Some(Key::KEY_F12));
        assert_eq!(key_by_name("key_f12"), Some(Key::KEY_F12));
        assert_eq!(key_by_name("rightalt"), Some(Key::KEY_RIGHTALT));
        assert_eq!(key_by_name("NOPE"), None);
    }
}
