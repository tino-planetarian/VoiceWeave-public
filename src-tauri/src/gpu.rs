//! How full the NVIDIA GPU's memory is, and whether other programs (a local model in
//! Ollama) hold part of it. A recognizer squeezed out of GPU memory stalls instead of
//! failing (seen 2026-10-16: a 5 GB Ollama model next to large-v3 on 6 GB, then a worker
//! timeout). Read with nvidia-smi, which comes with the driver. Numbers and flags only:
//! no program names or paths leave this module.
use serde::Serialize;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
pub struct GpuMemory {
    pub used_mb: u64,
    pub total_mb: u64,
    /// Ollama (a local AI model) is on the GPU.
    pub ollama: bool,
    /// Other programs using the GPU for computing, not counting the recognizer.
    pub other_programs: usize,
}

impl GpuMemory {
    pub fn free_mb(&self) -> u64 {
        self.total_mb.saturating_sub(self.used_mb)
    }
}

/// Below this much free memory with the recognizer loaded, recognition may stall.
pub const LOW_FREE_MB: u64 = 700;

/// The GPU's memory now, or None without an NVIDIA GPU or driver tool. Blocks for up to
/// a few seconds; call it off the pipeline's thread or where a short wait is fine.
pub fn snapshot() -> Option<GpuMemory> {
    let memory = run(&[
        "--query-gpu=memory.used,memory.total",
        "--format=csv,noheader,nounits",
    ])?;
    let programs =
        run(&["--query-compute-apps=process_name", "--format=csv,noheader"]).unwrap_or_default();
    parse(&memory, &programs)
}

fn parse(memory: &str, programs: &str) -> Option<GpuMemory> {
    // One line per GPU; the first is the one the recognizer uses.
    let mut numbers = memory
        .lines()
        .next()?
        .split(',')
        .map(|value| value.trim().parse::<u64>());
    let used_mb = numbers.next()?.ok()?;
    let total_mb = numbers.next()?.ok()?;
    let mut ollama = false;
    let mut other_programs = 0;
    for program in programs
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let program = program.to_ascii_lowercase();
        // The recognizer: the packaged worker, or Python in development.
        if program.contains("voiceweave") || program.ends_with("python.exe") {
            continue;
        }
        other_programs += 1;
        ollama |= program.contains("ollama");
    }
    Some(GpuMemory {
        used_mb,
        total_mb,
        ollama,
        other_programs,
    })
}

fn run(args: &[&str]) -> Option<String> {
    use std::os::windows::process::CommandExt;
    let tool = std::env::var_os("SystemRoot")
        .map(|root| {
            std::path::PathBuf::from(root)
                .join("System32")
                .join("nvidia-smi.exe")
        })
        .filter(|path| path.exists())
        .unwrap_or_else(|| "nvidia-smi".into());
    let mut child = Command::new(tool)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000)
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    let mut output = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut output).ok()?;
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_and_other_programs_are_read_without_names() {
        let gpu = parse(
            "3787, 6144\n",
            "C:\\Users\\a\\AppData\\Local\\Programs\\Ollama\\lib\\ollama\\llama-server.exe\n\
             C:\\Program Files\\Norton\\Suite\\NortonUI.exe\n\
             C:\\Users\\a\\AppData\\Local\\VoiceWeave\\voiceweave-worker.exe\n",
        )
        .unwrap();
        assert_eq!(
            gpu,
            GpuMemory {
                used_mb: 3787,
                total_mb: 6144,
                ollama: true,
                other_programs: 2
            }
        );
        assert_eq!(gpu.free_mb(), 2357);
        let alone = parse("1200, 6144", "").unwrap();
        assert!(!alone.ollama && alone.other_programs == 0);
        assert_eq!(parse("", ""), None);
        assert_eq!(parse("N/A, 6144", ""), None);
    }
}
