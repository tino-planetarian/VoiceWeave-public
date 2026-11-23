use crate::coordinator::{Action, Coordinator, EngineState, SessionState};
use crate::diagnostics::{
    self, BackendKind, DiagnosticModel, DiagnosticsStore, FailureCode, FailureReport, FailureStage,
    Outcome, SessionMetrics,
};
use crate::engine::{EngineSlot, WorkerBackend};
use crate::hotkeys::HotkeyEvent;
use crate::input::{self, CopiedReason, DeliveryOutcome, Target};
use crate::models::{self, ModelSelection};
use crate::overlay::RecordingOverlay;
use crate::recording::{CaptureEvent, CaptureService, CaptureStopReason};
use crate::settings::{Settings, SettingsStore};
use crate::EngineStatus;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::TryRecvError;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, oneshot};

type RuntimeEngine = EngineSlot<WorkerBackend>;

const LOADING: u8 = 0;
const IDLE: u8 = 1;
const STARTING: u8 = 2;
const RECORDING: u8 = 3;
const BUSY: u8 = 4;
const UNAVAILABLE: u8 = 5;
const DELIVERING: u8 = 6;

pub(crate) enum PipelineCommand {
    Hotkey(HotkeyEvent, Option<Target>, u64, Instant),
    CopyLastResult(u64),
    Cancel,
    RetryEngine,
    ReloadEngine,
    SaveSettings(
        Box<Settings>,
        u64,
        oneshot::Sender<Result<Settings, String>>,
    ),
}

#[derive(Clone)]
pub struct PipelineSender {
    sender: mpsc::Sender<PipelineCommand>,
    gate: Arc<AtomicU8>,
    cancellation: Arc<AtomicU64>,
    engine_status: Arc<Mutex<EngineStatus>>,
}

impl PipelineSender {
    pub(crate) fn new() -> (Self, mpsc::Receiver<PipelineCommand>) {
        let (sender, receiver) = mpsc::channel(4);
        (
            Self {
                sender,
                gate: Arc::new(AtomicU8::new(LOADING)),
                cancellation: Arc::new(AtomicU64::new(0)),
                engine_status: Arc::new(Mutex::new(EngineStatus {
                    enabled: true,
                    phase: "loading".into(),
                    active_preset: None,
                    external_override: std::env::var_os("VOICEWEAVE_MODEL_DIR").is_some(),
                    device: None,
                })),
            },
            receiver,
        )
    }

    pub fn gate(&self) -> Arc<AtomicU8> {
        self.gate.clone()
    }

    pub fn cancellation(&self) -> Arc<AtomicU64> {
        self.cancellation.clone()
    }

    /// Re-prepare the saved model, e.g. after the GPU runtime was installed.
    pub fn reload_engine(&self) {
        let _ = self.sender.try_send(PipelineCommand::ReloadEngine);
    }

    pub fn engine_status(&self) -> Arc<Mutex<EngineStatus>> {
        self.engine_status.clone()
    }

    pub fn status(&self) -> EngineStatus {
        let mut status = self
            .engine_status
            .lock()
            .expect("engine status lock")
            .clone();
        if self.gate.load(Ordering::Acquire) == UNAVAILABLE && status.phase == "ready" {
            status.phase = "unavailable".into();
            status.active_preset = None;
        }
        status
    }

    pub async fn save_settings(
        &self,
        settings: Settings,
        revision: u64,
    ) -> Result<Settings, String> {
        let previous = self.gate.load(Ordering::Acquire);
        if !matches!(previous, IDLE | UNAVAILABLE)
            || self
                .gate
                .compare_exchange(previous, LOADING, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return Err("処理中です。音声入力が終わってから設定を保存してください".into());
        }
        let (reply, receiver) = oneshot::channel();
        if self
            .sender
            .try_send(PipelineCommand::SaveSettings(
                Box::new(settings),
                revision,
                reply,
            ))
            .is_err()
        {
            let _ =
                self.gate
                    .compare_exchange(LOADING, previous, Ordering::AcqRel, Ordering::Acquire);
            return Err("設定要求を送れませんでした".into());
        }
        receiver
            .await
            .map_err(|_| "設定処理が終了しました。アプリを再起動してください".to_owned())?
    }

    pub fn dispatch(&self, app: &AppHandle, event: HotkeyEvent) {
        if event == HotkeyEvent::Cancel {
            self.cancel(app);
            return;
        }
        let received = Instant::now();
        let phase = self.gate.load(Ordering::Acquire);
        let allowed = match event {
            HotkeyEvent::Record
            | HotkeyEvent::Polish
            | HotkeyEvent::Compose
            | HotkeyEvent::English => {
                matches!(phase, IDLE | STARTING | RECORDING)
            }
            HotkeyEvent::Repaste => matches!(phase, IDLE | UNAVAILABLE),
            HotkeyEvent::Cancel => false,
        };
        if !allowed {
            let message = match phase {
                LOADING => "認識エンジンを準備中です",
                UNAVAILABLE => "認識エンジンを準備できませんでした",
                _ => "処理中です",
            };
            // Through notice(): F8 during start-up must be visible with settings closed.
            notice(app, message);
            return;
        }
        let generation = self.cancellation.load(Ordering::Acquire);
        let target = input::capture_target();
        if phase == IDLE {
            // A new session: typing is allowed again.
            COPY_ONLY.store(false, Ordering::Release);
        }
        if self
            .sender
            .try_send(PipelineCommand::Hotkey(event, target, generation, received))
            .is_err()
        {
            notice(app, "処理中です");
        } else if event != HotkeyEvent::Repaste && phase == RECORDING {
            // Switch immediately so the user sees that recording has stopped.
            crate::overlay::transcribing(app);
        }
    }

    pub fn cancel(&self, app: &AppHandle) {
        self.cancellation.fetch_add(1, Ordering::AcqRel);
        if self.gate.load(Ordering::Acquire) == DELIVERING {
            notice(app, "処理の取消を試みます。完了済みの操作は戻せません");
            return;
        }
        if self.sender.try_send(PipelineCommand::Cancel).is_err() {
            notice(
                app,
                "取消要求を送れませんでした。録音中なら再試行してください",
            );
        }
    }

    /// For the lock screen and wake-up: stop a recording and keep its words, but only on the
    /// clipboard (see COPY_ONLY); a recognition in progress ends the same way. Nothing
    /// happens when the app is idle. `message` is shown to the user.
    pub fn keep_after_lock(&self, app: &AppHandle, message: &'static str) -> bool {
        let phase = self.gate.load(Ordering::Acquire);
        if matches!(phase, IDLE | UNAVAILABLE | LOADING | DELIVERING) {
            return false;
        }
        // The result of this session is only copied, never typed: whoever unlocks the PC
        // later is not typing into whatever window comes back.
        COPY_ONLY.store(true, Ordering::Release);
        if matches!(phase, STARTING | RECORDING) {
            // Either key stops a recording; what was said so far is recognized and copied.
            let generation = self.cancellation.load(Ordering::Acquire);
            let stop =
                PipelineCommand::Hotkey(HotkeyEvent::Record, None, generation, Instant::now());
            if self.sender.try_send(stop).is_ok() {
                crate::overlay::transcribing(app);
            }
        }
        notice(app, message);
        true
    }

    pub fn retry_engine(&self, app: &AppHandle) {
        if !matches!(self.gate.load(Ordering::Acquire), IDLE | UNAVAILABLE) {
            notice(app, "処理中です");
            return;
        }
        if self.sender.try_send(PipelineCommand::RetryEngine).is_err() {
            notice(app, "処理中です");
        }
    }

    pub fn copy_last_result(&self, app: &AppHandle) {
        if !matches!(self.gate.load(Ordering::Acquire), IDLE | UNAVAILABLE) {
            notice(app, "処理中です");
            return;
        }
        let generation = self.cancellation.load(Ordering::Acquire);
        if self
            .sender
            .try_send(PipelineCommand::CopyLastResult(generation))
            .is_err()
        {
            notice(app, "処理中です");
        }
    }
}

fn update_gate(coordinator: &Coordinator, gate: &AtomicU8) {
    let phase = match &coordinator.session {
        SessionState::Idle if coordinator.engine == EngineState::Ready => IDLE,
        SessionState::Idle if coordinator.engine == EngineState::Loading => LOADING,
        SessionState::Idle => UNAVAILABLE,
        SessionState::StartingCapture(_) => STARTING,
        SessionState::Recording(_) => RECORDING,
        SessionState::Delivering(_) => DELIVERING,
        _ => BUSY,
    };
    gate.store(phase, Ordering::Release);
}

fn notice(app: &AppHandle, message: &'static str) {
    let _ = app.emit("user_notice", message);
    // The settings window is usually closed, so also show it briefly on screen.
    crate::overlay::flash(app, message);
}

/// A recording's outcome: shown once the recording indicator closes, never dropped
/// (ordinary notices are skipped while the indicator is shown).
fn outcome_notice(app: &AppHandle, message: &'static str) {
    let _ = app.emit("user_notice", message);
    crate::overlay::flash_outcome(app, message);
}

fn discard_queued(hotkeys: &mut mpsc::Receiver<PipelineCommand>) {
    while let Ok(command) = hotkeys.try_recv() {
        reject_settings(command);
    }
}

fn reject_settings(command: PipelineCommand) {
    if let PipelineCommand::SaveSettings(_, _, reply) = command {
        let _ = reply.send(Err("処理中です。設定は変更していません".into()));
    }
}

/// Whether a command must act during a wait for the worker in a recording (R-02): the
/// stop keys while recording, and the cancel key while recording or stopping. Everything
/// else waits for the main loop, which takes it right after the wait.
fn is_urgent(command: &PipelineCommand, session: &SessionState) -> bool {
    match command {
        PipelineCommand::Hotkey(
            HotkeyEvent::Record | HotkeyEvent::Polish | HotkeyEvent::Compose | HotkeyEvent::English,
            ..,
        ) => matches!(session, SessionState::Recording(_)),
        PipelineCommand::Cancel => matches!(
            session,
            SessionState::Recording(_) | SessionState::StoppingCapture(_)
        ),
        _ => false,
    }
}

/// Awaits the worker during a recording (recognition while speaking) while the stop and
/// cancel keys keep working: `urgent` acts on them at once, so the microphone stops when
/// the user asks, not when the worker answers (R-02). The worker's answer is still
/// awaited, never dropped: an abandoned request would leave its reply in the pipe for the
/// next one. Commands `urgent` hands back wait in `held`, in order, for the main loop.
async fn wait_for_worker<T>(
    work: impl std::future::Future<Output = T>,
    hotkeys: &mut mpsc::Receiver<PipelineCommand>,
    held: &mut std::collections::VecDeque<PipelineCommand>,
    mut urgent: impl FnMut(PipelineCommand) -> Option<PipelineCommand>,
) -> T {
    tokio::pin!(work);
    let mut open = true;
    loop {
        tokio::select! {
            biased;
            result = &mut work => return result,
            command = hotkeys.recv(), if open => match command {
                Some(command) => held.extend(urgent(command)),
                // The main loop sees the closed channel after the wait.
                None => open = false,
            },
        }
    }
}

/// The next command for the main loop: those held during a wait come first.
async fn next_command(
    held: &mut std::collections::VecDeque<PipelineCommand>,
    hotkeys: &mut mpsc::Receiver<PipelineCommand>,
) -> Option<PipelineCommand> {
    match held.pop_front() {
        Some(command) => Some(command),
        None => hotkeys.recv().await,
    }
}

/// A microphone that would not stop or cancel during a wait (handled after it).
struct CaptureFailure {
    id: uuid::Uuid,
    message: &'static str,
}

/// What the stop and cancel keys do during a wait for the worker: the same as in the main
/// loop. Other commands are handed back to wait for the main loop.
struct WaitKeys<'a> {
    app: &'a AppHandle,
    coordinator: &'a mut Coordinator,
    capture: &'a Mutex<CaptureService>,
    overlay: &'a RecordingOverlay,
    metrics: &'a mut Option<SessionMetrics>,
    gate: &'a AtomicU8,
    failure: Option<CaptureFailure>,
}

impl WaitKeys<'_> {
    fn act(&mut self, command: PipelineCommand) -> Option<PipelineCommand> {
        if !is_urgent(&command, &self.coordinator.session) {
            return Some(command);
        }
        match command {
            PipelineCommand::Hotkey(_, target, _, received) => {
                if let Some(metrics) = self.metrics.as_mut() {
                    metrics.stop_requested(received);
                }
                if let Action::StopCapture(id) = self.coordinator.on_f8(target) {
                    self.overlay.transcribing();
                    if self.capture.lock().expect("capture lock").stop(id).is_err() {
                        self.failure = Some(CaptureFailure {
                            id,
                            message: "録音を停止できませんでした",
                        });
                    }
                }
            }
            _ => {
                if let Action::CancelCapture(id) = self.coordinator.request_cancel() {
                    self.overlay.hide();
                    if self
                        .capture
                        .lock()
                        .expect("capture lock")
                        .cancel(id)
                        .is_err()
                    {
                        self.failure = Some(CaptureFailure {
                            id,
                            message: "録音を取り消せませんでした",
                        });
                    } else {
                        notice(self.app, "録音を取り消しています");
                    }
                }
            }
        }
        update_gate(self.coordinator, self.gate);
        None
    }

    /// After the wait: a microphone that would not stop ends the pipeline, as in the main
    /// loop. Returns whether it did.
    fn failed(self) -> bool {
        let Some(failure) = self.failure else {
            return false;
        };
        self.coordinator.cancel();
        self.coordinator.engine = EngineState::Unavailable;
        self.gate.store(UNAVAILABLE, Ordering::Release);
        notice(self.app, failure.message);
        diagnostic_failure(
            self.app,
            self.metrics,
            FailureStage::CaptureStop,
            FailureCode::AudioDispatch,
            None,
        );
        finish_metrics(self.app, self.metrics, failure.id, Outcome::Failed);
        true
    }
}

fn discard_after_settings(
    coordinator: &Coordinator,
    commands: &mut mpsc::Receiver<PipelineCommand>,
) {
    // A recording may have won the race against the settings reservation.
    // Its queued Stop/Cancel must still run when the settings request is rejected.
    if matches!(coordinator.session, SessionState::Idle) {
        discard_queued(commands);
    }
}

fn publish_engine(
    app: &AppHandle,
    coordinator: &mut Coordinator,
    engine: &RuntimeEngine,
    status: &Mutex<EngineStatus>,
) {
    coordinator.worker_generation = engine.generation();
    let phase = match coordinator.engine {
        EngineState::Loading => "loading",
        EngineState::MissingModel => "missing_model",
        EngineState::Ready => "ready",
        EngineState::Unavailable => "unavailable",
    };
    let mut current = status.lock().expect("engine status lock");
    current.phase = phase.into();
    current.active_preset = engine.selection().map(|selection| selection.preset.clone());
    current.device = engine.device().map(str::to_owned);
    let _ = app.emit("engine_status", current.clone());
}

/// The saved dictionary and text options, read for each recognition so edits apply to
/// the next one. A failed read means no dictionary and no filler removal.
/// The field glossary's terms.
fn glossary_terms(app: &AppHandle) -> Vec<String> {
    app.state::<SettingsStore>()
        .load()
        .map(|loaded| crate::glossary::terms(&loaded.into_settings().dictionary.glossary))
        .unwrap_or_default()
}

