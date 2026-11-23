//! Whether Ollama (the local AI runtime for punctuation and polishing) can be used, told
//! apart so the settings screen can say what to do: answering, answering too slowly,
//! installed but not running, or not installed. A failed check is retried a few times,
//! because Ollama is often only moments from answering (just after sign-in) and a busy PC
//! can be slow to reply; none of those is "not installed".

use crate::polish::{self, PolishError};
use serde::Serialize;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Ollama answered; `Status::models` lists what it has downloaded.
    Ready,
    /// Something is listening but did not answer in time (a busy or short-of-memory PC).
    Busy,
    /// Installed, but nothing is listening.
    NotRunning,
    /// Nothing is listening and no Ollama program was found in the usual places.
    NotInstalled,
    /// Something answered, but not like Ollama (another program on the same port).
    Unexpected,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Status {
    pub state: State,
    pub models: Vec<String>,
}

const ATTEMPTS: u32 = 3;
const GAP: Duration = Duration::from_millis(1500);
/// A just-started Ollama takes the connection but says nothing for about ten seconds (it
/// looks at the GPU first; longer on a PC short of memory). Waiting only five seconds made
/// that moment look like "not running" (measured 2026-11-06 on Ollama 0.40.14).
const ANSWER_WAIT: Duration = Duration::from_secs(25);
const TRAY_APP: &str = "ollama app.exe";
const COMMAND_LINE: &str = "ollama.exe";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Ask Ollama: wait out a silent start-up, and retry a refused connection a few times.
pub fn check(endpoint: &str) -> Result<Status, PolishError> {
    check_with(endpoint, ATTEMPTS, GAP, ANSWER_WAIT, || {
        installed_program().is_some()
    })
}

fn check_with(
    endpoint: &str,
    attempts: u32,
    gap: Duration,
    wait: Duration,
    installed: impl Fn() -> bool,
) -> Result<Status, PolishError> {
    let status = |state| {
        Ok(Status {
            state,
            models: Vec::new(),
        })
    };
    for attempt in 0..attempts {
        if attempt > 0 {
            std::thread::sleep(gap);
        }
        match polish::installed_models_within(endpoint, wait) {
            Ok(models) => {
                return Ok(Status {
                    state: State::Ready,
                    models,
                })
            }
            Err(PolishError::NotLocal) => return Err(PolishError::NotLocal),
            // Connected but silent for the whole wait: running, only too busy to answer.
            Err(PolishError::Timeout) => return status(State::Busy),
            // Refused: maybe only a moment before it listens; try again.
            Err(PolishError::Unreachable) => {}
            Err(_) => return status(State::Unexpected),
        }
    }
    status(if installed() {
        State::NotRunning
    } else {
        State::NotInstalled
    })
}

/// Ollama's program in the places its installer uses (never a path from outside).
pub fn installed_program() -> Option<PathBuf> {
    find_program(|name| std::env::var(name).ok(), |path| path.is_file())
}

/// Ollama's command-line program (`ollama.exe`), for `ollama pull`.
pub fn installed_cli() -> Option<PathBuf> {
    find_in(
        &[COMMAND_LINE],
        |name| std::env::var(name).ok(),
        |path| path.is_file(),
    )
}

fn find_program(
    variable: impl Fn(&str) -> Option<String>,
    exists: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    find_in(&[TRAY_APP, COMMAND_LINE], variable, exists)
}

/// The first of `names` (in this order, folder by folder) that exists in the installer's folders.
fn find_in(
    names: &[&str],
    variable: impl Fn(&str) -> Option<String>,
    exists: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let mut folders = Vec::new();
    if let Some(base) = variable("LOCALAPPDATA") {
        folders.push(Path::new(&base).join("Programs").join("Ollama"));
    }
    for name in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(base) = variable(name) {
            folders.push(Path::new(&base).join("Ollama"));
        }
    }
    folders
        .iter()
        .flat_map(|folder| names.iter().map(move |name| folder.join(name)))
        .find(|path| exists(path))
}

