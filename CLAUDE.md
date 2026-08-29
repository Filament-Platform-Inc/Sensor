# CLAUDE.md

Open-source Linux voice dictation — hold a key, speak, text appears in the focused app.
Working title `sensor`; will ship under Hasnain's company at `<product>.<domain>.com`.
Goal is real users, not revenue.

## The thesis

**Packaging is the product.** Transcription is a solved problem and the category is crowded
(soupawhisper, YazSes, OpenWhispr, Vocalinux). Every competitor ships as `git clone` + Python +
manual `sudo` steps, and none work cleanly on GNOME Wayland. The gap is the last mile.

So the differentiators are, in order:
1. **One-command install** that configures its own device permissions.
2. **One-command complete uninstall.** This app reads every keystroke; reversibility is what makes
   it feel safe to try. No install scripts — everything through the package manager, `postrm`
   reverses `postinst`.
3. **Sub-400ms** key-release to text, via a daemon holding the model warm.

Judge feature requests against this. Resist transcription features that don't serve it.

## Technical shape

Rust workspace, two binaries: `sensord` (daemon, user systemd unit) and `sensorctl` (CLI).
Audio via `cpal` → transcription via `whisper-rs` (whisper.cpp FFI, model held warm) → text via
a `/dev/uinput` virtual keyboard. Hotkey read from `/dev/input` (evdev).

**Why kernel-level I/O:** Wayland forbids global key capture and keystroke injection. The
`virtual-keyboard` protocol (`wtype`) is refused by GNOME, the most common desktop. evdev/uinput
sit below the compositor, so they work on GNOME, KDE, X11, and tty alike. This is not optional.

**Dev machine is the hard case:** Ubuntu 24.04, GNOME, Wayland, 8 cores, Intel Lunar Lake iGPU
(no CUDA → CPU int8, `base.en`). Most restrictive mainstream setup, and also the modal user.

LLM cleanup is deferred to v2 but designed in: transcription returns through a `TextTransform`
pipeline with only `Identity` wired. v2 runs cleanup *after* raw text is typed, replacing in place,
so it never costs latency. Ollama is installed locally and is the natural backend.

**Renaming:** `sensor` appears only in `Cargo.toml` crate names, binary names, `packaging/` files,
and one `const APP_NAME` (used for config paths, socket path, notifications). Keep it that way so
the eventual rename stays mechanical.

## Working agreements

- **Hasnain is learning Rust** — this is his first Rust project. Explain the Rust-specific
  reasoning in conversation as it comes up (why `Arc<Mutex<T>>`, what a lifetime is doing, why an
  FFI call is `unsafe`). Keep that teaching *out of* code comments — they age badly in a public repo.
- **Check in at each meaningful decision** — crate choices, architecture forks, tradeoffs. Surface
  them rather than deciding silently; the decisions are the learning.
- **Security-sensitive code: flag and explain, don't block.** Write the evdev/uinput and permissions
  code as needed, but explicitly call out anything that widens what the daemon can see or do.
- **Git:** commit logical chunks as work completes, conventional-commit messages. **Never push** —
  Hasnain pushes. No remote exists until M3.

## Standards

- `clippy` (warnings as errors) and `rustfmt` on every change, from the first commit.
- Unit-test the logic — ring buffer, config parsing, transform pipeline. Device I/O is
  integration-tested by hand; it needs real hardware.
- **Keep dependencies minimal and justify each new crate.** A keystroke-reading binary with a small,
  auditable dependency tree is easier for strangers to trust.
- Keep the README's "what this touches on your system" section accurate from the first commit, not
  written at M3. It is a trust artifact, not documentation.
