//! Records the evaluation sentences (scripts/eval/asr_eval_ja.tsv) in the user's own voice
//! for scripts/eval/eval-asr.py, through the same capture path as the app.
//!
//! Usage: cargo run --features experimental-audio --example record_eval -- OUT_DIR CONDITION
//!   CONDITION: "normal" (read naturally) or "nopause" (fast, without pausing).
//! For each sentence: Enter starts recording, Enter stops; then Enter keeps it or "r"
//! records it again, "s" skips it. Files are OUT_DIR/<condition>_<id>.wav (16 kHz mono).
//! OUT_DIR must be outside the repository; the recordings are personal voice data.
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;
use voiceweave_lib::recording::{CaptureEvent, CaptureService};

fn write_wav(path: &Path, samples: &[f32]) -> std::io::Result<()> {
    let data: Vec<u8> = samples
        .iter()
        .flat_map(|sample| ((sample.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())
        .collect();
    let mut file = std::fs::File::create(path)?;
    file.write_all(b"RIFF")?;
    file.write_all(&(36 + data.len() as u32).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16u32.to_le_bytes())?;
    file.write_all(&1u16.to_le_bytes())?; // PCM
    file.write_all(&1u16.to_le_bytes())?; // mono
    file.write_all(&16_000u32.to_le_bytes())?;
    file.write_all(&32_000u32.to_le_bytes())?;
    file.write_all(&2u16.to_le_bytes())?;
    file.write_all(&16u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&(data.len() as u32).to_le_bytes())?;
    file.write_all(&data)
}

fn record(service: &CaptureService, lines: &mut impl Iterator<Item = String>) -> Option<Vec<f32>> {
    let id = Uuid::new_v4();
    service.start(id).ok()?;
    match service.recv_timeout(Duration::from_secs(10)) {
        Ok(CaptureEvent::Started(started)) if started == id => {}
        _ => {
            eprintln!("録音を開始できませんでした");
            return None;
        }
    }
    println!("  ● 録音中…（読み終えたら Enter）");
    lines.next();
    service.stop(id).ok()?;
    match service.recv_timeout(Duration::from_secs(20)) {
        Ok(CaptureEvent::Stopped {
            session_id,
            result: Ok(audio),
            ..
        }) if session_id == id => Some(audio.samples),
        _ => {
            eprintln!("録音を停止できませんでした");
            None
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out = PathBuf::from(args.next().expect("OUT_DIR"));
    let condition = args.next().expect("CONDITION (normal or nopause)");
    assert!(
        matches!(condition.as_str(), "normal" | "nopause"),
        "CONDITION must be normal or nopause"
    );
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .canonicalize()
        .unwrap();
    std::fs::create_dir_all(&out).expect("create OUT_DIR");
    let out = out.canonicalize().expect("OUT_DIR");
    assert!(
        !out.starts_with(&repo),
        "OUT_DIR must be outside the repository"
    );
    let sentences = include_str!("../../scripts/eval/asr_eval_ja.tsv");
    let service = CaptureService::spawn().expect("start audio owner thread");
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines().map_while(Result::ok);
    let hint = if condition == "nopause" {
        "早口で、間を空けずに一息で読んでください"
    } else {
        "ふだん話す速さで読んでください"
    };
    println!("{hint}。保存先: {}", out.display());
    for row in sentences.lines().filter(|line| !line.trim().is_empty()) {
        let (id, text) = row.split_once('\t').expect("id<TAB>sentence");
        let shown = if condition == "nopause" {
            text.replace(['、', '。'], " ")
        } else {
            text.to_owned()
        };
        loop {
            println!("\n[{id}] {shown}\n  Enter で録音開始（s で飛ばす）");
            if lines.next().as_deref() == Some("s") {
                break;
            }
            let Some(samples) = record(&service, &mut lines) else {
                continue;
            };
            println!(
                "  {:.1} 秒。Enter で保存、r でやり直し",
                samples.len() as f32 / 16_000.0
            );
            if lines.next().as_deref() == Some("r") {
                continue;
            }
            let path = out.join(format!("{condition}_{id}.wav"));
            write_wav(&path, &samples).expect("write wav");
            println!("  保存しました");
            break;
        }
    }
    println!(
        "\n終わりました。録音は {} にあります（不要になったらフォルダーごと削除できます）",
        out.display()
    );
}
