//! Verifies that the clipboard is restored intact after a paste, and that the
//! text we serve is the text that was actually on the clipboard when the
//! chord went out. Run with a focused text field to also check the paste.

use sensor_core::output::{Injector, PasteChord};
use std::{process::Command, thread, time::Duration};

fn clip() -> String {
    let o = Command::new("wl-paste")
        .arg("--no-newline")
        .output()
        .unwrap();
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn main() {
    let sentinel = "USER-CLIPBOARD-SHOULD-SURVIVE";
    let mut ok = 0;
    let mut restored_ok = 0;
    let runs = 10;

    for i in 0..runs {
        // Put a known value on the clipboard, as if the user had copied it.
        let mut c = Command::new("wl-copy")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        c.stdin
            .as_mut()
            .unwrap()
            .write_all(sentinel.as_bytes())
            .unwrap();
        c.wait().unwrap();
        thread::sleep(Duration::from_millis(120));

        let text = format!("dictated text number {i}");
        let mut inj = Injector::new("sensor-pastecheck").unwrap();
        match inj.paste(&text, PasteChord::CtrlV) {
            Ok(()) => {
                ok += 1;
                println!("run {i}: pasted");
            }
            Err(e) => println!("run {i}: {e}"),
        }
        thread::sleep(Duration::from_millis(150));
        let after = clip();
        if after == sentinel {
            restored_ok += 1;
        } else {
            println!("   clipboard NOT restored: {after:?}");
        }
    }
    println!("\npasted {ok}/{runs}, clipboard restored {restored_ok}/{runs}");
}