/// Start the installed Ollama the way Windows does at sign-in: its tray app, or, when only
/// the command-line program is there, `ollama serve` without a window.
pub fn start() -> Result<(), &'static str> {
    let program =
        installed_program().ok_or("Ollamaが見つかりません（このPCに入っていないようです）")?;
    let mut command = Command::new(&program);
    let command_line = program
        .file_name()
        .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(COMMAND_LINE));
    if command_line {
        command
            .arg("serve")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW);
    }
    command
        .spawn()
        .map(drop)
        .map_err(|_| "Ollamaを起動できませんでした")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn closed_port_endpoint() -> String {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        // The listener is dropped, so nothing answers on this port.
        format!("http://127.0.0.1:{port}")
    }

    #[test]
    fn an_answering_ollama_is_ready_with_its_models_sorted() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let read = stream.read(&mut request).unwrap();
            let body = r#"{"models":[{"name":"qwen2.5:7b"},{"name":"gemma3:4b"}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            String::from_utf8_lossy(&request[..read]).into_owned()
        });
        let wait = Duration::from_secs(5);
        let status = check_with(&endpoint, 1, Duration::ZERO, wait, || false).unwrap();
        assert_eq!(status.state, State::Ready);
        assert_eq!(status.models, ["gemma3:4b", "qwen2.5:7b"]);
        assert!(server.join().unwrap().starts_with("GET /api/tags"));
    }

    #[test]
    fn nothing_listening_is_not_running_when_installed_and_missing_when_not() {
        let endpoint = closed_port_endpoint();
        let (gap, wait) = (Duration::from_millis(1), Duration::from_secs(1));
        let status = check_with(&endpoint, 2, gap, wait, || true).unwrap();
        assert_eq!(status.state, State::NotRunning);
        let status = check_with(&endpoint, 2, gap, wait, || false).unwrap();
        assert_eq!(status.state, State::NotInstalled);
        assert!(status.models.is_empty());
    }

    #[test]
    fn a_connected_but_silent_ollama_is_busy_not_missing() {
        // What a just-started Ollama does: the connection is taken, nothing is answered.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_millis(800));
            drop(stream);
        });
        let started = std::time::Instant::now();
        let status = check_with(
            &endpoint,
            3,
            Duration::ZERO,
            Duration::from_millis(200),
            || false,
        )
        .unwrap();
        assert_eq!(status.state, State::Busy);
        // Not retried: the wait was already spent once.
        assert!(started.elapsed() < Duration::from_millis(700));
        server.join().unwrap();
    }

    #[test]
    fn a_remote_endpoint_is_refused_not_reported_as_missing() {
        assert_eq!(
            check_with(
                "http://example.com:11434",
                1,
                Duration::ZERO,
                Duration::from_secs(1),
                || false
            ),
            Err(PolishError::NotLocal)
        );
    }

    #[test]
    fn the_program_is_found_only_in_the_installers_folders() {
        let variables = |name: &str| match name {
            "LOCALAPPDATA" => Some(r"C:\Users\a\AppData\Local".to_owned()),
            "ProgramFiles" => Some(r"C:\Program Files".to_owned()),
            _ => None,
        };
        let tray = Path::new(r"C:\Users\a\AppData\Local")
            .join("Programs")
            .join("Ollama")
            .join(TRAY_APP);
        let machine_wide = Path::new(r"C:\Program Files")
            .join("Ollama")
            .join(COMMAND_LINE);
        // The tray app is preferred when both programs are there.
        let both = |path: &Path| path.ends_with(TRAY_APP) || path.ends_with(COMMAND_LINE);
        assert_eq!(find_program(variables, both), Some(tray));
        // Only the machine-wide command-line program exists.
        let only_machine = |path: &Path| path == machine_wide;
        assert_eq!(
            find_program(variables, only_machine),
            Some(machine_wide.clone())
        );
        assert_eq!(find_program(variables, |_| false), None);
        assert_eq!(find_program(|_| None, |_| true), None);
    }
}
