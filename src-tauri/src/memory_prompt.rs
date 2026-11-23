//! The question shown when memory has been short for several recordings: "switch the
//! punctuation to the built-in rules for good?". A small window in the screen's lower
//! right that does not take the focus from the app being typed into; the user answers
//! with a click, without opening the settings window.

use tauri::{AppHandle, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder};

pub const LABEL: &str = "memory-prompt";
const WIDTH: f64 = 380.0;
const HEIGHT: f64 = 170.0;
/// Margin from the screen's right edge and from its bottom (above the taskbar).
const RIGHT_MARGIN: f64 = 16.0;
const BOTTOM_MARGIN: f64 = 72.0;

/// Show the question (nothing happens if it is already shown).
pub fn show(app: &AppHandle) -> tauri::Result<()> {
    if app.get_webview_window(LABEL).is_some() {
        return Ok(());
    }
    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html".into()))
        .title("VoiceWeave")
        .inner_size(WIDTH, HEIGHT)
        .resizable(false)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .focusable(false)
        .visible(false)
        .build()?;
    if let Ok(Some(monitor)) = app.primary_monitor() {
        let scale = monitor.scale_factor();
        let (size, origin) = (monitor.size(), monitor.position());
        let x = origin.x as f64 + size.width as f64 - (WIDTH + RIGHT_MARGIN) * scale;
        let y = origin.y as f64 + size.height as f64 - (HEIGHT + BOTTOM_MARGIN) * scale;
        let _ = window.set_position(PhysicalPosition::new(x as i32, y as i32));
    }
    window.show()
}

pub fn close(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.close();
    }
}
