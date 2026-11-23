//! Polishing with a cloud model (OpenAI or Anthropic), chosen by the user for drafts of
//! a quality the PC's own models cannot reach. Only the transcript text is sent, and
//! only for recordings made with the polish shortcut; audio never leaves the PC.
//!
//! Cloud recognition (Groq, the "クラウドで認識" mode) is the exception: there the
//! recording itself is sent, only when the user chose that mode (see `transcribe`).
//!
//! HTTPS goes through Windows' own WinHTTP (system TLS and proxy settings), and the API
//! keys are kept in the Windows Credential Manager, never in the settings file or logs.
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Networking::WinHttp::{
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryDataAvailable,
    WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
    WinHttpSetTimeouts, INTERNET_DEFAULT_HTTPS_PORT, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
    WINHTTP_FLAG_SECURE, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
};
use windows_sys::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
    CRED_TYPE_GENERIC,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    OpenAi,
    Anthropic,
    /// Speech recognition only (cloud recognition mode), never drafts.
    Groq,
}

impl Provider {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "openai" => Some(Self::OpenAi),
            "anthropic" => Some(Self::Anthropic),
            "groq" => Some(Self::Groq),
            _ => None,
        }
    }

    fn host(self) -> &'static str {
        match self {
            Self::OpenAi => "api.openai.com",
            Self::Anthropic => "api.anthropic.com",
            Self::Groq => "api.groq.com",
        }
    }

    fn credential_name(self) -> &'static str {
        match self {
            Self::OpenAi => "VoiceWeave/openai-api-key",
            Self::Anthropic => "VoiceWeave/anthropic-api-key",
            Self::Groq => "VoiceWeave/groq-api-key",
        }
    }

    /// The name used in settings and diagnostics.
    pub fn id(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
            Self::Groq => "groq",
        }
    }

    /// Shown in the comparison output and on screen.
    pub fn label(self) -> &'static str {
        match self {
            Self::OpenAi => "OpenAI",
            Self::Anthropic => "Anthropic",
            Self::Groq => "Groq",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CloudError {
    /// No API key saved for this service.
    NoKey,
    /// No connection (offline, DNS, TLS or proxy failure).
    Unreachable,
    Timeout,
    /// 401/403: the key is wrong or revoked.
    Unauthorized,
    /// 429: rate limit or no credit left.
    Limited,
    /// Another HTTP error, for example an unknown model name.
    Rejected,
    BadResponse,
    Empty,
}

impl CloudError {
    /// Shown on the settings screen after checking the connection.
    pub fn status(self) -> &'static str {
        match self {
            Self::NoKey => "APIキーが未登録です",
            Self::Unreachable => "接続できません（ネットワークを確認してください）",
            Self::Timeout => "応答が時間内に来ませんでした",
            Self::Unauthorized => "APIキーが正しくないか、無効になっています",
            Self::Limited => "利用上限に達したか、残高が足りません",
            Self::Rejected => "要求を受け付けませんでした",
            Self::BadResponse | Self::Empty => "応答を読めませんでした",
        }
    }

    /// Shown briefly on screen after the faithful transcript was used instead.
    pub fn notice(self) -> &'static str {
        match self {
            Self::NoKey => "外部AIのAPIキーが未登録のため、元の文を入力しました",
            Self::Unreachable => "外部AIに接続できないため、元の文を入力しました",
            Self::Timeout => "外部AIの応答が時間内に来なかったため、元の文を入力しました",
            Self::Unauthorized => "外部AIのAPIキーが正しくないため、元の文を入力しました",
            Self::Limited => "外部AIの利用上限か残高不足のため、元の文を入力しました",
            Self::Rejected => {
                "外部AIが要求を受け付けなかったため（モデル名など）、元の文を入力しました"
            }
            Self::BadResponse | Self::Empty => {
                "外部AIの応答を読めなかったため、元の文を入力しました"
            }
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

// ---- API keys (Windows Credential Manager) ----

pub fn save_key(provider: Provider, key: &str) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() || key.len() > 512 || key.chars().any(char::is_whitespace) {
        return Err("APIキーの形式が正しくありません".into());
    }
    let mut name = wide(provider.credential_name());
    let mut user = wide("VoiceWeave");
    let mut blob = key.as_bytes().to_vec();
    let credential = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: name.as_mut_ptr(),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        UserName: user.as_mut_ptr(),
        ..Default::default()
    };
    let saved = unsafe { CredWriteW(&credential, 0) } != 0;
    blob.fill(0);
    if saved {
        Ok(())
    } else {
        Err("APIキーをWindowsの資格情報に保存できませんでした".into())
    }
}

