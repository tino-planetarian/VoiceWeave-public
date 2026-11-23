use serde_json::{json, Value};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use uuid::Uuid;

const MAX_HEADER: usize = 65_536;
const MAX_PAYLOAD: usize = 19_200_000;

#[derive(Debug, PartialEq, Eq)]
pub enum WorkerError {
    Launch,
    Timeout,
    Io,
    Protocol,
    Remote(&'static str),
    ModelMissing,
    InvalidAudio,
    Termination,
}

struct Frame {
    header: Value,
    payload: Vec<u8>,
}

impl WorkerError {
    pub fn diagnostic(&self) -> crate::diagnostics::FailureCode {
        use crate::diagnostics::FailureCode;
        match self {
            Self::Launch => FailureCode::WorkerLaunch,
            Self::Timeout => FailureCode::WorkerTimeout,
            Self::Io => FailureCode::WorkerIo,
            Self::Protocol => FailureCode::WorkerProtocol,
            Self::Remote("MODEL_NOT_READY") => FailureCode::WorkerNotReady,
            Self::Remote("INVALID_AUDIO_FORMAT") => FailureCode::WorkerAudioFormat,
            Self::Remote("INVALID_AUDIO_LENGTH") => FailureCode::WorkerAudioLength,
            Self::Remote("INVALID_AUDIO_VALUES") => FailureCode::WorkerAudioValues,
            Self::Remote(_) => FailureCode::WorkerRemote,
            Self::ModelMissing => FailureCode::WorkerModelMissing,
            Self::InvalidAudio => FailureCode::WorkerInvalidAudio,
            Self::Termination => FailureCode::WorkerTermination,
        }
    }
}

pub struct TranscriptionResult {
    pub text: String,
    pub timings: WorkerTimings,
    /// Recognized again with the glossary topic's terms (worker `topics.py`).
    pub topic_pass: bool,
}

/// One step of recognition while speaking.
#[derive(Debug, PartialEq, Eq)]
pub struct StreamStep {
    /// Samples (16 kHz) recognized from the start of the sent audio; None: no finished
    /// pause yet, nothing was recognized.
    pub cut: Option<usize>,
    pub text: String,
    /// A fixed code when the part could not be trusted.
    pub reject: Option<String>,
    /// Recognized again with the glossary topic's terms.
    pub topic_pass: bool,
}

/// Whether the worker used the topic second pass ("topic_terms" above zero).
fn topic_pass(response: &Value) -> bool {
    response
        .get("topic_terms")
        .and_then(Value::as_u64)
        .is_some_and(|count| count > 0)
}

fn audio_payload(samples: &[f32]) -> Result<Vec<u8>, WorkerError> {
    if samples.is_empty()
        || samples.len() > MAX_PAYLOAD / 4
        || samples.iter().any(|x| !x.is_finite() || x.abs() > 1.0)
    {
        return Err(WorkerError::InvalidAudio);
    }
    let mut payload = Vec::with_capacity(samples.len() * 4);
    for sample in samples {
        payload.extend_from_slice(&sample.to_le_bytes());
    }
    Ok(payload)
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct WorkerTimings {
    pub total_ms: Option<u64>,
    pub frontend_ms: Option<u64>,
    pub decode_ms: Option<u64>,
    pub path: Option<crate::diagnostics::RecognitionPath>,
    pub fallback_reason: Option<crate::diagnostics::FallbackReason>,
}

impl WorkerTimings {
    fn from_response(response: &Value) -> Self {
        // Additive v1 metadata: old workers remain usable. Invalid diagnostics
        // must not discard an otherwise valid recognition result.
        let duration = |key| {
            response
                .get(key)
                .and_then(Value::as_u64)
                .filter(|value| *value <= 86_400_000)
        };
        let total_ms = duration("elapsed_ms");
        let frontend_ms = duration("frontend_ms");
        let decode_ms = duration("decode_ms");
        let path = crate::diagnostics::RecognitionPath::parse(
            response.get("recognition_path").and_then(Value::as_str),
        );
        let fallback_reason = crate::diagnostics::FallbackReason::parse(
            response.get("fallback_reason").and_then(Value::as_str),
        );
        if let (Some(total), Some(frontend), Some(decode)) = (total_ms, frontend_ms, decode_ms) {
            if frontend.saturating_add(decode).abs_diff(total) > 2 {
                return Self {
                    total_ms,
                    path,
                    fallback_reason,
                    ..Self::default()
                };
            }
        }
        Self {
            total_ms,
            frontend_ms,
            decode_ms,
            path,
            fallback_reason,
        }
    }
}

/// Stop the worker and every process it started. A development worker runs through the
/// virtual environment's python.exe, a launcher whose child is the real interpreter:
/// killing only the launcher left that child running with about 2 GB of GPU memory, and
/// every later start then ran out of memory (seen 2026-10-07).
fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    if let Some(pid) = child.id() {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    let _ = child.start_kill();
}

async fn terminate_child(child: &mut Child) -> Result<(), WorkerError> {
    kill_tree(child);
    timeout(Duration::from_secs(5), child.wait())
        .await
        .map_err(|_| WorkerError::Termination)?
        .map_err(|_| WorkerError::Termination)?;
    Ok(())
}

async fn read_frame(stdout: &mut ChildStdout) -> Result<Frame, WorkerError> {
    let mut prefix = [0_u8; 4];
    stdout
        .read_exact(&mut prefix)
        .await
        .map_err(|_| WorkerError::Io)?;
    let length = u32::from_le_bytes(prefix) as usize;
    if length == 0 || length > MAX_HEADER {
        return Err(WorkerError::Protocol);
    }
    let mut raw = vec![0_u8; length];
    stdout
        .read_exact(&mut raw)
        .await
        .map_err(|_| WorkerError::Io)?;
    let header: Value = serde_json::from_slice(&raw).map_err(|_| WorkerError::Protocol)?;
    if !header.is_object() || header.get("v").and_then(Value::as_u64) != Some(1) {
        return Err(WorkerError::Protocol);
    }
    if header.get("kind").and_then(Value::as_str).is_none() || !header.get("request_id").is_some() {
        return Err(WorkerError::Protocol);
    }
    let payload_len = header
        .get("payload_bytes")
        .and_then(Value::as_u64)
        .ok_or(WorkerError::Protocol)? as usize;
    if payload_len > MAX_PAYLOAD {
        return Err(WorkerError::Protocol);
    }
    let mut payload = vec![0_u8; payload_len];
    stdout
        .read_exact(&mut payload)
        .await
        .map_err(|_| WorkerError::Io)?;
    Ok(Frame { header, payload })
}

async fn write_frame(
    stdin: &mut ChildStdin,
    mut header: Value,
    payload: &[u8],
) -> Result<(), WorkerError> {
    if payload.len() > MAX_PAYLOAD {
        return Err(WorkerError::InvalidAudio);
    }
    let object = header.as_object_mut().ok_or(WorkerError::Protocol)?;
    object.insert("v".into(), json!(1));
    object.insert("payload_bytes".into(), json!(payload.len()));
    let raw = serde_json::to_vec(&header).map_err(|_| WorkerError::Protocol)?;
    if raw.is_empty() || raw.len() > MAX_HEADER {
        return Err(WorkerError::Protocol);
    }
    stdin
        .write_all(&(raw.len() as u32).to_le_bytes())
        .await
        .map_err(|_| WorkerError::Io)?;
    stdin.write_all(&raw).await.map_err(|_| WorkerError::Io)?;
    stdin
        .write_all(payload)
        .await
        .map_err(|_| WorkerError::Io)?;
    stdin.flush().await.map_err(|_| WorkerError::Io)
}

/// The shortened-window fast path is on unless `VOICEWEAVE_FAST_WINDOW=off` (comparison).
fn fast_window() -> bool {
    std::env::var("VOICEWEAVE_FAST_WINDOW").as_deref() != Ok("off")
}

/// Loading 1.6 GB of weights stalls under heavy paging; allow for that before giving up.
const PREPARE_TIMEOUT: Duration = Duration::from_secs(300);

pub struct Worker {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
    stderr_drain: JoinHandle<()>,
    generation: u64,
    device: &'static str,
    /// The field glossary by topic (one list per line), sent with each recognition so the
    /// worker can recognize a part again with its topic's terms (worker `topics.py`).
    topics: Vec<Vec<String>>,
}

impl Worker {
    pub async fn start(
        exe: &Path,
        args: &[&str],
        cwd: &Path,
        generation: u64,
    ) -> Result<(Self, String), WorkerError> {
        if !exe.is_file() {
            return Err(WorkerError::Launch);
        }
        let mut command = Command::new(exe);
        command
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.as_std_mut().creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().map_err(|_| WorkerError::Launch)?;
        // Ends with the app even after a crash or forced exit (best effort).
        crate::job::contain_child(&child);
        let stdin = child.stdin.take().ok_or(WorkerError::Launch)?;
        let mut stdout = child.stdout.take().ok_or(WorkerError::Launch)?;
        let mut stderr = child.stderr.take().ok_or(WorkerError::Launch)?;
        let stderr_drain = tokio::spawn(async move {
            let mut buffer = [0_u8; 4096];
            while let Ok(count) = stderr.read(&mut buffer).await {
                if count == 0 {
                    break;
                }
            }
        });
        let hello = match timeout(Duration::from_secs(10), read_frame(&mut stdout)).await {
            Ok(Ok(frame)) => frame,
            Ok(Err(error)) => {
                stderr_drain.abort();
                terminate_child(&mut child).await?;
                return Err(error);
            }
            Err(_) => {
                stderr_drain.abort();
                terminate_child(&mut child).await?;
                return Err(WorkerError::Timeout);
            }
        };
        if hello.header.get("kind").and_then(Value::as_str) != Some("hello")
            || hello.header.get("request_id") != Some(&Value::Null)
            || !hello.payload.is_empty()
        {
            stderr_drain.abort();
            terminate_child(&mut child).await?;
            return Err(WorkerError::Protocol);
        }
        let Some(version) = hello.header.get("worker_version").and_then(Value::as_str) else {
            stderr_drain.abort();
            terminate_child(&mut child).await?;
            return Err(WorkerError::Protocol);
        };
        if version.is_empty() {
            stderr_drain.abort();
            terminate_child(&mut child).await?;
            return Err(WorkerError::Protocol);
        }
        let version = version.to_owned();
        Ok((
            Self {
                child,
                stdin,
                stdout,
                stderr_drain,
                generation,
                device: "cpu",
                topics: Vec::new(),
            },
            version,
        ))
    }

    /// The glossary topics for the following recognitions (empty: no second pass).
    pub fn set_topics(&mut self, topics: Vec<Vec<String>>) {
        self.topics = topics;
    }

    /// The device the loaded model actually runs on ("cpu" until a prepare succeeds).
    pub fn device(&self) -> &'static str {
        self.device
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    async fn request(
        &mut self,
        kind: &str,
        fields: Value,
        payload: &[u8],
        duration: Duration,
    ) -> Result<Value, WorkerError> {
        let expected_reply = match kind {
            "prepare" | "prepare_english" => "ready",
            "transcribe" => "result",
            "stream_step" => "stream_result",
            "shutdown" => "bye",
            _ => return Err(WorkerError::Protocol),
        };
        let request_id = Uuid::new_v4().to_string();
        let mut header = fields;
        let object = header.as_object_mut().ok_or(WorkerError::Protocol)?;
        object.insert("kind".into(), json!(kind));
        object.insert("request_id".into(), json!(request_id));
        let result = timeout(duration, async {
            write_frame(&mut self.stdin, header, payload).await?;
            loop {
                let frame = read_frame(&mut self.stdout).await?;
                if !frame.payload.is_empty()
                    || frame.header.get("request_id").and_then(Value::as_str)
                        != Some(request_id.as_str())
                {
                    return Err(WorkerError::Protocol);
                }
                match frame.header.get("kind").and_then(Value::as_str) {
                    Some("progress") => continue,
                    Some(reply) if reply == expected_reply => return Ok(frame.header),
                    Some("error") => {
                        let error = match frame.header.get("code").and_then(Value::as_str) {
                            Some("MODEL_MISSING") => WorkerError::ModelMissing,
                            Some("MODEL_NOT_READY") => WorkerError::Remote("MODEL_NOT_READY"),
                            Some("INVALID_AUDIO_FORMAT") => {
                                WorkerError::Remote("INVALID_AUDIO_FORMAT")
                            }
                            Some("INVALID_AUDIO_LENGTH") => {
                                WorkerError::Remote("INVALID_AUDIO_LENGTH")
                            }
                            Some("INVALID_AUDIO_VALUES") => {
                                WorkerError::Remote("INVALID_AUDIO_VALUES")
                            }
                            _ => WorkerError::Remote("WORKER_OPERATION_FAILED"),
                        };
                        return Err(error);
                    }
                    _ => return Err(WorkerError::Protocol),
                }
            }
        })
        .await;
        match result {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(error)) => {
                if error != WorkerError::ModelMissing && !matches!(error, WorkerError::Remote(_)) {
                    kill_tree(&mut self.child);
                }
                Err(error)
            }
            Err(_) => {
                kill_tree(&mut self.child);
                Err(WorkerError::Timeout)
            }
        }
    }

    /// Parakeet (experimental): sherpa-onnx on the CPU. The worker checks
    /// the engine's own files.
    pub async fn prepare_engine(
        &mut self,
        model_dir: &Path,
        engine: &str,
    ) -> Result<(), WorkerError> {
        self.prepare(
            model_dir,
            json!({"device":"cpu","engine":engine,"cpu_threads":4}),
            "cpu",
        )
        .await
    }

    pub async fn prepare_cpu(&mut self, model_dir: &Path) -> Result<(), WorkerError> {
        self.prepare(
            model_dir,
            json!({"device":"cpu","compute_type":"int8","cpu_threads":4}),
            "cpu",
        )
        .await
    }

    /// `cuda_dir` must hold the CUDA 12 cuBLAS runtime; the worker verifies this.
    pub async fn prepare_cuda(
        &mut self,
        model_dir: &Path,
        cuda_dir: &Path,
    ) -> Result<(), WorkerError> {
        let cuda_dir = cuda_dir.to_str().ok_or(WorkerError::ModelMissing)?;
        self.prepare(
            model_dir,
            json!({"device":"cuda","compute_type":"int8_float16","cuda_dir":cuda_dir}),
            "cuda",
        )
        .await
    }

    async fn prepare(
        &mut self,
        model_dir: &Path,
        mut settings: Value,
        device: &'static str,
    ) -> Result<(), WorkerError> {
        let whisper = settings.get("engine").is_none();
        if !model_dir.is_absolute() || (whisper && !model_dir.join("model.bin").is_file()) {
            return Err(WorkerError::ModelMissing);
        }
        let path = model_dir.to_str().ok_or(WorkerError::ModelMissing)?;
        settings["model_dir"] = json!(path);
        let response = self
            .request("prepare", settings, &[], PREPARE_TIMEOUT)
            .await?;
        // Older workers omit the device; that is only acceptable for the CPU default.
        let reported = response.get("device").and_then(Value::as_str);
        if response.get("kind").and_then(Value::as_str) != Some("ready")
            || !(reported == Some(device) || (device == "cpu" && reported.is_none()))
        {
            return Err(WorkerError::Protocol);
        }
        self.device = device;
        Ok(())
    }

    /// Load the English recognizer next to the main one (the English shortcut). It runs
    /// on the CPU whatever the main one uses.
    pub async fn prepare_english(&mut self, model_dir: &Path) -> Result<(), WorkerError> {
        if !model_dir.is_absolute() {
            return Err(WorkerError::ModelMissing);
        }
        let path = model_dir.to_str().ok_or(WorkerError::ModelMissing)?;
        let response = self
            .request(
                "prepare_english",
                json!({"model_dir":path,"cpu_threads":4}),
                &[],
                PREPARE_TIMEOUT,
            )
            .await?;
        if response.get("kind").and_then(Value::as_str) != Some("ready") {
            return Err(WorkerError::Protocol);
        }
        Ok(())
    }

    pub async fn transcribe(
        &mut self,
        session_id: Uuid,
        samples: &[f32],
    ) -> Result<String, WorkerError> {
        Ok(self.transcribe_measured(session_id, samples).await?.text)
    }

    pub async fn transcribe_measured(
        &mut self,
        session_id: Uuid,
        samples: &[f32],
    ) -> Result<TranscriptionResult, WorkerError> {
        self.transcribe_with_hints(session_id, samples, &[], "")
            .await
    }

    /// Recognize with the user's dictionary terms as hints (already length-limited).
    pub async fn transcribe_with_hints(
        &mut self,
        session_id: Uuid,
        samples: &[f32],
        hints: &[String],
        context: &str,
    ) -> Result<TranscriptionResult, WorkerError> {
        self.transcribe_in(session_id, samples, hints, context, None)
            .await
    }

    /// Recognize a whole recording as English (the English recognizer; no hints).
    pub async fn transcribe_english(
        &mut self,
        session_id: Uuid,
        samples: &[f32],
    ) -> Result<TranscriptionResult, WorkerError> {
        self.transcribe_in(session_id, samples, &[], "", Some("en"))
            .await
    }

    async fn transcribe_in(
        &mut self,
        session_id: Uuid,
        samples: &[f32],
        hints: &[String],
        context: &str,
        language: Option<&str>,
    ) -> Result<TranscriptionResult, WorkerError> {
        let payload = audio_payload(samples)?;
        let seconds = samples.len().div_ceil(16_000) as u64;
        // On the GPU a request takes a fraction of the audio's length; one far slower has
        // stalled (GPU memory taken by other programs), and the app recovers sooner. The
        // English recognizer always runs on the CPU.
        let duration = if self.device == "cuda" && language.is_none() {
            Duration::from_secs(30_u64.max(seconds.saturating_mul(3)))
        } else {
            Duration::from_secs(60_u64.max(seconds.saturating_mul(10)))
        };
        let response = self.request("transcribe", json!({"session_id":session_id,"sample_rate":16000,"channels":1,"sample_count":samples.len(),"beam_size":1,"dictionary_hints":hints,"context":context,"fast_window":fast_window(),"topics":self.topics,"language":language}), &payload, duration).await?;
        if response.get("kind").and_then(Value::as_str) != Some("result")
            || response.get("session_id").and_then(Value::as_str)
                != Some(session_id.to_string().as_str())
        {
            return Err(WorkerError::Protocol);
        }
        let text = response
            .get("raw_text")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(WorkerError::Protocol)?;
        Ok(TranscriptionResult {
            text,
            timings: WorkerTimings::from_response(&response),
            topic_pass: topic_pass(&response),
        })
    }

    /// Recognize the part of `samples` before its last finished pause, if any (recognition
    /// while speaking). `context` is the text recognized before these samples.
    pub async fn stream_step(
        &mut self,
        session_id: Uuid,
        samples: &[f32],
        hints: &[String],
        context: &str,
    ) -> Result<StreamStep, WorkerError> {
        let payload = audio_payload(samples)?;
        let response = self
            .request(
                "stream_step",
                json!({"session_id":session_id,"sample_rate":16000,"channels":1,"sample_count":samples.len(),"dictionary_hints":hints,"context":context,"topics":self.topics}),
                &payload,
                Duration::from_secs(30),
            )
            .await?;
        if response.get("session_id").and_then(Value::as_str)
            != Some(session_id.to_string().as_str())
        {
            return Err(WorkerError::Protocol);
        }
        let cut = match response.get("cut") {
            Some(Value::Null) | None => None,
            Some(value) => Some(
                value
                    .as_u64()
                    .and_then(|cut| usize::try_from(cut).ok())
                    .filter(|cut| *cut <= samples.len())
                    .ok_or(WorkerError::Protocol)?,
            ),
        };
        Ok(StreamStep {
            cut,
            text: response
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            reject: response
                .get("reject")
                .and_then(Value::as_str)
                .map(str::to_owned),
            topic_pass: topic_pass(&response),
        })
    }

    /// Confirm process exit before loading a replacement model.
    pub async fn terminate(mut self) -> Result<(), WorkerError> {
        terminate_child(&mut self.child).await
    }

    pub async fn shutdown(mut self) -> Result<(), WorkerError> {
        let result = self
            .request("shutdown", json!({}), &[], Duration::from_secs(5))
            .await;
        if result
            .as_ref()
            .ok()
            .and_then(|v| v.get("kind"))
            .and_then(Value::as_str)
            != Some("bye")
        {
            kill_tree(&mut self.child);
            return Err(result.err().unwrap_or(WorkerError::Protocol));
        }
        timeout(Duration::from_secs(5), self.child.wait())
            .await
            .map_err(|_| WorkerError::Timeout)?
            .map_err(|_| WorkerError::Io)?;
        Ok(())
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        kill_tree(&mut self.child);
        self.stderr_drain.abort();
    }
}

#[cfg(test)]
mod timing_tests {
    use super::*;

