use crate::shortcut::{Bindings, Shortcut};
use std::sync::{mpsc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_NOREPEAT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetMessageW, PostThreadMessageW, MSG, WM_APP, WM_HOTKEY, WM_QUIT,
};

const RECORD_ID: i32 = 1;
const REPASTE_ID: i32 = 2;
const POLISH_ID: i32 = 3;
const ESCAPE_ID: i32 = 4;
const COMPOSE_ID: i32 = 5;
const ENGLISH_ID: i32 = 6;
/// Wakes the hotkey thread to apply a queued rebind request.
const WM_REBIND: u32 = WM_APP + 1;
/// Registers (wParam 1) or releases (wParam 0) Esc as the cancel key.
const WM_ESCAPE: u32 = WM_APP + 2;
const VK_ESCAPE: u32 = 0x1B;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyEvent {
    Record,
    Repaste,
    /// Record like `Record`, then turn the transcript into an email-style draft.
    Polish,
    /// Record like `Record`, then write a text from the spoken request (compose mode).
    Compose,
    /// Record like `Record`, recognized as English.
    English,
    /// Esc while recording: discard the recording and enter nothing.
    Cancel,
}

/// While the settings screen waits for a key combination, pressing a registered
/// shortcut fills the field instead of recording or re-pasting. The window holds the
/// capture only while a field has focus; a capture left behind (for example the window
/// hid while a field was focused) lapses after `CAPTURE_LIMIT` so the keys never stay dead.
#[derive(Default)]
pub struct ShortcutCapture(Mutex<Option<Instant>>);

const CAPTURE_LIMIT: Duration = Duration::from_secs(60);

impl ShortcutCapture {
    pub fn set(&self, active: bool) {
        *self.0.lock().expect("shortcut capture lock") = active.then(Instant::now);
    }

    pub fn active(&self) -> bool {
        self.active_at(Instant::now())
    }