pub fn load_key(provider: Provider) -> Option<String> {
    let name = wide(provider.credential_name());
    let mut credential: *mut CREDENTIALW = std::ptr::null_mut();
    if unsafe { CredReadW(name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) } == 0 {
        return None;
    }
    let key = unsafe {
        let blob = std::slice::from_raw_parts(
            (*credential).CredentialBlob,
            (*credential).CredentialBlobSize as usize,
        );
        let key = String::from_utf8(blob.to_vec()).ok();
        CredFree(credential as *const _);
        key
    };
    key.filter(|key| !key.is_empty())
}

pub fn has_key(provider: Provider) -> bool {
    load_key(provider).is_some()
}

pub fn delete_key(provider: Provider) {
    let name = wide(provider.credential_name());
    unsafe { CredDeleteW(name.as_ptr(), CRED_TYPE_GENERIC, 0) };
}

// ---- HTTPS (WinHTTP) ----

struct Handle(*mut core::ffi::c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

/// One HTTPS request to `host`. Returns the status code and body.
fn https(
    host: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
    timeout: Duration,
) -> Result<(u32, Vec<u8>), CloudError> {
    const ERROR_WINHTTP_TIMEOUT: u32 = 12002;
    let fail = || {
        if unsafe { GetLastError() } == ERROR_WINHTTP_TIMEOUT {
            CloudError::Timeout
        } else {
            CloudError::Unreachable
        }
    };
    let millis = timeout.as_millis().min(i32::MAX as u128) as i32;
    unsafe {
        let session = Handle(WinHttpOpen(
            wide("VoiceWeave").as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            std::ptr::null(),
            std::ptr::null(),
            0,
        ));
        if session.0.is_null() {
            return Err(CloudError::Unreachable);
        }
        WinHttpSetTimeouts(session.0, millis, millis, millis, millis);
        let connection = Handle(WinHttpConnect(
            session.0,
            wide(host).as_ptr(),
            INTERNET_DEFAULT_HTTPS_PORT,
            0,
        ));
        if connection.0.is_null() {
            return Err(fail());
        }
        let request = Handle(WinHttpOpenRequest(
            connection.0,
            wide(method).as_ptr(),
            wide(path).as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ));
        if request.0.is_null() {
            return Err(fail());
        }
        let header_text: String = headers
            .iter()
            .map(|(name, value)| format!("{name}: {value}\r\n"))
            .collect();
        let header_wide = wide(&header_text);
        let (data, length) = match body {
            Some(body) => (body.as_ptr() as *const core::ffi::c_void, body.len() as u32),
            None => (std::ptr::null(), 0),
        };
        if WinHttpSendRequest(
            request.0,
            header_wide.as_ptr(),
            u32::MAX, // the header text is null-terminated
            data,
            length,
            length,
            0,
        ) == 0
            || WinHttpReceiveResponse(request.0, std::ptr::null_mut()) == 0
        {
            return Err(fail());
        }
        let mut status: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        if WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            std::ptr::null(),
            &mut status as *mut u32 as *mut core::ffi::c_void,
            &mut size,
            std::ptr::null_mut(),
        ) == 0
        {
            return Err(CloudError::BadResponse);
        }
        let mut out = Vec::new();
        loop {
            let mut available: u32 = 0;
            if WinHttpQueryDataAvailable(request.0, &mut available) == 0 {
                return Err(fail());
            }
            if available == 0 {
                break;
            }
            let start = out.len();
            out.resize(start + available as usize, 0);
            let mut read: u32 = 0;
            if WinHttpReadData(
                request.0,
                out[start..].as_mut_ptr() as *mut core::ffi::c_void,
                available,
                &mut read,
            ) == 0
            {
                return Err(fail());
            }
            out.truncate(start + read as usize);
            if out.len() > 4 * 1024 * 1024 {
                return Err(CloudError::BadResponse);
            }
        }
        Ok((status, out))
    }
}

