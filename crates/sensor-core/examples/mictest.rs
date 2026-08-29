fn main() -> anyhow::Result<()> {
    let r = sensor_core::audio::Recorder::start()?;
    std::thread::sleep(std::time::Duration::from_secs(2));
    let s = r.finish()?;
    println!(
        "captured {} samples = {:.2}s at 16kHz",
        s.len(),
        s.len() as f32 / 16000.0
    );
    let peak = s.iter().fold(0f32, |m, v| m.max(v.abs()));
    println!(
        "peak amplitude: {peak:.4} ({})",
        if peak > 0.001 {
            "signal present"
        } else {
            "SILENT - check mic"
        }
    );
    sensor_core::audio::write_wav(std::path::Path::new("/tmp/mictest.wav"), &s)?;
    Ok(())
}
