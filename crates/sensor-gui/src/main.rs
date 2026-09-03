//! The settings window.
//!
//! Design brief: every Linux user, regardless of technical level, should be
//! able to see what is happening. So this shows real state read from the
//! running daemon rather than what the config file claims, and says plainly
//! when something is off.

use gtk::{gdk, glib, prelude::*};
use sensor_core::{
    audio,
    config::{self, Config},
    ipc, service, APP_NAME,
};
use std::{cell::RefCell, rc::Rc, time::Duration};

/// Chords offered in the dropdown, in config-file spelling.
///
/// Chords rather than single keys: a lone Right Alt is intercepted by
/// browsers, and a lone ordinary key steals a character. These pairs collide
/// with essentially nothing and are reachable with one hand.
const HOTKEY_CHOICES: &[&str] = &[
    "RIGHTALT + DOT",
    "RIGHTALT + COMMA",
    "RIGHTCTRL + DOT",
    "RIGHTCTRL + COMMA",
    "RIGHTALT + SLASH",
    "F12",
    "SCROLLLOCK",
];

const CSS: &str = "
window { background-color: #1e1f22; }
label { color: #f0f0f2; }
.title { font-size: 15pt; font-weight: 700; }
.section { font-size: 10pt; font-weight: 700; color: #f0f0f2; }
.muted { color: #9aa0aa; font-size: 9pt; }
.status-dot { font-size: 13pt; }
.running { color: #4ade80; }
.stopped { color: #fbbf24; }
.off     { color: #9aa0aa; }
.recent {
  font-family: monospace;
  font-size: 9pt;
  color: #c8ccd4;
}
.card {
  background-color: #26282d;
  border-radius: 8px;
  padding: 12px;
}
.danger {
  background-image: none;
  background-color: #c0392b;
  color: #ffffff;
  font-weight: 700;
  border: none;
}
.danger:hover { background-color: #d8483a; }
.mono {
  font-family: monospace;
  font-size: 9pt;
  color: #e8eaed;
}
";

fn main() -> glib::ExitCode {
    let app = gtk::Application::builder()
        .application_id("com.filamentplatform.sensor")
        .build();
    app.connect_startup(|_| load_css());
    app.connect_activate(build_window);
    // Without this, GTK treats stray argv as files to open and exits.
    app.run_with_args::<&str>(&[])
}

fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(CSS);
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn build_window(app: &gtk::Application) {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(APP_NAME)
        .default_width(420)
        .default_height(620)
        .resizable(true)
        .build();

    let root = gtk::Box::new(gtk::Orientation::Vertical, 16);
    root.set_margin_top(20);
    root.set_margin_bottom(20);
    root.set_margin_start(20);
    root.set_margin_end(20);

    let cfg = Rc::new(RefCell::new(Config::load().unwrap_or_default()));

    root.append(&header());
    let status = status_section();
    root.append(&status.widget);
    root.append(&hotkey_section(Rc::clone(&cfg)));
    root.append(&microphone_section(Rc::clone(&cfg)));
    root.append(&paste_section(Rc::clone(&cfg)));
    root.append(&model_section(Rc::clone(&cfg)));
    let recent = recent_section();
    root.append(&recent.widget);
    root.append(&gtk::Box::new(gtk::Orientation::Vertical, 0)); // spacer
    root.append(&delete_section(&window));

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&root)
        .build();
    window.set_child(Some(&scroll));

    // Poll the daemon so the window reflects reality, including the recording
    // indicator while the hotkey is held.
    let tick = move || {
        let daemon = ipc::query_status().ok().flatten();
        status.refresh(&daemon);
        recent.refresh(&daemon);
        glib::ControlFlow::Continue
    };
    tick();
    glib::timeout_add_local(Duration::from_millis(500), tick);

    window.present();
}

fn header() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let title = gtk::Label::new(Some(APP_NAME));
    title.add_css_class("title");
    title.set_xalign(0.0);
    let sub = gtk::Label::new(Some("Hold your key, speak, and the text appears."));
    sub.add_css_class("muted");
    sub.set_xalign(0.0);
    b.append(&title);
    b.append(&sub);
    b
}

fn section_label(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class("section");
    l.set_xalign(0.0);
    l
}

fn muted(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class("muted");
    l.set_xalign(0.0);
    l.set_wrap(true);
    l
}

// --- status ---------------------------------------------------------------

struct StatusSection {
    widget: gtk::Box,
    dot: gtk::Label,
    label: gtk::Label,
    detail: gtk::Label,
    button: gtk::Button,
}

fn status_section() -> Rc<StatusSection> {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
    card.add_css_class("card");

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let dot = gtk::Label::new(Some("●"));
    dot.add_css_class("status-dot");
    let label = gtk::Label::new(Some("Checking…"));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    let button = gtk::Button::with_label("…");
    row.append(&dot);
    row.append(&label);
    row.append(&button);

    let detail = muted("");
    card.append(&row);
    card.append(&detail);

    let s = Rc::new(StatusSection {
        widget: card,
        dot,
        label,
        detail,
        button,
    });

    let s2 = Rc::clone(&s);
    s.button.connect_clicked(move |_| {
        // Act on what the service is doing right now, not on stale state.
        let result = match service::state() {
            service::State::Running => service::stop(),
            _ => service::start(),
        };
        if let Err(e) = result {
            s2.detail.set_text(&format!("{e:#}"));
        }
    });
    s
}

impl StatusSection {
    fn refresh(&self, daemon: &Option<ipc::Status>) {
        let st = service::state();

        // The daemon answering IPC is stronger evidence than systemd's view:
        // it may have been started by hand.
        let live = daemon.is_some();
        let recording = daemon.as_ref().map(|d| d.recording).unwrap_or(false);

        let (dot_class, text) = if recording {
            ("running", "Recording…".to_string())
        } else if live {
            ("running", "Running".to_string())
        } else {
            (
                match st {
                    service::State::Disabled => "off",
                    _ => "stopped",
                },
                st.label().to_string(),
            )
        };

        for c in ["running", "stopped", "off"] {
            self.dot.remove_css_class(c);
        }
        self.dot.add_css_class(dot_class);
        self.label.set_text(&text);

        self.detail.set_text(if recording {
            "Listening to your microphone right now."
        } else if live && st == service::State::Unmanaged {
            "Running, but started by hand rather than as a service. It will not \
             come back on its own after you log out."
        } else if live {
            service::State::Running.description()
        } else {
            st.description()
        });

        self.button.set_label(if live { "Stop" } else { "Start" });
    }
}

// --- hotkey ---------------------------------------------------------------

fn hotkey_section(cfg: Rc<RefCell<Config>>) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
    b.append(&section_label("Recording key"));

    let parsed: Vec<config::Hotkey> = HOTKEY_CHOICES
        .iter()
        .filter_map(|s| config::Hotkey::parse(s))
        .collect();
    let labels: Vec<String> = parsed.iter().map(|h| h.describe()).collect();
    let label_refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
    let dropdown = gtk::DropDown::from_strings(&label_refs);

    let current = cfg.borrow().hotkey;
    let selected = parsed.iter().position(|h| *h == current);
    match selected {
        Some(i) => dropdown.set_selected(i as u32),
        // A key set via `sensorctl keys` may not be in the list; say so
        // rather than silently showing the wrong one.
        None => dropdown.set_selected(gtk::INVALID_LIST_POSITION),
    }

    let note = muted(&match selected {
        Some(_) => "Hold both keys while you speak.".to_string(),
        None => format!(
            "Currently {}, which is not in this list.",
            current.describe()
        ),
    });

    let note2 = note.clone();
    dropdown.connect_selected_notify(move |d| {
        let i = d.selected() as usize;
        let (Some(spelling), Some(hotkey)) = (HOTKEY_CHOICES.get(i), parsed.get(i)) else {
            return;
        };
        match save_setting("hotkey", spelling) {
            Ok(()) => {
                cfg.borrow_mut().hotkey = *hotkey;
                let restarted = service::restart().is_ok();
                note2.set_text(&if restarted {
                    format!("Saved. Hold {} while you speak.", hotkey.describe())
                } else {
                    format!(
                        "Saved as {}. Restart sensor for it to take effect.",
                        hotkey.describe()
                    )
                });
            }
            Err(e) => note2.set_text(&format!("Could not save: {e:#}")),
        }
    });

    b.append(&dropdown);
    b.append(&note);
    b.append(&muted(
        "Not listed? Run `sensorctl keys` in a terminal to press any key you like.",
    ));
    b
}

// --- microphone -----------------------------------------------------------

fn microphone_section(cfg: Rc<RefCell<Config>>) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
    b.append(&section_label("Microphone"));

    let devices = audio::input_devices().unwrap_or_default();
    if devices.is_empty() {
        b.append(&muted("No microphone was found."));
        return b;
    }

    let labels: Vec<&str> = devices.iter().map(|d| d.label.as_str()).collect();
    let dropdown = gtk::DropDown::from_strings(&labels);

    let want = cfg
        .borrow()
        .microphone
        .clone()
        .unwrap_or_else(|| "default".into());
    let idx = devices.iter().position(|d| d.id == want).unwrap_or(0);
    dropdown.set_selected(idx as u32);

    let note = muted("Where sensor listens from.");
    let note2 = note.clone();
    let ids: Vec<String> = devices.iter().map(|d| d.id.clone()).collect();

    dropdown.connect_selected_notify(move |d| {
        let Some(id) = ids.get(d.selected() as usize) else {
            return;
        };
        match save_setting("microphone", id) {
            Ok(()) => {
                cfg.borrow_mut().microphone = (id != "default").then(|| id.clone());
                let restarted = service::restart().is_ok();
                note2.set_text(if restarted {
                    "Saved."
                } else {
                    "Saved. Restart sensor for it to take effect."
                });
            }
            Err(e) => note2.set_text(&format!("Could not save: {e:#}")),
        }
    });

    b.append(&dropdown);
    b.append(&note);
    b
}

// --- model ----------------------------------------------------------------

fn model_section(cfg: Rc<RefCell<Config>>) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
    b.append(&section_label("Speech model"));

    let dropdown = gtk::DropDown::from_strings(&["tiny.en — fast, English only"]);
    dropdown.set_selected(0);
    let _ = cfg;

    b.append(&dropdown);
    b.append(&muted(
        "Runs on your machine. Audio is never uploaded anywhere.",
    ));
    b
}

// --- paste style ----------------------------------------------------------

/// Terminals paste with Ctrl+Shift+V; everything else uses Ctrl+V. Wayland
/// gives no way to ask what window has focus, so this cannot be automatic --
/// but leaving it to a config file meant terminal users had no way to find it,
/// which read as "pasting is broken" rather than "there is a setting".
fn paste_section(cfg: Rc<RefCell<Config>>) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
    b.append(&section_label("Where you type"));

    const CHOICES: [&str; 2] = ["Normal apps (Ctrl+V)", "Terminals (Ctrl+Shift+V)"];
    let dropdown = gtk::DropDown::from_strings(&CHOICES);
    dropdown.set_selected(u32::from(cfg.borrow().paste_shift));

    let note = muted(
        "Terminals treat Ctrl+V as something else, so they need the other \
         setting. Switch this if the text does not appear where you speak.",
    );

    let note2 = note.clone();
    dropdown.connect_selected_notify(move |d| {
        let shift = d.selected() == 1;
        match save_setting("paste_shift", if shift { "true" } else { "false" }) {
            Ok(()) => {
                cfg.borrow_mut().paste_shift = shift;
                note2.set_text(&if service::restart().is_ok() {
                    "Saved.".to_string()
                } else {
                    "Saved. Restart sensor for it to take effect.".to_string()
                });
            }
            Err(e) => note2.set_text(&format!("Could not save: {e:#}")),
        }
    });

    b.append(&dropdown);
    b.append(&note);
    b
}

// --- recent ---------------------------------------------------------------

struct RecentSection {
    widget: gtk::Box,
    list: gtk::Box,
    empty: gtk::Label,
}

fn recent_section() -> Rc<RecentSection> {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
    b.append(&section_label("Recent"));

    let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
    card.add_css_class("card");

    let list = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let empty = muted("Nothing dictated yet this session.");
    card.append(&empty);
    card.append(&list);
    b.append(&card);
    b.append(&muted(
        "Kept in memory only, never saved to a file, and cleared when sensor stops.",
    ));

    Rc::new(RecentSection {
        widget: b,
        list,
        empty,
    })
}

impl RecentSection {
    fn refresh(&self, daemon: &Option<ipc::Status>) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let recent = daemon
            .as_ref()
            .map(|d| d.recent.clone())
            .unwrap_or_default();
        self.empty.set_visible(recent.is_empty());
        for line in recent {
            self.list.append(&recent_row(&line));
        }
    }
}

/// One transcription, with a way to get it back out.
///
/// Selectable *and* given a button: dragging to select a wrapped label is
/// fiddly, and the text the user is looking at is the text they just spoke,
/// so taking it should be one click rather than a careful drag.
fn recent_row(line: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);

    let l = gtk::Label::new(Some(&format!("“{line}”")));
    l.add_css_class("recent");
    l.set_xalign(0.0);
    l.set_wrap(true);
    l.set_hexpand(true);
    l.set_selectable(true);
    // A selectable label takes focus on click and shows a caret; starting
    // unfocused keeps the list reading as text rather than as a form.
    l.set_can_focus(false);

    let copy = gtk::Button::from_icon_name("edit-copy-symbolic");
    copy.set_tooltip_text(Some("Copy this text"));
    copy.add_css_class("flat");
    copy.set_valign(gtk::Align::Start);

    let text = line.to_string();
    copy.connect_clicked(move |btn| {
        if let Some(display) = gdk::Display::default() {
            display.clipboard().set_text(&text);
            btn.set_tooltip_text(Some("Copied"));
            btn.set_icon_name("object-select-symbolic");
        }
    });

    row.append(&l);
    row.append(&copy);
    row
}

