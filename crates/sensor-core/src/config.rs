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

/// A key, or a modifier plus a key held together.
///
/// Chords rather than single keys by default: a lone modifier is grabbed by
/// browsers and desktops, and a lone ordinary key steals a character. A pair
/// like Right Alt + `.` collides with almost nothing while staying reachable
/// with one hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    /// Held first; `None` for a single-key hotkey.
    pub modifier: Option<Key>,
    /// The key that completes the chord, and the one devices are matched on.
    pub trigger: Key,
}

impl Hotkey {
    pub fn single(trigger: Key) -> Self {
        Self {
            modifier: None,
            trigger,
        }
    }

    pub fn chord(modifier: Key, trigger: Key) -> Self {
        Self {
            modifier: Some(modifier),
            trigger,
        }
    }

    /// Whether this hotkey is satisfied by the keys currently held.
    ///
    /// Other keys being down is fine: the user may type while dictating, and
    /// requiring an exact match would end the recording on any stray key.
    pub fn is_held(&self, down: &std::collections::HashSet<Key>) -> bool {
        down.contains(&self.trigger) && self.modifier.is_none_or(|m| down.contains(&m))
    }

    /// How the config file spells it.
    pub fn encode(&self) -> String {
        let short = |k: Key| {
            let n = format!("{k:?}");
            n.strip_prefix("KEY_").unwrap_or(&n).to_string()
        };
        match self.modifier {
            Some(m) => format!("{} + {}", short(m), short(self.trigger)),
            None => short(self.trigger),
        }
    }

    /// How a person should see it.
    pub fn describe(&self) -> String {
        let pretty = |k: Key| match k {
            Key::KEY_RIGHTALT => "Right Alt".to_string(),
            Key::KEY_LEFTALT => "Left Alt".to_string(),
            Key::KEY_RIGHTCTRL => "Right Ctrl".to_string(),
            Key::KEY_LEFTCTRL => "Left Ctrl".to_string(),
            Key::KEY_RIGHTSHIFT => "Right Shift".to_string(),
            Key::KEY_LEFTSHIFT => "Left Shift".to_string(),
            Key::KEY_DOT => ".".to_string(),
            Key::KEY_COMMA => ",".to_string(),
            Key::KEY_SEMICOLON => ";".to_string(),
            Key::KEY_SLASH => "/".to_string(),
            other => {
                let n = format!("{other:?}");
                n.strip_prefix("KEY_").unwrap_or(&n).to_string()
            }
        };
        match self.modifier {
            Some(m) => format!("{} + {}", pretty(m), pretty(self.trigger)),
            None => pretty(self.trigger),
        }
    }

    /// Parses `RIGHTALT + DOT`, `right alt + .`, or a bare key name.
    pub fn parse(text: &str) -> Option<Self> {
        let (modifier, trigger) = match text.split_once('+') {
            Some((m, t)) => (Some(key_by_name(m)?), key_by_name(t)?),
            None => (None, key_by_name(text)?),
        };
        Some(Self { modifier, trigger })
    }
}

/// Right Alt + `.` -- a chord, because single keys are all taken. A plain
/// modifier is intercepted by browsers and desktops, and a plain key steals a
/// character. No hotkey is safe on every machine either: laptops fold the
/// function row into an Fn layer, and 2024+ models replaced right Ctrl with a
/// Copilot key. So this is a starting point the config overrides.
pub const DEFAULT_HOTKEY: Hotkey = Hotkey {
    modifier: Some(Key::KEY_RIGHTALT),
    trigger: Key::KEY_DOT,
};

pub const DEFAULT_MODEL: &str = "ggml-tiny.en.bin";

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub hotkey: Hotkey,
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
                    cfg.hotkey = Hotkey::parse(value)
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
    // Accept what a person would type: "right alt", ".", "Right_Alt".
    let cleaned: String = name
        .trim()
        .to_uppercase()
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '_')
        .collect();
    let cleaned = match cleaned.as_str() {
        "." => "DOT".to_string(),
        "," => "COMMA".to_string(),
        ";" => "SEMICOLON".to_string(),
        "/" => "SLASH".to_string(),
        "'" => "APOSTROPHE".to_string(),
        other => other.to_string(),
    };
    let name = if cleaned.starts_with("KEY") && !cleaned.starts_with("KEYBOARD") {
        format!("KEY_{}", cleaned.trim_start_matches("KEY"))
    } else {
        format!("KEY_{cleaned}")
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
        assert_eq!(cfg.hotkey, Hotkey::single(Key::KEY_F12));
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
        assert_eq!(cfg.hotkey, Hotkey::single(Key::KEY_RIGHTALT));
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
    fn parses_chords_in_the_forms_a_person_would_write() {
        let alt_dot = Hotkey::chord(Key::KEY_RIGHTALT, Key::KEY_DOT);
        for spelling in [
            "RIGHTALT + DOT",
            "rightalt+dot",
            "right alt + .",
            "KEY_RIGHTALT + KEY_DOT",
        ] {
            assert_eq!(Hotkey::parse(spelling), Some(alt_dot), "{spelling:?}");
        }
        assert_eq!(Hotkey::parse("nonsense + dot"), None);
    }

    #[test]
    fn hotkey_round_trips_through_the_config_file() {
        for h in [
            Hotkey::chord(Key::KEY_RIGHTALT, Key::KEY_DOT),
            Hotkey::chord(Key::KEY_RIGHTCTRL, Key::KEY_COMMA),
            Hotkey::single(Key::KEY_F12),
        ] {
            let text = format!("hotkey = {}", h.encode());
            assert_eq!(Config::parse(&text).unwrap().hotkey, h, "{text}");
        }
    }

    #[test]
    fn describes_chords_readably() {
        assert_eq!(
            Hotkey::chord(Key::KEY_RIGHTALT, Key::KEY_DOT).describe(),
            "Right Alt + ."
        );
    }

    #[test]
    fn resolves_keys_with_or_without_prefix() {
        assert_eq!(key_by_name("F12"), Some(Key::KEY_F12));
        assert_eq!(key_by_name("key_f12"), Some(Key::KEY_F12));
        assert_eq!(key_by_name("rightalt"), Some(Key::KEY_RIGHTALT));
        assert_eq!(key_by_name("NOPE"), None);
    }
}