/// The field glossary by topic, for the worker's second pass.
fn glossary_topics(app: &AppHandle) -> Vec<Vec<String>> {
    app.state::<SettingsStore>()
        .load()
        .map(|loaded| crate::glossary::topics(&loaded.into_settings().dictionary.glossary))
        .unwrap_or_default()
}

/// Whether the saved settings ask for the number and restatement rules. A failed read
/// means neither (the text stays as recognized).
fn spoken_settings(app: &AppHandle) -> (bool, bool) {
    app.state::<SettingsStore>()
        .load()
        .map(|loaded| {
            let settings = loaded.into_settings();
            (settings.tidy_numbers, settings.fix_restatements)
        })
        .unwrap_or_default()
}

fn text_settings(app: &AppHandle) -> (Vec<crate::settings::DictionaryEntry>, bool) {
    app.state::<SettingsStore>()
        .load()
        .map(|loaded| {
            let settings = loaded.into_settings();
            (settings.dictionary.entries, settings.remove_fillers)
        })
        .unwrap_or_default()
}

/// The saved microphone, read at each recording start so a settings change applies to
/// the next recording. A failed read falls back to the system default.
/// Recognition while speaking, unless the user turned it off.
/// Whether the parts recognized while speaking are shown live (needs streaming).
fn live_preview_enabled(app: &AppHandle) -> bool {
    app.state::<SettingsStore>()
        .load()
        .ok()
        .is_some_and(|loaded| {
            let settings = loaded.into_settings();
            settings.streaming && settings.live_preview && settings.recognition.mode != "cloud"
        })
}

/// A guess needs this much unconfirmed speech, and is never longer than the last
/// `GUESS_MAX_SECONDS` (longer audio would take the worker too long while speaking).
const GUESS_MIN_SECONDS: f32 = 0.8;
const GUESS_MAX_SECONDS: usize = 20;
/// A guess that took this long stops the guesses for the rest of the recording: the
/// GPU is too busy, and the confirmed parts and the final text come first.
const GUESS_GIVE_UP: Duration = Duration::from_millis(2_500);

fn screen_context_enabled(app: &AppHandle) -> bool {
    app.state::<SettingsStore>()
        .load()
        .ok()
        .is_some_and(|loaded| loaded.into_settings().screen_context)
}

fn paragraphs_enabled(app: &AppHandle) -> bool {
    app.state::<SettingsStore>()
        .load()
        .ok()
        .is_none_or(|loaded| loaded.into_settings().paragraphs)
}

fn streaming_enabled(app: &AppHandle) -> bool {
    app.state::<SettingsStore>()
        .load()
        .ok()
        .is_none_or(|loaded| {
            let settings = loaded.into_settings();
            // In the cloud mode the whole recording is sent once, after stop.
            settings.streaming && settings.recognition.mode != "cloud"
        })
}

/// "クラウドで認識": recordings go to Groq; this PC's recognizer is only the fallback.
fn cloud_mode(app: &AppHandle) -> bool {
    app.state::<SettingsStore>()
        .load()
        .ok()
        .is_some_and(|loaded| loaded.into_settings().recognition.mode == "cloud")
}

/// Same threshold as the rule-based punctuation: shorter texts are left as they are.
const PUNCTUATION_RUN: usize = 24;

/// Recognition while speaking waits for a local model being loaded at most this long:
/// a large model (14B) takes far longer, and the recording would miss it entirely.
const MAX_WARM_UP_WAIT: Duration = Duration::from_secs(10);

/// A remainder shorter than this (after recognition while speaking) gets the rule
/// punctuation only: the local model would add a second or more to the wait for a few
/// marks (the parts before it were punctuated while the user spoke).
const TAIL_AI_MIN_CHARS: usize = 60;

/// At stop, how long to wait for the parts' background punctuation (recognition while
/// speaking). Parts whose answer is later keep the recognized text.
const STREAM_PUNCTUATION_WAIT: Duration = Duration::from_millis(1500);

/// Time allowed for a cloud draft. Cloud models answer in seconds; this only bounds a
/// stalled connection.
const CLOUD_TIMEOUT: Duration = Duration::from_secs(60);

/// Why no draft came back, from the local model or a cloud service.
#[derive(Clone, Copy)]
enum DraftError {
    Local(crate::polish::PolishError),
    Cloud(crate::cloud::CloudError),
}

impl DraftError {
    fn notice(self) -> &'static str {
        match self {
            Self::Local(error) => error.notice(),
            Self::Cloud(error) => error.notice(),
        }
    }

    /// The nearest diagnostics category.
    fn result(self) -> crate::polish::PolishResult {
        use crate::cloud::CloudError as C;
        use crate::polish::PolishError as P;
        match self {
            Self::Local(error) => error.into(),
            Self::Cloud(error) => match error {
                C::Unreachable => P::Unreachable,
                C::Timeout => P::Timeout,
                C::BadResponse => P::BadResponse,
                C::Empty => P::Empty,
                C::NoKey | C::Unauthorized | C::Limited | C::Rejected => P::Rejected,
            }
            .into(),
        }
    }
}

/// Write the draft with the model the settings choose. "compare" asks both cloud
/// services at once and pastes both drafts with their times, for choosing between them.
/// Also returns each cloud draft's service, model and time for diagnostics.
/// Blocking; run off the async runtime.
/// Offer the drafting model's term corrections as dictionary entries (in memory; the
/// user registers each one with a button).
fn suggest_entries(
    app: &AppHandle,
    original: &str,
    draft: &str,
    vocabulary: &[String],
    dictionary: &[crate::settings::DictionaryEntry],
) {
    let found = crate::candidates::find(original, draft, vocabulary);
    if found.is_empty() {
        return;
    }
    let known: Vec<String> = dictionary
        .iter()
        .flat_map(|entry| entry.aliases.iter().cloned())
        .collect();
    if app
        .state::<crate::candidates::CandidateStore>()
        .add(found, &known)
    {
        let _ = app.emit("dictionary_candidates", ());
    }
}

/// Compose mode for one recording: the copied reference text and the text composed just
/// before (for a spoken revision); both may be empty.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ComposeSession {
    reference: String,
    previous: String,
}

/// The last composed text and its reference, in memory only, for a spoken revision.
struct LastCompose {
    text: String,
    reference: String,
    at: Instant,
}

static LAST_COMPOSE: Mutex<Option<LastCompose>> = Mutex::new(None);

/// How long the last composed text stays available for a revision.
const REVISE_WITHIN: Duration = Duration::from_secs(600);

/// A new compose recording: a freshly copied text means a new task (no previous text);
/// otherwise the text composed within `REVISE_WITHIN` is offered for revision, with the
/// reference it was written from.
fn start_compose(copied: Option<String>) -> ComposeSession {
    let last = LAST_COMPOSE.lock().ok().and_then(|last| {
        last.as_ref()
            .filter(|last| last.at.elapsed() < REVISE_WITHIN)
            .map(|last| (last.text.clone(), last.reference.clone()))
    });
    compose_session_from(copied, last)
}

fn compose_session_from(copied: Option<String>, last: Option<(String, String)>) -> ComposeSession {
    match (copied, last) {
        (Some(reference), _) => ComposeSession {
            reference,
            previous: String::new(),
        },
        (None, Some((previous, reference))) => ComposeSession {
            reference,
            previous,
        },
        (None, None) => ComposeSession::default(),
    }
}

fn remember_compose(text: &str, reference: &str) {
    if let Ok(mut last) = LAST_COMPOSE.lock() {
        *last = Some(LastCompose {
            text: text.to_owned(),
            reference: reference.to_owned(),
            at: Instant::now(),
        });
    }
}

/// `abandoned` is set when the user cancelled: a request already sent cannot be taken
/// back, but no further service is asked (R-08).
fn write_draft(
    polish: &crate::settings::PolishSettings,
    original: &str,
    context: &crate::polish::Context,
    abandoned: &std::sync::atomic::AtomicBool,
) -> (
    Result<String, DraftError>,
    Vec<crate::diagnostics::CloudDraft>,
) {
    let mut reports = Vec::new();
    let draft = draft_text(polish, original, context, abandoned, &mut reports);
    (draft, reports)
}

/// How long to wait for the chosen cloud service before also asking the other one: 6 s,
/// plus 1 s for each 175 characters of transcript (GPT-6 Luna usually took 4–5 s for 700
/// characters, now and then 8–12 s, once 31 s; 2026-10-13).
fn fallback_after(original: &str) -> Duration {
    let chars = original.chars().count() as u64;
    Duration::from_secs(6 + chars / 175).min(Duration::from_secs(20))
}

/// The chosen service's draft; if it has not answered within `after` and a backup is
/// given, the backup is asked too and the first successful answer is used (the other one
/// is left to finish on its own). Reports the drafts that answered before the return.
fn first_draft(
    chosen: (crate::cloud::Provider, String),
    backup: Option<(crate::cloud::Provider, String)>,
    system: &'static str,
    user: &str,
    after: Duration,
    abandoned: &std::sync::atomic::AtomicBool,
    reports: &mut Vec<crate::diagnostics::CloudDraft>,
) -> crate::cloud::Timed {
    let user = user.to_owned();
    race_drafts(
        chosen,
        backup,
        after,
        abandoned,
        reports,
        move |provider, model| crate::cloud::timed(provider, &model, system, &user, CLOUD_TIMEOUT),
    )
}

/// `first_draft` with the request as a function (tests pass a slow fake).
fn race_drafts(
    chosen: (crate::cloud::Provider, String),
    backup: Option<(crate::cloud::Provider, String)>,
    after: Duration,
    abandoned: &std::sync::atomic::AtomicBool,
    reports: &mut Vec<crate::diagnostics::CloudDraft>,
    request: impl Fn(crate::cloud::Provider, String) -> crate::cloud::Timed + Send + Sync + 'static,
) -> crate::cloud::Timed {
    let request = std::sync::Arc::new(request);
    let (sender, receiver) = std::sync::mpsc::channel::<(bool, crate::cloud::Timed)>();
    let ask = |(provider, model): (crate::cloud::Provider, String), fallback: bool| {
        let sender = sender.clone();
        let request = request.clone();
        std::thread::spawn(move || {
            let _ = sender.send((fallback, request(provider, model)));
        });
    };
    let (chosen_provider, chosen_model) = (chosen.0, chosen.1.clone());
    ask(chosen, false);
    let mut answers: Vec<(bool, crate::cloud::Timed)> = Vec::new();
    let mut waiting = 1;
    match receiver.recv_timeout(after) {
        Ok(answer) => {
            waiting -= 1;
            answers.push(answer);
        }
        Err(_) => {
            // After a cancel nobody wants the draft: do not send the text to a second
            // service (it would cost money and the result is thrown away).
            if let Some(backup) = backup.filter(|_| !abandoned.load(Ordering::Acquire)) {
                ask(backup, true);
                waiting += 1;
            }
        }
    }
    drop(sender);
    // Wait until one answer succeeds or every asked service has answered.
    while waiting > 0 && !answers.iter().any(|(_, draft)| draft.result.is_ok()) {
        match receiver.recv() {
            Ok(answer) => {
                waiting -= 1;
                answers.push(answer);
            }
            Err(_) => break,
        }
    }
    for (fallback, draft) in &answers {
        reports.push(crate::diagnostics::CloudDraft {
            fallback: *fallback,
            ..draft.report()
        });
    }
    match answers.iter().position(|(_, draft)| draft.result.is_ok()) {
        Some(index) => answers.swap_remove(index).1,
        None => answers
            .pop()
            .map(|(_, draft)| draft)
            .unwrap_or(crate::cloud::Timed {
                provider: chosen_provider,
                model: chosen_model,
                seconds: 0.0,
                result: Err(crate::cloud::CloudError::BadResponse),
            }),
    }
}

