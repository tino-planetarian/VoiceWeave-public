use crate::input::Target;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineState {
    MissingModel,
    Loading,
    Ready,
    Unavailable,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub id: Uuid,
    pub worker_generation: u64,
    pub target: Option<Target>,
    stop_requested: bool,
    cancel_requested: bool,
}

#[derive(Clone, Debug)]
pub enum SessionState {
    Idle,
    StartingCapture(Session),
    Recording(Session),
    StoppingCapture(Session),
    CancellingCapture(Session),
    Transcribing(Session),
    Delivering(Session),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    StartCapture(Uuid),
    StopCapture(Uuid),
    CancelCapture(Uuid),
    Deliver(Uuid),
    NeedsEngine,
    Busy,
    None,
}

#[derive(Clone, Debug)]
pub struct LastResult {
    pub session_id: Uuid,
    pub raw: String,
    pub final_text: String,
}

pub struct Coordinator {
    pub engine: EngineState,
    pub session: SessionState,
    pub last_result: Option<LastResult>,
    pub worker_generation: u64,
    pub revision: u64,
}

impl Default for Coordinator {
    fn default() -> Self {
        Self {
            engine: EngineState::MissingModel,
            session: SessionState::Idle,
            last_result: None,
            worker_generation: 0,
            revision: 0,
        }
    }
}

impl Coordinator {
    pub fn on_f8(&mut self, target: Option<Target>) -> Action {
        let action = match &mut self.session {
            SessionState::Idle if self.engine != EngineState::Ready => Action::NeedsEngine,
            SessionState::Idle => {
                let session = Session {
                    id: Uuid::new_v4(),
                    worker_generation: self.worker_generation,
                    target,
                    stop_requested: false,
                    cancel_requested: false,
                };
                let id = session.id;
                self.session = SessionState::StartingCapture(session);
                Action::StartCapture(id)
            }
            SessionState::StartingCapture(session) => {
                if !session.stop_requested {
                    self.revision += 1;
                }
                session.stop_requested = true;
                Action::None
            }
            SessionState::Recording(session) => {
                let id = session.id;
                self.session = SessionState::StoppingCapture(session.clone());
                Action::StopCapture(id)
            }
            _ => Action::Busy,
        };
        if matches!(action, Action::StartCapture(_) | Action::StopCapture(_)) {
            self.revision += 1;
        }
        action
    }

    pub fn capture_started(&mut self, id: Uuid) -> Action {
        let SessionState::StartingCapture(session) = &self.session else {
            return Action::None;
        };
        if session.id != id {
            return Action::None;
        }
        let session = session.clone();
        let action = if session.cancel_requested {
            self.session = SessionState::CancellingCapture(session);
            Action::CancelCapture(id)
        } else if session.stop_requested {
            self.session = SessionState::StoppingCapture(session);
            Action::StopCapture(id)
        } else {
            self.session = SessionState::Recording(session);
            Action::None
        };
        self.revision += 1;
        action
    }

    pub fn capture_auto_stop(&mut self, id: Uuid) -> bool {
        let SessionState::Recording(session) = &self.session else {
            return false;
        };
        if session.id != id {
            return false;
        }
        self.session = SessionState::StoppingCapture(session.clone());
        self.revision += 1;
        true
    }

    pub fn capture_failed(&mut self, id: Uuid) -> bool {
        let matches = match &self.session {
            SessionState::StartingCapture(session)
            | SessionState::Recording(session)
            | SessionState::StoppingCapture(session)
            | SessionState::CancellingCapture(session) => session.id == id,
            _ => false,
        };
        if matches {
            self.session = SessionState::Idle;
            self.revision += 1;
        }
        matches
    }

    pub fn capture_stopped(&mut self, id: Uuid, has_audio: bool) -> bool {
        let SessionState::StoppingCapture(session) = &self.session else {
            return false;
        };
        if session.id != id {
            return false;
        }
        self.session = if has_audio {
            SessionState::Transcribing(session.clone())
        } else {
            SessionState::Idle
        };
        self.revision += 1;
        true
    }

