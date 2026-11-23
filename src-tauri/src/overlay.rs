use crate::input::Target;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, WebviewUrl, WebviewWindowBuilder,
};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SetWindowPos, ShowWindow, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
    SW_HIDE,
};

const LABEL: &str = "recording-overlay";
const WIDTH: f64 = 188.0;
const HEIGHT: f64 = 54.0;
/// Room for the "クラウド" badge (the cloud recognition mode) and the "English" badge.
const CLOUD_BADGE: f64 = 62.0;
const ENGLISH_BADGE: f64 = 70.0;
/// With the live text: room for about three lines above the indicator.
const PREVIEW_WIDTH: f64 = 560.0;
const PREVIEW_HEIGHT: f64 = 150.0;
/// The end of the text shown live (the box holds about three lines).
const PREVIEW_CHARS: usize = 110;
/// Notices are sized to their text (13px font: full-width ~13px, ASCII ~7.5px),
/// plus the pill's side padding, within these bounds.
const NOTICE_MIN_WIDTH: f64 = 150.0;
const NOTICE_MAX_WIDTH: f64 = 560.0;

fn notice_width(message: &str) -> f64 {
    let text: f64 = message
        .chars()
        .map(|c| if c.is_ascii() { 7.5 } else { 13.5 })
        .sum();
    (text + 44.0).clamp(NOTICE_MIN_WIDTH, NOTICE_MAX_WIDTH)
}
const NOTICE_SECONDS: u64 = 3;
/// Time for the blanked page to be drawn before the window is hidden (a few frames).
const BLANK_BEFORE_HIDE_MS: u64 = 150;
/// A recording's outcome (shown after the indicator closes) stays a little longer.
const OUTCOME_SECONDS: u64 = 5;

/// Show without activating, at the top of the always-on-top windows. Showing alone left
/// the overlay under other always-on-top windows (seen 2026-10-07).
fn show_on_top(window: &tauri::WebviewWindow) {
    if let Ok(hwnd) = window.hwnd() {
        unsafe {
            SetWindowPos(
                hwnd.0,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
        };
    }
}

/// True while the overlay shows recording or transcription; notices never replace it.
static BUSY: AtomicBool = AtomicBool::new(false);
/// Bumped by every show/hide/notice, so a notice's delayed hide cannot hide a newer view.
static EPOCH: AtomicU64 = AtomicU64::new(0);
/// The outcome of the recording on screen, shown once the indicator closes (the latest
/// wins). Without it, "no text found" and the like were never seen: the indicator just
/// disappeared (2026-10-17).
static OUTCOME: Mutex<Option<String>> = Mutex::new(None);

/// Show a short status message (non-activating, click-through) for a few seconds,
/// so the user sees it without opening the settings window. Skipped while recording.
pub fn flash(app: &AppHandle, message: &str) {
    flash_for(app, message, NOTICE_SECONDS);
}

/// Like `flash`, but while the indicator is shown, keep the message and show it when the
/// indicator closes: for a recording's outcome, which must not be lost.
pub fn flash_outcome(app: &AppHandle, message: &str) {
    if BUSY.load(Ordering::Acquire) {
        *OUTCOME.lock().expect("overlay outcome lock") = Some(message.to_owned());
        // The indicator may have closed in between; then show it now.
        if !BUSY.load(Ordering::Acquire) {
            if let Some(message) = OUTCOME.lock().expect("overlay outcome lock").take() {
                flash_for(app, &message, OUTCOME_SECONDS);
            }
        }
    } else {
        flash_for(app, message, OUTCOME_SECONDS);
    }
}

fn flash_for(app: &AppHandle, message: &str, seconds: u64) {
    if BUSY.load(Ordering::Acquire) {
        return;
    }
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };
    let epoch = EPOCH.fetch_add(1, Ordering::AcqRel) + 1;
    let _ = window.set_size(LogicalSize::new(notice_width(message), HEIGHT));
    if let (Ok(Some(monitor)), Ok(size)) = (window.primary_monitor(), window.outer_size()) {
        let work = monitor.work_area();
        let x = work.position.x + (work.size.width as i32 - size.width as i32) / 2;
        let y = work.position.y + work.size.height as i32 - size.height as i32 - 16;
        let _ = window.set_position(PhysicalPosition::new(x, y));
    }
    let _ = app.emit_to(LABEL, "overlay_notice", message);
    show_on_top(&window);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(seconds)).await;
        if EPOCH.load(Ordering::Acquire) == epoch && !BUSY.load(Ordering::Acquire) {
            hide(&app);
        }
    });
}

pub fn hide(app: &AppHandle) {
    BUSY.store(false, Ordering::Release);
    let epoch = EPOCH.fetch_add(1, Ordering::AcqRel) + 1;
    let outcome = OUTCOME.lock().expect("overlay outcome lock").take();
    if let Some(message) = outcome {
        flash_for(app, &message, OUTCOME_SECONDS);
        return;
    }
    // Blank the page first and hide the window once that has been drawn: a hidden window
    // keeps its last picture, and the next recording showed the previous one's text for a
    // moment (seen 2026-10-17). Blank, the overlay is fully transparent meanwhile.
    let _ = app.emit_to(LABEL, "overlay_phase", "hidden");
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(BLANK_BEFORE_HIDE_MS)).await;
        if EPOCH.load(Ordering::Acquire) == epoch {
            hide_window(&app);
        }
    });
}