fn draft_text(
    polish: &crate::settings::PolishSettings,
    original: &str,
    context: &crate::polish::Context,
    abandoned: &std::sync::atomic::AtomicBool,
    reports: &mut Vec<crate::diagnostics::CloudDraft>,
) -> Result<String, DraftError> {
    use crate::cloud::Provider;
    let cloud = |provider: Provider| -> (Provider, String) {
        let model = match provider {
            Provider::OpenAi => polish.openai_model.clone(),
            Provider::Anthropic => polish.anthropic_model.clone(),
            Provider::Groq => unreachable!("Groq only recognizes speech"),
        };
        (provider, model)
    };
    let system = crate::polish::system_prompt_for(context);
    let user = crate::polish::user_message(original, context);
    let draft = match polish.provider.as_str() {
        "openai" | "anthropic" => {
            let chosen = Provider::parse(&polish.provider).expect("checked");
            let other = match chosen {
                Provider::OpenAi | Provider::Groq => Provider::Anthropic,
                Provider::Anthropic => Provider::OpenAi,
            };
            let backup = (polish.fallback && crate::cloud::has_key(other)).then(|| cloud(other));
            let draft = first_draft(
                cloud(chosen),
                backup,
                system,
                &user,
                fallback_after(original),
                abandoned,
                reports,
            );
            draft
                .result
                .map(|text| crate::polish::cleanup(&text))
                .map_err(DraftError::Cloud)?
        }
        "compare" => {
            let jobs: Vec<_> = [Provider::OpenAi, Provider::Anthropic]
                .into_iter()
                .map(|provider| {
                    let (provider, model) = cloud(provider);
                    let user = user.clone();
                    std::thread::spawn(move || {
                        crate::cloud::timed(provider, &model, system, &user, CLOUD_TIMEOUT)
                    })
                })
                .collect();
            let drafts: Vec<crate::cloud::Timed> =
                jobs.into_iter().filter_map(|job| job.join().ok()).collect();
            reports.extend(drafts.iter().map(crate::cloud::Timed::report));
            if let Some(error) = drafts
                .iter()
                .all(|draft| draft.result.is_err())
                .then(|| drafts.first().and_then(|draft| draft.result.clone().err()))
                .flatten()
            {
                return Err(DraftError::Cloud(error));
            }
            drafts
                .iter()
                .map(|draft| {
                    let body = match &draft.result {
                        Ok(text) => crate::polish::cleanup(text),
                        Err(error) => format!("（失敗: {}）", error.status()),
                    };
                    format!(
                        "【{} {}（{:.1}秒）】\n{body}",
                        draft.provider.label(),
                        draft.model,
                        draft.seconds
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n")
        }
        _ => crate::polish::polish(
            &polish.endpoint,
            &polish.model,
            original,
            context,
            crate::polish::polish_timeout(original),
        )
        .map_err(DraftError::Local)?,
    };
    // A composed text stands alone: the spoken request is not pasted below it.
    Ok(crate::polish::compose(
        &draft,
        original,
        polish.append_original && !context.compose,
    ))
}

/// Punctuate `text` with the local model in the background; None on any failure.
fn spawn_punctuation(
    app: &AppHandle,
    punctuation: crate::settings::PunctuationSettings,
    text: String,
) -> std::thread::JoinHandle<Option<String>> {
    let endpoint = polish_endpoint(app);
    std::thread::spawn(move || {
        crate::polish::punctuate(
            &endpoint,
            &punctuation.model,
            &text,
            crate::polish::PUNCTUATION_TIMEOUT,
        )
        .ok()
    })
}

/// Local-model punctuation settings when that option is chosen.
fn punctuation_settings(app: &AppHandle) -> Option<crate::settings::PunctuationSettings> {
    punctuation_settings_gated(app, false)
}

/// Whether the PC is short of memory, shared by all recordings (src/memory_gate.rs).
static MEMORY_GATE: Mutex<crate::memory_gate::Gate> = Mutex::new(crate::memory_gate::Gate::new());

/// The local-AI punctuation settings, or None when the rules are to be used: the user
/// chose the rules, or the PC is short of memory (unless the user turned that off).
/// `recording`: a finished recording is asking (counted toward the question to the user).
fn punctuation_settings_gated(
    app: &AppHandle,
    recording: bool,
) -> Option<crate::settings::PunctuationSettings> {
    use crate::memory_gate::Event;
    let punctuation = app
        .state::<SettingsStore>()
        .load()
        .ok()
        .map(|loaded| loaded.into_settings().punctuation)
        .filter(|punctuation| punctuation.mode == "local_ai")?;
    if !punctuation.auto_rules_when_low_memory {
        return Some(punctuation);
    }
    let (short, event) = MEMORY_GATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .observe(crate::diagnostics::MemoryState::now(), recording);
    if event != Event::Nothing {
        // Take the model off now: it is the largest user of memory.
        let (endpoint, model) = (polish_endpoint(app), punctuation.model.clone());
        std::thread::spawn(move || {
            let _ = crate::polish::unload(&endpoint, &model);
        });
    }
    match event {
        Event::Nothing => {}
        Event::BecameTight => {
            let message =
                "メモリが少ないため、句読点は規則で打ちます（PC内のAIは一時的にやめました）";
            let _ = app.emit("user_notice", message);
            // This runs as a recording ends, while the indicator is still up: a plain
            // flash would be dropped then, so keep it until the indicator closes.
            crate::overlay::flash_outcome(app, message);
        }
        Event::AskUser => {
            let app = app.clone();
            // Built off this task's thread: creating a window waits for the main thread.
            std::thread::spawn(move || {
                let _ = crate::memory_prompt::show(&app);
            });
        }
    }
    (!short).then_some(punctuation)
}

/// The Ollama address shared by polishing and punctuation.
fn polish_endpoint(app: &AppHandle) -> String {
    app.state::<SettingsStore>()
        .load()
        .ok()
        .map(|loaded| loaded.into_settings().polish.endpoint)
        .unwrap_or_else(|| crate::polish::DEFAULT_ENDPOINT.into())
}

/// The polishing settings when that mode is turned on.
fn polish_settings(app: &AppHandle) -> Option<crate::settings::PolishSettings> {
    app.state::<SettingsStore>()
        .load()
        .ok()
        .map(|loaded| loaded.into_settings().polish)
        .filter(|polish| polish.enabled)
}

fn preferred_device(app: &AppHandle) -> Option<String> {
    let settings = app.state::<SettingsStore>().load().ok()?.into_settings();
    match settings.input_device {
        crate::settings::InputDevice::Endpoint { id, .. } => Some(id),
        crate::settings::InputDevice::SystemDefault => None,
    }
}

/// The English recognizer's folder to load next to the main one: only when the English
/// shortcut is on, the main recognizer cannot listen in English itself (Parakeet Japanese;
/// Whisper is multilingual and takes the English recordings itself), and Parakeet English
/// is installed. Otherwise None.
/// The model's files are checked against the catalog (size and hash) like the main model,
/// off the async runtime; files that are there but do not match are reported, not loaded.
async fn english_dir(
    app: &AppHandle,
    settings: &crate::settings::Settings,
) -> Option<std::path::PathBuf> {
    if !settings.shortcuts.english_enabled
        || models::engine(&settings.recognition.preset) == "whisper"
    {
        return None;
    }
    let selection = crate::model_roots(app)
        .ok()?
        .resolve(models::ENGLISH_PRESET)
        .ok()?;
    let (present, verified) = tokio::task::spawn_blocking(move || {
        let present = models::installed(&selection);
        let verified = present && models::verify(&selection).is_ok();
        (present, verified.then_some(selection.directory))
    })
    .await
    .ok()?;
    if present && verified.is_none() {
        notice(
            app,
            "英語のモデルのファイルが壊れているか、想定と違う版です。英語の認識は使えません。設定からモデルを入れ直してください",
        );
    }
    verified
}

/// The loaded main recognizer cannot take English recordings itself (Parakeet Japanese).
fn needs_english_model(engine: &RuntimeEngine) -> bool {
    engine
        .selection()
        .is_some_and(|model| models::engine(&model.preset) != "whisper")
}

fn selected_model(app: &AppHandle, preset: &str) -> Result<ModelSelection, String> {
    if let Some(directory) = std::env::var_os("VOICEWEAVE_MODEL_DIR") {
        return Ok(ModelSelection {
            preset: "external".into(),
            directory: directory.into(),
        });
    }
    crate::model_roots(app)?
        .resolve(preset)
        .map_err(|error| error.message().to_owned())
}

async fn verified_model(app: &AppHandle, preset: &str) -> Result<ModelSelection, String> {
    let selection = selected_model(app, preset)?;
    tokio::task::spawn_blocking(move || {
        models::verify(&selection).map_err(|error| error.message().to_owned())?;
        Ok(selection)
    })
    .await
    .map_err(|_| "認識モデルの検査を完了できませんでした".to_owned())?
}

fn engine_state(engine: &RuntimeEngine) -> EngineState {
    if engine.selection().is_some() {
        EngineState::Ready
    } else {
        EngineState::Unavailable
    }
}

async fn apply_settings(
    app: &AppHandle,
    engine: &mut RuntimeEngine,
    coordinator: &mut Coordinator,
    gate: &AtomicU8,
    status: &Mutex<EngineStatus>,
    candidate: Settings,
    revision: u64,
) -> Result<Settings, String> {
    if !matches!(coordinator.session, SessionState::Idle)
        || coordinator.engine == EngineState::Loading
    {
        return Err("処理中です。設定は変更していません".into());
    }
    candidate.validate().map_err(|error| error.to_string())?;
    let current = app
        .state::<SettingsStore>()
        .load()
        .map_err(|error| error.to_string())?
        .into_settings();
    if current.revision != revision {
        return Err("設定が別の操作で変更されました。再読み込みしてください".into());
    }
    // A later (fallback) preparation loads the English recognizer too, when it is wanted.
    // Turning English on or off with Parakeet as the main recognizer loads or frees the
    // English one: the engine is prepared again.
    // Staged, not set: whichever way this goes wrong, the previous setting comes back.
    use crate::engine::EngineBackend as _;
    let english = english_dir(app, &candidate).await;
    let english_changed = engine.backend_mut().english_dir != english;
    engine.backend_mut().stage_english(english);
    if candidate.recognition.mode == "cloud" {
        let saved = match app.state::<SettingsStore>().save(&candidate, revision) {
            Ok(saved) => {
                engine.backend_mut().confirm_config();
                saved
            }
            Err(error) => {
                engine.backend_mut().rollback_config();
                return Err(error.to_string());
            }
        };
        if current.recognition.mode != "cloud" {
            // Free the memory (and the GPU) this PC's recognizer used; it is loaded again
            // only if a cloud recognition fails.
            let _ = engine.clear().await;
        }
        coordinator.engine = EngineState::Ready;
        update_gate(coordinator, gate);
        publish_engine(app, coordinator, engine, status);
        return Ok(saved);
    }
    let preset_changed = candidate.recognition.preset != current.recognition.preset;
    // Turning English on or off loads or frees its recognizer: the engine is prepared again.
    if preset_changed && std::env::var_os("VOICEWEAVE_MODEL_DIR").is_some() {
        engine.backend_mut().rollback_config();
        return Err("開発用のモデル指定を解除してからプリセットを変更してください".into());
    }
    let active_matches = engine.selection().is_some_and(|model| {
        model.preset == candidate.recognition.preset
            || (model.preset == "external" && std::env::var_os("VOICEWEAVE_MODEL_DIR").is_some())
    });
    if !preset_changed && !english_changed && active_matches {
        let saved = app.state::<SettingsStore>().save(&candidate, revision);
        match &saved {
            Ok(_) => engine.backend_mut().confirm_config(),
            Err(_) => engine.backend_mut().rollback_config(),
        }
        return saved.map_err(|error| error.to_string());
    }
    coordinator.engine = EngineState::Loading;
    update_gate(coordinator, gate);
    publish_engine(app, coordinator, engine, status);
    let selection = match verified_model(app, &candidate.recognition.preset).await {
        Ok(selection) => selection,
        Err(message) => {
            engine.backend_mut().rollback_config();
            coordinator.engine = engine_state(engine);
            return Err(message);
        }
    };
    let result = engine
        .apply(selection, || {
            app.state::<SettingsStore>()
                .save(&candidate, revision)
                .map_err(|error| error.to_string())
        })
        .await;
    coordinator.engine = engine_state(engine);
    if result.is_ok() {
        // Also on screen: switching takes a while and the settings window may be behind.
        crate::overlay::flash(app, "認識エンジンの準備ができました");
    }
    result.map_err(|failure| {
        let recovery = if failure.restored {
            "以前の認識エンジンに戻しました"
        } else if failure.restart_required {
            "エンジンの終了状態を確認できないため、アプリを再起動してください"
        } else {
            "エンジンは利用できません。再準備してください"
        };
        format!("{}。{}", failure.message, recovery)
    })
}

enum Transcription {
    Completed(Result<crate::worker::TranscriptionResult, crate::worker::WorkerError>),
    Cancelled,
    Disconnected,
}

/// Set when the PC was locked (or slept) during this session: the result is copied to the
/// clipboard and not typed. Cleared when the next session starts.
static COPY_ONLY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

async fn deliver(
    app: &AppHandle,
    text: String,
    target: Option<Target>,
    cancellation: Arc<AtomicU64>,
    expected_generation: u64,
) -> Result<DeliveryOutcome, ()> {
    let target = target.filter(|_| !COPY_ONLY.load(Ordering::Acquire));
    let outcome = tokio::task::spawn_blocking(move || {
        input::deliver_cancellable(&text, target, &cancellation, expected_generation)
    })
    .await;
    match &outcome {
        Ok(DeliveryOutcome::Cancelled) => {
            outcome_notice(app, "入力を取り消しました。直前の結果は保持しています")
        }
        Ok(DeliveryOutcome::SentUnverified) => notice(app, "貼り付けを実行しました"),
        Ok(DeliveryOutcome::CopiedOnly(CopiedReason::Cancelled)) => outcome_notice(
            app,
            "入力を取り消しました。結果はクリップボードと直前の結果にあります",
        ),
        Ok(DeliveryOutcome::CopiedOnly(_)) => outcome_notice(
            app,
            "結果をコピーしました。入力先が変わった場合は手動で貼り付けてください",
        ),
        Ok(DeliveryOutcome::ClipboardUnavailable) => outcome_notice(
            app,
            "クリップボードを使用できません。直前の結果は保持しています",
        ),
        Ok(DeliveryOutcome::SendFailed(_)) => outcome_notice(
            app,
            "入力先への送信は確認できません。直前の結果は保持しています",
        ),
        Err(_) => outcome_notice(app, "入力処理を完了できませんでした"),
    }
    outcome.map_err(|_| ())
}

fn diagnostic_failure(
    app: &AppHandle,
    metrics: &Option<SessionMetrics>,
    stage: FailureStage,
    code: FailureCode,
    backend_kind: Option<BackendKind>,
) {
    diagnostic_failure_at(app, metrics, stage, code, backend_kind, None);
}

fn diagnostic_failure_at(
    app: &AppHandle,
    metrics: &Option<SessionMetrics>,
    stage: FailureStage,
    code: FailureCode,
    backend_kind: Option<BackendKind>,
    fault_position: Option<crate::diagnostics::FaultPosition>,
) {
    record_failure(
        app,
        metrics,
        stage,
        code,
        backend_kind,
        fault_position,
        None,
    );
}

fn record_failure(
    app: &AppHandle,
    metrics: &Option<SessionMetrics>,
    stage: FailureStage,
    code: FailureCode,
    backend_kind: Option<BackendKind>,
    fault_position: Option<crate::diagnostics::FaultPosition>,
    gpu: Option<crate::gpu::GpuMemory>,
) {
    let attempt = metrics.as_ref().map_or(0, |metrics| metrics.report.attempt);
    let state = app.state::<DiagnosticsStore>().failed(FailureReport {
        attempt,
        stage,
        code,
        backend_kind,
        fault_position,
        memory: diagnostics::MemoryState::now(),
        gpu,
    });
    let _ = app.emit_to("main", "diagnostics", state);
}

fn finish_metrics(
    app: &AppHandle,
    metrics: &mut Option<SessionMetrics>,
    id: uuid::Uuid,
    outcome: Outcome,
) {
    if metrics.as_ref().is_some_and(|metrics| metrics.id == id) {
        let report = metrics.take().expect("matched metrics").finish(outcome);
        let state = app.state::<DiagnosticsStore>().completed(report);
        let _ = app.emit_to("main", "diagnostics", state);
    }
}

fn delivery_outcome(
    app: &AppHandle,
    metrics: &Option<SessionMetrics>,
    result: Result<DeliveryOutcome, ()>,
) -> Outcome {
    match result {
        Ok(DeliveryOutcome::SentUnverified) => Outcome::SentUnverified,
        Ok(DeliveryOutcome::CopiedOnly(CopiedReason::Cancelled))
        | Ok(DeliveryOutcome::Cancelled) => Outcome::Cancelled,
        Ok(DeliveryOutcome::CopiedOnly(_)) => Outcome::CopiedOnly,
        other => {
            let code = match other {
                Ok(DeliveryOutcome::ClipboardUnavailable) => FailureCode::ClipboardUnavailable,
                Ok(DeliveryOutcome::SendFailed(_)) => FailureCode::SendFailed,
                _ => FailureCode::DeliveryTask,
            };
            diagnostic_failure(app, metrics, FailureStage::Delivery, code, None);
            Outcome::Failed
        }
    }
}

async fn copy_last_result(
    app: &AppHandle,
    text: String,
    cancellation: Arc<AtomicU64>,
    expected_generation: u64,
) {
    // A missing target makes the input service copy only; no SendInput path is reachable.
    let outcome = tokio::task::spawn_blocking(move || {
        input::deliver_cancellable(&text, None, &cancellation, expected_generation)
    })
    .await;
    match outcome {
        Ok(DeliveryOutcome::CopiedOnly(CopiedReason::NoTarget)) => notice(
            app,
            "直前の結果をコピーしました。必要な場所へ手動で貼り付けてください",
        ),
        Ok(DeliveryOutcome::CopiedOnly(CopiedReason::Cancelled)) => {
            notice(app, "取消前にコピー済みです。直前の結果も保持しています")
        }
        Ok(DeliveryOutcome::Cancelled) => notice(app, "コピーを取り消しました"),
        Ok(DeliveryOutcome::CopiedOnly(CopiedReason::ClipboardChanged)) => {
            notice(app, "コピー後にクリップボードが変わった可能性があります")
        }
        Ok(DeliveryOutcome::ClipboardUnavailable) | Err(_) => {
            notice(app, "クリップボードへコピーできませんでした")
        }
        Ok(_) => notice(app, "コピーの結果を確認できませんでした"),
    }
}

struct PipelineContext<'a> {
    metrics: &'a mut Option<SessionMetrics>,
    capture: &'a Mutex<CaptureService>,
    coordinator: &'a mut Coordinator,
    worker: &'a mut RuntimeEngine,
    hotkeys: &'a mut mpsc::Receiver<PipelineCommand>,
    gate: &'a AtomicU8,
    overlay: &'a RecordingOverlay,
    cancellation: &'a Arc<AtomicU64>,
    expected_generation: u64,
    /// Set when this recording was started with the polish shortcut.
    polish: Option<crate::settings::PolishSettings>,
    /// Set when this recording was started with the compose key.
    compose: Option<ComposeSession>,
    /// Set when this recording was started with the English shortcut.
    english: bool,
    /// Parts already recognized while the user was speaking.
    stream: Option<crate::streaming::StreamState>,
    /// What the screen showed when the recording started (that option only).
    screen: crate::screen::ScreenRead,
}

async fn on_capture_event(event: CaptureEvent, context: PipelineContext<'_>) -> bool {
    let PipelineContext {
        metrics,
        capture,
        coordinator,
        worker,
        hotkeys,
        gate,
        overlay,
        cancellation,
        expected_generation,
        polish,
        compose,
        english,
        stream,
        screen,
    } = context;
    let app = overlay.app();
    match event {
        CaptureEvent::PreferredDeviceMissing(_) => {
            notice(
                app,
                "選択したマイクが見つからないため、既定のマイクで録音します",
            );
        }
        CaptureEvent::Started(id) => {
            if let Some(metrics) = metrics.as_mut().filter(|metrics| metrics.id == id) {
                metrics.started();
            }
            match coordinator.capture_started(id) {
                Action::StopCapture(id) => {
                    if capture.lock().expect("capture lock").stop(id).is_err() {
                        coordinator.cancel();
                        coordinator.engine = EngineState::Unavailable;
                        gate.store(UNAVAILABLE, Ordering::Release);
                        notice(app, "録音を停止できませんでした");
                        diagnostic_failure(
                            app,
                            metrics,
                            FailureStage::CaptureStop,
                            FailureCode::AudioDispatch,
                            None,
                        );
                        finish_metrics(app, metrics, id, Outcome::Failed);
                        return false;
                    }
                }
                Action::CancelCapture(id) => {
                    if capture.lock().expect("capture lock").cancel(id).is_err() {
                        coordinator.cancel();
                        coordinator.engine = EngineState::Unavailable;
                        gate.store(UNAVAILABLE, Ordering::Release);
                        notice(app, "録音を取り消せませんでした");
                        diagnostic_failure(
                            app,
                            metrics,
                            FailureStage::CaptureStop,
                            FailureCode::AudioDispatch,
                            None,
                        );
                        finish_metrics(app, metrics, id, Outcome::Failed);
                        return false;
                    }
                }
                Action::None => {
                    if let SessionState::Recording(session) = &coordinator.session {
                        if overlay
                            .show(
                                session.target,
                                live_preview_enabled(app) && !english,
                                cloud_mode(app),
                                english,
                            )
                            .is_err()
                        {
                            notice(app, "録音中表示を出せませんでした");
                            diagnostic_failure(
                                app,
                                metrics,
                                FailureStage::Overlay,
                                FailureCode::OverlayUnavailable,
                                None,
                            );
                        }
                    }
                    notice(app, "録音中です。もう一度押すと停止します");
                }
                _ => {}
            }
        }
        CaptureEvent::StartFailed(id, error) => {
            if coordinator.capture_failed(id) {
                overlay.hide();
                let (code, backend_kind) = error.diagnostic();
                diagnostic_failure(app, metrics, FailureStage::CaptureStart, code, backend_kind);
                if let Some(metrics) = metrics.as_mut() {
                    metrics.started();
                }
                finish_metrics(app, metrics, id, Outcome::Failed);
                notice(
                    app,
                    "マイクから録音を開始できませんでした。設定の「診断」タブを確認してください",
                );
            }
        }
        CaptureEvent::Stopped {
            session_id: id,
            reason,
            diagnostics,
            result,
        } => {
            if let Some(metrics) = metrics.as_mut().filter(|metrics| metrics.id == id) {
                metrics.capture_finished(diagnostics.timings.began);
                metrics.report.timings.stream_stop_ms = Some(diagnostics.timings.stream_stop_ms);
                metrics.report.timings.conversion_ms = Some(diagnostics.timings.conversion_ms);
                metrics.report.startup_xruns = Some(diagnostics.startup_xruns);
                metrics.report.capture_callbacks = Some(diagnostics.callbacks);
                metrics.report.input_frames = Some(diagnostics.frames);
                if let Ok(audio) = &result {
                    metrics.report.timings.audio_duration_ms =
                        Some(audio.samples.len() as u64 / 16);
                    metrics.report.loudness = Some(diagnostics::loudness(&audio.samples));
                }
            }
            if matches!(&coordinator.session, SessionState::CancellingCapture(session) if session.id == id)
            {
                coordinator.capture_failed(id);
                overlay.hide();
                notice(app, "録音を取り消しました");
                finish_metrics(app, metrics, id, Outcome::Cancelled);
                update_gate(coordinator, gate);
                return true;
            }
            if reason == CaptureStopReason::MaxDuration && coordinator.capture_auto_stop(id) {
                overlay.transcribing();
            }
            if reason == CaptureStopReason::Fault || result.is_err() {
                if coordinator.capture_failed(id) {
                    overlay.hide();
                    let (code, _) = result
                        .as_ref()
                        .err()
                        .copied()
                        .unwrap_or(crate::audio::AudioError::CaptureFailed)
                        .diagnostic();
                    diagnostic_failure_at(
                        app,
                        metrics,
                        if reason == CaptureStopReason::Fault {
                            FailureStage::CaptureStream
                        } else {
                            FailureStage::CaptureStop
                        },
                        code,
                        if code == FailureCode::AudioStream {
                            diagnostics.stream_error_kind
                        } else {
                            None
                        },
                        diagnostics
                            .fault_position
                            .map(
                                |(callback, audio_ms, xruns)| crate::diagnostics::FaultPosition {
                                    callback,
                                    audio_ms,
                                    xruns,
                                },
                            ),
                    );
                    finish_metrics(app, metrics, id, Outcome::Failed);
                    notice(app, "録音中に問題が発生しました。設定の「診断」タブを確認してください。入力は行いません");
                }
                update_gate(coordinator, gate);
                return true;
            }
            let Ok(audio) = result else { return true };
            if cancellation.load(Ordering::Acquire) != expected_generation {
                if coordinator.capture_failed(id) {
                    overlay.hide();
                    notice(app, "録音を取り消しました");
                    finish_metrics(app, metrics, id, Outcome::Cancelled);
                }
                update_gate(coordinator, gate);
                return true;
            }
            if !coordinator.capture_stopped(id, true) {
                return true;
            }
            gate.store(BUSY, Ordering::Release);
            if reason == CaptureStopReason::MaxDuration {
                notice(app, "300秒の上限で録音を停止しました。認識中です");
            } else {
                notice(app, "録音を停止しました。認識中です");
            }
            if diagnostics.startup_xruns > 0 || audio.startup_xrun {
                outcome_notice(app, "録音開始時に音声の欠落があった可能性があります");
            }
            let cloud = cloud_mode(app);
            let generation = if cloud {
                match &coordinator.session {
                    SessionState::Transcribing(session) => session.worker_generation,
                    _ => worker.generation(),
                }
            } else {
                let Some(active_worker) = worker.handle_mut() else {
                    coordinator.cancel();
                    coordinator.engine = EngineState::Unavailable;
                    overlay.hide();
                    update_gate(coordinator, gate);
                    diagnostic_failure(
                        app,
                        metrics,
                        FailureStage::Worker,
                        FailureCode::WorkerRemote,
                        None,
                    );
                    finish_metrics(app, metrics, id, Outcome::Failed);
                    return true;
                };
                active_worker.generation()
            };
            // Recognized on Groq: a cancel or failure leaves this PC's engine as it was.
            let mut cloud_used = false;
            let (dictionary, remove_fillers) = text_settings(app);
            let (tidy_numbers, fix_restatements) = spoken_settings(app);
            let dictionary_hints =
                crate::screen::with_hints(crate::dictionary::hints(&dictionary), &screen.terms);
            let worker_started = Instant::now();
            // Recognition while speaking: only the part after the last cut is left.
            let stream_failure = stream.as_ref().and_then(|stream| stream.failure);
            // A stream that gave up after recognizing some parts is not used to finish the
            // recording (the whole audio is recognized instead), but is kept for the
            // rescue: if that fails too, its parts are still copied (R-05).
            let (mut streamed, failed_stream) = match stream {
                Some(stream) if stream.usable() => (Some(stream), None),
                Some(stream) if stream.parts() > 0 => (None, Some(stream)),
                _ => (None, None),
            };
            let (samples, context) = match &streamed {
                Some(stream) => {
                    let start = stream.committed_16k().min(audio.samples.len());
                    (&audio.samples[start..], stream.context())
                }
                None => (&audio.samples[..], String::new()),
            };
            if let Some(metrics) = metrics.as_mut() {
                metrics.report.stream_parts = streamed.as_ref().map(|stream| stream.parts());
                metrics.report.screen_terms =
                    screen_context_enabled(app).then(|| screen.terms.clone());
                metrics.report.stream_rescued = streamed.as_ref().map(|stream| stream.rescued);
                metrics.report.stream_failure = stream_failure.map(str::to_owned);
            }
            let transcription = if cloud {
                // The whole recording, after stop. If the cloud fails, this PC recognizes
                // it (loading its model first, slow on a PC without a GPU): the speech is
                // never lost to a bad connection.
                match recognize_in_cloud(app, hotkeys, &audio.samples, &dictionary_hints, english)
                    .await
                {
                    Ok((model, transcription)) => {
                        cloud_used = true;
                        if let Some(metrics) = metrics.as_mut() {
                            metrics.report.device = Some("cloud".into());
                            metrics.report.cloud_model =
                                (!model.is_empty()).then(|| model.to_owned());
                        }
                        transcription
                    }
                    Err(error) => {
                        notice(app, error.transcribe_notice());
                        if let Some(metrics) = metrics.as_mut() {
                            metrics.report.cloud_fallback = Some(error);
                        }
                        let local = recognize_again(
                            app,
                            worker,
                            hotkeys,
                            id,
                            &audio.samples,
                            &dictionary_hints,
                            english,
                        )
                        .await;
                        if let Some(metrics) = metrics.as_mut() {
                            metrics.report.device = worker.device().map(str::to_owned);
                            metrics.report.model = DiagnosticModel::from_preset(
                                worker.selection().map(|model| model.preset.as_str()),
                            );
                        }
                        if matches!(local, Transcription::Completed(Ok(_))) {
                            outcome_notice(app, error.transcribe_notice());
                        }
                        local
                    }
                }
            } else if streamed.is_some() && samples.len() < crate::streaming::MIN_TAIL_SAMPLES {
                Transcription::Completed(Ok(crate::worker::TranscriptionResult {
                    text: String::new(),
                    timings: Default::default(),
                    topic_pass: false,
                }))
            } else {
                // Topics only for a recording recognized in one go: while speaking, the
                // earlier parts already serve as context and the second pass added time
                // without better terms (synthetic astronomy speech, 2026-10-13).
                let active_worker = worker.handle_mut().expect("checked above");
                active_worker.set_topics(if streamed.is_none() {
                    glossary_topics(app)
                } else {
                    Vec::new()
                });
                let request = async {
                    if english {
                        active_worker.transcribe_english(id, samples).await
                    } else {
                        active_worker
                            .transcribe_with_hints(id, samples, &dictionary_hints, &context)
                            .await
                    }
                };
                tokio::pin!(request);
                loop {
                    tokio::select! {
                        response = &mut request => break Transcription::Completed(response),
                        command = hotkeys.recv() => match command {
                            Some(PipelineCommand::Cancel) => break Transcription::Cancelled,
                            Some(command) => {
                                reject_settings(command);
                                notice(app, "認識中です");
                            },
                            None => break Transcription::Disconnected,
                        }
                    }
                }
            };
            if let Some(metrics) = metrics.as_mut() {
                metrics.report.timings.worker_roundtrip_ms =
                    Some(diagnostics::elapsed_ms(worker_started));
            }
            // A failed recognition (a worker stalled by a full GPU, or one that crashed)
            // loses nothing yet: the whole recording is still here. Restart the engine
            // (which also takes this app's local models off the GPU) and recognize the
            // recording once more from the start. Its text goes through the usual input
            // checks; if this fails too, the text recognized while speaking is copied.
            let transcription = match transcription {
                Transcription::Completed(Err(error)) if cloud => {
                    record_failure(
                        app,
                        metrics,
                        FailureStage::Worker,
                        error.diagnostic(),
                        None,
                        None,
                        None,
                    );
                    Transcription::Completed(Err(error))
                }
                Transcription::Completed(Err(error)) => {
                    let gpu = tokio::task::spawn_blocking(crate::gpu::snapshot)
                        .await
                        .ok()
                        .flatten();
                    record_failure(
                        app,
                        metrics,
                        FailureStage::Worker,
                        error.diagnostic(),
                        None,
                        None,
                        gpu,
                    );
                    if retry_helps(&error) {
                        notice(app, failure_notice(gpu));
                        let retry_started = Instant::now();
                        let again = recognize_again(
                            app,
                            worker,
                            hotkeys,
                            id,
                            &audio.samples,
                            &dictionary_hints,
                            english,
                        )
                        .await;
                        if let Some(metrics) = metrics.as_mut() {
                            metrics.report.timings.retry_ms =
                                Some(diagnostics::elapsed_ms(retry_started));
                        }
                        if matches!(again, Transcription::Completed(Ok(_))) {
                            // The whole recording was recognized: no parts go in front.
                            streamed = None;
                            if let Some(metrics) = metrics.as_mut() {
                                metrics.report.recovery = Some(diagnostics::Recovery::Retried);
                                metrics.report.device = worker.device().map(str::to_owned);
                            }
                            outcome_notice(app, "認識をやり直して成功しました");
                        }
                        again
                    } else {
                        Transcription::Completed(Err(error))
                    }
                }
                other => other,
            };
            // Recognition while speaking: the parts go in front of the remainder, each
            // with the local-AI punctuation it got in the background while the user was
            // still speaking. Only the remainder is punctuated now, so the wait does not
            // grow with the length of the recording. Polishing rewrites the text anyway
            // and skips the AI punctuation (one model on the GPU at a time).
            let ai_punctuation =
                punctuation_settings_gated(app, true).filter(|_| polish.is_none() && !english);
            let mut stream_punctuated = false;
            let transcription = match (transcription, streamed.as_mut()) {
                (Transcription::Completed(Ok(mut result)), Some(stream)) => {
                    let started = Instant::now();
                    let tail_job = ai_punctuation
                        .clone()
                        .filter(|_| result.text.chars().count() >= TAIL_AI_MIN_CHARS)
                        .filter(|_| crate::polish::needs_punctuation(&result.text, PUNCTUATION_RUN))
                        .map(|punctuation| {
                            spawn_punctuation(app, punctuation, result.text.clone())
                        });
                    let (prefix, mut punctuated) = stream
                        .finished_text(Instant::now() + STREAM_PUNCTUATION_WAIT)
                        .await;
                    let mut tail = std::mem::take(&mut result.text);
                    if let Some(job) = tail_job {
                        while !job.is_finished() {
                            tokio::time::sleep(Duration::from_millis(20)).await;
                        }
                        if let Ok(Some(answer)) = job.join() {
                            tail = answer;
                            punctuated += 1;
                        }
                    }
                    result.text = format!("{prefix}{tail}");
                    if ai_punctuation.is_some() {
                        stream_punctuated = true;
                        if let Some(metrics) = metrics.as_mut() {
                            metrics.report.timings.punctuation_ms =
                                Some(diagnostics::elapsed_ms(started));
                            metrics.report.punctuation =
                                (punctuated > 0).then_some(crate::polish::PolishResult::Drafted);
                        }
                    }
                    Transcription::Completed(Ok(result))
                }
                (transcription, _) => transcription,
            };
            let mut outcome = Outcome::Failed;
            let disconnected = matches!(&transcription, Transcription::Disconnected);
            match transcription {
                Transcription::Cancelled | Transcription::Disconnected if cloud_used => {
                    coordinator.request_cancel();
                    overlay.hide();
                    update_gate(coordinator, gate);
                    notice(app, "認識を取り消しました");
                    finish_metrics(app, metrics, id, Outcome::Cancelled);
                    return !disconnected;
                }
                Transcription::Cancelled | Transcription::Disconnected => {
                    coordinator.request_cancel();
                    coordinator.engine = if cloud {
                        EngineState::Ready
                    } else {
                        EngineState::Unavailable
                    };
                    let _ = worker.clear().await; // Never resend an interrupted request.
                    overlay.hide();
                    update_gate(coordinator, gate);
                    notice(
                        app,
                        if cloud {
                            "認識を取り消しました"
                        } else {
                            "認識を取り消しました。再利用するにはエンジンを再準備してください"
                        },
                    );
                    finish_metrics(app, metrics, id, Outcome::Cancelled);
                    return !disconnected;
                }
                Transcription::Completed(Ok(response)) => {
                    if let Some(metrics) = metrics.as_mut() {
                        metrics.report.timings.worker_total_ms = response.timings.total_ms;
                        metrics.report.timings.worker_frontend_ms = response.timings.frontend_ms;
                        metrics.report.timings.worker_decode_ms = response.timings.decode_ms;
                        metrics.report.recognition_path = response.timings.path;
                        metrics.report.fallback_reason = response.timings.fallback_reason;
                        metrics.report.topic_passes = Some(
                            streamed.as_ref().map_or(0, |stream| stream.topic_passes)
                                + usize::from(response.topic_pass),
                        );
                    }
                    if cancellation.load(Ordering::Acquire) != expected_generation {
                        coordinator.request_cancel();
                        coordinator.engine = EngineState::Unavailable;
                        let _ = worker.clear().await;
                        overlay.hide();
                        update_gate(coordinator, gate);
                        notice(
                            app,
                            "認識を取り消しました。再利用するにはエンジンを再準備してください",
                        );
                        finish_metrics(app, metrics, id, Outcome::Cancelled);
                        return true;
                    }
                    let processing = Instant::now();
                    // Insert only 、/。 at confident boundaries; the raw text stays untouched.
                    // VOICEWEAVE_PUNCTUATION=off disables it for before/after comparison.
                    // Declared aliases first (design §9), then punctuation.
                    // A stock subtitle line Whisper invented over trailing silence goes
                    // first, before anything else reads the text.
                    let (spoken, tail_removed) =
                        crate::postprocess::remove_subtitle_tail(&response.text);
                    // A phrase written over and over (a Whisper decoding loop) becomes one.
                    let (spoken, repeats_collapsed) = crate::postprocess::collapse_repeats(&spoken);
                    if let Some(metrics) = metrics.as_mut() {
                        metrics.report.subtitle_tail_removed = Some(tail_removed);
                        metrics.report.repeats_collapsed = Some(repeats_collapsed);
                    }
                    let (with_terms, _) = crate::dictionary::apply_aliases(&spoken, &dictionary);
                    // Other spellings of the glossary's names (白鳥座) its way (はくちょう座).
                    // English: the Japanese word lists and filler rules do not apply.
                    let with_terms = if english {
                        with_terms
                    } else {
                        crate::glossary::align(&with_terms, &glossary_terms(app)).0
                    };
                    let with_terms = if remove_fillers && !english {
                        let (text, removed) = crate::postprocess::remove_fillers(&with_terms);
                        if let Some(metrics) = metrics.as_mut() {
                            metrics.report.fillers_removed = Some(removed);
                        }
                        text
                    } else {
                        with_terms
                    };
                    // How speech is written: a restatement keeps its correction, and kanji
                    // numbers with a unit become digits (Japanese only).
                    let with_terms = if fix_restatements && !english {
                        let (text, fixed) = crate::spoken::fix_restatements(&with_terms);
                        if let Some(metrics) = metrics.as_mut() {
                            metrics.report.restatements_fixed = Some(fixed);
                        }
                        text
                    } else {
                        with_terms
                    };
                    let with_terms = if tidy_numbers && !english {
                        let (text, tidied) = crate::spoken::tidy_numbers(&with_terms);
                        if let Some(metrics) = metrics.as_mut() {
                            metrics.report.numbers_tidied = Some(tidied);
                        }
                        text
                    } else {
                        with_terms
                    };
                    // Local-model punctuation (opt-in) only where a long stretch has no
                    // marks; the answer is used only if it changed nothing but 、/。.
                    let with_terms = match ai_punctuation.filter(|_| !stream_punctuated) {
                        Some(punctuation)
                            if crate::polish::needs_punctuation(&with_terms, PUNCTUATION_RUN) =>
                        {
                            let started = Instant::now();
                            let endpoint = polish_endpoint(app);
                            let text = with_terms.clone();
                            let result = tokio::task::spawn_blocking(move || {
                                crate::polish::punctuate(
                                    &endpoint,
                                    &punctuation.model,
                                    &text,
                                    crate::polish::PUNCTUATION_TIMEOUT,
                                )
                            })
                            .await
                            .unwrap_or(Err(crate::polish::PolishError::BadResponse));
                            if let Some(metrics) = metrics.as_mut() {
                                metrics.report.timings.punctuation_ms =
                                    Some(diagnostics::elapsed_ms(started));
                                metrics.report.punctuation = Some(match &result {
                                    Ok(_) => crate::polish::PolishResult::Drafted,
                                    Err(error) => (*error).into(),
                                });
                            }
                            result.unwrap_or(with_terms)
                        }
                        _ => with_terms,
                    };
                    let final_text = if english
                        || std::env::var("VOICEWEAVE_PUNCTUATION").as_deref() == Ok("off")
                    {
                        with_terms
                    } else {
                        crate::postprocess::punctuate(&with_terms).text
                    };
                    // No 、/。 at the start, none doubled where parts meet.
                    let final_text = if english {
                        final_text
                    } else {
                        crate::postprocess::close_final(&crate::postprocess::tidy_marks(
                            &final_text,
                        ))
                    };
                    // A long transcript (not polished) is split into paragraphs before
                    // topic words; only blank lines are added.
                    let final_text = if polish.is_none() && !english && paragraphs_enabled(app) {
                        let (split, breaks) = crate::postprocess::paragraphs(&final_text);
                        if let Some(metrics) = metrics.as_mut() {
                            metrics.report.paragraph_breaks = Some(breaks);
                        }
                        split
                    } else {
                        final_text
                    };
                    // Polishing mode: replace the transcript with a local draft. Any failure
                    // keeps the transcript; a cancel keeps it as the last result only.
                    let mut polish_cancelled = false;
                    let final_text = match polish {
                        Some(polish) if !final_text.trim().is_empty() => {
                            crate::overlay::polishing(app);
                            let started = Instant::now();
                            let original = final_text.clone();
                            let local_polish = polish.provider == "local";
                            let context = crate::polish::Context {
                                profile: polish.profile.clone(),
                                terms: crate::dictionary::terms(&dictionary),
                                notes: crate::dictionary::notes(&dictionary),
                                screen: screen.terms.clone(),
                                screen_text: screen.excerpt.clone(),
                                glossary: glossary_terms(app),
                                compose: compose.is_some(),
                                reference: compose
                                    .as_ref()
                                    .map(|session| session.reference.clone())
                                    .unwrap_or_default(),
                                previous: compose
                                    .as_ref()
                                    .map(|session| session.previous.clone())
                                    .unwrap_or_default(),
                            };
                            let composing = context.compose;
                            let compose_reference = context.reference.clone();
                            if let Some(metrics) = metrics.as_mut() {
                                metrics.report.compose_reference_chars = compose
                                    .as_ref()
                                    .map(|session| session.reference.chars().count());
                                metrics.report.compose_revision =
                                    compose.as_ref().map(|session| !session.previous.is_empty());
                            }
                            // Terms whose corrections become dictionary suggestions.
                            let vocabulary: Vec<String> = context
                                .glossary
                                .iter()
                                .chain(&context.terms)
                                .cloned()
                                .collect();
                            if let Some(metrics) = metrics.as_mut() {
                                metrics.report.screen_chars = (!context.screen_text.is_empty())
                                    .then(|| context.screen_text.chars().count());
                            }
                            let abandoned = Arc::new(std::sync::atomic::AtomicBool::new(false));
                            let request = tokio::task::spawn_blocking({
                                let abandoned = abandoned.clone();
                                move || write_draft(&polish, &original, &context, &abandoned)
                            });
                            let mut cloud_drafts = Vec::new();
                            tokio::pin!(request);
                            let drafted = loop {
                                tokio::select! {
                                    joined = &mut request => break match joined {
                                        Ok((draft, reports)) => {
                                            cloud_drafts = reports;
                                            draft
                                        }
                                        Err(_) => Err(DraftError::Local(crate::polish::PolishError::BadResponse)),
                                    },
                                    command = hotkeys.recv() => match command {
                                        Some(PipelineCommand::Cancel) | None => {
                                            polish_cancelled = true;
                                            abandoned.store(true, Ordering::Release);
                                            break Err(DraftError::Local(crate::polish::PolishError::Empty));
                                        }
                                        Some(command) => {
                                            reject_settings(command);
                                            notice(app, "整形中です");
                                        }
                                    }
                                }
                            };
                            // The chosen cloud service was slow and the other one wrote the draft.
                            let used_backup =
                                cloud_drafts.iter().any(|draft| draft.fallback && draft.ok);
                            // The polish model pushed the punctuation model off the GPU
                            // (6 GB holds one); load it back now, not at the next recording.
                            if let Some(punctuation) =
                                punctuation_settings(app).filter(|_| local_polish)
                            {
                                let endpoint = polish_endpoint(app);
                                std::thread::spawn(move || {
                                    let _ = crate::polish::warm_up(&endpoint, &punctuation.model);
                                });
                            }
                            if let Some(metrics) = metrics.as_mut() {
                                metrics.report.cloud_drafts =
                                    (!cloud_drafts.is_empty()).then_some(cloud_drafts);
                                metrics.report.timings.polish_ms =
                                    Some(diagnostics::elapsed_ms(started));
                                metrics.report.polish = Some(match &drafted {
                                    _ if polish_cancelled => crate::polish::PolishResult::Cancelled,
                                    Ok(_) => crate::polish::PolishResult::Drafted,
                                    Err(error) => error.result(),
                                });
                            }
                            match drafted {
                                Ok(text) => {
                                    // Kept for a spoken revision ("shorter") within minutes.
                                    if composing {
                                        remember_compose(&text, &compose_reference);
                                    }
                                    // A composed text is new writing, not a corrected transcript.
                                    if !composing {
                                        suggest_entries(
                                            app,
                                            &final_text,
                                            &text,
                                            &vocabulary,
                                            &dictionary,
                                        );
                                    }
                                    if used_backup {
                                        notice(
                                            app,
                                            "選んだAIの応答が遅いため、もう一方のAIで整形しました",
                                        );
                                    }
                                    text
                                }
                                Err(error) => {
                                    if !polish_cancelled {
                                        notice(app, error.notice());
                                    }
                                    // The transcript goes in instead; a polish recording
                                    // skipped the AI punctuation, so add it now.
                                    match punctuation_settings(app).filter(|_| {
                                        !polish_cancelled
                                            && crate::polish::needs_punctuation(
                                                &final_text,
                                                PUNCTUATION_RUN,
                                            )
                                    }) {
                                        Some(punctuation) => {
                                            let text = final_text.clone();
                                            let job = spawn_punctuation(app, punctuation, text);
                                            while !job.is_finished() {
                                                tokio::time::sleep(Duration::from_millis(20)).await;
                                            }
                                            job.join().ok().flatten().unwrap_or(final_text)
                                        }
                                        None => final_text,
                                    }
                                }
                            }
                        }
                        _ => final_text,
                    };
                    let action = coordinator.recognized(id, generation, response.text, final_text);
                    if polish_cancelled {
                        // Keep the transcript for re-paste/copy, but enter nothing.
                        if action == Action::Deliver(id) {
                            coordinator.delivery_done(id);
                        }
                        overlay.hide();
                        update_gate(coordinator, gate);
                        notice(
                            app,
                            "整形を取り消しました。元の文は直前の結果として残しています",
                        );
                        finish_metrics(app, metrics, id, Outcome::Cancelled);
                        discard_queued(hotkeys);
                        return true;
                    }
                    if action == Action::Deliver(id) {
                        gate.store(DELIVERING, Ordering::Release);
                        let target = match &coordinator.session {
                            SessionState::Delivering(session) => session.target,
                            _ => None,
                        };
                        let text = coordinator
                            .last_result
                            .as_ref()
                            .map(|last| last.final_text.clone())
                            .unwrap_or_default();
                        if let Some(metrics) = metrics.as_mut() {
                            metrics.report.timings.processing_ms =
                                Some(diagnostics::elapsed_ms(processing));
                        }
                        let delivery = Instant::now();
                        let result =
                            deliver(app, text, target, cancellation.clone(), expected_generation)
                                .await;
                        if let Some(metrics) = metrics.as_mut() {
                            metrics.report.timings.delivery_ms =
                                Some(diagnostics::elapsed_ms(delivery));
                        }
                        outcome = delivery_outcome(app, metrics, result);
                        coordinator.delivery_done(id);
                    } else if matches!(coordinator.session, SessionState::Idle) {
                        outcome = Outcome::Empty;
                        if let Some(metrics) = metrics.as_mut() {
                            metrics.report.timings.processing_ms =
                                Some(diagnostics::elapsed_ms(processing));
                        }
                        outcome_notice(app, "声を文字にできませんでした。入力はしていません");
                    }
                }
                Transcription::Completed(Err(_)) => {
                    // Recorded above, with the GPU's memory at the first failure.
                    coordinator.cancel();
                    // In the cloud mode, the next recording goes to the cloud again.
                    coordinator.engine = if cloud {
                        EngineState::Ready
                    } else {
                        EngineState::Unavailable
                    };
                    let _ = worker.clear().await;
                    // The parts recognized while speaking are copied (never typed), so
                    // the speech is not lost; the part after the last of them is missing.
                    let salvaged = streamed
                        .as_ref()
                        .or(failed_stream.as_ref())
                        .map(|stream| salvage_text(app, stream, &dictionary))
                        .filter(|text| !text.trim().is_empty());
                    let recovery = match salvaged {
                        Some(text) => {
                            let cancellation = cancellation.clone();
                            let copied = tokio::task::spawn_blocking(move || {
                                input::deliver_cancellable(
                                    &text,
                                    None,
                                    &cancellation,
                                    expected_generation,
                                )
                            })
                            .await;
                            if matches!(
                                copied,
                                Ok(DeliveryOutcome::CopiedOnly(CopiedReason::NoTarget))
                            ) {
                                outcome_notice(app, "認識に失敗しました。話している間に認識できた部分（最後の部分を除く）をクリップボードにコピーしました。Ctrl+Vで貼り付けられます");
                                diagnostics::Recovery::Salvaged
                            } else {
                                outcome_notice(app, "認識に失敗しました。入力は行いません");
                                diagnostics::Recovery::Lost
                            }
                        }
                        None => {
                            outcome_notice(app, "認識に失敗しました。入力は行いません");
                            diagnostics::Recovery::Lost
                        }
                    };
                    if let Some(metrics) = metrics.as_mut() {
                        metrics.report.recovery = Some(recovery);
                    }
                }
            }
            discard_queued(hotkeys);
            overlay.hide();
            finish_metrics(app, metrics, id, outcome);
        }
        CaptureEvent::Cancelled(id) => {
            if coordinator.capture_failed(id) {
                overlay.hide();
                notice(app, "録音を取り消しました");
                finish_metrics(app, metrics, id, Outcome::Cancelled);
            }
        }
        CaptureEvent::Rejected(id) => {
            if coordinator.capture_failed(id) {
                overlay.hide();
                notice(app, "録音を完了できませんでした");
                diagnostic_failure(
                    app,
                    metrics,
                    FailureStage::CaptureStop,
                    FailureCode::AudioDispatch,
                    None,
                );
                finish_metrics(app, metrics, id, Outcome::Failed);
            }
        }
    }
    update_gate(coordinator, gate);
    true
}

/// Live text as shown: the dictionary's and the glossary's spellings applied.
fn shown_text(app: &AppHandle, text: &str) -> String {
    let (dictionary, _) = text_settings(app);
    let (shown, _) = crate::dictionary::apply_aliases(text, &dictionary);
    crate::glossary::align(&shown, &glossary_terms(app)).0
}

/// A warning when other programs leave the recognizer little GPU memory to work in.
fn crowded_gpu_notice(gpu: crate::gpu::GpuMemory) -> Option<&'static str> {
    if gpu.ollama {
        Some("Ollama（ローカルAI）のモデルがGPUのメモリを使っています。認識が遅くなったり失敗したりする場合は、Ollamaを終了してください")
    } else if gpu.free_mb() < crate::gpu::LOW_FREE_MB {
        Some("GPUのメモリの空きが少なくなっています。認識が遅くなったり失敗したりする場合は、GPUを使うほかのソフトを終了するか、標準モデルを選んでください")
    } else {
        None
    }
}

/// A failure not caused by the audio itself: worth restarting the engine for.
fn retry_helps(error: &crate::worker::WorkerError) -> bool {
    use crate::worker::WorkerError;
    !matches!(
        error,
        WorkerError::InvalidAudio
            | WorkerError::ModelMissing
            | WorkerError::Remote(
                "INVALID_AUDIO_FORMAT" | "INVALID_AUDIO_LENGTH" | "INVALID_AUDIO_VALUES"
            )
    )
}

/// What to tell the user when recognition failed, from the GPU's memory at that moment.
fn failure_notice(gpu: Option<crate::gpu::GpuMemory>) -> &'static str {
    match gpu {
        Some(gpu) if gpu.ollama => {
            "認識に失敗しました。Ollama（ローカルAI）がGPUのメモリを使っていたためと思われます。認識エンジンを準備し直して、もう一度認識します"
        }
        Some(gpu) if gpu.free_mb() < crate::gpu::LOW_FREE_MB => {
            "認識に失敗しました。GPUのメモリが足りなかったためと思われます。認識エンジンを準備し直して、もう一度認識します"
        }
        _ => "認識に失敗しました。認識エンジンを準備し直して、もう一度認識します",
    }
}

