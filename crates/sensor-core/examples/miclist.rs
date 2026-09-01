fn main() {
    for d in sensor_core::audio::input_devices().unwrap() {
        println!(
            "{}{}  ({})",
            if d.is_default { "* " } else { "  " },
            d.label,
            d.id
        );
    }
}