fn check_status(status: u32) -> Result<(), CloudError> {
    match status {
        200..=299 => Ok(()),
        401 | 403 => Err(CloudError::Unauthorized),
        429 => Err(CloudError::Limited),
        _ => Err(CloudError::Rejected),
    }
}

fn auth_headers(provider: Provider, key: &str) -> Vec<(&'static str, String)> {
    match provider {
        Provider::OpenAi | Provider::Groq => vec![("Authorization", format!("Bearer {key}"))],
        Provider::Anthropic => vec![
            ("x-api-key", key.to_owned()),
            ("anthropic-version", "2023-06-01".to_owned()),
        ],
    }
}

/// How much Claude Haiku 5.5 thinks before answering: "low" is its lightest level
/// (GPT-6 Luna is sent "none", which Haiku does not offer).
const EFFORT: &str = "low";

/// Model names the account can use, for the settings screen (also checks the key).
pub fn list_models(provider: Provider) -> Result<Vec<String>, CloudError> {
    let key = load_key(provider).ok_or(CloudError::NoKey)?;
    let headers = auth_headers(provider, &key);
    let headers: Vec<(&str, &str)> = headers.iter().map(|(n, v)| (*n, v.as_str())).collect();
    let path = match provider {
        Provider::Groq => "/openai/v1/models",
        _ => "/v1/models?limit=1000",
    };
    let (status, body) = https(
        provider.host(),
        "GET",
        path,
        &headers,
        None,
        Duration::from_secs(15),
    )?;
    check_status(status)?;
    let value: serde_json::Value =
        serde_json::from_slice(&body).map_err(|_| CloudError::BadResponse)?;
    let mut names: Vec<String> = value["data"]
        .as_array()
        .ok_or(CloudError::BadResponse)?
        .iter()
        .filter_map(|model| model["id"].as_str().map(str::to_owned))
        .filter(|id| match provider {
            // Only chat models; leave out speech, image, embedding and moderation models.
            Provider::OpenAi => {
                id.starts_with("gpt-")
                    && !["audio", "realtime", "image", "transcribe", "tts", "search"]
                        .iter()
                        .any(|word| id.contains(word))
            }
            Provider::Anthropic => id.starts_with("claude-"),
            Provider::Groq => id.starts_with("whisper"),
        })
        .collect();
    names.sort();
    names.dedup();
    Ok(names)
}

/// Ask a cloud model for a draft: `system` holds the instructions, `user` the request
/// with the transcript. Blocking; run off the async runtime.
pub fn complete(
    provider: Provider,
    model: &str,
    system: &str,
    user: &str,
    timeout: Duration,
) -> Result<String, CloudError> {
    if provider == Provider::Groq {
        return Err(CloudError::Rejected);
    }
    let key = load_key(provider).ok_or(CloudError::NoKey)?;
    let (path, body) = match provider {
        Provider::Groq => unreachable!("checked above"),
        Provider::OpenAi => (
            "/v1/chat/completions",
            serde_json::json!({
                "model": model,
                "messages": [
                    {"role": "system", "content": system},
                    {"role": "user", "content": user},
                ],
                // Rewriting needs no reasoning: at "low" GPT-6 Luna still took 15–29 s
                // for a long recording (Claude Haiku 5.5 about 3–5 s), 2026-10-10.
                "reasoning_effort": "none",
            }),
        ),
        Provider::Anthropic => (
            "/v1/messages",
            serde_json::json!({
                "model": model,
                "max_tokens": 4096,
                "system": system,
                "messages": [{"role": "user", "content": user}],
                // The lightest level Haiku offers (default: medium).
                "output_config": {"effort": EFFORT},
                // No thinking: even at "low", Haiku 5.5 thought for 3,300–3,800 tokens on a
                // long transcript with many mishearings, took 13–18 s and twice ran out of
                // max_tokens before the text (2026-10-13, scripts/eval/polish-compare.py).
                // Without it: about 3 s, no failures, term fixes about the same.
                "thinking": {"type": "disabled"},
            }),
        ),
    };
    let mut headers = auth_headers(provider, &key);
    headers.push(("Content-Type", "application/json".to_owned()));
    let headers: Vec<(&str, &str)> = headers.iter().map(|(n, v)| (*n, v.as_str())).collect();
    let body = body.to_string();
    let (status, response) = https(
        provider.host(),
        "POST",
        path,
        &headers,
        Some(body.as_bytes()),
        timeout,
    )?;
    check_status(status)?;
    let value: serde_json::Value =
        serde_json::from_slice(&response).map_err(|_| CloudError::BadResponse)?;
    completion_text(provider, &value)
}