    #[test]
    fn absent_invalid_and_inconsistent_timings_do_not_become_fake_measurements() {
        assert_eq!(
            WorkerTimings::from_response(&json!({"raw_text":"synthetic"})),
            WorkerTimings::default()
        );
        let timings = WorkerTimings::from_response(
            &json!({"elapsed_ms":9000,"frontend_ms":200,"decode_ms":8800}),
        );
        assert_eq!(timings.total_ms, Some(9000));
        assert_eq!(timings.decode_ms, Some(8800));
        for value in [
            json!(-1),
            json!("private error"),
            json!(true),
            json!(1.5),
            json!(86_400_001_u64),
        ] {
            let timings = WorkerTimings::from_response(&json!({"elapsed_ms":value}));
            assert_eq!(timings.total_ms, None);
        }
        let timings = WorkerTimings::from_response(
            &json!({"elapsed_ms":10,"frontend_ms":300,"decode_ms":200}),
        );
        assert_eq!(timings.total_ms, Some(10));
        assert_eq!(timings.frontend_ms, None);
        assert_eq!(timings.decode_ms, None);
        let path = |value| {
            WorkerTimings::from_response(&json!({"elapsed_ms":10,"recognition_path":value})).path
        };
        use crate::diagnostics::RecognitionPath;
        assert_eq!(path(json!("short")), Some(RecognitionPath::Short));
        assert_eq!(
            path(json!("short_fallback")),
            Some(RecognitionPath::ShortFallback)
        );
        assert_eq!(path(json!("full")), Some(RecognitionPath::Full));
        for invalid in [json!("private/path"), json!(3), json!(null)] {
            assert_eq!(path(invalid), None);
        }
        let reason = |value| {
            WorkerTimings::from_response(&json!({"elapsed_ms":10,"fallback_reason":value}))
                .fallback_reason
        };
        use crate::diagnostics::FallbackReason;
        assert_eq!(reason(json!("no_speech")), Some(FallbackReason::NoSpeech));
        assert_eq!(reason(json!("repeat")), Some(FallbackReason::Repeat));
        assert_eq!(reason(json!("recognized words")), None);
    }
}
