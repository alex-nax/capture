//! Smoke runner for Windows input-device enumeration (#66 slice C):
//!   cargo run -p capture-platform --example windows_audio_devices
fn main() {
    let devs = capture_platform::audio_input_devices();
    eprintln!("{} input device(s):", devs.len());
    for d in &devs {
        println!("  {}{}  [{}]", if d.default { "* " } else { "  " }, d.name, d.id);
    }
}