/// Restart the engine and recognize the whole recording once more. Takes longer than the
/// usual path (the model loads again), so the limit is generous but bounded.
async fn recognize_again(
    app: &AppHandle,
    worker: &mut RuntimeEngine,
    hotkeys: &mut mpsc::Receiver<PipelineCommand>,
    id: uuid::Uuid,
    samples: &[f32],
    hints: &[String],
    english: bool,
) -> Transcription {
    use crate::worker::WorkerError;
    if prepare_saved_model(app, worker).await.is_err() {
        return Transcription::Completed(Err(WorkerError::Launch));
    }
    let Some(active) = worker.handle_mut() else {
        return Transcription::Completed(Err(WorkerError::Launch));
    };
    active.set_topics(glossary_topics(app));
    let seconds = samples.len() as u64 / 16_000;
    let limit = if worker.device() == Some("cuda") {
        Duration::from_secs(30 + seconds / 2)
    } else {
        Duration::from_secs(60 + seconds * 2)
    };
    let active = worker.handle_mut().expect("prepared above");
    let request = tokio::time::timeout(limit, async {
        if english {
            active.transcribe_english(id, samples).await
        } else {
            active.transcribe_with_hints(id, samples, hints, "").await
        }
    });
    tokio::pin!(request);
    loop {
        tokio::select! {
            response = &mut request => {
                break Transcription::Completed(response.unwrap_or(Err(WorkerError::Timeout)))
            }
            command = hotkeys.recv() => match command {
                Some(PipelineCommand::Cancel) => break Transcription::Cancelled,
                Some(command) => {
                    reject_settings(command);
                    notice(app, "認識中です");
                }
                None => break Transcription::Disconnected,
            }
        }
    }
}

