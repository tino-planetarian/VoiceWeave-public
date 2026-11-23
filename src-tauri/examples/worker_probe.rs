use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("project root")
        .to_path_buf();
    let worker_dir = root.join("worker");
    let exe = root.join(".venv/Scripts/python.exe");
    match voiceweave_lib::probe_executable(
        &exe,
        &["-m", "voiceweave_worker", "--self-check"],
        &worker_dir,
    ) {
        Ok(version) => println!("Python worker hello: {version}"),
        Err(error) => {
            eprintln!("Worker probe failed: {error}");
            std::process::exit(1);
        }
    }
}
