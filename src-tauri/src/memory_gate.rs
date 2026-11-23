//! Whether the PC is short of memory for the local AI that punctuates (Ollama, about 5 GB
//! for a 7B model). Lost audio (Xrun) and long waits were seen at 96% memory use
//! (2026-11-03), and the model is the biggest single user. While memory is short the
//! recording uses the built-in rules for punctuation instead and the model is taken off.
//!
//! The decision is made with a margin (it turns on at one level and off at a lower one),
//! so it does not flip back and forth around the limit. After a few recordings in a
//! short-memory state the user is asked once whether to switch to the rules for good.

use crate::diagnostics::MemoryState;

/// Short of memory at or beyond one of these.
const TIGHT_LOAD_PERCENT: u32 = 90;
const TIGHT_COMMIT_PERCENT: u32 = 92;
const TIGHT_FREE_MB: u64 = 1024;
/// Back to normal only when all of these hold.
const CLEAR_LOAD_PERCENT: u32 = 85;
const CLEAR_COMMIT_PERCENT: u32 = 88;
const CLEAR_FREE_MB: u64 = 1536;
/// Recordings made while short of memory before the user is asked.
pub const ASK_AFTER_RECORDINGS: u32 = 3;

#[derive(Default)]
pub struct Gate {
    tight: bool,
    recordings: u32,
    asked: bool,
}

impl Gate {
    pub const fn new() -> Self {
        Self {
            tight: false,
            recordings: 0,
            asked: false,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    Nothing,
    /// Memory just became short: take the model off, tell the user.
    BecameTight,
    /// Short of memory for several recordings: ask whether to switch for good (once).
    AskUser,
}

impl Gate {
    /// `memory`: the PC's memory now (None when unknown, which counts as enough).
    /// `recording`: this call belongs to a finished recording (counted toward asking).
    /// Returns whether the memory is short, and what the caller should do about it.
    pub fn observe(&mut self, memory: Option<MemoryState>, recording: bool) -> (bool, Event) {
        let was_tight = self.tight;
        self.tight = match memory {
            None => false,
            Some(memory) if was_tight => {
                !(memory.load_percent < CLEAR_LOAD_PERCENT
                    && memory.commit_percent < CLEAR_COMMIT_PERCENT
                    && memory.available_mb >= CLEAR_FREE_MB)
            }
            Some(memory) => {
                memory.load_percent >= TIGHT_LOAD_PERCENT
                    || memory.commit_percent >= TIGHT_COMMIT_PERCENT
                    || memory.available_mb < TIGHT_FREE_MB
            }
        };
        if !self.tight {
            return (false, Event::Nothing);
        }
        let mut event = if was_tight {
            Event::Nothing
        } else {
            Event::BecameTight
        };
        if recording {
            self.recordings += 1;
            if self.recordings >= ASK_AFTER_RECORDINGS && !self.asked {
                self.asked = true;
                event = Event::AskUser;
            }
        }
        (true, event)
    }
}

/// How long the question stays on screen before it counts as "leave as it is".
pub const ASK_SECONDS: u64 = 40;

#[cfg(test)]
mod tests {
    use super::*;

    fn memory(load: u32, commit: u32, free_mb: u64) -> Option<MemoryState> {
        Some(MemoryState {
            load_percent: load,
            available_mb: free_mb,
            commit_percent: commit,
        })
    }

    #[test]
    fn enough_memory_changes_nothing() {
        let mut gate = Gate::default();
        assert_eq!(
            gate.observe(memory(60, 50, 6000), true),
            (false, Event::Nothing)
        );
        assert_eq!(gate.observe(None, true), (false, Event::Nothing));
    }

    #[test]
    fn each_limit_alone_makes_it_short() {
        for state in [
            memory(90, 50, 6000),
            memory(60, 92, 6000),
            memory(60, 50, 1000),
        ] {
            let mut gate = Gate::default();
            assert_eq!(
                gate.observe(state, false),
                (true, Event::BecameTight),
                "{state:?}"
            );
        }
    }

    #[test]
    fn it_does_not_flip_back_and_forth_around_the_limit() {
        let mut gate = Gate::default();
        assert!(gate.observe(memory(95, 97, 500), false).0);
        // Slightly better, but not past the lower level: still short, no new event.
        assert_eq!(
            gate.observe(memory(88, 90, 1200), false),
            (true, Event::Nothing)
        );
        // Past the lower level on every count: back to normal.
        assert_eq!(
            gate.observe(memory(80, 80, 3000), false),
            (false, Event::Nothing)
        );
        // Short again: the event fires again.
        assert_eq!(
            gate.observe(memory(95, 97, 500), false),
            (true, Event::BecameTight)
        );
    }

    #[test]
    fn the_user_is_asked_once_after_a_few_recordings() {
        let mut gate = Gate::default();
        let short = memory(95, 97, 500);
        assert_eq!(gate.observe(short, true).1, Event::BecameTight);
        // Calls that are not a finished recording (while speaking) are not counted.
        for _ in 0..10 {
            assert_eq!(gate.observe(short, false).1, Event::Nothing);
        }
        assert_eq!(gate.observe(short, true).1, Event::Nothing);
        assert_eq!(gate.observe(short, true).1, Event::AskUser);
        // Only once.
        assert_eq!(gate.observe(short, true).1, Event::Nothing);
    }

    #[test]
    fn recordings_with_enough_memory_are_not_counted() {
        let mut gate = Gate::default();
        for _ in 0..5 {
            assert_eq!(gate.observe(memory(50, 50, 8000), true).1, Event::Nothing);
        }
        assert_eq!(
            gate.observe(memory(95, 97, 500), true).1,
            Event::BecameTight
        );
    }
}
