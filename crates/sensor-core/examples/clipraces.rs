//! Measures how long wl-copy takes to actually own the clipboard selection.
//! If this is ever above zero, sending the paste chord immediately after
//! spawning is a race.

use std::{
    io::Write,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

fn main() {
    let mut delays = Vec::new();
    for i in 0..15 {
        let text = format!("sensor-probe-{i}");
        let start = Instant::now();
        let mut child = Command::new("wl-copy")
            .args(["--paste-once", "--foreground"])
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();

        let mut claimed = None;
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            let out = Command::new("wl-paste")
                .arg("--no-newline")
                .output()
                .unwrap();
            if String::from_utf8_lossy(&out.stdout).trim_end() == text {
                claimed = Some(start.elapsed());
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        let _ = child.kill();
        let _ = child.wait();
        match claimed {
            Some(d) => {
                println!("run {i:2}: claimed after {:?}", d);
                delays.push(d);
            }
            None => println!("run {i:2}: NEVER claimed within 500ms"),
        }
        thread::sleep(Duration::from_millis(50));
    }
    delays.sort();
    if !delays.is_empty() {
        println!(
            "\nmin {:?} | median {:?} | max {:?}",
            delays[0],
            delays[delays.len() / 2],
            delays[delays.len() - 1]
        );
    }
}
