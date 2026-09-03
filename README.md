# sensor

Hold a key, speak, and the text appears in whatever you're focused on.
Local voice dictation for Linux — including GNOME Wayland, where most
alternatives don't work at all.

```
curl -fsSL https://sensor.filamentplatform.com/install.sh | sh
sensorctl setup
```

The script downloads the `.deb` and hands it to `apt` — apt does the
installing, which is what lets `apt purge` undo it. [Read it first](https://sensor.filamentplatform.com/install.sh)
if you would rather, or grab the
[`.deb` directly](https://github.com/Filament-Platform-Inc/Sensor/releases/latest).

Then open **sensor** from your applications, or just hold the key and talk.

Nothing else. No `git clone`, no Python environment, no manual `sudo chmod`
on device nodes, no editing udev rules by hand.

## What this touches on your system

This app reads every keystroke and can synthesise keystrokes. You should know
exactly what it does before installing it, so:

| What | Why |
|---|---|
| `/usr/bin/sensord`, `/usr/bin/sensorctl` | the daemon and CLI |
| `/usr/lib/udev/rules.d/99-sensor.rules` | grants `input`/`uinput` groups access to the device nodes |
| `/usr/lib/modules-load.d/sensor.conf` | loads the `uinput` kernel module at boot |
| `/usr/lib/systemd/user/sensord.service` | the daemon, as a **user** service — it never runs as root |
| adds you to `input` and `uinput` | required to read the hotkey and type |
| `~/.local/share/sensor/` | the speech model, downloaded once (~75MB) |
| `~/.config/sensor/config` | your settings |
| `/usr/share/applications/sensor.desktop` | the launcher entry |
| `/usr/share/icons/hicolor/*/apps/sensor.png` | the app icon |

`sudo apt purge sensor` reverses all of it, including the group membership —
and only the memberships the installer actually added.

## Why it needs device-level access

Wayland deliberately forbids applications from reading global keys or
injecting keystrokes. The `virtual-keyboard` protocol that `wtype` uses is
refused by GNOME, the most widely used desktop. There is no portable
compositor API for this.

So sensor works one level down: it reads the hotkey from `/dev/input` (evdev)
and types through `/dev/uinput`, which sit below the compositor and therefore
behave identically on GNOME, KDE, X11 and a bare tty.

The tradeoff is honest: **the daemon can see every key you press.** That is
true of any global-hotkey tool on Linux. What limits it here:

- The systemd unit sets `PrivateNetwork=yes`. The process has no network
  namespace — it cannot send anything anywhere, by construction.
- Transcription is entirely on-device. No audio or text leaves the machine.
- Only the configured hotkey is acted on. Other events are discarded and
  never logged.
- It runs as your user, never as root.
- The source is here.

## Speed

About 700ms from key release to text, measured end to end on a 2024 Intel
laptop with no GPU:

| stage | typical |
|---|---|
| transcription (`tiny.en`, CPU) | ~630ms |
| paste into the focused window | ~80ms |

The daemon keeps the whisper model *and its compute state* resident. Creating
that state per utterance roughly doubles latency, which is why this runs as a
daemon rather than a script.

## Usage

Hold **Right Alt + `.`**, speak, release.

A chord rather than a single key: a lone modifier gets intercepted by browsers,
and a lone ordinary key steals a character. `Right Alt + .` collides with
essentially nothing and is reachable with one hand.

Not every keyboard has the same keys — laptops fold the function row into an
`Fn` layer, and 2024+ models replaced Right Ctrl with a Copilot key. Change it
in the settings window, or:

```
sensorctl keys      # press a key, see its name, save it as your hotkey
sensorctl config    # show current settings
sensorctl doctor    # check permissions, model, and daemon state
```

Terminals paste with `Ctrl+Shift+V` rather than `Ctrl+V`, and Wayland gives no
way to ask which window has focus, so sensor cannot switch automatically. Pick
where you type under **Where you type** in the settings window, or set
`paste_shift = true` in `~/.config/sensor/config`.

## Building from source

Needs a Rust toolchain, `cmake`, `libasound2-dev`, and `libgtk-4-dev`.

```
cargo build --release
./packaging/build-deb.sh          # produces dist/sensor_<version>_amd64.deb
./packaging/test-install.sh       # install/purge check in a container
```

`test-install.sh` needs Docker. It runs 28 checks in a clean Ubuntu 24.04
container: that every file lands, that the groups are created and joined,
that both binaries run, and that `apt purge` removes all of it — verified by
diffing the filesystem against a pre-install snapshot. It also checks that a
user who was already in `input` keeps that membership afterwards.

## Status

Working: the dictation loop, packaging, clean removal. English only
(`tiny.en`). An optional local-LLM cleanup pass is planned, running *after*
the raw text is typed so it never costs latency.

## Licence

MIT — see [LICENSE](LICENSE). Copyright 2026 Filament Platform Inc.
