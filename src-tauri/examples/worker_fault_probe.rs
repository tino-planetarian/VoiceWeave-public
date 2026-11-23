use std::{fs, path::PathBuf};
use uuid::Uuid;
use voiceweave_lib::worker::{Worker, WorkerError};

#[tokio::main]
async fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("root")
        .to_path_buf();
    let exe = root.join(".venv/Scripts/python.exe");
    let cwd = root.join("worker");
    let script = cwd.join("tests/fake_worker.py");
    let script_arg = script.to_str().expect("script path");
    for (scenario, expected) in [
        ("bad_hello", WorkerError::Protocol),
        ("eof", WorkerError::Io),
        ("wrong_id", WorkerError::Protocol),
        ("oversize", WorkerError::Protocol),
    ] {
        let actual = match Worker::start(&exe, &[script_arg, scenario], &cwd, 0).await {
            Ok((worker, _)) => worker.shutdown().await.err(),
            Err(error) => Some(error),
        };
        assert_eq!(actual, Some(expected), "scenario {scenario}");
        println!("{scenario}: rejected");
    }
    let actual = Worker::start(&exe, &[script_arg, "hello_hang"], &cwd, 0)
        .await
        .err();
    assert_eq!(actual, Some(WorkerError::Timeout));
    println!("hello_hang: timed out");

    let (worker, _) = Worker::start(&exe, &[script_arg, "shutdown_hang"], &cwd, 0)
        .await
        .expect("start shutdown hang worker");
    assert_eq!(worker.shutdown().await, Err(WorkerError::Timeout));
    println!("shutdown_hang: timed out");

    let (worker, _) = Worker::start(&exe, &[script_arg, "stderr_flood"], &cwd, 0)
        .await
        .expect("start stderr flood worker");
    worker
        .shutdown()
        .await
        .expect("stderr flood must not block reply");
    println!("stderr_flood: clean shutdown");

    let model_dir =
        std::env::temp_dir().join(format!("voiceweave-worker-probe-{}", Uuid::new_v4()));
    fs::create_dir(&model_dir).expect("create temporary model directory");
    fs::write(model_dir.join("model.bin"), b"x").expect("create dummy model marker");
    let samples = [0.0_f32; 16_000];

    let (mut failed_worker, _) = Worker::start(&exe, &[script_arg, "transcribe_eof"], &cwd, 0)
        .await
        .expect("start failing transcribe worker");
    failed_worker
        .prepare_cpu(&model_dir)
        .await
        .expect("prepare fake worker");
    assert_eq!(
        failed_worker.transcribe(Uuid::new_v4(), &samples).await,
        Err(WorkerError::Io)
    );
    drop(failed_worker);
    println!("transcribe_eof: interrupted request rejected");

    let (mut fresh_worker, _) = Worker::start(&exe, &[script_arg, "transcribe_recover"], &cwd, 1)
        .await
        .expect("start fresh worker");
    fresh_worker
        .prepare_cpu(&model_dir)
        .await
        .expect("prepare fresh fake worker");
    assert_eq!(
        fresh_worker.transcribe(Uuid::new_v4(), &samples).await,
        Ok(String::new())
    );
    fresh_worker
        .shutdown()
        .await
        .expect("fresh worker shutdown");
    fs::remove_file(model_dir.join("model.bin")).expect("remove dummy model marker");
    fs::remove_dir(model_dir).expect("remove temporary model directory");
    println!("transcribe_recover: fresh worker accepted synthetic silence");
}
