//! Runs the worker's verified installers (CUDA runtime, recognition model) in the
//! background. The worker executable carries them behind `--cuda-install` and
//! `--model-install`; this module only starts one, relays fixed-shape progress,
//! and offers cancellation. Only one installer runs at a time.
use serde::Serialize;
use std::ffi::OsString;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::Notify;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    Cuda,
    Model,
}

impl Which {
    fn flag(self) -> &'static str {
        match self {
            Self::Cuda => "--cuda-install",
            Self::Model => "--model-install",
        }
    }

    /// Frontend event carrying this installer's progress and final outcome.
    fn event(self) -> &'static str {
        match self {
            Self::Cuda => "cuda_install",
            Self::Model => "model_install",
        }
    }
}

#[derive(Default)]
pub struct InstallSlot {
    installing: AtomicBool,
    cancel: Notify,
}

impl InstallSlot {
    pub fn installing(&self) -> bool {
        self.installing.load(Ordering::Acquire)
    }
}

#[derive(Default)]
pub struct Installers {
    pub cuda: InstallSlot,
    pub model: InstallSlot,
}

impl Installers {
    pub fn slot(&self, which: Which) -> &InstallSlot {
        match which {
            Which::Cuda => &self.cuda,
            Which::Model => &self.model,
        }
    }
}

/// Progress or a final outcome. Only fixed phase names and byte counts; never paths.
#[derive(Clone, Serialize, PartialEq, Debug)]
pub struct InstallEvent {
    pub phase: &'static str,
    pub completed: u64,
    pub total: u64,
}

const PHASES: [&str; 10] = [
    "starting",
    "downloading",
    "copying",
    "checking-wheel",
    "extracting",
    "verifying",
    "installed",
    "cancelled",
    "failed",
    "busy",
];

/// Parse one installer line. Unknown phases or malformed lines are ignored.
pub fn parse_event(line: &str) -> Option<InstallEvent> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let phase = value.get("phase")?.as_str()?;
    let phase = PHASES.iter().find(|known| **known == phase)?;
    let count = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    Some(InstallEvent {
        phase,
        completed: count("completed"),
        total: count("total"),
    })
}

pub fn cancel(app: &AppHandle, which: Which) {
    app.state::<Installers>().slot(which).cancel.notify_one();
}

/// Start an installer in the background with tool-specific arguments. Returns at once;
/// progress arrives as events and the last one is `installed`, `cancelled` or `failed`.
/// `on_installed` runs after a verified installation (for example to reload the engine).
pub fn start(
    app: &AppHandle,
    which: Which,
    arguments: Vec<OsString>,
    on_installed: fn(&AppHandle),
) -> Result<(), String> {
    let installers = app.state::<Installers>();
    let other = match which {
        Which::Cuda => Which::Model,
        Which::Model => Which::Cuda,
    };
    if installers.slot(other).installing() {
        return Err("別の導入が進行中です。終わってから実行してください".into());
    }
    if installers
        .slot(which)
        .installing
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("すでに導入中です".into());
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let outcome = run(&app, which, arguments).await;
        app.state::<Installers>()
            .slot(which)
            .installing
            .store(false, Ordering::Release);
        let _ = app.emit(
            which.event(),
            InstallEvent {
                phase: outcome,
                completed: 0,
                total: 0,
            },
        );
        if outcome == "installed" {
            on_installed(&app);
        }
    });
    Ok(())
}

async fn run(app: &AppHandle, which: Which, arguments: Vec<OsString>) -> &'static str {
    let Ok((executable, base, cwd)) = crate::worker_command(app) else {
        return "failed";
    };
    let mut command = Command::new(executable);
    // Development runs `python -m voiceweave_worker`; the package runs the executable.
    command
        .args(base.iter().filter(|arg| **arg != "--self-check"))
        .arg(which.flag())
        .args(arguments)
        .arg("--json-progress")
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.as_std_mut().creation_flags(0x0800_0000);
    }
    let Ok(mut child) = command.spawn() else {
        return "failed";
    };
    crate::job::contain_child(&child);
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        return "failed";
    };
    let mut lines = BufReader::new(stdout).lines();
    let mut last = "failed";
    let _ = app.emit(
        which.event(),
        InstallEvent {
            phase: "starting",
            completed: 0,
            total: 0,
        },
    );
    let installers = app.state::<Installers>();
    let slot = installers.slot(which);
    let mut cancel_sent = false;
    loop {
        tokio::select! {
            line = lines.next_line() => {
                let Ok(Some(line)) = line else { break };
                if let Some(event) = parse_event(&line) {
                    if matches!(event.phase, "installed" | "cancelled" | "failed" | "busy") {
                        last = if event.phase == "busy" { "failed" } else { event.phase };
                    } else {
                        let _ = app.emit(which.event(), event);
                    }
                }
            }
            _ = slot.cancel.notified(), if !cancel_sent => {
                cancel_sent = true;
                let _ = stdin.write_all(b"cancel\n").await;
                let _ = stdin.flush().await;
            }
        }
    }
    drop(stdin);
    match tokio::time::timeout(Duration::from_secs(30), child.wait()).await {
        Ok(Ok(status)) if status.success() && last == "installed" => "installed",
        Ok(Ok(_)) if last == "cancelled" => "cancelled",
        Ok(_) => "failed",
        Err(_) => {
            let _ = child.start_kill();
            "failed"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_known_phases_with_byte_counts() {
        assert_eq!(
            parse_event(r#"{"phase":"downloading","completed":5,"total":10}"#),
            Some(InstallEvent {
                phase: "downloading",
                completed: 5,
                total: 10
            })
        );
        assert_eq!(
            parse_event(r#"{"phase":"copying","completed":1,"total":2}"#)
                .unwrap()
                .phase,
            "copying"
        );
        for line in [
            "not json",
            r#"{"phase":"C:\\private\\path"}"#,
            r#"{"completed":1}"#,
            r#"{"phase":7}"#,
        ] {
            assert_eq!(parse_event(line), None);
        }
        // Negative or fractional counts do not become bogus progress.
        assert_eq!(
            parse_event(r#"{"phase":"extracting","completed":-1,"total":1.5}"#)
                .unwrap()
                .completed,
            0
        );
    }

    #[test]
    fn each_installer_has_its_own_flag_and_event() {
        assert_eq!(Which::Cuda.flag(), "--cuda-install");
        assert_eq!(Which::Model.flag(), "--model-install");
        assert_ne!(Which::Cuda.event(), Which::Model.event());
    }
}
