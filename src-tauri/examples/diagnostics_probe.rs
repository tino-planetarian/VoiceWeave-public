//! Synthetic process-boundary test. No microphone, real weights, or user text.
use std::path::PathBuf;
use uuid::Uuid;
use voiceweave_lib::worker::Worker;

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        // Only our UUID-named directory, never a shared model/Temp directory.
        let parent = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
        if self.0.parent() == Some(parent.as_path())
            && self
                .0
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("diagnostics-probe-"))
        {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[tokio::main]
async fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let fixture_path = root
        .join("src-tauri/target")
        .join(format!("diagnostics-probe-{}", Uuid::new_v4()));
    std::fs::create_dir(&fixture_path).expect("create owned fixture");
    let fixture = Fixture(fixture_path);
    std::fs::write(fixture.0.join("model.bin"), b"synthetic").expect("write synthetic marker");
    let python = root.join(".venv/Scripts/python.exe");
    for (generation, scenario) in [(1, "transcribe_measured"), (2, "transcribe_recover")] {
        let (mut worker, _) = Worker::start(
            &python,
            &["tests/fake_worker.py", scenario],
            &root.join("worker"),
            generation,
        )
        .await
        .expect("start fake worker");
        worker.prepare_cpu(&fixture.0).await.expect("fake prepare");
        let response = worker
            .transcribe_measured(Uuid::new_v4(), &[0.0; 16000])
            .await
            .expect("fake result");
        assert!(response.text.is_empty());
        if scenario == "transcribe_measured" {
            assert_eq!(response.timings.total_ms, Some(9000));
            assert_eq!(response.timings.frontend_ms, Some(200));
            assert_eq!(response.timings.decode_ms, Some(8800));
        } else {
            assert_eq!(response.timings.total_ms, None);
            assert_eq!(response.timings.frontend_ms, None);
            assert_eq!(response.timings.decode_ms, None);
        }
        worker.shutdown().await.expect("confirmed fake exit");
    }
    println!(
        "synthetic_timing_metadata=true; legacy_metadata_compatible=true; fake_workers_exited=true"
    );
}