/// Recognize the whole recording on Groq (the cloud mode). Esc cancels the wait (the
/// request may still finish; its answer is dropped).
async fn recognize_in_cloud(
    app: &AppHandle,
    hotkeys: &mut mpsc::Receiver<PipelineCommand>,
    samples: &[f32],
    hints: &[String],
    english: bool,
) -> Result<(&'static str, Transcription), crate::cloud::CloudError> {
    // The hints (dictionary words, registered words seen on screen) leave the PC with the
    // voice only when the user turned that on (R-04).
    let send_hints = app
        .state::<SettingsStore>()
        .load()
        .ok()
        .is_some_and(|loaded| loaded.into_settings().recognition.cloud_hints);
    let hints = if send_hints {
        hints.to_vec()
    } else {
        Vec::new()
    };
    let samples = samples.to_vec();
    let started = Instant::now();
    let request =
        tokio::task::spawn_blocking(move || crate::cloud::transcribe(&samples, &hints, english));
    tokio::pin!(request);
    loop {
        tokio::select! {
            answer = &mut request => {
                let (text, model) = answer.map_err(|_| crate::cloud::CloudError::BadResponse)??;
                let total = Some(diagnostics::elapsed_ms(started));
                break Ok((model, Transcription::Completed(Ok(crate::worker::TranscriptionResult {
                    text,
                    timings: crate::worker::WorkerTimings { total_ms: total, ..Default::default() },
                    topic_pass: false,
                }))));
            }
            command = hotkeys.recv() => match command {
                Some(PipelineCommand::Cancel) => break Ok(("", Transcription::Cancelled)),
                Some(command) => {
                    reject_settings(command);
                    notice(app, "認識中です");
                }
                None => break Ok(("", Transcription::Disconnected)),
            }
        }
    }
}

