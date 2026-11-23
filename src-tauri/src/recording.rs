use crate::audio::{
    AudioCapture, AudioError, AudioTap, CaptureStopTimings, CapturedAudio, MAX_CAPTURE_SECONDS,
};
use crate::diagnostics::BackendKind;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

const POLL_INTERVAL: Duration = Duration::from_millis(20);
const COMMAND_CAPACITY: usize = 4;
const EVENT_CAPACITY: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureStopReason {
    User,
    MaxDuration,
    Fault,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureDispatchError {
    Busy,
    Disconnected,
}

#[derive(Clone, Debug)]
pub struct CaptureDiagnostics {
    pub callbacks: usize,
    pub frames: usize,
    pub startup_xruns: usize,
    pub stream_error: Option<String>,
    pub stream_error_kind: Option<BackendKind>,
    pub fault_position: Option<(usize, u64, usize)>,
    pub timings: CaptureStopTimings,
}

pub enum CaptureEvent {
    Started(Uuid),
    /// The saved microphone was not present; this recording uses the system default.
    PreferredDeviceMissing(Uuid),
    StartFailed(Uuid, AudioError),
    Stopped {
        session_id: Uuid,
        reason: CaptureStopReason,
        diagnostics: CaptureDiagnostics,
        result: Result<CapturedAudio, AudioError>,
    },
    Cancelled(Uuid),
    Rejected(Uuid),
}

enum Command {
    /// Session id and the preferred device id (None = system default).
    Start(Uuid, Option<String>),
    Stop(Uuid),
    Cancel(Uuid),
}

pub struct CaptureService {
    commands: SyncSender<Command>,
    events: Receiver<CaptureEvent>,
    level_bits: Arc<AtomicU32>,
    callbacks: Arc<AtomicUsize>,
    frames: Arc<AtomicUsize>,
    /// The recording in progress, for reading its audio while it continues.
    tap: Arc<Mutex<Option<AudioTap>>>,
}

impl CaptureService {
    pub fn spawn() -> std::io::Result<Self> {
        Self::spawn_with_duration(Duration::from_secs(MAX_CAPTURE_SECONDS))
    }

    pub fn spawn_with_duration(max_duration: Duration) -> std::io::Result<Self> {
        crate::audio::prepare_capture_priority();
        if !(Duration::from_millis(300)..=Duration::from_secs(MAX_CAPTURE_SECONDS))
            .contains(&max_duration)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "capture duration must be between 300 ms and 300 seconds",
            ));
        }
        let (commands, incoming) = mpsc::sync_channel(COMMAND_CAPACITY);
        let (outgoing, events) = mpsc::sync_channel(EVENT_CAPACITY);
        let level_bits = Arc::new(AtomicU32::new(0));
        let worker_level = level_bits.clone();
        let callbacks = Arc::new(AtomicUsize::new(0));
        let frames = Arc::new(AtomicUsize::new(0));
        let worker_callbacks = callbacks.clone();
        let worker_frames = frames.clone();
        let tap: Arc<Mutex<Option<AudioTap>>> = Arc::new(Mutex::new(None));
        let worker_tap = tap.clone();
        thread::Builder::new()
            .name("voiceweave-audio".into())
            .spawn(move || {
                run_capture_thread(
                    incoming,
                    outgoing,
                    worker_level,
                    worker_callbacks,
                    worker_frames,
                    worker_tap,
                    max_duration,
                )
            })?;
        Ok(Self {
            commands,
            events,
            level_bits,
            callbacks,
            frames,
            tap,
        })
    }

    /// Read access to the recording in progress, if any.
    pub fn tap(&self) -> Option<AudioTap> {
        self.tap.lock().ok().and_then(|slot| slot.clone())
    }

    pub fn start(&self, session_id: Uuid) -> Result<(), CaptureDispatchError> {
        self.send(Command::Start(session_id, None))
    }

    /// Start on the preferred microphone, falling back to the system default.
    pub fn start_with(
        &self,
        session_id: Uuid,
        preferred: Option<String>,
    ) -> Result<(), CaptureDispatchError> {
        self.send(Command::Start(session_id, preferred))
    }

    pub fn stop(&self, session_id: Uuid) -> Result<(), CaptureDispatchError> {
        self.send(Command::Stop(session_id))
    }

    pub fn cancel(&self, session_id: Uuid) -> Result<(), CaptureDispatchError> {
        self.send(Command::Cancel(session_id))
    }

    pub fn recv_timeout(&self, duration: Duration) -> Result<CaptureEvent, RecvTimeoutError> {
        self.events.recv_timeout(duration)
    }

    pub fn try_recv(&self) -> Result<CaptureEvent, TryRecvError> {
        self.events.try_recv()
    }

    pub fn level(&self) -> f32 {
        f32::from_bits(self.level_bits.load(Ordering::Acquire))
    }

    pub fn capture_counts(&self) -> (usize, usize) {
        (
            self.callbacks.load(Ordering::Acquire),
            self.frames.load(Ordering::Acquire),
        )
    }

    fn send(&self, command: Command) -> Result<(), CaptureDispatchError> {
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                TrySendError::Full(_) => CaptureDispatchError::Busy,
                TrySendError::Disconnected(_) => CaptureDispatchError::Disconnected,
            })
    }
}

