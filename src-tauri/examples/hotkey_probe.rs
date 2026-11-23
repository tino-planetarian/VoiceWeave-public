//! Registers the default shortcuts, rebinds them to an unusual set (with a polish key)
//! and back, then exits. Shows whether another program holds a key; nothing is typed or
//! pressed.
use voiceweave_lib::hotkeys::HotkeyService;
use voiceweave_lib::shortcut::{parse_bindings, DEFAULT_RECORD, DEFAULT_REPASTE};

fn main() {
    let defaults = parse_bindings(DEFAULT_RECORD, DEFAULT_REPASTE, None).expect("defaults parse");
    let service = HotkeyService::start(defaults, |_| {}).expect("hotkey thread");
    println!("defaults: {:?}", service.status());
    let unusual = parse_bindings(
        "Ctrl+Alt+Shift+F11",
        "Ctrl+Alt+Shift+F12",
        Some("Ctrl+Alt+Shift+F10"),
    )
    .unwrap();
    println!(
        "rebind to Ctrl+Alt+Shift+F11/F12/F10: {:?} -> {:?}",
        service.rebind(unusual),
        service.status()
    );
    println!(
        "rebind back to defaults: {:?} -> {:?}",
        service.rebind(defaults),
        service.status()
    );
}
