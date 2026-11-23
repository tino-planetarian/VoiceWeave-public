use std::path::PathBuf;
use uuid::Uuid;
use voiceweave_lib::worker::Worker;

#[tokio::main]
async fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("root")
        .to_path_buf();
    let exe = root.join(".venv/Scripts/python.exe");
    let worker_dir = root.join("worker");
    let model_dir = std::env::var_os("VOICEWEAVE_MODEL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("models/dev-tiny"));
    let (mut worker, version) = Worker::start(&exe, &["-m", "voiceweave_worker"], &worker_dir, 0)
        .await
        .expect("start worker");
    println!("Worker hello: {version}");
    worker
        .prepare_cpu(&model_dir)
        .await
        .expect("prepare local model on CPU");
    println!("Local model prepared");
    let silence = vec![0.0_f32; 16_000];
    for pass in 1..=2 {
        let started = std::time::Instant::now();
        let text = worker
            .transcribe(Uuid::new_v4(), &silence)
            .await
            .expect("transcribe silence");
        println!(
            "Pass {pass}: {} chars in {} ms",
            text.chars().count(),
            started.elapsed().as_millis()
        );
    }
    worker.shutdown().await.expect("shutdown worker");
}