fn hide_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(LABEL) {
        if let Ok(hwnd) = window.hwnd() {
            unsafe { ShowWindow(hwnd.0, SW_HIDE) };
        }
        let _ = window.hide();
    }
}

/// The draft is being written by the local model.
pub fn polishing(app: &AppHandle) {
    let _ = app.emit_to(LABEL, "overlay_phase", "polishing");
}

/// The text recognized so far while speaking: `confirmed` (parts finished at a pause)
/// and `guess` (the part still being spoken, which may change; speed-first mode only,
/// else empty). Only the end of both together is shown.
pub fn preview_text(app: &AppHandle, confirmed: &str, guess: &str) {
    let (confirmed, guess) = preview_end(confirmed, guess, PREVIEW_CHARS);
    let _ = app.emit_to(LABEL, "overlay_text", confirmed);
    let _ = app.emit_to(LABEL, "overlay_guess", guess);
}

fn preview_end(confirmed: &str, guess: &str, limit: usize) -> (String, String) {
    let guess: Vec<char> = guess.chars().collect();
    let guess_kept = &guess[guess.len().saturating_sub(limit)..];
    let room = limit - guess_kept.len();
    let confirmed: Vec<char> = confirmed.chars().collect();
    let confirmed_kept = &confirmed[confirmed.len().saturating_sub(room)..];
    (confirmed_kept.iter().collect(), guess_kept.iter().collect())
}

pub fn transcribing(app: &AppHandle) {
    let _ = app.emit_to(LABEL, "overlay_phase", "transcribing");
}

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("index.html".into()))
        .title("VoiceWeave recording")
        .inner_size(WIDTH, HEIGHT)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .focusable(false)
        .visible(false)
        .build()?;
    window.set_ignore_cursor_events(true)?;
    Ok(())
}

pub struct RecordingOverlay {
    app: AppHandle,
}

impl RecordingOverlay {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }

    pub fn app(&self) -> &AppHandle {
        &self.app
    }

    /// `preview`: make room for the live text above the indicator. `cloud`: the
    /// recording will be sent to the cloud, which the indicator says the whole time.
    pub fn show(
        &self,
        target: Option<Target>,
        preview: bool,
        cloud: bool,
        english: bool,
    ) -> tauri::Result<()> {
        let Some(window) = self.app.get_webview_window(LABEL) else {
            return Ok(());
        };
        BUSY.store(true, Ordering::Release);
        EPOCH.fetch_add(1, Ordering::AcqRel);
        // An earlier recording's outcome is not shown after this one.
        OUTCOME.lock().expect("overlay outcome lock").take();
        // A notice may have widened the window; recording uses the compact size.
        if preview {
            window.set_size(LogicalSize::new(PREVIEW_WIDTH, PREVIEW_HEIGHT))?;
        } else if cloud || english {
            let width = WIDTH
                + if cloud { CLOUD_BADGE } else { 0.0 }
                + if english { ENGLISH_BADGE } else { 0.0 };
            window.set_size(LogicalSize::new(width, HEIGHT))?;
        } else {
            window.set_size(LogicalSize::new(WIDTH, HEIGHT))?;
        }
        let size = window.outer_size()?;
        let work = target.and_then(|target| {
            let hwnd = target.foreground_handle() as *mut core::ffi::c_void;
            let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
            if monitor.is_null() {
                return None;
            }
            let mut info = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            (unsafe { GetMonitorInfoW(monitor, &mut info) } != 0).then_some(info.rcWork)
        });
        let (left, right, bottom) = if let Some(work) = work {
            (work.left, work.right, work.bottom)
        } else if let Some(monitor) = window.primary_monitor()? {
            let work = monitor.work_area();
            (
                work.position.x,
                work.position.x + work.size.width as i32,
                work.position.y + work.size.height as i32,
            )
        } else {
            (0, size.width as i32, size.height as i32)
        };
        let x = left + ((right - left - size.width as i32) / 2);
        let y = bottom - size.height as i32 - 16;
        window.set_position(PhysicalPosition::new(x, y))?;
        let mode = match (cloud, english) {
            (true, true) => "cloud_english",
            (true, false) => "cloud",
            (false, true) => "english",
            (false, false) => "local",
        };
        let _ = self.app.emit_to(LABEL, "overlay_mode", mode);
        let _ = self.app.emit_to(LABEL, "overlay_phase", "recording");
        self.level(0.0);
        // A recording indicator must never activate itself over the user's input target.
        show_on_top(&window);
        Ok(())
    }

    pub fn hide(&self) {
        hide(&self.app);
    }

    pub fn transcribing(&self) {
        transcribing(&self.app);
    }

    pub fn level(&self, level: f32) {
        let _ = self
            .app
            .emit_to(LABEL, "audio_level", level.clamp(0.0, 1.0));
    }
}

impl Drop for RecordingOverlay {
    fn drop(&mut self) {
        self.hide();
    }
}

#[cfg(test)]
mod tests {
    use super::notice_width;

    #[test]
    fn notice_width_follows_the_text_within_bounds() {
        let short = notice_width("処理中です");
        let long = notice_width("認識モデルが未導入です。設定画面から導入してください");
        assert!(short < long);
        assert_eq!(short, 150.0);
        assert!(long > 380.0 && long < 560.0);
        assert_eq!(notice_width(&"長".repeat(100)), 560.0);
    }
}
