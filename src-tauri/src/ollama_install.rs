//! Installing Ollama and its model from the settings screen, for a PC that does not have
//! them (or after a broken install). Started only by the user's click.
//!
//! The installer comes from Ollama's official site over HTTPS and is started only when its
//! digital signature is valid and names the publisher seen on Ollama's own programs
//! (checked 2026-11-09: "Ollama Inc.", registration number 2713355). Anything else is
//! deleted unopened. Windows' own `curl.exe` and PowerShell are used, by absolute path.
//! The installer's window is the user's: nothing is installed silently.

use serde::Serialize;
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

const INSTALLER_URL: &str = "https://ollama.com/download/OllamaSetup.exe";
const INSTALLER_NAME: &str = "OllamaSetup.exe";
/// What the certificate's subject must contain (both), besides a valid signature.
const PUBLISHER: &str = "O=Ollama Inc.";
const REGISTRATION: &str = "SERIALNUMBER=2713355";
/// The real installer is about a gigabyte; anything much smaller is an error page.
const MIN_BYTES: u64 = 50 * 1024 * 1024;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

static INSTALLING: AtomicBool = AtomicBool::new(false);
static CANCEL_INSTALL: AtomicBool = AtomicBool::new(false);
static PULLING: AtomicBool = AtomicBool::new(false);
static CANCEL_PULL: AtomicBool = AtomicBool::new(false);

/// What the screen is told (fixed phases and codes; no paths, no messages).
#[derive(Clone, Serialize)]
pub struct Progress {
    /// install: downloading | verifying | launched | failed | cancelled.
    /// pull: pulling | done | failed | cancelled.
    pub phase: &'static str,
    pub megabytes: Option<u64>,
    pub percent: Option<u32>,
    pub code: Option<&'static str>,
}

fn emit(app: &AppHandle, event: &str, progress: Progress) {
    let _ = app.emit(event, progress);
}

fn progress(phase: &'static str) -> Progress {
    Progress {
        phase,
        megabytes: None,
        percent: None,
        code: None,
    }
}

fn failed(code: &'static str) -> Progress {
    Progress {
        code: Some(code),
        ..progress("failed")
    }
}

fn system_tool(relative: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    Path::new(&root).join("System32").join(relative)
}

/// The signature is accepted only when valid and issued to the expected publisher.
fn accept_signature(status: &str, subject: &str) -> bool {
    status.trim() == "Valid" && subject.contains(PUBLISHER) && subject.contains(REGISTRATION)
}

fn signature_ok(file: &Path) -> bool {
    let script = "$s = Get-AuthenticodeSignature -LiteralPath $env:VOICEWEAVE_CHECK_FILE; \
                  $s.Status; $s.SignerCertificate.Subject";
    let output = Command::new(system_tool("WindowsPowerShell/v1.0/powershell.exe"))
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ])
        .env("VOICEWEAVE_CHECK_FILE", file)
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    let Ok(output) = output else { return false };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut lines = text.lines();
    let (status, subject) = (lines.next().unwrap_or(""), lines.next().unwrap_or(""));
    accept_signature(status, subject)
}

/// Download the installer into `folder` as `INSTALLER_NAME` (curl runs there, so no path
/// with a non-ASCII user name is ever an argument). Reports the size while it runs.
fn download(app: &AppHandle, folder: &Path) -> Result<PathBuf, &'static str> {
    let file = folder.join(INSTALLER_NAME);
    let _ = std::fs::remove_file(&file);
    let mut child = Command::new(system_tool("curl.exe"))
        .current_dir(folder)
        .args([
            "--fail",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--max-redirs",
            "5",
            "--connect-timeout",
            "20",
            // Give up when nothing arrives for a minute.
            "--speed-limit",
            "1000",
            "--speed-time",
            "60",
            "--output",
            INSTALLER_NAME,
            INSTALLER_URL,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|_| "no_curl")?;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) | Err(_) => {
                let _ = std::fs::remove_file(&file);
                return Err("download_failed");
            }
            Ok(None) => {}
        }
        if CANCEL_INSTALL.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(&file);
            return Err("cancelled");
        }
        let megabytes = std::fs::metadata(&file).map(|m| m.len() >> 20).unwrap_or(0);
        emit(
            app,
            "ollama_install",
            Progress {
                megabytes: Some(megabytes),
                ..progress("downloading")
            },
        );
        std::thread::sleep(Duration::from_millis(500));
    }
    match std::fs::metadata(&file) {
        Ok(meta) if meta.len() >= MIN_BYTES => Ok(file),
        _ => {
            let _ = std::fs::remove_file(&file);
            Err("download_failed")
        }
    }
}