/// The parts recognized while speaking, with the dictionary's spellings and rule
/// punctuation, for copying when the recording could not be finished.
fn salvage_text(
    app: &AppHandle,
    stream: &crate::streaming::StreamState,
    dictionary: &[crate::settings::DictionaryEntry],
) -> String {
    let (text, _) = crate::dictionary::apply_aliases(&stream.text(), dictionary);
    let (text, _) = crate::glossary::align(&text, &glossary_terms(app));
    crate::postprocess::tidy_marks(&crate::postprocess::punctuate(&text).text)
}

async fn prepare_saved_model(app: &AppHandle, worker: &mut RuntimeEngine) -> Result<(), String> {
    let settings = app
        .state::<SettingsStore>()
        .load()
        .map_err(|error| error.to_string())?
        .into_settings();
    let model = verified_model(app, &settings.recognition.preset).await?;
    worker.backend_mut().english_dir = english_dir(app, &settings).await;
    // A local model left on the GPU (Ollama keeps one for 30 minutes, also from an
    // earlier run of the app) can leave too little memory for the recognizer, which then
    // falls back to the CPU and is several times slower (seen 2026-10-11). Take the
    // local models off the GPU first; they load again when next used.
    let endpoint = settings.polish.endpoint.clone();
    let models = [
        settings.punctuation.model.clone(),
        settings.polish.model.clone(),
    ];
    let _ = tokio::task::spawn_blocking(move || {
        for model in models {
            let _ = crate::polish::unload(&endpoint, &model);
        }
    })
    .await;
    worker
        .apply(model, || Ok(()))
        .await
        .map_err(|failure| failure.message)?;
    // With the recognizer loaded and this app's local models off the GPU, what is left
    // is the room recognition has to work in (before the punctuation model loads below).
    if worker.device() == Some("cuda") {
        let gpu = tokio::task::spawn_blocking(crate::gpu::snapshot)
            .await
            .ok()
            .flatten();
        if let Some(message) = gpu.and_then(crowded_gpu_notice) {
            notice(app, message);
        }
    }
    // Load the punctuation model now, after the recognizer, while nobody is speaking: at
    // the first recording it is then ready, and Ollama fits it into the GPU memory the
    // recognizer left (loading it at F8 made the first recordings slow, 2026-10-11).
    // Through the same memory check as a recording: when the PC is short of memory the
    // model stays off, so recovering the recognizer does not bring the shortage back.
    if let Some(punctuation) = punctuation_settings(app) {
        let endpoint = settings.polish.endpoint.clone();
        std::thread::spawn(move || {
            let _ = crate::polish::warm_up(&endpoint, &punctuation.model);
        });
    }
    Ok(())
}

struct PipelineLifetime {
    app: AppHandle,
    gate: Arc<AtomicU8>,
    status: Arc<Mutex<EngineStatus>>,
}

impl Drop for PipelineLifetime {
    fn drop(&mut self) {
        self.gate.store(UNAVAILABLE, Ordering::Release);
        if let Ok(mut status) = self.status.lock() {
            status.phase = "unavailable".into();
            status.active_preset = None;
            let _ = self.app.emit("engine_status", status.clone());
        }
    }
}

