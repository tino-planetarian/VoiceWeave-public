//! Bounded, in-memory diagnostics. This boundary accepts no speech, paths,
//! window metadata, dictionary entries, or arbitrary error messages.
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Instant;
use uuid::Uuid;

pub fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticModel {
    Standard,
    Accurate,
    DevTiny,
    External,
    /// Recognized on Groq (the cloud mode).
    Cloud,
    Parakeet,
    /// Recognized by the English shortcut on this PC (Parakeet English).
    ParakeetEn,
    Unknown,
}

impl DiagnosticModel {
    pub fn from_preset(preset: Option<&str>) -> Self {
        match preset {
            Some("standard") => Self::Standard,
            Some("accurate") => Self::Accurate,
            Some("dev-tiny") => Self::DevTiny,
            Some("external") => Self::External,
            Some("parakeet-ja") => Self::Parakeet,
            Some("parakeet-en") => Self::ParakeetEn,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    SentUnverified,
    CopiedOnly,
    Empty,
    Cancelled,
    Failed,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FailureStage {
    CaptureStart,
    CaptureStop,
    CaptureStream,
    Overlay,
    Worker,
    Delivery,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FailureCode {
    NoInputDevice,
    UnsupportedFormat,
    AudioConfig,
    AudioBuild,
    AudioPlay,
    AudioStream,
    AudioOverflow,
    AudioTooShort,
    AudioConversion,
    AudioDispatch,
    AudioDisconnected,
    OverlayUnavailable,
    WorkerLaunch,
    WorkerTimeout,
    WorkerIo,
    WorkerProtocol,
    WorkerRemote,
    WorkerNotReady,
    WorkerAudioFormat,
    WorkerAudioLength,
    WorkerAudioValues,
    WorkerModelMissing,
    WorkerInvalidAudio,
    WorkerTermination,
    ClipboardUnavailable,
    SendFailed,
    DeliveryTask,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    DeviceBusy,
    DeviceChanged,
    DeviceNotAvailable,
    HostUnavailable,
    InvalidInput,
    PermissionDenied,
    RealtimeDenied,
    ResourceExhausted,
    StreamInvalidated,
    UnsupportedConfig,
    UnsupportedOperation,
    Xrun,
    Other,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct FailureReport {
    pub attempt: u64,
    pub stage: FailureStage,
    pub code: FailureCode,
    pub backend_kind: Option<BackendKind>,
    /// Where a capture stream failed. Counts and milliseconds only.
    pub fault_position: Option<FaultPosition>,
    /// The PC's memory when it failed, to tell a memory shortage (other programs, a
    /// large local model) from other causes of lost audio.
    pub memory: Option<MemoryState>,
    /// The GPU's memory when recognition failed (recognition failures only).
    pub gpu: Option<crate::gpu::GpuMemory>,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub struct MemoryState {
    /// Percent of physical memory in use.
    pub load_percent: u32,
    pub available_mb: u64,
    /// Committed memory (RAM plus page file) in use, in percent of the limit.
    pub commit_percent: u32,
}

impl MemoryState {
    pub fn now() -> Option<Self> {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut status = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 || status.ullTotalPageFile == 0 {
            return None;
        }
        let used_commit = status.ullTotalPageFile - status.ullAvailPageFile;
        Some(Self {
            load_percent: status.dwMemoryLoad,
            available_mb: status.ullAvailPhys / (1024 * 1024),
            commit_percent: (used_commit * 100 / status.ullTotalPageFile) as u32,
        })
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub struct FaultPosition {
    /// Data callbacks delivered before the fatal error (0 = before any audio arrived).
    pub callback: usize,
    /// Milliseconds of audio captured before the fatal error.
    pub audio_ms: u64,
    /// All Xruns reported during the recording, including a tolerated start-up one.
    pub xruns: usize,
}

/// Which recognition path produced the text. Fixed values only.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecognitionPath {
    /// Encoder ran on the recorded length only.
    Short,
    /// Normal 30 s window (fast path disabled).
    Full,
    /// Fast path was doubted, so the normal 30 s window decided.
    ShortFallback,
}

impl RecognitionPath {
    pub fn parse(value: Option<&str>) -> Option<Self> {
        match value? {
            "short" => Some(Self::Short),
            "full" => Some(Self::Full),
            "short_fallback" => Some(Self::ShortFallback),
            _ => None,
        }
    }
}

/// Why the fast path was not trusted. Fixed values from the worker only.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FallbackReason {
    Empty,
    TooLong,
    TokenLimit,
    Compression,
    LowConfidence,
    NoSpeech,
    Repeat,
    /// More characters than the speech could hold (a repeated or invented passage).
    TooDense,
    Exception,
}

impl FallbackReason {
    pub fn parse(value: Option<&str>) -> Option<Self> {
        Some(match value? {
            "empty" => Self::Empty,
            "too_long" => Self::TooLong,
            "too_dense" => Self::TooDense,
            "token_limit" => Self::TokenLimit,
            "compression" => Self::Compression,
            "low_confidence" => Self::LowConfidence,
            "no_speech" => Self::NoSpeech,
            "repeat" => Self::Repeat,
            "exception" => Self::Exception,
            _ => return None,
        })
    }
}

#[derive(Clone, Default, Debug, Serialize, PartialEq, Eq)]
pub struct Timings {
    pub capture_start_ms: Option<u64>,
    pub audio_duration_ms: Option<u64>,
    pub stop_queue_ms: Option<u64>,
    pub stream_stop_ms: Option<u64>,
    pub conversion_ms: Option<u64>,
    pub worker_roundtrip_ms: Option<u64>,
    pub worker_total_ms: Option<u64>,
    pub worker_frontend_ms: Option<u64>,
    pub worker_decode_ms: Option<u64>,
    /// After a failed recognition: preparing the engine again and recognizing the whole
    /// recording again. Not part of `worker_roundtrip_ms`, which ends at the failure.
    pub retry_ms: Option<u64>,
    pub processing_ms: Option<u64>,
    pub delivery_ms: Option<u64>,
    pub stop_to_complete_ms: Option<u64>,
    /// Waiting for the local model's draft (polishing mode only).
    pub polish_ms: Option<u64>,
    /// Waiting for the local model's punctuation (that option only).
    pub punctuation_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct AttemptReport {
    pub attempt: u64,
    pub model: DiagnosticModel,
    pub outcome: Outcome,
    pub timings: Timings,
    pub startup_xruns: Option<usize>,
    pub capture_callbacks: Option<usize>,
    pub input_frames: Option<usize>,
    pub recognition_path: Option<RecognitionPath>,
    pub fallback_reason: Option<FallbackReason>,
    /// Hesitations removed (count only), when the user enabled filler removal.
    pub fillers_removed: Option<usize>,
    /// Kanji numbers written as digits and restatements shortened (counts only).
    pub numbers_tidied: Option<usize>,
    pub restatements_fixed: Option<usize>,
    /// Polishing mode only: whether a draft was made, or why the transcript was used.
    pub polish: Option<crate::polish::PolishResult>,
    /// Local-model punctuation: used, or why the rules were used instead.
    pub punctuation: Option<crate::polish::PolishResult>,
    /// A stock subtitle line ("ご視聴ありがとうございました") was removed from the end.
    pub subtitle_tail_removed: Option<bool>,
    /// A phrase repeated over and over (a decoding loop) was collapsed to one.
    pub repeats_collapsed: Option<bool>,
    /// Paragraph breaks added before topic words (None: not tried).
    pub paragraph_breaks: Option<usize>,
    /// Words read from the input window (that option only), shown so the user can check
    /// what was used; kept in memory only.
    pub screen_terms: Option<Vec<String>>,
    /// Characters of screen text given to the drafting model (polishing with that option).
    pub screen_chars: Option<usize>,
    /// Parts recognized while the user was speaking (None: the usual path).
    pub stream_parts: Option<usize>,
    /// Of those, parts recognized again the careful way after the quick way doubted them.
    pub stream_rescued: Option<usize>,
    /// Why recognition while speaking was given up (a fixed code), if it was.
    pub stream_failure: Option<String>,
    /// Parts (while speaking and the rest) recognized again with the glossary topic's terms.
    pub topic_passes: Option<usize>,
    /// Compose mode: characters of copied reference text used (0: none). None: not compose.
    pub compose_reference_chars: Option<usize>,
    /// Compose mode: the text composed just before was offered for revision.
    pub compose_revision: Option<bool>,
    /// Cloud drafts of this attempt: service, model and time only (never the text).
    pub cloud_drafts: Option<Vec<CloudDraft>>,
    /// Where the recognizer ran: "cuda" (GPU) or "cpu".
    pub device: Option<String>,
    /// The GPU's memory during the recording (read while the user spoke).
    pub gpu: Option<crate::gpu::GpuMemory>,
    /// The PC's memory when recording began and when processing finished: a slow
    /// recognition on a PC short of memory (parts of the model paged out) shows here.
    pub memory_start: Option<MemoryState>,
    pub memory_end: Option<MemoryState>,
    /// The local model (Ollama) this recording used for punctuation or the draft, if any.
    pub local_model: Option<String>,
    /// Recognition failed and the recording was recognized again after restarting the
    /// engine (None: no failure).
    pub recovery: Option<Recovery>,
    /// Live text: guesses of the part still being spoken, shown before it is confirmed.
    pub tentative_previews: Option<usize>,
    /// The slowest of those guesses.
    pub tentative_max_ms: Option<u64>,
    /// How loud the recording was (numbers only, never the sound): see `Loudness`.
    pub loudness: Option<Loudness>,
    /// Cloud recognition failed (why) and this PC recognized the recording instead.
    pub cloud_fallback: Option<crate::cloud::CloudError>,
    /// The Groq model that recognized the recording (cloud mode).
    pub cloud_model: Option<String>,
}

/// The loudest moment (dB below full scale, 0 = loudest possible) and the time that
/// sounded like a voice (louder than -45 dB). Tells a silent microphone from speech the
/// recognizer could not read.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub struct Loudness {
    pub peak_db: i32,
    pub voiced_ms: u64,
    pub audio_ms: u64,
}

/// Loudness of 16 kHz samples, in 20 ms steps.
pub fn loudness(samples: &[f32]) -> Loudness {
    const STEP: usize = 320;
    let voiced_rms = 10_f32.powf(-45.0 / 20.0);
    let peak = samples
        .iter()
        .fold(0_f32, |peak, sample| peak.max(sample.abs()));
    let voiced_steps = samples
        .chunks(STEP)
        .filter(|step| {
            let power = step.iter().map(|sample| sample * sample).sum::<f32>() / step.len() as f32;
            power.sqrt() > voiced_rms
        })
        .count();
    Loudness {
        peak_db: if peak > 0.0 {
            (20.0 * peak.log10()).floor().max(-100.0) as i32
        } else {
            -100
        },
        voiced_ms: voiced_steps as u64 * 20,
        audio_ms: samples.len() as u64 / 16,
    }
}

/// The last recording that gave no text, kept (like the last error) after later
/// attempts, since the user often looks only after several more.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub struct EmptyReport {
    pub attempt: u64,
    pub loudness: Option<Loudness>,
    pub startup_xruns: Option<usize>,
}

/// What happened after a recognition failure. Fixed values only.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Recovery {
    /// Recognized again from the start after restarting the engine.
    Retried,
    /// That failed too; the text recognized while speaking was copied.
    Salvaged,
    /// That failed too and nothing was recognized while speaking.
    Lost,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct CloudDraft {
    /// "openai" or "anthropic".
    pub service: String,
    pub model: String,
    pub ms: u64,
    pub ok: bool,
    /// Asked because the chosen service was slow (see `pipeline::fallback_after`).
    #[serde(default)]
    pub fallback: bool,
}

/// Per cloud model since start: how often it answered, and how fast.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct CloudStats {
    pub service: String,
    pub model: String,
    pub ok: u64,
    pub failed: u64,
    /// Times of successful drafts (the most recent ones only).
    pub time: Option<DurationStats>,
}

#[derive(Clone, Default, Debug, Serialize, PartialEq, Eq)]
pub struct DiagnosticsSnapshot {
    pub revision: u64,
    pub latest: Option<AttemptReport>,
    /// Deliberately retained after later successes, until process exit.
    pub last_error: Option<FailureReport>,
    /// Likewise the last recording that gave no text.
    pub last_empty: Option<EmptyReport>,
    /// Counts and duration statistics since start. No per-attempt records or text.
    pub summary: SessionSummary,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub struct ReasonCount {
    pub reason: FallbackReason,
    pub count: u64,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub struct DurationStats {
    /// Number of durations behind the statistics (the most recent ones only).
    pub count: usize,
    pub median_ms: u64,
    pub p90_ms: u64,
    pub max_ms: u64,
}

#[derive(Clone, Default, Debug, Serialize, PartialEq, Eq)]
pub struct SessionSummary {
    pub attempts: u64,
    /// Text was sent or copied.
    pub delivered: u64,
    pub failed: u64,
    /// Recordings that gave no text (not failures, but nothing was typed).
    pub empty: u64,
    pub fast_path: u64,
    pub fallback: u64,
    pub full_path: u64,
    pub fallback_reasons: Vec<ReasonCount>,
    /// Stop-to-complete over recent delivered attempts.
    pub stop_to_complete: Option<DurationStats>,
    /// Cloud polishing per model, for choosing between services.
    pub cloud: Vec<CloudStats>,
}

/// Only this many recent stop-to-complete durations are kept, as bare numbers.
const DURATION_WINDOW: usize = 50;

fn duration_stats(values: &VecDeque<u64>) -> Option<DurationStats> {
    let mut sorted: Vec<u64> = values.iter().copied().collect();
    sorted.sort_unstable();
    let last = *sorted.last()?;
    let at = |fraction: f64| sorted[((sorted.len() - 1) as f64 * fraction).round() as usize];
    Some(DurationStats {
        count: sorted.len(),
        median_ms: at(0.5),
        p90_ms: at(0.9),
        max_ms: last,
    })
}

#[derive(Default)]
struct StoreState {
    snapshot: DiagnosticsSnapshot,
    durations: VecDeque<u64>,
    /// Recent successful draft times per (service, model), in the order first seen.
    cloud_durations: Vec<((String, String), VecDeque<u64>)>,
}

#[derive(Default)]
pub struct DiagnosticsStore(Mutex<StoreState>);

impl DiagnosticsStore {
    pub fn snapshot(&self) -> DiagnosticsSnapshot {
        self.0.lock().expect("diagnostics lock").snapshot.clone()
    }

    pub fn completed(&self, report: AttemptReport) -> DiagnosticsSnapshot {
        let mut state = self.0.lock().expect("diagnostics lock");
        let delivered = matches!(
            report.outcome,
            Outcome::SentUnverified | Outcome::CopiedOnly
        );
        let summary = &mut state.snapshot.summary;
        summary.attempts += 1;
        if delivered {
            summary.delivered += 1;
        } else if report.outcome == Outcome::Failed {
            summary.failed += 1;
        } else if report.outcome == Outcome::Empty {
            summary.empty += 1;
        }
        match report.recognition_path {
            Some(RecognitionPath::Short) => summary.fast_path += 1,
            Some(RecognitionPath::ShortFallback) => summary.fallback += 1,
            Some(RecognitionPath::Full) => summary.full_path += 1,
            None => {}
        }
        if let Some(reason) = report.fallback_reason {
            match summary
                .fallback_reasons
                .iter_mut()
                .find(|entry| entry.reason == reason)
            {
                Some(entry) => entry.count += 1,
                None => summary
                    .fallback_reasons
                    .push(ReasonCount { reason, count: 1 }),
            }
        }
        if let (true, Some(ms)) = (delivered, report.timings.stop_to_complete_ms) {
            if state.durations.len() == DURATION_WINDOW {
                state.durations.pop_front();
            }
            state.durations.push_back(ms);
        }
        state.snapshot.summary.stop_to_complete = duration_stats(&state.durations);
        for draft in report.cloud_drafts.iter().flatten() {
            let key = (draft.service.clone(), draft.model.clone());
            let index = match state.cloud_durations.iter().position(|(k, _)| *k == key) {
                Some(index) => index,
                None => {
                    state.cloud_durations.push((key.clone(), VecDeque::new()));
                    state.snapshot.summary.cloud.push(CloudStats {
                        service: key.0,
                        model: key.1,
                        ok: 0,
                        failed: 0,
                        time: None,
                    });
                    state.cloud_durations.len() - 1
                }
            };
            let times = &mut state.cloud_durations[index].1;
            if draft.ok {
                if times.len() == DURATION_WINDOW {
                    times.pop_front();
                }
                times.push_back(draft.ms);
            }
            let time = duration_stats(times);
            let stats = &mut state.snapshot.summary.cloud[index];
            if draft.ok {
                stats.ok += 1;
            } else {
                stats.failed += 1;
            }
            stats.time = time;
        }
        if report.outcome == Outcome::Empty {
            state.snapshot.last_empty = Some(EmptyReport {
                attempt: report.attempt,
                loudness: report.loudness,
                startup_xruns: report.startup_xruns,
            });
        }
        state.snapshot.latest = Some(report);
        state.snapshot.revision += 1;
        state.snapshot.clone()
    }

    pub fn failed(&self, report: FailureReport) -> DiagnosticsSnapshot {
        let mut state = self.0.lock().expect("diagnostics lock");
        state.snapshot.last_error = Some(report);
        state.snapshot.revision += 1;
        state.snapshot.clone()
    }
}

/// Session IDs and monotonic timestamps stay inside Rust and are never exported.
pub struct SessionMetrics {
    pub id: Uuid,
    pub report: AttemptReport,
    began: Instant,
    stopped: Option<Instant>,
    /// Reading the GPU's memory in the background while the user speaks.
    pub gpu_job: Option<std::thread::JoinHandle<Option<crate::gpu::GpuMemory>>>,
}

impl SessionMetrics {
    pub fn new(id: Uuid, attempt: u64, model: DiagnosticModel, began: Instant) -> Self {
        Self {
            id,
            report: AttemptReport {
                attempt,
                model,
                outcome: Outcome::Failed,
                timings: Timings::default(),
                startup_xruns: None,
                capture_callbacks: None,
                input_frames: None,
                recognition_path: None,
                fallback_reason: None,
                fillers_removed: None,
                numbers_tidied: None,
                restatements_fixed: None,
                polish: None,
                punctuation: None,
                subtitle_tail_removed: None,
                repeats_collapsed: None,
                paragraph_breaks: None,
                screen_terms: None,
                screen_chars: None,
                stream_parts: None,
                stream_rescued: None,
                stream_failure: None,
                topic_passes: None,
                compose_reference_chars: None,
                compose_revision: None,
                cloud_drafts: None,
                device: None,
                gpu: None,
                memory_start: None,
                memory_end: None,
                local_model: None,
                recovery: None,
                tentative_previews: None,
                tentative_max_ms: None,
                loudness: None,
                cloud_fallback: None,
                cloud_model: None,
            },
            gpu_job: None,
            began,
            stopped: None,
        }
    }

    pub fn started(&mut self) {
        self.report.timings.capture_start_ms = Some(elapsed_ms(self.began));
        self.report.memory_start = MemoryState::now();
    }

    pub fn stop_requested(&mut self, at: Instant) {
        // A second press during StartingCapture must keep its original timestamp.
        self.stopped.get_or_insert(at);
    }

    pub fn capture_finished(&mut self, finish_started: Instant) {
        if let Some(stopped) = self.stopped {
            self.report.timings.stop_queue_ms = Some(
                finish_started
                    .saturating_duration_since(stopped)
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64,
            );
        } else {
            // Automatic/fault stop: there was no user F8 stop request.
            self.stopped = Some(finish_started);
        }
    }

    pub fn finish(mut self, outcome: Outcome) -> AttemptReport {
        self.report.outcome = outcome;
        self.report.memory_end = MemoryState::now();
        self.report.timings.stop_to_complete_ms = self.stopped.map(elapsed_ms);
        if let Some(job) = self.gpu_job.take().filter(|job| job.is_finished()) {
            self.report.gpu = job.join().ok().flatten();
        }
        self.report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn later_success_preserves_error_and_keeps_only_latest_attempt() {
        let store = DiagnosticsStore::default();
        let failure = FailureReport {
            attempt: 1,
            stage: FailureStage::CaptureStart,
            code: FailureCode::AudioBuild,
            backend_kind: Some(BackendKind::DeviceBusy),
            fault_position: None,
            memory: MemoryState::now(),
            gpu: None,
        };
        let memory = failure.memory.expect("Windows reports the memory state");
        assert!(memory.load_percent <= 100 && memory.commit_percent <= 100);
        store.failed(failure.clone());
        for attempt in 1..=20 {
            store.completed(
                SessionMetrics::new(
                    Uuid::new_v4(),
                    attempt,
                    DiagnosticModel::Standard,
                    Instant::now(),
                )
                .finish(Outcome::SentUnverified),
            );
        }
        let state = store.snapshot();
        assert_eq!(state.last_error, Some(failure));
        assert_eq!(state.latest.unwrap().attempt, 20);
        assert_eq!(state.revision, 21);
        let json = serde_json::to_value(store.snapshot()).unwrap();
        assert_eq!(json.as_object().unwrap().len(), 5);
        assert!(state.last_empty.is_none());
        assert!(json.get("history").is_none());
        assert_eq!(state.summary.attempts, 20);
        assert_eq!(state.summary.delivered, 20);
    }

    #[test]
    fn memory_is_read_when_recording_starts_and_when_processing_finishes() {
        let mut metrics =
            SessionMetrics::new(Uuid::new_v4(), 1, DiagnosticModel::Standard, Instant::now());
        assert!(metrics.report.memory_start.is_none() && metrics.report.memory_end.is_none());
        metrics.started();
        assert!(metrics.report.memory_start.is_some());
        assert!(metrics.report.memory_end.is_none());
        let report = metrics.finish(Outcome::SentUnverified);
        assert!(report.memory_start.is_some() && report.memory_end.is_some());
        let json = serde_json::to_value(&report).unwrap();
        for key in ["memory_start", "memory_end"] {
            let memory = json[key]
                .as_object()
                .expect("memory is exported as numbers");
            assert_eq!(memory.len(), 3);
        }
    }

    #[test]
    fn summary_counts_paths_reasons_and_bounded_duration_statistics() {
        let store = DiagnosticsStore::default();
        let report = |path, reason, outcome, ms| {
            let mut report =
                SessionMetrics::new(Uuid::new_v4(), 1, DiagnosticModel::Standard, Instant::now())
                    .finish(outcome);
            report.recognition_path = path;
            report.fallback_reason = reason;
            report.timings.stop_to_complete_ms = ms;
            report
        };
        for ms in 1..=60_u64 {
            store.completed(report(
                Some(RecognitionPath::Short),
                None,
                Outcome::SentUnverified,
                Some(ms * 10),
            ));
        }
        // An empty result is neither delivered nor failed, and its time is not counted.
        store.completed(report(
            Some(RecognitionPath::ShortFallback),
            Some(FallbackReason::NoSpeech),
            Outcome::Empty,
            Some(99_999),
        ));
        store.completed(report(
            Some(RecognitionPath::ShortFallback),
            Some(FallbackReason::NoSpeech),
            Outcome::Failed,
            None,
        ));
        let summary = store.snapshot().summary;
        assert_eq!(summary.attempts, 62);
        assert_eq!((summary.delivered, summary.failed), (60, 1));
        assert_eq!(summary.empty, 1);
        assert_eq!(
            store.snapshot().last_empty.map(|empty| empty.attempt),
            Some(1)
        );
        assert_eq!(
            (summary.fast_path, summary.fallback, summary.full_path),
            (60, 2, 0)
        );
        assert_eq!(
            summary.fallback_reasons,
            vec![ReasonCount {
                reason: FallbackReason::NoSpeech,
                count: 2
            }]
        );
        // Only the 50 most recent delivered durations (110..=600 ms) are kept.
        let stats = summary.stop_to_complete.unwrap();
        assert_eq!(stats.count, 50);
        assert_eq!(stats.max_ms, 600);
        assert_eq!(stats.median_ms, 360);
        assert_eq!(stats.p90_ms, 550);
    }

    #[test]
    fn cloud_drafts_are_summarized_per_model_with_times_of_successes_only() {
        let store = DiagnosticsStore::default();
        let draft = |service: &str, ms, ok| CloudDraft {
            service: service.into(),
            model: format!("{service}-model"),
            ms,
            ok,
            fallback: false,
        };
        for drafts in [
            vec![
                draft("openai", 1_800, true),
                draft("anthropic", 2_400, true),
            ],
            vec![
                draft("openai", 2_000, true),
                draft("anthropic", 60_000, false),
            ],
            vec![draft("openai", 1_600, true)],
        ] {
            let mut report =
                SessionMetrics::new(Uuid::new_v4(), 1, DiagnosticModel::Standard, Instant::now())
                    .finish(Outcome::SentUnverified);
            report.cloud_drafts = Some(drafts);
            store.completed(report);
        }
        let cloud = store.snapshot().summary.cloud;
        assert_eq!(cloud.len(), 2);
        let openai = &cloud[0];
        assert_eq!(
            (openai.service.as_str(), openai.ok, openai.failed),
            ("openai", 3, 0)
        );
        let time = openai.time.unwrap();
        assert_eq!((time.count, time.median_ms, time.max_ms), (3, 1_800, 2_000));
        let anthropic = &cloud[1];
        assert_eq!((anthropic.ok, anthropic.failed), (1, 1));
        // The failed attempt's time is not mixed into the speed.
        assert_eq!(anthropic.time.unwrap().max_ms, 2_400);
    }

    #[test]
    fn early_stop_counts_start_delay_and_auto_stop_has_no_queue_measurement() {
        let now = Instant::now();
        let mut metrics = SessionMetrics::new(Uuid::new_v4(), 1, DiagnosticModel::Unknown, now);
        let stop = now - Duration::from_millis(40);
        metrics.stop_requested(stop);
        metrics.stop_requested(now);
        metrics.capture_finished(now);
        let report = metrics.finish(Outcome::Cancelled);
        assert_eq!(report.timings.stop_queue_ms, Some(40));
        assert!(report.timings.stop_to_complete_ms.unwrap() >= 40);
        let mut automatic = SessionMetrics::new(Uuid::new_v4(), 2, DiagnosticModel::Unknown, now);
        automatic.capture_finished(now);
        let report = automatic.finish(Outcome::Failed);
        assert_eq!(report.timings.stop_queue_ms, None);
        assert!(report.timings.stop_to_complete_ms.is_some());
    }

    #[test]
    fn arbitrary_preset_never_reaches_diagnostics() {
        let marker = "private/path or dictionary content";
        let report = SessionMetrics::new(
            Uuid::new_v4(),
            1,
            DiagnosticModel::from_preset(Some(marker)),
            Instant::now(),
        )
        .finish(Outcome::Failed);
        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains(marker));
        assert!(!json.contains("session_id"));
        assert_eq!(report.model, DiagnosticModel::Unknown);
        assert_eq!(report.timings.stop_to_complete_ms, None);
    }

    #[test]
    fn loudness_tells_silence_from_a_voice() {
        let silent = loudness(&[0.0_f32; 16_000]);
        assert_eq!(
            (silent.peak_db, silent.voiced_ms, silent.audio_ms),
            (-100, 0, 1_000)
        );
        // Half a second of a tone at a tenth of full scale (-20 dB), then quiet.
        let mut samples: Vec<f32> = (0..8_000).map(|i| 0.1 * (i as f32 * 0.2).sin()).collect();
        samples.extend(std::iter::repeat_n(0.001_f32, 8_000));
        let voice = loudness(&samples);
        assert!((-21..=-20).contains(&voice.peak_db), "{}", voice.peak_db);
        assert_eq!(voice.voiced_ms, 500);
    }
}
