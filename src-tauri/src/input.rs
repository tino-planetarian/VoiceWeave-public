use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
    VK_F8, VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_RCONTROL, VK_RMENU, VK_RSHIFT, VK_RWIN,
    VK_V,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId, IsWindow, GUITHREADINFO,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    foreground: usize,
    focus: usize,
    process_id: u32,
    thread_id: u32,
}

impl Target {
    #[cfg(feature = "experimental-audio")]
    pub(crate) fn foreground_handle(self) -> usize {
        self.foreground
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopiedReason {
    NoTarget,
    TargetChanged,
    KeysHeld,
    ClipboardChanged,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryOutcome {
    Cancelled,
    SentUnverified,
    CopiedOnly(CopiedReason),
    ClipboardUnavailable,
    SendFailed(u32),
}

/// The clipboard's sequence number right after this app put its own text there (0: never).
/// Compose mode reads the clipboard as reference text only when it changed since.
static OWN_CLIPBOARD: AtomicU32 = AtomicU32::new(0);

/// Longest reference text taken from the clipboard (characters).
pub const REFERENCE_CHARS: usize = 6_000;

/// Text the user copied for compose mode to refer to (an email to answer), or None: the
/// clipboard holds this app's own last input, no text, or something that looks like a
/// password or key (ASCII only, no spaces, up to 128 characters). Never logged.
pub fn copied_reference() -> Option<String> {
    let sequence = clipboard_win::raw::seq_num()?;
    if sequence.get() == OWN_CLIPBOARD.load(Ordering::Acquire) {
        return None;
    }
    let text: String = clipboard_win::get_clipboard_string().ok()?;
    reference_from(&text)
}

fn reference_from(text: &str) -> Option<String> {
    let text = text.trim();
    let secret_like =
        text.is_ascii() && !text.contains(char::is_whitespace) && text.chars().count() <= 128;
    if text.is_empty() || secret_like {
        return None;
    }
    Some(text.chars().take(REFERENCE_CHARS).collect())
}

pub fn capture_target() -> Option<Target> {
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.is_null() {
        return None;
    }
    let mut process_id = 0;
    let thread_id = unsafe { GetWindowThreadProcessId(foreground, &mut process_id) };
    if thread_id == 0 || process_id == 0 {
        return None;
    }
    let mut info = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetGUIThreadInfo(thread_id, &mut info) } == 0 || info.hwndFocus.is_null() {
        return None;
    }
    Some(Target {
        foreground: foreground as usize,
        focus: info.hwndFocus as usize,
        process_id,
        thread_id,
    })
}

fn target_is_current(target: Target) -> bool {
    let foreground = target.foreground as *mut core::ffi::c_void;
    let focus = target.focus as *mut core::ffi::c_void;
    if unsafe { IsWindow(foreground) } == 0 || unsafe { IsWindow(focus) } == 0 {
        return false;
    }
    capture_target() == Some(target)
}

fn key_is_down(key: u16) -> bool {
    (unsafe { GetAsyncKeyState(i32::from(key)) } as u16 & 0x8000) != 0
}

fn wait_for_released_keys() -> bool {
    const KEYS: [u16; 10] = [
        VK_LCONTROL,
        VK_RCONTROL,
        VK_LMENU,
        VK_RMENU,
        VK_LSHIFT,
        VK_RSHIFT,
        VK_LWIN,
        VK_RWIN,
        VK_V,
        VK_F8,
    ];
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if KEYS.iter().all(|key| !key_is_down(*key)) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn keyboard_input(key: u16, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Copies first, then attempts an unverified Ctrl+V only if the original target remains current.
/// This call must run outside the UI thread because clipboard and modifier waits can block.
pub fn deliver(text: &str, target: Option<Target>) -> DeliveryOutcome {
    deliver_inner(text, target, None)
}

pub fn deliver_cancellable(
    text: &str,
    target: Option<Target>,
    cancellation_generation: &AtomicU64,
    expected_generation: u64,
) -> DeliveryOutcome {
    deliver_inner(
        text,
        target,
        Some((cancellation_generation, expected_generation)),
    )
}

fn was_cancelled(cancelled: Option<(&AtomicU64, u64)>) -> bool {
    cancelled.is_some_and(|(generation, expected)| generation.load(Ordering::Acquire) != expected)
}

/// Keys our own Ctrl+V batch (Ctrl down, V down, V up, Ctrl up) may have left pressed
/// after only `sent` events went through. Only key releases: a second paste is never
/// attempted, because whether the first one reached the target is unknown.
fn keys_to_release(sent: u32) -> Vec<u16> {
    match sent {
        1 | 3 => vec![VK_LCONTROL],
        2 => vec![VK_V, VK_LCONTROL],
        _ => Vec::new(),
    }
}

fn deliver_inner(
    text: &str,
    target: Option<Target>,
    cancelled: Option<(&AtomicU64, u64)>,
) -> DeliveryOutcome {
    if was_cancelled(cancelled) {
        return DeliveryOutcome::Cancelled;
    }
    if text.is_empty() {
        return DeliveryOutcome::ClipboardUnavailable;
    }
    let mut copied = false;
    for wait_ms in [0, 50, 100, 200] {
        if wait_ms != 0 {
            thread::sleep(Duration::from_millis(wait_ms));
        }
        if was_cancelled(cancelled) {
            return DeliveryOutcome::Cancelled;
        }
        if clipboard_win::set_clipboard_string(text).is_ok() {
            copied = true;
            break;
        }
    }
    if !copied {
        return DeliveryOutcome::ClipboardUnavailable;
    }
    if was_cancelled(cancelled) {
        return DeliveryOutcome::CopiedOnly(CopiedReason::Cancelled);
    }
    let Some(sequence) = clipboard_win::raw::seq_num() else {
        return DeliveryOutcome::CopiedOnly(CopiedReason::ClipboardChanged);
    };
    OWN_CLIPBOARD.store(sequence.get(), Ordering::Release);
    let Some(target) = target else {
        return DeliveryOutcome::CopiedOnly(CopiedReason::NoTarget);
    };
    if !target_is_current(target) {
        return DeliveryOutcome::CopiedOnly(CopiedReason::TargetChanged);
    }
    if !wait_for_released_keys() {
        return DeliveryOutcome::CopiedOnly(CopiedReason::KeysHeld);
    }
    if !target_is_current(target) {
        return DeliveryOutcome::CopiedOnly(CopiedReason::TargetChanged);
    }
    if clipboard_win::raw::seq_num() != Some(sequence) {
        return DeliveryOutcome::CopiedOnly(CopiedReason::ClipboardChanged);
    }
    // This is the last cancellation check before an irreversible OS key submission.
    if was_cancelled(cancelled) {
        return DeliveryOutcome::CopiedOnly(CopiedReason::Cancelled);
    }
    let inputs = [
        keyboard_input(VK_LCONTROL, false),
        keyboard_input(VK_V, false),
        keyboard_input(VK_V, true),
        keyboard_input(VK_LCONTROL, true),
    ];
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    };
    if sent == inputs.len() as u32 {
        return DeliveryOutcome::SentUnverified;
    }
    let release: Vec<INPUT> = keys_to_release(sent)
        .into_iter()
        .map(|key| keyboard_input(key, true))
        .collect();
    if !release.is_empty() {
        unsafe {
            SendInput(
                release.len() as u32,
                release.as_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            )
        };
    }
    DeliveryOutcome::SendFailed(sent)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_partial_paste_only_releases_what_it_pressed_and_never_pastes_again() {
        assert_eq!(keys_to_release(0), Vec::<u16>::new());
        assert_eq!(keys_to_release(1), vec![VK_LCONTROL]);
        assert_eq!(keys_to_release(2), vec![VK_V, VK_LCONTROL]);
        assert_eq!(keys_to_release(3), vec![VK_LCONTROL]);
        assert_eq!(keys_to_release(4), Vec::<u16>::new());
        for sent in 0..=4 {
            // Every follow-up event is a key-up: nothing can type or paste a second time.
            for input in keys_to_release(sent)
                .into_iter()
                .map(|key| keyboard_input(key, true))
            {
                let flags = unsafe { input.Anonymous.ki.dwFlags };
                assert_ne!(flags & KEYEVENTF_KEYUP, 0);
            }
        }
    }

    #[test]
    fn copied_reference_skips_empty_and_secret_like_text_and_is_bounded() {
        assert_eq!(reference_from("   "), None);
        assert_eq!(reference_from("sk-proj-abcdEFGH1234"), None);
        assert_eq!(reference_from("P@ssw0rd!"), None);
        // Japanese has no spaces: a one-line sentence is still a reference.
        assert_eq!(
            reference_from("明日の会議は四時からです。"),
            Some("明日の会議は四時からです。".to_owned())
        );
        assert_eq!(
            reference_from(" 山田様 来週の打ち合わせの件です "),
            Some("山田様 来週の打ち合わせの件です".to_owned())
        );
        let long = "あ い".repeat(5_000);
        assert_eq!(
            reference_from(&long).map(|text| text.chars().count()),
            Some(REFERENCE_CHARS)
        );
    }

    #[test]
    fn pre_cancelled_delivery_does_not_start_clipboard_work() {
        let cancelled = AtomicU64::new(1);
        assert_eq!(
            deliver_cancellable("not for the clipboard", None, &cancelled, 0),
            DeliveryOutcome::Cancelled
        );
        assert_eq!(
            deliver_cancellable("", None, &cancelled, 1),
            DeliveryOutcome::ClipboardUnavailable
        );
    }
}