fn install(app: &AppHandle) -> Result<(), &'static str> {
    let folder = app
        .path()
        .app_local_data_dir()
        .map_err(|_| "download_failed")?
        .join("downloads");
    std::fs::create_dir_all(&folder).map_err(|_| "download_failed")?;
    let file = download(app, &folder)?;
    emit(app, "ollama_install", progress("verifying"));
    if !signature_ok(&file) {
        // Never opened, never kept.
        let _ = std::fs::remove_file(&file);
        return Err("signature_invalid");
    }
    Command::new(&file)
        .spawn()
        .map(drop)
        .map_err(|_| "launch_failed")
}

/// Start the download, check and launch in the background (one at a time).
pub fn start_install(app: &AppHandle) -> Result<(), &'static str> {
    if INSTALLING.swap(true, Ordering::AcqRel) {
        return Err("すでにダウンロード中です");
    }
    CANCEL_INSTALL.store(false, Ordering::Release);
    let app = app.clone();
    std::thread::spawn(move || {
        let last = match install(&app) {
            Ok(()) => progress("launched"),
            Err("cancelled") => progress("cancelled"),
            Err(code) => failed(code),
        };
        emit(&app, "ollama_install", last);
        INSTALLING.store(false, Ordering::Release);
    });
    Ok(())
}

pub fn cancel_install() {
    CANCEL_INSTALL.store(true, Ordering::Release);
}

/// A model name such as `qwen2.5:7b` or `library/gemma3:4b`: it becomes an argument of
/// `ollama pull`, so it must not look like an option or hold anything unusual.
pub fn valid_model_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 80
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | ':' | '_' | '-' | '/'))
}

/// The last "NN%" in a chunk of `ollama pull` output (it redraws one line, with `\r`).
fn last_percent(chunk: &[u8]) -> Option<u32> {
    let mut found = None;
    for (index, byte) in chunk.iter().enumerate() {
        if *byte != b'%' {
            continue;
        }
        let digits: Vec<u8> = chunk[..index]
            .iter()
            .rev()
            .take_while(|b| b.is_ascii_digit())
            .take(3)
            .copied()
            .collect();
        if digits.is_empty() {
            continue;
        }
        let number: String = digits.iter().rev().map(|b| char::from(*b)).collect();
        if let Ok(value) = number.parse::<u32>() {
            if value <= 100 {
                found = Some(value);
            }
        }
    }
    found
}

fn pull(app: &AppHandle, model: &str) -> Result<(), &'static str> {
    let cli = crate::ollama::installed_cli().ok_or("not_installed")?;
    let mut child: Child = Command::new(cli)
        .args(["pull", model])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|_| "launch_failed")?;
    let mut output = child.stderr.take().ok_or("launch_failed")?;
    // The reading runs apart so that a cancel can end the program while it is silent.
    let (sender, receiver) = std::sync::mpsc::channel::<Option<u32>>();
    std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        while let Ok(count) = output.read(&mut buffer) {
            if count == 0 {
                break;
            }
            let _ = sender.send(last_percent(&buffer[..count]));
        }
    });
    let mut shown = None;
    loop {
        while let Ok(percent) = receiver.try_recv() {
            if percent.is_some() && percent != shown {
                shown = percent;
                emit(
                    app,
                    "ollama_pull",
                    Progress {
                        percent,
                        ..progress("pulling")
                    },
                );
            }
        }
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) | Err(_) => return Err("pull_failed"),
            Ok(None) => {}
        }
        if CANCEL_PULL.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("cancelled");
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// Download `model` with the installed Ollama (which must be running), in the background.
pub fn start_pull(app: &AppHandle, model: &str) -> Result<(), &'static str> {
    if !valid_model_name(model) {
        return Err("モデル名が正しくありません");
    }
    if PULLING.swap(true, Ordering::AcqRel) {
        return Err("すでにダウンロード中です");
    }
    CANCEL_PULL.store(false, Ordering::Release);
    let (app, model) = (app.clone(), model.to_owned());
    std::thread::spawn(move || {
        let last = match pull(&app, &model) {
            Ok(()) => progress("done"),
            Err("cancelled") => progress("cancelled"),
            Err(code) => failed(code),
        };
        emit(&app, "ollama_pull", last);
        PULLING.store(false, Ordering::Release);
    });
    Ok(())
}