    pub fn recognized(
        &mut self,
        id: Uuid,
        generation: u64,
        raw: String,
        final_text: String,
    ) -> Action {
        let SessionState::Transcribing(session) = &self.session else {
            return Action::None;
        };
        if session.id != id || session.worker_generation != generation {
            return Action::None;
        }
        if final_text.trim().is_empty() {
            self.session = SessionState::Idle;
            self.revision += 1;
            return Action::None;
        }
        self.last_result = Some(LastResult {
            session_id: id,
            raw,
            final_text,
        });
        self.session = SessionState::Delivering(session.clone());
        self.revision += 1;
        Action::Deliver(id)
    }

    pub fn delivery_done(&mut self, id: Uuid) {
        if let SessionState::Delivering(session) = &self.session {
            if session.id == id {
                self.session = SessionState::Idle;
                self.revision += 1;
            }
        }
    }

    pub fn cancel(&mut self) {
        if !matches!(self.session, SessionState::Idle) {
            self.session = SessionState::Idle;
            self.worker_generation += 1;
            self.revision += 1;
        }
    }

    pub fn request_cancel(&mut self) -> Action {
        let action = match &mut self.session {
            SessionState::Idle | SessionState::CancellingCapture(_) => Action::None,
            SessionState::StartingCapture(session) => {
                session.cancel_requested = true;
                Action::None
            }
            SessionState::Recording(session) | SessionState::StoppingCapture(session) => {
                let session = session.clone();
                let id = session.id;
                self.session = SessionState::CancellingCapture(session);
                Action::CancelCapture(id)
            }
            SessionState::Transcribing(_) => {
                self.cancel();
                Action::None
            }
            SessionState::Delivering(_) => Action::Busy,
        };
        if matches!(action, Action::CancelCapture(_)) {
            self.revision += 1;
        }
        action
    }