/// The answer's text. An answer cut off at the output limit is refused (as an unreadable
/// response): used as it is, a draft missing its end would be typed as if it were whole.
fn completion_text(provider: Provider, value: &serde_json::Value) -> Result<String, CloudError> {
    let cut_off = match provider {
        Provider::OpenAi | Provider::Groq => value["choices"][0]["finish_reason"] == "length",
        Provider::Anthropic => value["stop_reason"] == "max_tokens",
    };
    if cut_off {
        return Err(CloudError::BadResponse);
    }
    let text = match provider {
        Provider::OpenAi | Provider::Groq => value["choices"][0]["message"]["content"]
            .as_str()
            .map(str::to_owned),
        Provider::Anthropic => value["content"].as_array().map(|blocks| {
            blocks
                .iter()
                .filter(|block| block["type"] == "text")
                .filter_map(|block| block["text"].as_str())
                .collect::<String>()
        }),
    }
    .ok_or(CloudError::BadResponse)?;
    let text = text.trim().to_owned();
    if text.is_empty() {
        Err(CloudError::Empty)
    } else {
        Ok(text)
    }
}

// ---- Cloud recognition (Groq) ----

/// Whisper large-v3 (the PC's "高精度") on Groq's servers: on that hardware it is about
/// as fast as turbo (189x vs 216x real time) and makes fewer errors, with the same free
/// limits (2026-10: 2,000 requests and 8 hours of audio a day), so the cloud uses it.
/// Turbo is tried only if Groq refuses large-v3 (for example after retiring it).
pub const TRANSCRIBE_MODELS: [&str; 2] = ["whisper-large-v3", "whisper-large-v3-turbo"];
/// Whisper reads at most 224 tokens of prompt; Japanese is about one token per
/// character, so the hints are cut well before that.
const PROMPT_CHARS: usize = 150;

impl CloudError {
    /// Shown when cloud recognition failed and the PC recognizes the recording instead.
    pub fn transcribe_notice(self) -> &'static str {
        match self {
            Self::NoKey => "GroqのAPIキーが未登録のため、このPCで認識します",
            Self::Unreachable => "クラウドに接続できないため、このPCで認識します",
            Self::Timeout => "クラウドの応答が遅いため、このPCで認識します",
            Self::Unauthorized => "GroqのAPIキーが正しくないため、このPCで認識します",
            Self::Limited => "Groqの利用上限に達したため、このPCで認識します",
            Self::Rejected | Self::BadResponse | Self::Empty => {
                "クラウドで認識できなかったため、このPCで認識します"
            }
        }
    }
}

/// 16 kHz mono samples as a 16-bit PCM WAV file (32 KB per second).
fn wav(samples: &[f32]) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16_u32.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1_u16.to_le_bytes()); // mono
    out.extend_from_slice(&16_000_u32.to_le_bytes());
    out.extend_from_slice(&32_000_u32.to_le_bytes());
    out.extend_from_slice(&2_u16.to_le_bytes());
    out.extend_from_slice(&16_u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&((sample.clamp(-1.0, 1.0) * 32_767.0) as i16).to_le_bytes());
    }
    out
}