// --- delete ---------------------------------------------------------------

fn delete_section(parent: &gtk::ApplicationWindow) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let button = gtk::Button::with_label("Permanently Delete");
    button.add_css_class("danger");

    let parent = parent.clone();
    button.connect_clicked(move |_| confirm_delete(&parent));

    b.append(&button);
    b.append(&muted(
        "Removes sensor, its model, your settings, and the permissions it added.",
    ));
    b
}

const PURGE_CMD: &str = "sudo apt purge sensor";

fn confirm_delete(parent: &gtk::ApplicationWindow) {
    let dialog = gtk::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title("Remove sensor?")
        .default_width(400)
        .build();

    let b = gtk::Box::new(gtk::Orientation::Vertical, 12);
    b.set_margin_top(20);
    b.set_margin_bottom(20);
    b.set_margin_start(20);
    b.set_margin_end(20);

    let heading = gtk::Label::new(Some("Are you sure?"));
    heading.add_css_class("section");
    heading.set_xalign(0.0);
    b.append(&heading);

    b.append(&muted(
        "This removes the program, the speech model, your settings, and the \
         group permissions the installer added.",
    ));

    let cmd_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let cmd = gtk::Label::new(Some(PURGE_CMD));
    cmd.add_css_class("mono");
    cmd.set_xalign(0.0);
    cmd.set_hexpand(true);
    cmd.set_selectable(true);
    let copy = gtk::Button::with_label("Copy");
    cmd_row.append(&cmd);
    cmd_row.append(&copy);

    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("card");
    card.append(&cmd_row);
    b.append(&card);

    b.append(&muted(
        "Run this in a terminal. sensor will not do it itself: removing system \
         packages needs administrator rights, and a program that can uninstall \
         itself can be tricked into doing it.",
    ));

    let copy_note = muted("");
    b.append(&copy_note);

    copy.connect_clicked(move |btn| {
        if let Some(display) = gdk::Display::default() {
            display.clipboard().set_text(PURGE_CMD);
            btn.set_label("Copied");
            copy_note.set_text("Paste it into a terminal to finish removing sensor.");
        }
    });

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let close = gtk::Button::with_label("Close");
    let d = dialog.clone();
    close.connect_clicked(move |_| d.close());
    buttons.append(&close);
    b.append(&buttons);

    dialog.set_child(Some(&b));
    dialog.present();
}

// --- config writing -------------------------------------------------------

/// Rewrites one setting, preserving every other line including comments.
fn save_setting(key: &str, value: &str) -> anyhow::Result<()> {
    let path = config::config_path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let existing = std::fs::read_to_string(&path).unwrap_or_default();

    let mut out = String::new();
    let mut replaced = false;
    for line in existing.lines() {
        let is_target = line
            .split('#')
            .next()
            .unwrap_or("")
            .split_once('=')
            .is_some_and(|(k, _)| k.trim() == key);
        if is_target {
            if !replaced {
                out.push_str(&format!("{key} = {value}\n"));
                replaced = true;
            }
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !replaced {
        out.push_str(&format!("{key} = {value}\n"));
    }
    std::fs::write(&path, out)?;
    Ok(())
}
