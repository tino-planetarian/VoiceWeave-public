//! Explicit, microphone-free integration test against already installed local models.
use std::{fs, path::PathBuf, time::Instant};
use voiceweave_lib::engine::{EngineSlot, WorkerBackend};
use voiceweave_lib::models::{self, ModelSelection};
use voiceweave_lib::settings::{Settings, SettingsStore};

#[tokio::main]
async fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let tiny = models::select(&root.join("models"), "dev-tiny").unwrap();
    let standard = models::select(&root.join("models"), "standard").unwrap();
    let started = Instant::now();
    models::verify(&tiny).expect("tiny files");
    models::verify(&standard).expect("pinned standard model files");
    println!(
        "Installed model verification: {} ms",
        started.elapsed().as_millis()
    );

    let fixture = std::env::temp_dir().join(format!("voiceweave-switch-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&fixture).unwrap();
    let store = SettingsStore::new(fixture.join("settings.json"));
    let mut settings = Settings::default();
    settings.recognition.preset = "dev-tiny".into();
    let mut engine = EngineSlot::new(WorkerBackend {
        executable: root.join(".venv/Scripts/python.exe"),
        args: vec!["-m".into(), "voiceweave_worker".into()],
        cwd: root.join("worker"),
        // Optional GPU run: point VOICEWEAVE_CUDA_DIR at a folder with the cuBLAS DLLs.
        cuda_dir: std::env::var_os("VOICEWEAVE_CUDA_DIR").map(Into::into),
        english_dir: None,
        english_before: None,
    });
    let started = Instant::now();
    let saved = engine
        .apply(tiny.clone(), || {
            store.save(&settings, 0).map_err(|error| error.to_string())
        })
        .await
        .expect("tiny prepare and commit");
    println!(
        "tiny prepare + synthetic silence + save: {} ms",
        started.elapsed().as_millis()
    );
    assert_eq!(store.load().unwrap().into_settings(), saved);
    assert_eq!(engine.selection().unwrap().preset, "dev-tiny");

    settings.recognition.preset = "standard".into();
    let started = Instant::now();
    let saved = engine
        .apply(standard.clone(), || {
            store
                .save(&settings, saved.revision)
                .map_err(|error| error.to_string())
        })
        .await
        .expect("standard switch and commit");
    println!(
        "standard prepare + synthetic silence + save: {} ms",
        started.elapsed().as_millis()
    );
    assert_eq!(store.load().unwrap().into_settings(), saved);
    assert_eq!(engine.selection().unwrap().preset, "standard");
    println!("standard runs on: {}", engine.device().unwrap());

    let generation = engine.generation();
    let missing = ModelSelection {
        preset: "dev-tiny".into(),
        directory: fixture.join("missing"),
    };
    assert!(models::verify(&missing).is_err());
    assert_eq!(engine.generation(), generation);
    assert_eq!(engine.selection().unwrap().preset, "standard");
    println!("missing files: rejected before changing active engine");

    // All files exist, but these synthetic weights cannot initialize an STT engine.
    let broken_dir = fixture.join("broken");
    fs::create_dir(&broken_dir).unwrap();
    for (name, bytes) in [
        ("config.json", "{}"),
        ("model.bin", "not weights"),
        ("tokenizer.json", "{}"),
        ("vocabulary.txt", "token"),
    ] {
        fs::write(broken_dir.join(name), bytes).unwrap();
    }
    let broken = ModelSelection {
        preset: "dev-tiny".into(),
        directory: broken_dir.clone(),
    };
    models::verify(&broken).unwrap();
    let failure = engine
        .apply(broken, || -> Result<(), String> {
            panic!("failed model must not save")
        })
        .await
        .unwrap_err();
    assert!(failure.restored);
    assert_eq!(engine.selection().unwrap().preset, "standard");
    assert_eq!(store.load().unwrap().into_settings(), saved);
    println!("failed real model prepare: restored standard; settings unchanged");

    let failure = engine
        .apply(tiny, || {
            store.save(&settings, 0).map_err(|error| error.to_string())
        })
        .await
        .unwrap_err();
    assert!(failure.restored);
    assert_eq!(engine.selection().unwrap().preset, "standard");
    assert_eq!(store.load().unwrap().into_settings(), saved);
    println!("save revision conflict: restored standard; settings unchanged");
    engine.clear().await.expect("confirmed final worker exit");
    assert!(engine.selection().is_none());
    for name in [
        "config.json",
        "model.bin",
        "tokenizer.json",
        "vocabulary.txt",
    ] {
        fs::remove_file(broken_dir.join(name)).unwrap();
    }
    fs::remove_dir(broken_dir).unwrap();
    fs::remove_file(fixture.join("settings.json")).unwrap();
    fs::remove_dir(fixture).unwrap();
    println!("all probe workers reaped; synthetic fixtures removed");
}
