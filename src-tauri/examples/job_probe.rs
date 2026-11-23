//! Starts a real worker, then exits abruptly without any cleanup (no drop, no
//! shutdown), like a crash. The job object should end the worker with this process.
use std::path::PathBuf;
use voiceweave_lib::worker::Worker;

#[tokio::main]
async fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let python = root.join(".venv/Scripts/python.exe");
    let (worker, version) = Worker::start(
        &python,
        &["-m", "voiceweave_worker"],
        &root.join("worker"),
        1,
    )
    .await
    .expect("worker starts");
    println!("worker {version} started; exiting abruptly");
    std::mem::forget(worker);
    std::process::exit(0);
}