    fn active_at(&self, now: Instant) -> bool {
        self.0
            .lock()
            .expect("shortcut capture lock")
            .is_some_and(|started| now.saturating_duration_since(started) < CAPTURE_LIMIT)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct HotkeyStatus {
    pub record: bool,
    pub repaste: bool,
    /// False also when no polish shortcut is requested.
    pub polish: bool,
    /// False also when no compose shortcut is requested.
    pub compose: bool,
    /// False also when English recording is off.
    pub english: bool,
}

impl HotkeyStatus {
    /// Every requested shortcut is registered.
    pub fn covers(self, bindings: Bindings) -> bool {
        self.record
            && self.repaste
            && (bindings.polish.is_none() || self.polish)
            && (bindings.compose.is_none() || self.compose)
            && (bindings.english.is_none() || self.english)
    }
}

/// Whether saving `requested` must touch Windows registrations. The same pair as the
/// one bound is skipped only when both keys are actually held; otherwise saving the
/// same pair retries (for example after a start-up fallback or another app released it).
pub fn needs_rebind(requested: Bindings, bound: Bindings, held: HotkeyStatus) -> bool {
    requested != bound || !held.covers(requested)
}

type RebindRequest = (Bindings, mpsc::SyncSender<Result<(), HotkeyStatus>>);

pub struct HotkeyService {
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
    requests: Mutex<mpsc::Sender<RebindRequest>>,
    status: Mutex<HotkeyStatus>,
    /// The pair last requested successfully (or at start), whether or not Windows
    /// granted it; compared against new settings to decide whether to rebind.
    bound: Mutex<Bindings>,
}

fn register(id: i32, shortcut: Shortcut) -> bool {
    // Hotkeys belong to the thread that registers them, so this runs on the hotkey thread.
    unsafe {
        RegisterHotKey(
            std::ptr::null_mut(),
            id,
            shortcut.modifiers | MOD_NOREPEAT,
            shortcut.virtual_key,
        ) != 0
    }
}

fn unregister(id: i32) {
    unsafe { UnregisterHotKey(std::ptr::null_mut(), id) };
}

fn release(held: HotkeyStatus) {
    if held.record {
        unregister(RECORD_ID);
    }
    if held.repaste {
        unregister(REPASTE_ID);
    }
    if held.polish {
        unregister(POLISH_ID);
    }
    if held.compose {
        unregister(COMPOSE_ID);
    }
    if held.english {
        unregister(ENGLISH_ID);
    }
}

fn register_all(bindings: Bindings) -> HotkeyStatus {
    HotkeyStatus {
        record: register(RECORD_ID, bindings.record),
        repaste: register(REPASTE_ID, bindings.repaste),
        polish: bindings
            .polish
            .is_some_and(|polish| register(POLISH_ID, polish)),
        compose: bindings
            .compose
            .is_some_and(|compose| register(COMPOSE_ID, compose)),
        english: bindings
            .english
            .is_some_and(|english| register(ENGLISH_ID, english)),
    }
}

/// Replace all shortcuts, or keep the old ones. Returns the registered state.
fn rebind(
    current: Bindings,
    held: HotkeyStatus,
    wanted: Bindings,
) -> Result<HotkeyStatus, HotkeyStatus> {
    release(held);
    let status = register_all(wanted);
    if status.covers(wanted) {
        return Ok(status);
    }
    // One of them is taken by another program: put the previous set back.
    release(status);
    Err(HotkeyStatus {
        record: held.record && register(RECORD_ID, current.record),
        repaste: held.repaste && register(REPASTE_ID, current.repaste),
        polish: held.polish
            && current
                .polish
                .is_some_and(|polish| register(POLISH_ID, polish)),
        compose: held.compose
            && current
                .compose
                .is_some_and(|compose| register(COMPOSE_ID, compose)),
        english: held.english
            && current
                .english
                .is_some_and(|english| register(ENGLISH_ID, english)),
    })
}

impl HotkeyService {
    pub fn start<F>(bindings: Bindings, on_key: F) -> Result<Self, String>
    where
        F: Fn(HotkeyEvent) + Send + 'static,
    {
        let (sender, receiver) = mpsc::sync_channel(1);
        let (requests, incoming) = mpsc::channel::<RebindRequest>();
        let thread = thread::Builder::new()
            .name("voiceweave-hotkeys".into())
            .spawn(move || {
                let thread_id = unsafe { GetCurrentThreadId() };
                // RegisterHotKey creates this thread's message queue before the caller can send WM_QUIT.
                let mut held = register_all(bindings);
                let mut current = bindings;
                // Esc is taken from other programs only while a recording runs.
                let mut escape = false;
                if sender.send((thread_id, held)).is_err() {
                    release(held);
                    return;
                }
                let mut msg = MSG::default();
                loop {
                    let result = unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) };
                    if result <= 0 {
                        break;
                    }
                    if msg.message == WM_HOTKEY {
                        match msg.wParam as i32 {
                            RECORD_ID if held.record => on_key(HotkeyEvent::Record),
                            REPASTE_ID if held.repaste => on_key(HotkeyEvent::Repaste),
                            POLISH_ID if held.polish => on_key(HotkeyEvent::Polish),
                            COMPOSE_ID if held.compose => on_key(HotkeyEvent::Compose),
                            ENGLISH_ID if held.english => on_key(HotkeyEvent::English),
                            ESCAPE_ID if escape => on_key(HotkeyEvent::Cancel),
                            _ => {}
                        }
                    } else if msg.message == WM_ESCAPE {
                        let wanted = msg.wParam != 0;
                        if wanted && !escape {
                            escape = unsafe {
                                RegisterHotKey(
                                    std::ptr::null_mut(),
                                    ESCAPE_ID,
                                    MOD_NOREPEAT,
                                    VK_ESCAPE,
                                ) != 0
                            };
                        } else if !wanted && escape {
                            unregister(ESCAPE_ID);
                            escape = false;
                        }
                    } else if msg.message == WM_REBIND {
                        while let Ok((wanted, reply)) = incoming.try_recv() {
                            let outcome = rebind(current, held, wanted);
                            match outcome {
                                Ok(status) => {
                                    held = status;
                                    current = wanted;
                                    let _ = reply.send(Ok(()));
                                }
                                Err(status) => {
                                    held = status;
                                    let _ = reply.send(Err(status));
                                }
                            }
                        }
                    }
                }
                release(held);
                if escape {
                    unregister(ESCAPE_ID);
                }
            })
            .map_err(|_| "Could not start hotkey thread")?;
        let (thread_id, status) = receiver
            .recv()
            .map_err(|_| "Hotkey thread failed to initialize")?;
        Ok(Self {
            thread_id,
            thread: Some(thread),
            requests: Mutex::new(requests),
            status: Mutex::new(status),
            bound: Mutex::new(bindings),
        })
    }