fn finish(
    session_id: Uuid,
    capture: AudioCapture,
    requested_reason: CaptureStopReason,
    max_duration: Duration,
    outgoing: &SyncSender<CaptureEvent>,
    level_bits: &AtomicU32,
) -> bool {
    let reason = if capture.has_capture_fault() {
        CaptureStopReason::Fault
    } else if capture.max_duration_reached() {
        CaptureStopReason::MaxDuration
    } else {
        requested_reason
    };
    let (callbacks, frames) = capture.capture_counts();
    let startup_xruns = capture.startup_xrun_count();
    let stream_error = capture.capture_error();
    let stream_error_kind = capture.capture_error_kind();
    let fault_position = capture.fault_position();
    let (result, timings) = capture.stop_measured();
    let diagnostics = CaptureDiagnostics {
        callbacks,
        frames,
        startup_xruns,
        stream_error,
        stream_error_kind,
        fault_position,
        timings,
    };
    let result = result.and_then(|audio| {
        if reason == CaptureStopReason::MaxDuration {
            let expected = (max_duration.as_secs_f64() * 16_000.0).round() as usize;
            let tolerance = (expected / 100).clamp(800, 16_000);
            if audio.samples.len().saturating_add(tolerance) < expected {
                return Err(AudioError::CaptureFailed);
            }
        }
        Ok(audio)
    });
    level_bits.store(0, Ordering::Release);
    outgoing
        .send(CaptureEvent::Stopped {
            session_id,
            reason,
            diagnostics,
            result,
        })
        .is_ok()
}

fn run_capture_thread(
    incoming: Receiver<Command>,
    outgoing: SyncSender<CaptureEvent>,
    level_bits: Arc<AtomicU32>,
    callbacks: Arc<AtomicUsize>,
    frames: Arc<AtomicUsize>,
    tap: Arc<Mutex<Option<AudioTap>>>,
    max_duration: Duration,
) {
    let mut active: Option<(Uuid, AudioCapture)> = None;
    let mut started_at: Option<Instant> = None;
    loop {
        // Publish or withdraw the running recording for reading while it continues.
        if let Ok(mut slot) = tap.lock() {
            if active.is_some() != slot.is_some() {
                *slot = active.as_ref().map(|(_, capture)| capture.tap());
            }
        }
        let command = if active.is_some() {
            match incoming.recv_timeout(POLL_INTERVAL) {
                Ok(command) => Some(command),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match incoming.recv() {
                Ok(command) => Some(command),
                Err(_) => break,
            }
        };

        match command {
            Some(Command::Start(session_id, preferred)) if active.is_none() => {
                match AudioCapture::start_preferred(preferred.as_deref()) {
                    Ok((capture, fell_back)) => {
                        if fell_back
                            && outgoing
                                .send(CaptureEvent::PreferredDeviceMissing(session_id))
                                .is_err()
                        {
                            break;
                        }
                        active = Some((session_id, capture));
                        started_at = Some(Instant::now());
                        callbacks.store(0, Ordering::Release);
                        frames.store(0, Ordering::Release);
                        if outgoing.send(CaptureEvent::Started(session_id)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        if outgoing
                            .send(CaptureEvent::StartFailed(session_id, error))
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
            Some(Command::Stop(session_id)) => {
                if active.as_ref().is_some_and(|(id, _)| *id == session_id) {
                    let (_, capture) = active.take().expect("matching active capture");
                    started_at = None;
                    if !finish(
                        session_id,
                        capture,
                        CaptureStopReason::User,
                        max_duration,
                        &outgoing,
                        &level_bits,
                    ) {
                        break;
                    }
                } else if outgoing.send(CaptureEvent::Rejected(session_id)).is_err() {
                    break;
                }
            }
            Some(Command::Cancel(session_id)) => {
                if active.as_ref().is_some_and(|(id, _)| *id == session_id) {
                    let (_, capture) = active.take().expect("matching active capture");
                    started_at = None;
                    capture.cancel();
                    level_bits.store(0, Ordering::Release);
                    if outgoing.send(CaptureEvent::Cancelled(session_id)).is_err() {
                        break;
                    }
                } else if outgoing.send(CaptureEvent::Rejected(session_id)).is_err() {
                    break;
                }
            }
            Some(Command::Start(session_id, _)) => {
                let sent = outgoing.send(CaptureEvent::Rejected(session_id));
                if sent.is_err() {
                    break;
                }
            }
            None => {}
        }

        if let Some((session_id, capture)) = active.as_ref() {
            level_bits.store(capture.level().to_bits(), Ordering::Release);
            let (callback_count, frame_count) = capture.capture_counts();
            callbacks.store(callback_count, Ordering::Release);
            frames.store(frame_count, Ordering::Release);
            let time_limit_reached =
                started_at.is_some_and(|start| start.elapsed() >= max_duration);
            if capture.has_capture_fault() || capture.max_duration_reached() || time_limit_reached {
                let session_id = *session_id;
                let reason = if capture.has_capture_fault() {
                    CaptureStopReason::Fault
                } else {
                    CaptureStopReason::MaxDuration
                };
                let (_, capture) = active.take().expect("active capture at termination");
                started_at = None;
                if !finish(
                    session_id,
                    capture,
                    reason,
                    max_duration,
                    &outgoing,
                    &level_bits,
                ) {
                    break;
                }
            }
        }
    }
    if let Some((_, capture)) = active {
        capture.cancel();
    }
    level_bits.store(0, Ordering::Release);
}
