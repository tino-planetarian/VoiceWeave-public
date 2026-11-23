//! Lists input devices the way the settings screen does. Opens no stream, records nothing.
fn main() {
    for device in voiceweave_lib::audio::input_devices() {
        println!(
            "{} | {} | id_chars={}",
            if device.is_default {
                "default"
            } else {
                "       "
            },
            device.name,
            device.id.chars().count()
        );
        let parsed: Result<cpal::DeviceId, _> = device.id.parse();
        assert!(parsed.is_ok(), "saved id must parse back");
    }
}