    /// Take Esc as the cancel key (while recording) or give it back to other programs.
    pub fn set_escape(&self, active: bool) {
        unsafe { PostThreadMessageW(self.thread_id, WM_ESCAPE, usize::from(active), 0) };
    }

    pub fn bound(&self) -> Bindings {
        *self.bound.lock().expect("hotkey bound lock")
    }

    pub fn status(&self) -> HotkeyStatus {
        *self.status.lock().expect("hotkey status lock")
    }

    /// Switch all shortcuts. On failure the previous set stays registered (as far
    /// as Windows allows) and the resulting state is returned as the error.
    pub fn rebind(&self, bindings: Bindings) -> Result<(), HotkeyStatus> {
        let (reply, response) = mpsc::sync_channel(1);
        let sent = self
            .requests
            .lock()
            .expect("hotkey request lock")
            .send((bindings, reply))
            .is_ok()
            && unsafe { PostThreadMessageW(self.thread_id, WM_REBIND, 0, 0) } != 0;
        let outcome = if sent {
            response
                .recv_timeout(Duration::from_secs(2))
                .unwrap_or(Err(self.status()))
        } else {
            Err(self.status())
        };
        let status = match outcome {
            Ok(()) => HotkeyStatus {
                record: true,
                repaste: true,
                polish: bindings.polish.is_some(),
                compose: bindings.compose.is_some(),
                english: bindings.english.is_some(),
            },
            Err(status) => status,
        };
        *self.status.lock().expect("hotkey status lock") = status;
        if outcome.is_ok() {
            *self.bound.lock().expect("hotkey bound lock") = bindings;
        }
        outcome
    }
}

impl Drop for HotkeyService {
    fn drop(&mut self) {
        if unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0) } != 0 {
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shortcut::parse;

    #[test]
    fn shortcut_capture_lapses_and_ends() {
        let capture = ShortcutCapture::default();
        assert!(!capture.active());
        capture.set(true);
        assert!(capture.active());
        assert!(!capture.active_at(Instant::now() + CAPTURE_LIMIT));
        capture.set(false);
        assert!(!capture.active());
    }

    #[test]
    fn rebinding_is_skipped_only_for_the_same_set_fully_held() {
        let set = |record: &str, polish: Option<&str>| Bindings {
            record: parse(record).unwrap(),
            repaste: parse("Ctrl+Alt+V").unwrap(),
            polish: polish.map(|key| parse(key).unwrap()),
            compose: None,
            english: None,
        };
        let status = |record, repaste, polish| HotkeyStatus {
            record,
            repaste,
            polish,
            compose: false,
            english: false,
        };
        let f8 = set("F8", None);
        assert!(!needs_rebind(f8, f8, status(true, true, false)));
        assert!(needs_rebind(
            set("Ctrl+Alt+R", None),
            f8,
            status(true, true, false)
        ));
        // Same set but Windows did not grant a key: saving again retries.
        assert!(needs_rebind(f8, f8, status(false, true, false)));
        // Turning the polish shortcut on or off is a change; a missing one is retried.
        let with_polish = set("F8", Some("F9"));
        assert!(needs_rebind(with_polish, f8, status(true, true, false)));
        assert!(needs_rebind(f8, with_polish, status(true, true, true)));
        assert!(needs_rebind(
            with_polish,
            with_polish,
            status(true, true, false)
        ));
        assert!(!needs_rebind(
            with_polish,
            with_polish,
            status(true, true, true)
        ));
    }
}
