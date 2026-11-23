//! Stops a recording when the PC is locked or wakes from sleep, and makes the result of the
//! session in progress clipboard-only (PHASE1_DESIGN: lock and suspend end the session;
//! the words already spoken are kept on the clipboard at the user's request, never typed).
//! Without it, a user who walks away from a locked PC could leave the microphone running,
//! and a result that arrives after the lock could be typed into the same window once the PC
//! is unlocked.
//!
//! No window procedure is needed: while the lock screen is shown, the input desktop cannot
//! be opened; after sleep, the clock jumps between two ticks of this watcher.

use crate::pipeline::PipelineSender;
use std::time::{Duration, Instant};
use tauri::AppHandle;
use windows_sys::Win32::System::StationsAndDesktops::{CloseDesktop, OpenInputDesktop};

const TICK: Duration = Duration::from_millis(500);
/// A gap this long between two ticks means the PC slept.
const SLEEP_GAP: Duration = Duration::from_secs(5);
/// DESKTOP_SWITCHDESKTOP: refused for the lock screen's desktop.
const SWITCH_DESKTOP: u32 = 0x0100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Locked,
    Resumed,
}

/// The decision, kept apart from the operating system calls so it can be tested.
#[derive(Default)]
pub struct Watch {
    locked: bool,
}

impl Watch {
    /// `locked`: the input desktop is not available now. `gap`: time since the last tick.
    /// Reports the moment of locking once (not at every tick while locked), and a wake-up.
    pub fn observe(&mut self, locked: bool, gap: Duration) -> Option<Reason> {
        let was_locked = std::mem::replace(&mut self.locked, locked);
        if locked && !was_locked {
            Some(Reason::Locked)
        } else if gap > SLEEP_GAP {
            Some(Reason::Resumed)
        } else {
            None
        }
    }
}

fn input_desktop_available() -> bool {
    // SAFETY: plain Win32 calls; the handle is closed at once.
    unsafe {
        let desktop = OpenInputDesktop(0, 0, SWITCH_DESKTOP);
        if desktop.is_null() {
            false
        } else {
            CloseDesktop(desktop);
            true
        }
    }
}

/// Watch for the rest of the run; stops the session in progress (if any).
pub fn start(app: AppHandle, pipeline: PipelineSender) {
    std::thread::spawn(move || {
        let mut watch = Watch::default();
        let mut last = Instant::now();
        loop {
            std::thread::sleep(TICK);
            let now = Instant::now();
            let gap = now.duration_since(last);
            last = now;
            if let Some(reason) = watch.observe(!input_desktop_available(), gap) {
                let message = match reason {
                    Reason::Locked => {
                        "画面がロックされたため、録音を止めました。入力はせず、結果はクリップボードにコピーします"
                    }
                    Reason::Resumed => {
                        "スリープから復帰したため、録音を止めました。入力はせず、結果はクリップボードにコピーします"
                    }
                };
                pipeline.keep_after_lock(&app, message);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICKS: Duration = Duration::from_millis(500);

    #[test]
    fn locking_is_reported_once_not_at_every_tick() {
        let mut watch = Watch::default();
        assert_eq!(watch.observe(false, TICKS), None);
        assert_eq!(watch.observe(true, TICKS), Some(Reason::Locked));
        assert_eq!(watch.observe(true, TICKS), None);
        assert_eq!(watch.observe(true, TICKS), None);
        assert_eq!(watch.observe(false, TICKS), None);
        // Locking again is a new event.
        assert_eq!(watch.observe(true, TICKS), Some(Reason::Locked));
    }

    #[test]
    fn a_long_gap_between_ticks_means_the_pc_slept() {
        let mut watch = Watch::default();
        assert_eq!(watch.observe(false, TICKS), None);
        assert_eq!(
            watch.observe(false, Duration::from_secs(90)),
            Some(Reason::Resumed)
        );
        assert_eq!(watch.observe(false, TICKS), None);
        // A brief stall of the app itself is not a sleep.
        assert_eq!(watch.observe(false, Duration::from_secs(2)), None);
    }
}
