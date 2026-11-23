use std::path::PathBuf;
use voiceweave_lib::worker::Worker;

#[tokio::main]
async fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("root")
        .to_path_buf();
    let exe = root.join(".venv/Scripts/python.exe");
    let worker_dir = root.join("worker");
    let (worker, version) = Worker::start(&exe, &["-m", "voiceweave_worker"], &worker_dir, 0)
        .await
        .expect("start persistent worker");
    println!(
        "Persistent worker hello: {version}; generation: {}",
        worker.generation()
    );
    worker.shutdown().await.expect("shutdown worker");
    println!("Worker shutdown acknowledged");
}