pub async fn run(
    app: AppHandle,
    mut hotkeys: mpsc::Receiver<PipelineCommand>,
    gate: Arc<AtomicU8>,
    cancellation: Arc<AtomicU64>,
    engine_status: Arc<Mutex<EngineStatus>>,
) {
    let _lifetime = PipelineLifetime {
        app: app.clone(),
        gate: gate.clone(),
        status: engine_status.clone(),
    };
    let overlay = RecordingOverlay::new(app.clone());
    let mut coordinator = Coordinator::default();
    let mut metrics: Option<SessionMetrics> = None;
    let mut attempt = 0_u64;
    let capture = match CaptureService::spawn() {
        Ok(capture) => Mutex::new(capture),
        Err(_) => {
            gate.store(UNAVAILABLE, Ordering::Release);
            notice(&app, "録音サービスを開始できませんでした");
            return;
        }
    };
    let (exe, mut args, cwd) = match super::worker_command(&app) {
        Ok(command) => command,
        Err(_) => {
            notice(&app, "認識ワーカーが見つかりません");
            return;
        }
    };
    args.pop();
    let mut worker = RuntimeEngine::new(WorkerBackend {
        executable: exe,
        args: args.into_iter().map(str::to_owned).collect(),
        cwd,
        cuda_dir: crate::cuda_dir(&app),
        // Set from the settings before each preparation (prepare_saved_model).
        english_dir: None,
        english_before: None,
    });
    let startup = if cloud_mode(&app) {
        Ok(())
    } else {
        prepare_saved_model(&app, &mut worker).await
    };
    match startup {
        Ok(()) => {
            coordinator.engine = EngineState::Ready;
        }
        Err(message) => {
            coordinator.engine = EngineState::Unavailable;
            let missing_model = message == crate::models::ModelError::Missing.message();
            crate::overlay::flash(&app, &message);
            let _ = app.emit("user_notice", message);
            // First run (or a removed model): the app lives in the tray, so open the
            // settings window where the model can be installed, and say so on screen.
            if missing_model {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
                crate::overlay::flash(&app, "認識モデルが未導入です。設定画面から導入してください");
            }
        }
    }
    publish_engine(&app, &mut coordinator, &worker, &engine_status);
    update_gate(&coordinator, &gate);
    if cloud_mode(&app) {
        notice(&app, "音声入力の準備ができました（クラウドで認識）");
    } else if worker.selection().is_some() {
        notice(&app, "音声入力の準備ができました");
    }
    let mut ticker = tokio::time::interval(Duration::from_millis(20));
    let mut level_tick = false;
    let mut active_generation = cancellation.load(Ordering::Acquire);
    let mut polish_session: Option<crate::settings::PolishSettings> = None;
    // Compose mode for this recording: the copied reference text (maybe empty).
    let mut compose_session: Option<ComposeSession> = None;
    // This recording was started with the English shortcut: the whole recording is
    // recognized as English after stop (no recognition while speaking), without the
    // Japanese text steps.
    let mut english_session = false;
    let mut stream: Option<crate::streaming::StreamState> = None;
    let mut next_stream_step = Instant::now();
    // Live text, speed first: when the next guess of the unconfirmed part may run, and
    // whether guesses were stopped for this recording (too slow).
    let mut next_guess = Instant::now();
    let mut guesses_off = false;
    // The local model being loaded for this recording. Recognition while speaking waits
    // for it: loading it next to the recognizer's work could run the GPU out of memory.
    let mut warming: Option<(std::thread::JoinHandle<()>, Instant)> = None;
    // Whether Esc is currently taken as the cancel key (only while recording).
    let mut escape_armed = false;
    // The input window being read when a recording starts, and the words it gave.
    let mut screen_job: Option<std::thread::JoinHandle<crate::screen::ScreenRead>> = None;
    let mut screen_read = crate::screen::ScreenRead::default();
    // Commands that came during a wait for the worker in a recording (wait_for_worker).
    let mut held = std::collections::VecDeque::new();
    loop {
        tokio::select! {
            biased;
            command = next_command(&mut held, &mut hotkeys) => {
                let Some(command) = command else { break };
                match command {
                    PipelineCommand::Hotkey(event @ (HotkeyEvent::Record | HotkeyEvent::Polish | HotkeyEvent::Compose | HotkeyEvent::English), target, generation, received) => {
                      if event == HotkeyEvent::English
                          && matches!(coordinator.session, SessionState::Idle)
                          && !cloud_mode(&app)
                          && needs_english_model(&worker)
                          && worker.backend_mut().english_dir.is_none()
                      {
                          notice(&app, "Parakeetで英語を使うには英語のモデルが必要です。設定の「基本」タブから導入してください");
                          continue;
                      }
                      if matches!(coordinator.session, SessionState::StartingCapture(_) | SessionState::Recording(_)) {
                          if let Some(metrics) = metrics.as_mut() { metrics.stop_requested(received); }
                      }
                      match coordinator.on_f8(target) {
                        Action::StartCapture(id) => {
                            attempt += 1;
                            stream = None;
                            guesses_off = false;
                            next_guess = Instant::now();
                            // Read the input window's words while the user speaks (option).
                            screen_read = crate::screen::ScreenRead::default();
                            screen_job = target.filter(|_| screen_context_enabled(&app)).map(|target| {
                                let window = target.foreground_handle();
                                let mut vocabulary = glossary_terms(&app);
                                vocabulary.extend(crate::dictionary::terms(&text_settings(&app).0));
                                std::thread::spawn(move || crate::screen::collect(window, &vocabulary))
                            });
                            // The key that starts a recording decides its mode; either key stops it.
                            english_session = event == HotkeyEvent::English;
                            polish_session = matches!(event, HotkeyEvent::Polish | HotkeyEvent::Compose)
                                .then(|| polish_settings(&app))
                                .flatten();
                            // Compose mode: take the text the user copied (an email to
                            // answer) now, before anything is pasted.
                            compose_session = polish_session
                                .as_ref()
                                .filter(|polish| event == HotkeyEvent::Compose && polish.compose_enabled)
                                .map(|polish| {
                                    let copied = polish
                                        .compose_clipboard
                                        .then(crate::input::copied_reference)
                                        .flatten();
                                    let session = start_compose(copied);
                                    notice(&app, if !session.previous.is_empty() {
                                        "作文モード：直前の文章を直すか、新しく作ります"
                                    } else if !session.reference.is_empty() {
                                        "作文モード：コピーした文章を参考にして作ります"
                                    } else {
                                        "作文モード：話した依頼から文章を作ります"
                                    });
                                    session
                                });
                            if event == HotkeyEvent::Compose && compose_session.is_none() {
                                // Compose off: an ordinary polish recording would surprise.
                                polish_session = None;
                            }
                            // Load the model this recording will use while the user speaks.
                            // Only one fits next to the recognizer, so never both.
                            if let Some(punctuation) = punctuation_settings(&app).filter(|_| polish_session.is_none()) {
                                let endpoint = polish_endpoint(&app);
                                warming = Some((std::thread::spawn(move || {
                                    let _ = crate::polish::warm_up(&endpoint, &punctuation.model);
                                }), Instant::now()));
                            }
                            if let Some(polish) = polish_session.clone().filter(|polish| polish.provider == "local") {
                                // Load the model while the user speaks; errors surface later.
                                warming = Some((std::thread::spawn(move || {
                                    let _ = crate::polish::warm_up(&polish.endpoint, &polish.model);
                                }), Instant::now()));
                            } else if polish_session.is_none() && matches!(event, HotkeyEvent::Polish | HotkeyEvent::Compose) {
                                notice(&app, "整形モードは設定でオフのため、通常の音声入力で録音します");
                            }
                            let model = if cloud_mode(&app) {
                                DiagnosticModel::Cloud
                            } else if english_session && needs_english_model(&worker) {
                                DiagnosticModel::ParakeetEn
                            } else {
                                DiagnosticModel::from_preset(worker.selection().map(|model| model.preset.as_str()))
                            };
                            metrics = Some(SessionMetrics::new(id, attempt, model, received));
                            if let Some(metrics) = metrics.as_mut() {
                                metrics.report.device = worker.device().map(str::to_owned);
                                metrics.report.local_model = match polish_session.as_ref() {
                                    Some(polish) if polish.provider == "local" => Some(polish.model.clone()),
                                    Some(_) => None,
                                    None => punctuation_settings(&app).map(|punctuation| punctuation.model),
                                };
                                // Read once the recording runs, not while the microphone starts.
                                metrics.gpu_job = Some(std::thread::spawn(|| {
                                    std::thread::sleep(Duration::from_millis(1500));
                                    crate::gpu::snapshot()
                                }));
                            }
                            if cancellation.load(Ordering::Acquire) != generation {
                                coordinator.capture_failed(id);
                                notice(&app, "録音開始前に取り消しました");
                                finish_metrics(&app, &mut metrics, id, Outcome::Cancelled);
                            } else if capture.lock().expect("capture lock").start_with(id, preferred_device(&app)).is_err() {
                                coordinator.cancel();
                                coordinator.engine = EngineState::Unavailable;
                                gate.store(UNAVAILABLE, Ordering::Release);
                                notice(&app, "録音を開始できませんでした");
                                diagnostic_failure(&app, &metrics, FailureStage::CaptureStart, FailureCode::AudioDispatch, None);
                                finish_metrics(&app, &mut metrics, id, Outcome::Failed);
                                return;
                            } else {
                                active_generation = generation;
                            }
                        }
                        Action::StopCapture(id) => {
                            overlay.transcribing();
                            if capture.lock().expect("capture lock").stop(id).is_err() {
                                coordinator.cancel();
                                coordinator.engine = EngineState::Unavailable;
                                gate.store(UNAVAILABLE, Ordering::Release);
                                notice(&app, "録音を停止できませんでした");
                                diagnostic_failure(&app, &metrics, FailureStage::CaptureStop, FailureCode::AudioDispatch, None);
                                finish_metrics(&app, &mut metrics, id, Outcome::Failed);
                                return;
                            }
                        }
                        Action::Busy => notice(&app, "処理中です"),
                        Action::NeedsEngine => notice(&app, "認識エンジンを準備中です"),
                        _ => {}
                      }
                    },
                    // Esc is handled in dispatch() as a cancel request and never queued.
                    PipelineCommand::Hotkey(HotkeyEvent::Cancel, ..) => {}
                    PipelineCommand::Hotkey(HotkeyEvent::Repaste, target, generation, _) => {
                        if cancellation.load(Ordering::Acquire) != generation {
                            notice(&app, "再貼り付けを取り消しました");
                        } else if coordinator.can_repaste() {
                            let text = coordinator.last_result.as_ref()
                                .map(|last| last.final_text.clone()).unwrap_or_default();
                            gate.store(DELIVERING, Ordering::Release);
                            let _ = deliver(&app, text, target, cancellation.clone(), generation).await;
                            discard_queued(&mut hotkeys);
                        } else {
                            notice(&app, "直前の結果はありません");
                        }
                    }
                    PipelineCommand::CopyLastResult(generation) => {
                        if cancellation.load(Ordering::Acquire) != generation {
                            notice(&app, "コピーを取り消しました");
                        } else if coordinator.can_repaste() {
                            let text = coordinator.last_result.as_ref()
                                .map(|last| last.final_text.clone()).unwrap_or_default();
                            gate.store(DELIVERING, Ordering::Release);
                            copy_last_result(&app, text, cancellation.clone(), generation).await;
                        } else {
                            notice(&app, "直前の結果はありません");
                        }
                    }
                    PipelineCommand::Cancel => {
                        match coordinator.request_cancel() {
                            Action::CancelCapture(id) => {
                                overlay.hide();
                                if capture.lock().expect("capture lock").cancel(id).is_err() {
                                    coordinator.cancel();
                                    coordinator.engine = EngineState::Unavailable;
                                    gate.store(UNAVAILABLE, Ordering::Release);
                                    notice(&app, "録音を取り消せませんでした");
                                    diagnostic_failure(&app, &metrics, FailureStage::CaptureStop, FailureCode::AudioDispatch, None);
                                    finish_metrics(&app, &mut metrics, id, Outcome::Failed);
                                    return;
                                }
                                notice(&app, "録音を取り消しています");
                            }
                            Action::None if matches!(coordinator.session, SessionState::StartingCapture(_)) => {
                                notice(&app, "録音開始後に取り消します");
                            }
                            Action::Busy => notice(&app, "入力処理中の取り消しはできません"),
                            _ => notice(&app, "取り消す録音はありません"),
                        }
                    }
                    PipelineCommand::RetryEngine => {
                        if !matches!(coordinator.session, SessionState::Idle) {
                            notice(&app, "処理中です");
                        } else if coordinator.engine == EngineState::Ready {
                            notice(&app, "認識エンジンは利用できます");
                        } else {
                            coordinator.engine = EngineState::Loading;
                            update_gate(&coordinator, &gate);
                            publish_engine(&app, &mut coordinator, &worker, &engine_status);
                            match prepare_saved_model(&app, &mut worker).await {
                                Ok(()) => {
                                    coordinator.engine = EngineState::Ready;
                                    notice(&app, "認識エンジンを再準備しました");
                                }
                                Err(message) => {
                                    coordinator.engine = engine_state(&worker);
                                    crate::overlay::flash(&app, &message);
                                    let _ = app.emit("user_notice", message);
                                }
                            }
                            discard_queued(&mut hotkeys);
                        }
                    }
                    PipelineCommand::ReloadEngine => {
                        if !matches!(coordinator.session, SessionState::Idle)
                            || coordinator.engine == EngineState::Loading
                        {
                            notice(&app, "処理中のため、GPUでの再準備は後で行ってください");
                        } else if worker.device() != Some("cuda") {
                            // Pick up a newly installed runtime. A failure restores the previous engine.
                            worker.backend_mut().cuda_dir = crate::cuda_dir(&app);
                            coordinator.engine = EngineState::Loading;
                            update_gate(&coordinator, &gate);
                            publish_engine(&app, &mut coordinator, &worker, &engine_status);
                            match prepare_saved_model(&app, &mut worker).await {
                                Ok(()) => {
                                    coordinator.engine = EngineState::Ready;
                                    notice(&app, "認識エンジンを再準備しました");
                                }
                                Err(message) => {
                                    coordinator.engine = engine_state(&worker);
                                    crate::overlay::flash(&app, &message);
                                    let _ = app.emit("user_notice", message);
                                }
                            }
                            discard_queued(&mut hotkeys);
                        }
                    }
                    PipelineCommand::SaveSettings(candidate, revision, reply) => {
                        let result = apply_settings(&app, &mut worker, &mut coordinator, &gate,
                            &engine_status, *candidate, revision).await;
                        discard_after_settings(&coordinator, &mut hotkeys);
                        let _ = reply.send(result);
                    }
                }
                publish_engine(&app, &mut coordinator, &worker, &engine_status);
                update_gate(&coordinator, &gate);
            }
            _ = ticker.tick() => {
                if screen_job.as_ref().is_some_and(std::thread::JoinHandle::is_finished) {
                    screen_read = screen_job.take().and_then(|job| job.join().ok()).unwrap_or_default();
                    if app.state::<crate::candidates::ScreenWordStore>().saw(&screen_read.words) {
                        let _ = app.emit("dictionary_candidates", ());
                    }
                }
                // Esc cancels a recording; outside a recording it belongs to other programs.
                let recording = matches!(coordinator.session, SessionState::StartingCapture(_) | SessionState::Recording(_));
                if recording != escape_armed {
                    if let Some(service) = app.try_state::<crate::hotkeys::HotkeyService>() {
                        service.set_escape(recording);
                        escape_armed = recording;
                    }
                }
                loop {
                    let event = { capture.lock().expect("capture lock").try_recv() };
                    match event {
                        Ok(event) => {
                            if !on_capture_event(event, PipelineContext {
                                metrics: &mut metrics,
                                capture: &capture,
                                coordinator: &mut coordinator,
                                worker: &mut worker,
                                hotkeys: &mut hotkeys,
                                gate: &gate,
                                overlay: &overlay,
                                cancellation: &cancellation,
                                expected_generation: active_generation,
                                polish: polish_session.clone(),
                                compose: compose_session.clone(),
                                english: english_session,
                                stream: stream.take(),
                                screen: screen_read.clone(),
                            }).await {
                                return;
                            }
                            publish_engine(&app, &mut coordinator, &worker, &engine_status);
                        }
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            coordinator.cancel();
                            coordinator.engine = EngineState::Unavailable;
                            update_gate(&coordinator, &gate);
                            notice(&app, "録音サービスが停止しました。入力は行いません");
                            diagnostic_failure(&app, &metrics, FailureStage::CaptureStream, FailureCode::AudioDisconnected, None);
                            if let Some(id) = metrics.as_ref().map(|metrics| metrics.id) {
                                finish_metrics(&app, &mut metrics, id, Outcome::Failed);
                            }
                            return;
                        }
                    }
                }
                level_tick = !level_tick;
                if level_tick && matches!(coordinator.session, SessionState::Recording(_)) {
                    overlay.level(capture.lock().expect("capture lock").level());
                }
                // Recognition while speaking: about once a second, send the audio since
                // the last cut. The worker is idle while recording. The waits for the local
                // AI (its loading, a part's punctuation) exist because the GPU (6 GB) cannot
                // hold it and the Whisper recognizer at once; Parakeet runs on the CPU, so
                // it never waits for them (a cold AI model made recordings crawl).
                let cpu_engine = worker
                    .selection()
                    .is_some_and(|model| models::engine(&model.preset) != "whisper");
                if matches!(coordinator.session, SessionState::Recording(_))
                    && Instant::now() >= next_stream_step
                    && (cpu_engine
                        || warming.as_ref().is_none_or(|(job, started)| {
                            job.is_finished() || started.elapsed() >= MAX_WARM_UP_WAIT
                        }))
                    && !english_session
                    && streaming_enabled(&app)
                {
                    next_stream_step = Instant::now() + Duration::from_secs(1);
                    if stream.is_none() {
                        stream = capture.lock().expect("capture lock").tap().map(crate::streaming::StreamState::new);
                    }
                    let slow_model = worker.selection().is_some_and(|model| model.preset == "accurate");
                    if let (Some(state), Some(active_worker)) = (stream.as_mut(), worker.handle_mut()) {
                        state.pull();
                        let mut committed = false;
                        if !state.failed
                            && (cpu_engine || !state.punctuation_busy())
                            && state.pending_seconds() >= crate::streaming::MIN_PENDING_SECONDS
                        {
                            match state.pending_16k() {
                                Some(pending) => {
                                    let (dictionary, _) = text_settings(&app);
                                    let hints = crate::screen::with_hints(crate::dictionary::hints(&dictionary), &screen_read.terms);
                                    active_worker.set_topics(Vec::new());
                                    let mut keys = WaitKeys { app: &app, coordinator: &mut coordinator, capture: &capture, overlay: &overlay, metrics: &mut metrics, gate: &gate, failure: None };
                                    let step = wait_for_worker(
                                        active_worker.stream_step(uuid::Uuid::new_v4(), &pending, &hints, &state.context()),
                                        &mut hotkeys, &mut held, |command| keys.act(command),
                                    ).await;
                                    if keys.failed() {
                                        return;
                                    }
                                    let recognized = match step {
                                        Ok(step) if step.reject.is_none() => {
                                            if step.cut.is_some() && step.topic_pass {
                                                state.topic_passes += 1;
                                            }
                                            step.cut.map(|cut| (cut, step.text))
                                        }
                                        // The quick way doubted this part (too dense, repeated…).
                                        // Recognize just this part the careful way and go on,
                                        // instead of giving up for the whole recording: one
                                        // doubtful part in a long recording is likely.
                                        Ok(crate::worker::StreamStep { cut: Some(cut), reject: Some(_), .. }) => {
                                            let mut keys = WaitKeys { app: &app, coordinator: &mut coordinator, capture: &capture, overlay: &overlay, metrics: &mut metrics, gate: &gate, failure: None };
                                            let rescued = wait_for_worker(
                                                active_worker.transcribe_with_hints(uuid::Uuid::new_v4(), &pending[..cut], &hints, &state.context()),
                                                &mut hotkeys, &mut held, |command| keys.act(command),
                                            ).await;
                                            if keys.failed() {
                                                return;
                                            }
                                            match rescued
                                            {
                                                Ok(result) => {
                                                    state.rescued += 1;
                                                    Some((cut, result.text))
                                                }
                                                Err(_) => {
                                                    state.give_up("rescue_failed");
                                                    None
                                                }
                                            }
                                        }
                                        Ok(_) => {
                                            state.give_up("rejected");
                                            None
                                        }
                                        Err(_) => {
                                            state.give_up("worker_error");
                                            None
                                        }
                                    };
                                    if let Some((cut, text)) = recognized {
                                        state.commit(cut, text);
                                        committed = true;
                                        if live_preview_enabled(&app) {
                                            crate::overlay::preview_text(&app, &shown_text(&app, &state.text()), "");
                                        }
                                        let text = state.last_text().map(str::to_owned).unwrap_or_default();
                                        if let Some(punctuation) = punctuation_settings(&app)
                                            .filter(|_| polish_session.is_none())
                                            .filter(|_| crate::polish::needs_punctuation(&text, PUNCTUATION_RUN))
                                        {
                                            state.punctuate_last(spawn_punctuation(&app, punctuation, text));
                                        }
                                        // More may already be waiting.
                                        next_stream_step = Instant::now() + Duration::from_millis(200);
                                    }
                                }
                                None => state.give_up("resample"),
                            }
                        }
                        // Live text, speed first: a guess of the part still being spoken,
                        // shown after the confirmed text and replaced as more is heard.
                        // Display only: the input never uses it. One at a time, at most
                        // once a second (every 2 s with the slower accurate model).
                        // Not after a stop that came during the wait above.
                        if !committed
                            && matches!(coordinator.session, SessionState::Recording(_))
                            && !guesses_off
                            && !state.failed
                            && !state.punctuation_busy()
                            && Instant::now() >= next_guess
                            && state.pending_seconds() >= GUESS_MIN_SECONDS
                            && live_preview_enabled(&app)
                        {
                            if let Some(pending) = state.pending_16k() {
                                let start = pending.len().saturating_sub(GUESS_MAX_SECONDS * 16_000);
                                let (dictionary, _) = text_settings(&app);
                                let hints = crate::screen::with_hints(crate::dictionary::hints(&dictionary), &screen_read.terms);
                                active_worker.set_topics(Vec::new());
                                let started = Instant::now();
                                let mut keys = WaitKeys { app: &app, coordinator: &mut coordinator, capture: &capture, overlay: &overlay, metrics: &mut metrics, gate: &gate, failure: None };
                                let guess = wait_for_worker(
                                    active_worker.transcribe_with_hints(uuid::Uuid::new_v4(), &pending[start..], &hints, &state.context()),
                                    &mut hotkeys, &mut held, |command| keys.act(command),
                                ).await;
                                if keys.failed() {
                                    return;
                                }
                                let took = started.elapsed();
                                next_guess = Instant::now() + Duration::from_secs(if slow_model { 2 } else { 1 });
                                guesses_off = took >= GUESS_GIVE_UP;
                                if let Some(metrics) = metrics.as_mut() {
                                    let count = metrics.report.tentative_previews.get_or_insert(0);
                                    *count += 1;
                                    let slowest = metrics.report.tentative_max_ms.get_or_insert(0);
                                    *slowest = (*slowest).max(diagnostics::elapsed_ms(started));
                                }
                                match guess {
                                    Ok(guess) => {
                                        crate::overlay::preview_text(
                                            &app,
                                            &shown_text(&app, &state.text()),
                                            &shown_text(&app, guess.text.trim()),
                                        );
                                    }
                                    // A failed guess changes nothing; the parts go on as usual.
                                    Err(_) => guesses_off = true,
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    gate.store(UNAVAILABLE, Ordering::Release);
    drop(capture);
    let _ = worker.clear().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_copy_starts_a_new_text_otherwise_the_last_one_can_be_revised() {
        let last = Some(("前の文".to_owned(), "元のメール".to_owned()));
        assert_eq!(
            compose_session_from(Some("新しいメール".into()), last.clone()),
            ComposeSession {
                reference: "新しいメール".into(),
                previous: String::new()
            }
        );
        assert_eq!(
            compose_session_from(None, last),
            ComposeSession {
                reference: "元のメール".into(),
                previous: "前の文".into()
            }
        );
        assert_eq!(compose_session_from(None, None), ComposeSession::default());
    }

    mod drafts {
        use super::*;
        use crate::cloud::{CloudError, Provider, Timed};

        /// A fake service: Luna answers after `luna_ms`, Haiku after `haiku_ms`.
        fn fake(
            luna_ms: u64,
            luna_ok: bool,
            haiku_ms: u64,
        ) -> impl Fn(Provider, String) -> Timed + Send + Sync + 'static {
            move |provider, model| {
                let (ms, ok) = match provider {
                    Provider::OpenAi | Provider::Groq => (luna_ms, luna_ok),
                    Provider::Anthropic => (haiku_ms, true),
                };
                std::thread::sleep(Duration::from_millis(ms));
                Timed {
                    provider,
                    result: if ok {
                        Ok(format!("{model}の文"))
                    } else {
                        Err(CloudError::Rejected)
                    },
                    model,
                    seconds: ms as f64 / 1000.0,
                }
            }
        }

        fn race(
            fake: impl Fn(Provider, String) -> Timed + Send + Sync + 'static,
            backup: bool,
        ) -> (Timed, Vec<crate::diagnostics::CloudDraft>) {
            let mut reports = Vec::new();
            let draft = race_drafts(
                (Provider::OpenAi, "luna".into()),
                backup.then(|| (Provider::Anthropic, "haiku".into())),
                Duration::from_millis(100),
                &std::sync::atomic::AtomicBool::new(false),
                &mut reports,
                fake,
            );
            (draft, reports)
        }

        #[test]
        fn after_a_cancel_the_other_service_is_not_asked() {
            let mut reports = Vec::new();
            let abandoned = std::sync::atomic::AtomicBool::new(true);
            let draft = race_drafts(
                (Provider::OpenAi, "luna".into()),
                Some((Provider::Anthropic, "haiku".into())),
                Duration::from_millis(50),
                &abandoned,
                &mut reports,
                fake(300, true, 10),
            );
            // Only the chosen service was asked, although it was slower than `after`.
            assert_eq!(draft.result.unwrap(), "lunaの文");
            assert!(reports.iter().all(|report| !report.fallback));
        }

        #[test]
        fn a_prompt_answer_never_asks_the_other_service() {
            let (draft, reports) = race(fake(10, true, 10), true);
            assert_eq!(draft.result.unwrap(), "lunaの文");
            assert_eq!(reports.len(), 1);
            assert!(!reports[0].fallback);
        }

        #[test]
        fn a_slow_answer_lets_the_backup_write_the_draft() {
            let (draft, reports) = race(fake(1_000, true, 20), true);
            assert_eq!(draft.result.unwrap(), "haikuの文");
            assert!(reports.iter().any(|report| report.fallback && report.ok));
        }

        #[test]
        fn the_slow_service_is_still_used_when_it_answers_first() {
            let (draft, reports) = race(fake(150, true, 1_000), true);
            assert_eq!(draft.result.unwrap(), "lunaの文");
            assert!(!reports.iter().any(|report| report.fallback && report.ok));
        }

        #[test]
        fn a_failed_slow_answer_waits_for_the_backup_and_no_backup_means_waiting() {
            let (draft, _) = race(fake(300, false, 500), true);
            assert_eq!(draft.result.unwrap(), "haikuの文");
            let (draft, reports) = race(fake(300, true, 10), false);
            assert_eq!(draft.result.unwrap(), "lunaの文");
            assert_eq!(reports.len(), 1);
        }

        #[test]
        fn the_wait_grows_with_the_transcript() {
            assert_eq!(fallback_after("短い"), Duration::from_secs(6));
            assert_eq!(fallback_after(&"あ".repeat(700)), Duration::from_secs(10));
            assert_eq!(
                fallback_after(&"あ".repeat(10_000)),
                Duration::from_secs(20)
            );
        }
    }

    #[tokio::test]
    async fn recording_or_loading_rejects_settings_without_queueing() {
        let (pipeline, mut receiver) = PipelineSender::new();
        for phase in [LOADING, STARTING, RECORDING, BUSY, DELIVERING] {
            pipeline.gate.store(phase, Ordering::Release);
            assert!(pipeline
                .save_settings(Settings::default(), 0)
                .await
                .is_err());
            assert!(receiver.try_recv().is_err());
        }
    }

    #[test]
    fn a_recording_that_won_the_settings_race_keeps_its_cancel_command() {
        let (pipeline, mut receiver) = PipelineSender::new();
        assert!(pipeline.sender.try_send(PipelineCommand::Cancel).is_ok());
        let mut coordinator = Coordinator {
            engine: EngineState::Ready,
            ..Coordinator::default()
        };
        assert!(matches!(coordinator.on_f8(None), Action::StartCapture(_)));
        discard_after_settings(&coordinator, &mut receiver);
        assert!(matches!(receiver.try_recv(), Ok(PipelineCommand::Cancel)));
    }

    fn hotkey(event: HotkeyEvent) -> PipelineCommand {
        PipelineCommand::Hotkey(event, None, 0, Instant::now())
    }

    #[test]
    fn only_stop_and_cancel_act_during_a_wait_and_only_while_recording() {
        let mut coordinator = Coordinator {
            engine: EngineState::Ready,
            ..Coordinator::default()
        };
        assert!(!is_urgent(
            &hotkey(HotkeyEvent::Record),
            &coordinator.session
        ));
        assert!(!is_urgent(&PipelineCommand::Cancel, &coordinator.session));
        let Action::StartCapture(id) = coordinator.on_f8(None) else {
            panic!("no recording")
        };
        coordinator.capture_started(id);
        assert!(matches!(coordinator.session, SessionState::Recording(_)));
        for event in [
            HotkeyEvent::Record,
            HotkeyEvent::Polish,
            HotkeyEvent::Compose,
            HotkeyEvent::English,
        ] {
            assert!(is_urgent(&hotkey(event), &coordinator.session));
        }
        assert!(is_urgent(&PipelineCommand::Cancel, &coordinator.session));
        assert!(!is_urgent(
            &hotkey(HotkeyEvent::Repaste),
            &coordinator.session
        ));
        assert!(!is_urgent(
            &PipelineCommand::RetryEngine,
            &coordinator.session
        ));
        assert!(!is_urgent(
            &PipelineCommand::CopyLastResult(0),
            &coordinator.session
        ));
        // Stopping: a second stop waits (the main loop says "busy"); cancel still acts.
        assert!(matches!(coordinator.on_f8(None), Action::StopCapture(_)));
        assert!(!is_urgent(
            &hotkey(HotkeyEvent::Record),
            &coordinator.session
        ));
        assert!(is_urgent(&PipelineCommand::Cancel, &coordinator.session));
    }

    #[tokio::test]
    async fn the_stop_key_acts_while_a_slow_worker_is_still_working() {
        let (pipeline, mut receiver) = PipelineSender::new();
        let mut held = std::collections::VecDeque::new();
        let started = Instant::now();
        // A worker that takes 400 ms; the stop key comes after 30 ms.
        let slow_worker = async {
            tokio::time::sleep(Duration::from_millis(400)).await;
            "answer"
        };
        let sender = pipeline.sender.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            let _ = sender.send(PipelineCommand::Cancel).await;
            let _ = sender.send(PipelineCommand::RetryEngine).await;
            let _ = sender.send(PipelineCommand::CopyLastResult(7)).await;
        });
        let mut acted_at = None;
        let answer = wait_for_worker(
            slow_worker,
            &mut receiver,
            &mut held,
            |command| match command {
                PipelineCommand::Cancel => {
                    acted_at = Some(started.elapsed());
                    None
                }
                other => Some(other),
            },
        )
        .await;
        // The worker's answer is still taken, not dropped.
        assert_eq!(answer, "answer");
        let acted_at = acted_at.expect("the cancel key was not handled during the wait");
        assert!(acted_at < Duration::from_millis(300), "{acted_at:?}");
        assert!(started.elapsed() >= Duration::from_millis(400));
        // The rest waits for the main loop, in order, and comes before newer commands.
        pipeline.sender.try_send(PipelineCommand::Cancel).unwrap();
        assert!(matches!(
            next_command(&mut held, &mut receiver).await,
            Some(PipelineCommand::RetryEngine)
        ));
        assert!(matches!(
            next_command(&mut held, &mut receiver).await,
            Some(PipelineCommand::CopyLastResult(7))
        ));
        assert!(matches!(
            next_command(&mut held, &mut receiver).await,
            Some(PipelineCommand::Cancel)
        ));
    }

    #[tokio::test]
    async fn a_closed_command_channel_does_not_end_the_wait_early() {
        let (pipeline, mut receiver) = PipelineSender::new();
        drop(pipeline);
        let mut held = std::collections::VecDeque::new();
        let answer = wait_for_worker(
            async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                1
            },
            &mut receiver,
            &mut held,
            Some,
        )
        .await;
        assert_eq!(answer, 1);
        assert!(held.is_empty());
        assert!(next_command(&mut held, &mut receiver).await.is_none());
    }

    #[tokio::test]
    async fn disconnected_settings_channel_releases_reservation() {
        let (pipeline, receiver) = PipelineSender::new();
        drop(receiver);
        pipeline.gate.store(IDLE, Ordering::Release);
        assert!(pipeline
            .save_settings(Settings::default(), 0)
            .await
            .is_err());
        assert_eq!(pipeline.gate.load(Ordering::Acquire), IDLE);
    }

    #[tokio::test]
    async fn queued_settings_request_gets_a_reply_when_discarded() {
        let (pipeline, mut receiver) = PipelineSender::new();
        let (reply, response) = oneshot::channel();
        pipeline
            .sender
            .try_send(PipelineCommand::SaveSettings(Box::default(), 0, reply))
            .unwrap();
        discard_queued(&mut receiver);
        assert!(response.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn settings_request_reserves_gate_before_worker_receives_it() {
        let (pipeline, mut receiver) = PipelineSender::new();
        pipeline.gate.store(IDLE, Ordering::Release);
        let request = pipeline.clone();
        let task = tokio::spawn(async move { request.save_settings(Settings::default(), 0).await });
        let command = receiver.recv().await.unwrap();
        assert_eq!(pipeline.gate.load(Ordering::Acquire), LOADING);
        assert!(pipeline
            .save_settings(Settings::default(), 0)
            .await
            .is_err());
        reject_settings(command);
        assert!(task.await.unwrap().is_err());
    }
}
