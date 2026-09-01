//! Prints the evdev name of every key pressed, so a hotkey can be chosen by
//! observation rather than assumption. Ctrl+C to stop.

use anyhow::Result;
use evdev::{Device, InputEventKind};
use std::thread;

fn main() -> Result<()> {
    let mut handles = Vec::new();
    for (path, dev) in evdev::enumerate() {
        if !dev
            .supported_keys()
            .is_some_and(|k| k.contains(evdev::Key::KEY_A))
        {
            continue;
        }
        let name = dev.name().unwrap_or("unnamed").to_string();
        println!("watching {} ({})", path.display(), name);
        handles.push(thread::spawn(move || watch(dev, name)));
    }
    if handles.is_empty() {
        eprintln!("no key-capable devices readable — are you in the 'input' group?");
        return Ok(());
    }
    println!("\npress keys; Ctrl+C to stop\n");
    for h in handles {
        let _ = h.join();
    }
    Ok(())
}

fn watch(mut dev: Device, name: String) {
    loop {
        let Ok(events) = dev.fetch_events() else {
            return;
        };
        for ev in events {
            if let InputEventKind::Key(key) = ev.kind() {
                let action = match ev.value() {
                    0 => "release",
                    1 => "press",
                    _ => continue,
                };
                println!("  {key:?}  (code {})  {action}  [{name}]", key.code());
            }
        }
    }
}