pub fn cancel_pull() {
    CANCEL_PULL.store(true, Ordering::Release);
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLLAMA_SUBJECT: &str = "CN=Ollama Inc., O=Ollama Inc., L=Toronto, S=Ontario, C=CA, \
        SERIALNUMBER=2713355, OID.2.5.4.15=Private Organization";

    #[test]
    fn only_a_valid_signature_of_the_expected_publisher_is_accepted() {
        assert!(accept_signature("Valid", OLLAMA_SUBJECT));
        assert!(accept_signature("Valid\r", OLLAMA_SUBJECT));
        // Not valid (altered, expired, unsigned, unknown root).
        for status in [
            "HashMismatch",
            "NotSigned",
            "UnknownError",
            "NotTrusted",
            "",
        ] {
            assert!(!accept_signature(status, OLLAMA_SUBJECT), "{status}");
        }
        // Valid, but another publisher, or the name without the registration number.
        assert!(!accept_signature("Valid", "CN=Evil Corp, O=Evil Corp"));
        assert!(!accept_signature(
            "Valid",
            "CN=Ollama Inc., O=Ollama Inc., C=CA"
        ));
        assert!(!accept_signature(
            "Valid",
            "CN=Fake, O=Fake Ollama Inc. Ltd, SERIALNUMBER=2713355"
        ));
        assert!(!accept_signature("Valid", ""));
    }

    #[test]
    fn the_real_check_accepts_ollamas_own_program_and_refuses_an_unsigned_file() {
        // An unsigned file is refused (this test's own source file will do).
        let unsigned = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(!signature_ok(&unsigned));
        // Where Ollama is installed, its own signed program must pass: this is what
        // the downloaded installer is held to (same publisher).
        if let Some(program) = crate::ollama::installed_cli() {
            assert!(signature_ok(&program), "{}", program.display());
        }
    }

    #[test]
    fn the_download_comes_from_the_official_site_over_https() {
        assert!(INSTALLER_URL.starts_with("https://ollama.com/"));
    }

    #[test]
    fn a_model_name_cannot_become_an_option_or_carry_odd_characters() {
        for good in ["qwen2.5:7b", "gemma3:4b", "library/gemma3:4b", "llama3.2"] {
            assert!(valid_model_name(good), "{good}");
        }
        for bad in [
            "",
            "-h",
            "--help",
            "a b",
            "a;b",
            "a&b",
            "../x",
            ":x",
            "名前",
            &"a".repeat(81),
        ] {
            assert!(!valid_model_name(bad), "{bad}");
        }
    }

    #[test]
    fn the_percent_is_read_from_a_redrawn_progress_line() {
        assert_eq!(
            last_percent(b"pulling 123abc:  45% |####   | 2.1 GB/4.7 GB"),
            Some(45)
        );
        // Several redraws in one chunk: the last one counts.
        assert_eq!(last_percent(b"x  5%\rx 12%\rx 97%\r"), Some(97));
        assert_eq!(last_percent(b"pulling manifest"), None);
        assert_eq!(last_percent(b"100% done"), Some(100));
        // Not a percentage.
        assert_eq!(last_percent(b"abc %"), None);
        assert_eq!(last_percent(b"1234%"), None);
    }
}