    pub fn can_repaste(&self) -> bool {
        matches!(self.session, SessionState::Idle) && self.last_result.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f8_during_start_stops_after_start_completes() {
        let mut coordinator = Coordinator {
            engine: EngineState::Ready,
            ..Default::default()
        };
        let Action::StartCapture(id) = coordinator.on_f8(None) else {
            panic!("expected start")
        };
        assert_eq!(coordinator.on_f8(None), Action::None);
        assert_eq!(coordinator.capture_started(id), Action::StopCapture(id));
        assert!(matches!(
            coordinator.session,
            SessionState::StoppingCapture(_)
        ));
    }

    #[test]
    fn a_burst_of_f8_presses_never_starts_a_second_session_or_queues_work() {
        // Acceptance A01: auto-repeat or hammering F8 must give one session, one stop.
        let mut coordinator = Coordinator {
            engine: EngineState::Ready,
            ..Default::default()
        };
        let Action::StartCapture(id) = coordinator.on_f8(None) else {
            panic!("expected start")
        };
        for _ in 0..5 {
            assert_eq!(coordinator.on_f8(None), Action::None);
        }
        assert_eq!(coordinator.capture_started(id), Action::StopCapture(id));
        // Stopping, recognizing and delivering all refuse new work instead of queuing it.
        for _ in 0..3 {
            assert_eq!(coordinator.on_f8(None), Action::Busy);
        }
        assert!(coordinator.capture_stopped(id, true));
        assert_eq!(coordinator.on_f8(None), Action::Busy);
        let generation = coordinator.worker_generation;
        assert_eq!(
            coordinator.recognized(id, generation, "raw".into(), "final".into()),
            Action::Deliver(id)
        );
        assert_eq!(coordinator.on_f8(None), Action::Busy);
        coordinator.delivery_done(id);
        // Only now may the next recording begin, as a different session.
        let Action::StartCapture(next) = coordinator.on_f8(None) else {
            panic!("expected a new start after completion")
        };
        assert_ne!(next, id);
    }

    #[test]
    fn automatic_limit_stops_active_capture_once() {
        let mut coordinator = Coordinator {
            engine: EngineState::Ready,
            ..Default::default()
        };
        let Action::StartCapture(id) = coordinator.on_f8(None) else {
            panic!("expected start")
        };
        coordinator.capture_started(id);
        assert!(!coordinator.capture_auto_stop(Uuid::new_v4()));
        assert!(coordinator.capture_auto_stop(id));
        assert!(!coordinator.capture_auto_stop(id));
        coordinator.capture_stopped(id, true);
        assert!(matches!(coordinator.session, SessionState::Transcribing(_)));
    }

    #[test]
    fn start_failure_returns_to_idle_without_erasing_last_result() {
        let mut coordinator = Coordinator {
            engine: EngineState::Ready,
            last_result: Some(LastResult {
                session_id: Uuid::new_v4(),
                raw: "前".into(),
                final_text: "前".into(),
            }),
            ..Default::default()
        };
        let Action::StartCapture(id) = coordinator.on_f8(None) else {
            panic!("expected start")
        };
        assert!(!coordinator.capture_failed(Uuid::new_v4()));
        assert!(coordinator.capture_failed(id));
        assert!(matches!(coordinator.session, SessionState::Idle));
        assert!(coordinator.can_repaste());
        assert!(!coordinator.capture_failed(id));
    }

    #[test]
    fn stale_response_never_replaces_last_result() {
        let mut coordinator = Coordinator {
            engine: EngineState::Ready,
            ..Default::default()
        };
        let Action::StartCapture(id) = coordinator.on_f8(None) else {
            panic!("expected start")
        };
        coordinator.capture_started(id);
        coordinator.on_f8(None);
        coordinator.capture_stopped(id, true);
        coordinator.cancel();
        assert_eq!(
            coordinator.recognized(id, 0, "raw".into(), "text".into()),
            Action::None
        );
        assert!(coordinator.last_result.is_none());
    }

    #[test]
    fn empty_result_keeps_previous_repaste_text() {
        let mut coordinator = Coordinator {
            engine: EngineState::Ready,
            ..Default::default()
        };
        let previous = LastResult {
            session_id: Uuid::new_v4(),
            raw: "前".into(),
            final_text: "前".into(),
        };
        coordinator.last_result = Some(previous);
        let Action::StartCapture(id) = coordinator.on_f8(None) else {
            panic!("expected start")
        };
        coordinator.capture_started(id);
        coordinator.on_f8(None);
        coordinator.capture_stopped(id, true);
        assert_eq!(
            coordinator.recognized(id, 0, "".into(), "".into()),
            Action::None
        );
        assert_eq!(coordinator.last_result.as_ref().unwrap().final_text, "前");
        assert!(coordinator.can_repaste());
    }

    #[test]
    fn cancel_during_start_waits_for_capture_to_open_then_closes_it() {
        let mut coordinator = Coordinator {
            engine: EngineState::Ready,
            ..Default::default()
        };
        let Action::StartCapture(id) = coordinator.on_f8(None) else {
            panic!("expected start")
        };
        assert_eq!(coordinator.request_cancel(), Action::None);
        assert_eq!(coordinator.capture_started(id), Action::CancelCapture(id));
        assert!(matches!(
            coordinator.session,
            SessionState::CancellingCapture(_)
        ));
        assert!(!coordinator.capture_stopped(id, true));
        assert!(coordinator.capture_failed(id));
        assert!(matches!(coordinator.session, SessionState::Idle));
        assert!(matches!(coordinator.on_f8(None), Action::StartCapture(_)));
    }

    #[test]
    fn cancel_after_stop_discards_audio_before_recognition() {
        let mut coordinator = Coordinator {
            engine: EngineState::Ready,
            ..Default::default()
        };
        let Action::StartCapture(id) = coordinator.on_f8(None) else {
            panic!("expected start")
        };
        assert_eq!(coordinator.capture_started(id), Action::None);
        assert_eq!(coordinator.on_f8(None), Action::StopCapture(id));
        assert_eq!(coordinator.request_cancel(), Action::CancelCapture(id));
        assert!(!coordinator.capture_stopped(id, true));
        assert!(coordinator.capture_failed(id));
        assert_eq!(
            coordinator.recognized(id, 0, "raw".into(), "text".into()),
            Action::None
        );
        assert!(coordinator.last_result.is_none());
    }

    #[test]
    fn cancel_during_recognition_invalidates_late_result() {
        let mut coordinator = Coordinator {
            engine: EngineState::Ready,
            ..Default::default()
        };
        let Action::StartCapture(id) = coordinator.on_f8(None) else {
            panic!("expected start")
        };
        coordinator.capture_started(id);
        coordinator.on_f8(None);
        assert!(coordinator.capture_stopped(id, true));
        assert_eq!(coordinator.request_cancel(), Action::None);
        assert_eq!(coordinator.worker_generation, 1);
        assert_eq!(
            coordinator.recognized(id, 0, "raw".into(), "text".into()),
            Action::None
        );
        assert!(coordinator.last_result.is_none());
    }
}
