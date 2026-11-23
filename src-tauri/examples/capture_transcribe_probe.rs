use std::io;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;
use voiceweave_lib::recording::{CaptureEvent, CaptureService, CaptureStopReason};
use voiceweave_lib::worker::Worker;

fn normalized(text: &str) -> String {
    text.chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[tokio::main]
async fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("project root")
        .to_path_buf();
    let model_dir = std::env::var_os("VOICEWEAVE_MODEL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("models/dev-tiny"));
    let (mut worker, _) = Worker::start(
        &root.join(".venv/Scripts/python.exe"),
        &["-m", "voiceweave_worker"],
        &root.join("worker"),
        0,
    )
    .await
    .expect("start local worker");
    worker
        .prepare_cpu(&model_dir)
        .await
        .expect("prepare local CPU model");
    println!("model_ready; waiting_for_start_signal");
    let mut start_signal = String::new();
    assert!(
        io::stdin()
            .read_line(&mut start_signal)
            .expect("read start signal")
            > 0,
        "start signal unavailable"
    );
    println!("recording_starts_in=10s; duration=5s");
    thread::sleep(Duration::from_secs(10));

    let capture = CaptureService::spawn().expect("start audio owner thread");
    let id = Uuid::new_v4();
    capture.start(id).expect("request recording start");
    match capture.recv_timeout(Duration::from_secs(10)) {
        Ok(CaptureEvent::Started(started)) if started == id => {}
        Ok(CaptureEvent::StartFailed(_, error)) => panic!("recording start failed: {error:?}"),
        _ => panic!("recording start event unavailable"),
    }
    let callback_deadline = Instant::now() + Duration::from_secs(15);
    while capture.capture_counts().0 == 0 {
        assert!(
            Instant::now() < callback_deadline,
            "microphone delivered no audio callbacks within 15 seconds"
        );
        thread::sleep(Duration::from_millis(20));
    }
    println!("recording_started");
    thread::sleep(Duration::from_secs(5));
    capture.stop(id).expect("request recording stop");
    let (diagnostics, result) = match capture.recv_timeout(Duration::from_secs(30)) {
        Ok(CaptureEvent::Stopped {
            session_id,
            reason: CaptureStopReason::User,
            diagnostics,
            result,
        }) if session_id == id => (diagnostics, result),
        _ => panic!("recording stop event unavailable"),
    };
    let audio = result.unwrap_or_else(|error| {
        panic!(
            "recording conversion failed: {error:?}; callbacks={}; frames={}; startup_xruns={}",
            diagnostics.callbacks, diagnostics.frames, diagnostics.startup_xruns
        )
    });
    let peak = audio
        .samples
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    let rms = (audio
        .samples
        .iter()
        .map(|sample| f64::from(*sample) * f64::from(*sample))
        .sum::<f64>()
        / audio.samples.len() as f64)
        .sqrt();
    println!(
        "recording_stopped; samples_16k={}; startup_xruns={}; peak={peak:.4}; rms={rms:.4}",
        audio.samples.len(),
        diagnostics.startup_xruns
    );
    let started = Instant::now();
    let recognized = worker
        .transcribe(id, &audio.samples)
        .await
        .expect("transcribe recorded audio");
    let expected = normalized("今日はいい天気ですね");
    let actual = normalized(&recognized);
    println!(
        "recognition_done; chars={}; expected_phrase_present={}; elapsed_ms={}",
        recognized.chars().count(),
        actual.contains(&expected),
        started.elapsed().as_millis()
    );
    worker.shutdown().await.expect("shutdown local worker");
}