/// The dictionary and screen hints as the prompt, joined like the PC's worker does,
/// cut at a whole hint within `PROMPT_CHARS`.
fn prompt(hints: &[String]) -> String {
    let mut prompt = String::new();
    for hint in hints {
        let separator = usize::from(!prompt.is_empty());
        if prompt.chars().count() + separator + hint.chars().count() > PROMPT_CHARS {
            break;
        }
        if separator == 1 {
            prompt.push('、');
        }
        prompt.push_str(hint);
    }
    prompt
}

fn multipart(boundary: &str, fields: &[(&str, &str)], audio: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(audio.len() + 1024);
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(audio);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

/// Recognize a recording (16 kHz mono) on Groq: the text and the model that wrote it.
/// Sends the audio and the hints; nothing is kept on this PC. Blocking; run off the
/// async runtime.
///
/// `english`: the English shortcut; the audio is taken as English and the Japanese hints
/// are not sent.
pub fn transcribe(
    samples: &[f32],
    hints: &[String],
    english: bool,
) -> Result<(String, &'static str), CloudError> {
    let key = load_key(Provider::Groq).ok_or(CloudError::NoKey)?;
    let mut result = Err(CloudError::Rejected);
    for model in TRANSCRIBE_MODELS {
        result = transcribe_with(&key, model, samples, hints, english).map(|text| (text, model));
        if result != Err(CloudError::Rejected) {
            break;
        }
    }
    result
}

fn transcribe_with(
    key: &str,
    model: &str,
    samples: &[f32],
    hints: &[String],
    english: bool,
) -> Result<String, CloudError> {
    // Uploading takes longest on a slow office line: 32 KB per second of speech.
    let timeout = Duration::from_secs(15 + samples.len() as u64 / 16_000 / 3);
    let boundary = format!("VoiceWeave{}", uuid::Uuid::new_v4().simple());
    let prompt = if english {
        String::new()
    } else {
        prompt(hints)
    };
    let mut fields = vec![
        ("model", model),
        ("language", if english { "en" } else { "ja" }),
        ("response_format", "json"),
        ("temperature", "0"),
    ];
    if !prompt.is_empty() {
        fields.push(("prompt", prompt.as_str()));
    }
    let body = multipart(&boundary, &fields, &wav(samples));
    let content_type = format!("multipart/form-data; boundary={boundary}");
    let mut headers = auth_headers(Provider::Groq, key);
    headers.push(("Content-Type", content_type));
    let headers: Vec<(&str, &str)> = headers.iter().map(|(n, v)| (*n, v.as_str())).collect();
    let (status, response) = https(
        Provider::Groq.host(),
        "POST",
        "/openai/v1/audio/transcriptions",
        &headers,
        Some(&body),
        timeout,
    )?;
    check_status(status)?;
    let value: serde_json::Value =
        serde_json::from_slice(&response).map_err(|_| CloudError::BadResponse)?;
    // An empty text is a valid answer (silence); the caller treats it like the PC's.
    Ok(value["text"]
        .as_str()
        .ok_or(CloudError::BadResponse)?
        .trim()
        .to_owned())
}

/// A draft from one service with the time it took, for the comparison output.
pub struct Timed {
    pub provider: Provider,
    pub model: String,
    pub seconds: f64,
    pub result: Result<String, CloudError>,
}

pub fn timed(
    provider: Provider,
    model: &str,
    system: &str,
    user: &str,
    timeout: Duration,
) -> Timed {
    let started = Instant::now();
    let result = complete(provider, model, system, user, timeout);
    Timed {
        provider,
        model: model.to_owned(),
        seconds: started.elapsed().as_secs_f64(),
        result,
    }
}

impl Timed {
    /// Service, model and time for diagnostics (never the text).
    pub fn report(&self) -> crate::diagnostics::CloudDraft {
        crate::diagnostics::CloudDraft {
            service: self.provider.id().to_owned(),
            model: self.model.clone(),
            ms: (self.seconds * 1000.0).round() as u64,
            ok: self.result.is_ok(),
            fallback: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_cut_off_at_the_output_limit_is_not_used() {
        let whole = serde_json::json!({
            "choices": [{"message": {"content": " 明日は四時です。 "}, "finish_reason": "stop"}]
        });
        assert_eq!(
            completion_text(Provider::OpenAi, &whole).as_deref(),
            Ok("明日は四時です。")
        );
        let cut = serde_json::json!({
            "choices": [{"message": {"content": "明日は四時"}, "finish_reason": "length"}]
        });
        assert_eq!(
            completion_text(Provider::OpenAi, &cut),
            Err(CloudError::BadResponse)
        );
        let whole = serde_json::json!({
            "content": [{"type": "text", "text": "明日は四時です。"}], "stop_reason": "end_turn"
        });
        assert_eq!(
            completion_text(Provider::Anthropic, &whole).as_deref(),
            Ok("明日は四時です。")
        );
        let cut = serde_json::json!({
            "content": [{"type": "text", "text": "明日は"}], "stop_reason": "max_tokens"
        });
        assert_eq!(
            completion_text(Provider::Anthropic, &cut),
            Err(CloudError::BadResponse)
        );
        let empty = serde_json::json!({"content": [], "stop_reason": "end_turn"});
        assert_eq!(
            completion_text(Provider::Anthropic, &empty),
            Err(CloudError::Empty)
        );
    }

    #[test]
    fn providers_and_statuses_map_as_expected() {
        assert_eq!(Provider::parse("openai"), Some(Provider::OpenAi));
        assert_eq!(Provider::parse("anthropic"), Some(Provider::Anthropic));
        assert_eq!(Provider::parse("local"), None);
        assert_eq!(check_status(200), Ok(()));
        assert_eq!(check_status(401), Err(CloudError::Unauthorized));
        assert_eq!(check_status(429), Err(CloudError::Limited));
        assert_eq!(check_status(404), Err(CloudError::Rejected));
    }

    /// Network check without a key: both services must answer over HTTPS with "not
    /// authorized". Run with `cargo test https_reaches -- --ignored`.
    #[test]
    #[ignore = "needs the internet"]
    fn https_reaches_the_services_without_a_key() {
        for provider in [Provider::OpenAi, Provider::Anthropic] {
            let (status, _) = https(
                provider.host(),
                "GET",
                "/v1/models",
                &[],
                None,
                Duration::from_secs(20),
            )
            .expect("an HTTPS answer");
            assert_eq!(
                check_status(status),
                Err(CloudError::Unauthorized),
                "{provider:?}"
            );
        }
    }

    #[test]
    fn audio_is_sent_as_16_bit_wav_with_the_hints_as_prompt() {
        let file = wav(&[0.0, 1.0, -1.0, 0.5]);
        assert_eq!(&file[..4], b"RIFF");
        assert_eq!(file.len(), 44 + 8);
        assert_eq!(&file[46..48], &[0xff, 0x7f]);
        let hints: Vec<String> = ["VoiceWeave", "衝", "カシオペヤ座"]
            .map(String::from)
            .into();
        assert_eq!(prompt(&hints), "VoiceWeave、衝、カシオペヤ座");
        let many: Vec<String> = (0..100).map(|i| format!("用語{i}")).collect();
        let cut = prompt(&many);
        assert!(cut.chars().count() <= PROMPT_CHARS && cut.ends_with(char::is_numeric));
        let body = multipart("B", &[("model", TRANSCRIBE_MODELS[0])], b"WAV");
        let text = String::from_utf8(body).unwrap();
        assert!(text.starts_with("--B\r\nContent-Disposition: form-data; name=\"model\""));
        assert!(text.ends_with("WAV\r\n--B--\r\n"));
        assert_eq!(Provider::parse("groq"), Some(Provider::Groq));
    }

    #[test]
    fn malformed_keys_are_refused_before_saving() {
        assert!(save_key(Provider::OpenAi, "").is_err());
        assert!(save_key(Provider::OpenAi, "sk abc").is_err());
    }
}
