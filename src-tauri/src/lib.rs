#[cfg(feature = "experimental-audio")]
pub mod audio;
pub mod autostart;
pub mod candidates;
pub mod cloud;
pub mod coordinator;
pub mod cuda;
pub mod diagnostics;
pub mod dictionary;
pub mod engine;
pub mod glossary;
pub mod gpu;
pub mod hotkeys;
pub mod input;
pub mod install;
pub mod job;
#[cfg(feature = "experimental-audio")]
pub mod memory_gate;
mod memory_prompt;
pub mod migration;
pub mod models;
pub mod ollama;
mod ollama_install;
#[cfg(feature = "experimental-audio")]
mod overlay;
#[cfg(feature = "experimental-audio")]
mod pipeline;
pub mod polish;
pub mod postprocess;
#[cfg(feature = "experimental-audio")]
pub mod recording;
pub mod screen;
#[cfg(feature = "experimental-audio")]
mod session_guard;
pub mod settings;
pub mod shortcut;
pub mod spoken;
#[cfg(feature = "experimental-audio")]
pub mod streaming;
pub mod worker;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{Emitter, Manager, State};

#[derive(Serialize)]
struct WorkerProbe {
    version: String,
}

#[derive(Serialize)]
struct SettingsResponse {
    settings: settings::Settings,
    recovered: bool,
}

fn settings_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_config_dir()
        .map_err(|error| error.to_string())?
        .join("settings.json"))
}

/// Models Ollama has downloaded, or why it cannot be reached.
#[tauri::command]
async fn polish_models(endpoint: String) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || polish::installed_models(&endpoint))
        .await
        .map_err(|_| "確認を完了できませんでした".to_owned())?
        .map_err(|error| error.status().to_owned())
}

/// Whether Ollama can be used, told apart as answering / slow / installed but not running
/// / not installed (retried a few times, so a start-up moment is not taken for "missing").
#[tauri::command]
async fn ollama_status(endpoint: String) -> Result<ollama::Status, String> {
    tauri::async_runtime::spawn_blocking(move || ollama::check(&endpoint))
        .await
        .map_err(|_| "確認を完了できませんでした".to_owned())?
        .map_err(|error| error.status().to_owned())
}

/// Download Ollama's installer from the official site, check its signature and start it
/// (progress arrives as "ollama_install" events). Only on the user's click.
#[tauri::command]
fn ollama_install_start(app: tauri::AppHandle) -> Result<(), String> {
    ollama_install::start_install(&app).map_err(str::to_owned)
}

#[tauri::command]
fn ollama_install_cancel() {
    ollama_install::cancel_install();
}

/// Download a model with the installed (and running) Ollama ("ollama_pull" events).
#[tauri::command]
fn ollama_pull_start(app: tauri::AppHandle, model: String) -> Result<(), String> {
    ollama_install::start_pull(&app, &model).map_err(str::to_owned)
}

#[tauri::command]
fn ollama_pull_cancel() {
    ollama_install::cancel_pull();
}

/// Start the installed Ollama (its own tray app, as at sign-in).
#[tauri::command]
async fn ollama_start() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(ollama::start)
        .await
        .map_err(|_| "起動を完了できませんでした".to_owned())?
        .map_err(str::to_owned)
}

/// The glossary text with the starting glossary's missing terms added (nothing saved).
#[tauri::command]
fn glossary_with_defaults(current: String) -> String {
    glossary::with_defaults(&current)
}

/// Dictionary suggestions from the drafting model's corrections (in memory only).
#[tauri::command]
fn dictionary_candidates(
    store: State<'_, candidates::CandidateStore>,
) -> Vec<candidates::Candidate> {
    store.list()
}

/// Drop a suggestion once the user registered it (`dismissed` false) or turned it down
/// (`dismissed` true: not offered again while the app runs).
#[tauri::command]
fn remove_dictionary_candidate(
    store: State<'_, candidates::CandidateStore>,
    term: String,
    alias: String,
    dismissed: bool,
) {
    store.remove(&candidates::Candidate { term, alias }, dismissed);
}

/// Names often on the screen, offered for the field glossary (in memory only).
#[tauri::command]
fn screen_word_candidates(store: State<'_, candidates::ScreenWordStore>) -> Vec<String> {
    store.list()
}

/// Drop a screen-name suggestion once added to the glossary (`dismissed` false) or turned
/// down (`dismissed` true: not offered again while the app runs).
#[tauri::command]
fn remove_screen_word_candidate(
    store: State<'_, candidates::ScreenWordStore>,
    word: String,
    dismissed: bool,
) {
    store.remove(&word, dismissed);
}

/// Save an API key for a cloud service in the Windows Credential Manager. The key never
/// comes back to the screen; only whether one is saved.
#[tauri::command]
fn save_api_key(provider: String, key: String) -> Result<(), String> {
    let provider = cloud::Provider::parse(&provider).ok_or("不明なサービスです")?;
    cloud::save_key(provider, &key)
}

#[tauri::command]
fn delete_api_key(provider: String) -> Result<(), String> {
    let provider = cloud::Provider::parse(&provider).ok_or("不明なサービスです")?;
    cloud::delete_key(provider);
    Ok(())
}

/// Whether this PC has an NVIDIA GPU (nvidia-smi answers). Without one, a local model
/// (Ollama) runs on the CPU: slow, and it competes with recording.
#[tauri::command]
async fn nvidia_gpu_present() -> bool {
    tauri::async_runtime::spawn_blocking(|| gpu::snapshot().is_some())
        .await
        .unwrap_or(false)
}

/// Which cloud services have a key saved.
#[tauri::command]
fn api_key_status() -> serde_json::Value {
    serde_json::json!({
        "openai": cloud::has_key(cloud::Provider::OpenAi),
        "anthropic": cloud::has_key(cloud::Provider::Anthropic),
        "groq": cloud::has_key(cloud::Provider::Groq),
    })
}

/// Chat models the account can use (this also checks the key and the connection).
#[tauri::command]
async fn cloud_models(provider: String) -> Result<Vec<String>, String> {
    let provider = cloud::Provider::parse(&provider).ok_or("不明なサービスです")?;
    tauri::async_runtime::spawn_blocking(move || cloud::list_models(provider))
        .await
        .map_err(|_| "確認を完了できませんでした".to_owned())?
        .map_err(|error| error.status().to_owned())
}

/// The tray image drawn for the size Windows shows it at (16 px at 100% scaling gets a
/// thicker tile edge so it stays visible on a light taskbar; see scripts/make-icon.py).
fn tray_icon() -> tauri::image::Image<'static> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSMICON};
    let wanted = unsafe { GetSystemMetrics(SM_CXSMICON) };
    let (size, rgba): (u32, &'static [u8]) = match wanted {
        ..=17 => (16, include_bytes!("../icons/tray-16.rgba")),
        18..=21 => (20, include_bytes!("../icons/tray-20.rgba")),
        22..=27 => (24, include_bytes!("../icons/tray-24.rgba")),
        _ => (32, include_bytes!("../icons/tray-32.rgba")),
    };
    tauri::image::Image::new(rgba, size, size)
}

#[tauri::command]
fn capture_shortcut(app: tauri::AppHandle, active: bool) {
    app.state::<hotkeys::ShortcutCapture>().set(active);
}

#[tauri::command]
fn get_settings(app: tauri::AppHandle) -> Result<SettingsResponse, String> {
    match app.state::<settings::SettingsStore>().load() {
        Ok(settings::LoadResult::Defaults(settings))
        | Ok(settings::LoadResult::Loaded(settings)) => Ok(SettingsResponse {
            settings,
            recovered: false,
        }),
        Ok(settings::LoadResult::RecoveredDefaults { settings, .. }) => {
            let _ = app.emit(
                "user_notice",
                "設定ファイルを読み込めなかったため、既定値で起動しました",
            );
            Ok(SettingsResponse {
                settings,
                recovered: true,
            })
        }
        Err(error) => Err(error.to_string()),
    }
}

/// The answer to the memory question (src/memory_prompt.rs): switch the punctuation to the
/// built-in rules for good, or leave it. Works without the settings window.
#[tauri::command]
async fn answer_memory_prompt(app: tauri::AppHandle, switch_to_rules: bool) -> Result<(), String> {
    if switch_to_rules {
        let current = app
            .state::<settings::SettingsStore>()
            .load()
            .map_err(|error| error.to_string())?
            .into_settings();
        let model = current.punctuation.model.clone();
        let endpoint = current.polish.endpoint.clone();
        let revision = current.revision;
        let mut next = current;
        next.punctuation.mode = "rules".into();
        let saved = persist_settings(&app, next, revision).await?;
        let _ = app.emit("settings_reloaded", saved.revision);
        // The model is no longer needed: take it off now, to free its memory.
        std::thread::spawn(move || {
            let _ = polish::unload(&endpoint, &model);
        });
        let _ = app.emit(
            "user_notice",
            "句読点を「規則」に切り替えました（設定の「句読点の補い方」でいつでも戻せます）",
        );
    }
    memory_prompt::close(&app);
    Ok(())
}

#[tauri::command]
async fn save_settings(
    app: tauri::AppHandle,
    settings: settings::Settings,
    expected_revision: u64,
) -> Result<settings::Settings, String> {
    let restore = apply_shortcuts(&app, &settings)?;
    let saved = persist_settings(&app, settings, expected_revision).await;
    let saved = match (saved, restore) {
        (Ok(saved), _) => saved,
        (Err(error), Some(previous)) => {
            // Saving failed: the screen still shows the old shortcuts, so restore them.
            let _ = app.state::<hotkeys::HotkeyService>().rebind(previous);
            return Err(error);
        }
        (Err(error), None) => return Err(error),
    };
    // The setting is saved either way; a Windows registration failure is reported
    // separately, and the screen re-reads what Windows will actually do.
    if sync_autostart(saved.autostart).is_err() {
        let _ = app.emit(
            "user_notice",
            "設定は保存しましたが、Windowsの自動起動を変更できませんでした",
        );
    }
    Ok(saved)
}

/// Register changed shortcuts before saving them. Returns the previous pair to restore
/// if the save then fails; an error leaves the old shortcuts registered and unsaved.
/// The shortcuts a settings value asks for; the polish key only while that mode is on.
fn bindings_of(
    settings: &settings::Settings,
) -> Result<shortcut::Bindings, shortcut::ShortcutError> {
    let bindings = shortcut::parse_bindings(
        &settings.shortcuts.record,
        &settings.shortcuts.repaste,
        settings
            .polish
            .enabled
            .then_some(settings.polish.shortcut.as_str()),
    )?;
    let bindings = shortcut::with_compose(
        bindings,
        (settings.polish.enabled && settings.polish.compose_enabled)
            .then_some(settings.polish.compose_shortcut.as_str()),
    )?;
    shortcut::with_english(
        bindings,
        settings
            .shortcuts
            .english_enabled
            .then_some(settings.shortcuts.english.as_str()),
    )
}

fn apply_shortcuts(
    app: &tauri::AppHandle,
    candidate: &settings::Settings,
) -> Result<Option<shortcut::Bindings>, String> {
    let requested = bindings_of(candidate).map_err(|error| error.message())?;
    // Compare with what is actually registered, not with the saved file: after a
    // start-up fallback to the defaults, saving the same set retries registration.
    let service = app.state::<hotkeys::HotkeyService>();
    let previous = service.bound();
    let held = service.status();
    if !hotkeys::needs_rebind(requested, previous, held) {
        return Ok(None);
    }
    service
        .rebind(requested)
        .map_err(|_| {
            "ショートカットを登録できませんでした。ほかのアプリが使っている可能性があります。以前のショートカットのままです"
                .to_owned()
        })?;
    Ok(Some(previous))
}

async fn persist_settings(
    app: &tauri::AppHandle,
    settings: settings::Settings,
    expected_revision: u64,
) -> Result<settings::Settings, String> {
    #[cfg(feature = "experimental-audio")]
    let saved = match app.state::<AudioPipeline>().0.as_ref() {
        Some(pipeline) => pipeline.save_settings(settings, expected_revision).await?,
        None => app
            .state::<settings::SettingsStore>()
            .save(&settings, expected_revision)
            .map_err(|error| error.to_string())?,
    };
    #[cfg(not(feature = "experimental-audio"))]
    let saved = app
        .state::<settings::SettingsStore>()
        .save(&settings, expected_revision)
        .map_err(|error| error.to_string())?;
    Ok(saved)
}

/// Only an installed (release) build registers itself; a development build's
/// executable is a temporary build output and must not start at sign-in.
fn autostart_executable() -> Option<PathBuf> {
    if cfg!(debug_assertions) {
        None
    } else {
        std::env::current_exe().ok()
    }
}

/// Width of the settings window in logical pixels; its height fills the work area.
const SETTINGS_WIDTH: f64 = 640.0;

/// Size the (still hidden) settings window to nearly the full height of the primary
/// monitor's work area, so the tabs fit without the user stretching it every time.
/// Runs once at startup; sizes the user picks later are kept until the app restarts.
fn fit_settings_window(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let Ok(Some(monitor)) = window.primary_monitor() else {
        return;
    };
    let (Ok(outer), Ok(inner)) = (window.outer_size(), window.inner_size()) else {
        return;
    };
    let scale = monitor.scale_factor();
    let work = monitor.work_area();
    let margin = (12.0 * scale) as u32;
    // Title bar and borders, so the outer frame fits the work area.
    let frame_height = outer.height.saturating_sub(inner.height);
    let frame_width = outer.width.saturating_sub(inner.width);
    let height = work
        .size
        .height
        .saturating_sub(margin * 2 + frame_height)
        .max(inner.height);
    let width = ((SETTINGS_WIDTH * scale) as u32).min(work.size.width.saturating_sub(frame_width));
    let _ = window.set_size(tauri::PhysicalSize::new(width, height));
    let x = work.position.x + (work.size.width as i32 - (width + frame_width) as i32) / 2;
    let _ = window.set_position(tauri::PhysicalPosition::new(
        x,
        work.position.y + margin as i32,
    ));
}

fn sync_autostart(enabled: bool) -> Result<(), autostart::RegistryError> {
    let Some(executable) = autostart_executable() else {
        return Ok(());
    };
    let entry = autostart::WindowsRunEntry::current_user();
    if autostart::is_registered(&entry, &executable)? == enabled {
        return Ok(());
    }
    match autostart::apply(&entry, enabled, &executable)? {
        registered if registered == enabled => Ok(()),
        _ => Err(autostart::RegistryError),
    }
}

/// Microphones currently present, for the settings screen. Enumerated off the UI
/// thread because the audio backend initializes COM on the calling thread.
#[cfg(feature = "experimental-audio")]
#[tauri::command]
async fn list_input_devices() -> Vec<audio::InputDeviceInfo> {
    tauri::async_runtime::spawn_blocking(audio::input_devices)
        .await
        .unwrap_or_default()
}

#[cfg(not(feature = "experimental-audio"))]
#[tauri::command]
async fn list_input_devices() -> Vec<String> {
    Vec::new()
}

#[derive(Serialize)]
struct AutostartStatus {
    supported: bool,
    registered: bool,
}

#[tauri::command]
fn autostart_status() -> AutostartStatus {
    match autostart_executable() {
        None => AutostartStatus {
            supported: false,
            registered: false,
        },
        Some(executable) => AutostartStatus {
            supported: true,
            registered: autostart::is_registered(
                &autostart::WindowsRunEntry::current_user(),
                &executable,
            )
            .unwrap_or(false),
        },
    }
}

#[derive(Clone, Default, Serialize)]
pub(crate) struct EngineStatus {
    enabled: bool,
    phase: String,
    active_preset: Option<String>,
    external_override: bool,
    /// "cpu" or "cuda": what the loaded engine actually runs on.
    device: Option<String>,
}

#[cfg(feature = "experimental-audio")]
struct AudioPipeline(Option<pipeline::PipelineSender>);

#[tauri::command]
fn engine_status(app: tauri::AppHandle) -> EngineStatus {
    #[cfg(feature = "experimental-audio")]
    if let Some(pipeline) = app.state::<AudioPipeline>().0.as_ref() {
        return pipeline.status();
    }
    let _ = app;
    EngineStatus::default()
}

#[tauri::command]
fn diagnostics_snapshot(app: tauri::AppHandle) -> diagnostics::DiagnosticsSnapshot {
    app.state::<diagnostics::DiagnosticsStore>().snapshot()
}

#[derive(Serialize)]
struct DictionaryPreview {
    text: String,
    replacements: usize,
    hints: Vec<String>,
}

/// Show what the unsaved dictionary would do to a sample sentence, using the same
/// replacement code as recognition. The sample is not stored.
#[tauri::command]
fn preview_dictionary(text: String, entries: Vec<settings::DictionaryEntry>) -> DictionaryPreview {
    let sample: String = text.chars().take(500).collect();
    let (text, replacements) = dictionary::apply_aliases(&sample, &entries);
    DictionaryPreview {
        text,
        replacements,
        hints: dictionary::hints(&entries),
    }
}

/// Optional GPU runtime location. `VOICEWEAVE_DEVICE=cpu` forces CPU for comparison;
/// `VOICEWEAVE_CUDA_DIR` overrides the managed `cuda` folder for development.
/// Only a directory that already holds both cuBLAS libraries is offered to the worker.
#[cfg(feature = "experimental-audio")]
fn cuda_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    if std::env::var("VOICEWEAVE_DEVICE").is_ok_and(|value| value == "cpu") {
        return None;
    }
    let directory = match std::env::var_os("VOICEWEAVE_CUDA_DIR") {
        Some(directory) => PathBuf::from(directory),
        None => cuda::managed_dir(app)?,
    };
    cuda::is_complete(&directory).then_some(directory)
}

#[tauri::command]
fn cuda_status(app: tauri::AppHandle) -> cuda::CudaStatus {
    cuda::status(&app)
}

#[tauri::command]
fn install_cuda(app: tauri::AppHandle) -> Result<(), String> {
    cuda::start_install(&app)
}

#[tauri::command]
fn cancel_cuda_install(app: tauri::AppHandle) {
    install::cancel(&app, install::Which::Cuda);
}

/// Re-prepare the saved model, e.g. after a GPU runtime or model was installed.
#[cfg(feature = "experimental-audio")]
fn reload_engine(app: &tauri::AppHandle) {
    if let Some(pipeline) = app.state::<AudioPipeline>().0.as_ref() {
        pipeline.reload_engine();
    }
}

#[cfg(not(feature = "experimental-audio"))]
fn reload_engine(_app: &tauri::AppHandle) {}

#[derive(Serialize)]
struct ModelSetupStatus {
    installing: bool,
}

#[tauri::command]
fn model_setup_status(app: tauri::AppHandle) -> ModelSetupStatus {
    ModelSetupStatus {
        installing: app.state::<install::Installers>().model.installing(),
    }
}

/// Install the pinned standard model into the app's own storage: download it, or copy
/// it from a folder that already holds the same pinned files. Either way every file is
/// checked against the catalog before it is published, and nothing is overwritten.
#[tauri::command]
fn install_model(
    app: tauri::AppHandle,
    source: Option<String>,
    preset: Option<String>,
) -> Result<(), String> {
    let root = model_roots(&app)?.managed;
    let mut arguments: Vec<std::ffi::OsString> = vec!["--model-root".into(), root.into()];
    // Only presets with a pinned catalog can be installed; the worker checks it again.
    match preset.as_deref() {
        None | Some("standard") => {}
        Some(preset @ ("accurate" | "parakeet-ja" | "parakeet-en")) => {
            arguments.extend(["--preset".into(), preset.into()])
        }
        Some(_) => return Err("この認識プリセットは導入できません".into()),
    }
    if let Some(source) = source.map(|value| value.trim().to_owned()) {
        let path = PathBuf::from(&source);
        if source.is_empty() || !path.is_absolute() || !path.is_dir() {
            return Err(
                "コピー元のフォルダーが見つかりません。フォルダーの場所を確認してください".into(),
            );
        }
        arguments.extend(["--source".into(), path.into()]);
    }
    install::start(&app, install::Which::Model, arguments, reload_engine)
}

#[tauri::command]
fn cancel_model_install(app: tauri::AppHandle) {
    install::cancel(&app, install::Which::Model);
}

fn model_roots(app: &tauri::AppHandle) -> Result<models::ModelRoots, String> {
    let managed = app
        .path()
        .app_local_data_dir()
        .map_err(|_| "モデル保存先を確認できませんでした".to_owned())?
        .join("models");
    #[cfg(debug_assertions)]
    let development = Some(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("project root")
            .join("models"),
    );
    #[cfg(not(debug_assertions))]
    let development = None;
    Ok(models::ModelRoots {
        managed,
        development,
    })
}

#[tauri::command]
fn model_inventory(app: tauri::AppHandle) -> Result<Vec<models::ModelAvailability>, String> {
    Ok(model_roots(&app)?.inventory())
}

fn worker_command(
    _app: &tauri::AppHandle,
) -> Result<(PathBuf, Vec<&'static str>, PathBuf), String> {
    #[cfg(debug_assertions)]
    {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .ok_or("Cannot locate project root")?
            .to_path_buf();
        let executable = std::env::var_os("VOICEWEAVE_PYTHON")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join(".venv/Scripts/python.exe"));
        Ok((
            executable,
            vec!["-m", "voiceweave_worker", "--self-check"],
            root.join("worker"),
        ))
    }
    #[cfg(not(debug_assertions))]
    {
        let exe = _app
            .path()
            .resource_dir()
            .map_err(|e| e.to_string())?
            .join("stt/voiceweave_worker/voiceweave_worker.exe");
        let cwd = exe
            .parent()
            .ok_or("Invalid worker resource path")?
            .to_path_buf();
        Ok((exe, vec!["--self-check"], cwd))
    }
}

fn hello_version(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() < 4 {
        return Err("Worker did not send a hello frame".into());
    }
    let header_len =
        u32::from_le_bytes(bytes[..4].try_into().map_err(|_| "Bad frame length")?) as usize;
    if header_len == 0 || header_len > 65_536 || bytes.len() != 4 + header_len {
        return Err("Invalid worker frame size".into());
    }
    let header: serde_json::Value =
        serde_json::from_slice(&bytes[4..]).map_err(|_| "Invalid worker JSON")?;
    if header.get("v").and_then(|v| v.as_u64()) != Some(1)
        || header.get("kind").and_then(|v| v.as_str()) != Some("hello")
        || header.get("request_id") != Some(&serde_json::Value::Null)
        || header.get("payload_bytes").and_then(|v| v.as_u64()) != Some(0)
    {
        return Err("Worker protocol mismatch".into());
    }
    header
        .get("worker_version")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .ok_or_else(|| "Worker version missing".into())
}

/// Used by the Tauri command and by the independent executable probe.
pub fn probe_executable(exe: &Path, args: &[&str], cwd: &Path) -> Result<String, String> {
    if !exe.is_file() {
        return Err("Worker executable is not installed".into());
    }
    let mut command = Command::new(exe);
    command
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().map_err(|_| "Could not launch worker")?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if child
            .try_wait()
            .map_err(|_| "Could not inspect worker")?
            .is_some()
        {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Worker self-check timed out".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let output = child
        .wait_with_output()
        .map_err(|_| "Could not read worker output")?;
    if !output.status.success() {
        return Err("Worker self-check failed".into());
    }
    hello_version(&output.stdout)
}

#[tauri::command]
async fn probe_worker(app: tauri::AppHandle) -> Result<WorkerProbe, String> {
    let (exe, mut args, cwd) = worker_command(&app)?;
    args.pop(); // The persistent worker must remain open for shutdown.
    let (worker, version) = worker::Worker::start(&exe, &args, &cwd, 0)
        .await
        .map_err(|error| format!("Worker start failed: {error:?}"))?;
    worker
        .shutdown()
        .await
        .map_err(|error| format!("Worker shutdown failed: {error:?}"))?;
    Ok(WorkerProbe { version })
}

/// Same as the tray item: prepare a fresh engine after a failure. Never resends a
/// failed recognition; it only makes the next recording possible again.
#[tauri::command]
fn retry_engine(app: tauri::AppHandle) {
    #[cfg(feature = "experimental-audio")]
    if let Some(pipeline) = app.state::<AudioPipeline>().0.as_ref() {
        pipeline.retry_engine(&app);
    }
    let _ = app;
}

/// Development builds expose test-only choices (the tiny validation model).
#[tauri::command]
fn development_build() -> bool {
    cfg!(debug_assertions)
}

#[tauri::command]
fn input_probe_enabled() -> bool {
    cfg!(debug_assertions) && std::env::var("VOICEWEAVE_TEST_INPUT").as_deref() == Ok("1")
}

/// Voice input runs in every build that includes audio support. The debug-only fixed
/// text probe takes precedence, and `VOICEWEAVE_DISABLE_CAPTURE=1` turns voice input off
/// (for example to rule it out while diagnosing another problem).
fn capture_enabled() -> bool {
    cfg!(feature = "experimental-audio")
        && !input_probe_enabled()
        && std::env::var("VOICEWEAVE_DISABLE_CAPTURE").as_deref() != Ok("1")
}

#[tauri::command]
fn capture_probe_enabled() -> bool {
    capture_enabled()
}

#[tauri::command]
fn hotkey_status(state: State<hotkeys::HotkeyService>) -> hotkeys::HotkeyStatus {
    state.status()
}

#[cfg(debug_assertions)]
#[derive(Default)]
struct InputProbeState {
    last: std::sync::Mutex<Option<String>>,
    busy: std::sync::atomic::AtomicBool,
}

#[cfg(debug_assertions)]
fn run_input_probe(
    app: tauri::AppHandle,
    event: hotkeys::HotkeyEvent,
    state: std::sync::Arc<InputProbeState>,
) {
    use std::sync::atomic::Ordering;
    if event == hotkeys::HotkeyEvent::Cancel {
        return;
    }
    if state.busy.swap(true, Ordering::AcqRel) {
        let _ = app.emit("user_notice", "入力処理中です");
        return;
    }
    let target = input::capture_target();
    let text = match event {
        hotkeys::HotkeyEvent::Record
        | hotkeys::HotkeyEvent::Polish
        | hotkeys::HotkeyEvent::Compose
        | hotkeys::HotkeyEvent::English => {
            let text = "音声入力の確認です。FDF2026".to_owned();
            if let Ok(mut last) = state.last.lock() {
                *last = Some(text.clone());
            }
            text
        }
        hotkeys::HotkeyEvent::Cancel => unreachable!("returned above"),
        hotkeys::HotkeyEvent::Repaste => {
            match state.last.lock().ok().and_then(|last| last.clone()) {
                Some(text) => text,
                None => {
                    state.busy.store(false, Ordering::Release);
                    let _ = app.emit("user_notice", "直前の結果はありません");
                    return;
                }
            }
        }
    };
    let delay_ms = std::env::var("VOICEWEAVE_TEST_INPUT_DELAY_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value <= 10_000)
        .unwrap_or(0);
    std::thread::spawn(move || {
        if delay_ms != 0 {
            std::thread::sleep(Duration::from_millis(delay_ms));
        }
        let outcome = input::deliver(&text, target);
        let message = match outcome {
            input::DeliveryOutcome::Cancelled => "入力を取り消しました",
            input::DeliveryOutcome::SentUnverified => "貼り付けを実行しました",
            input::DeliveryOutcome::CopiedOnly(_) => {
                "コピーしました。対象またはキー状態を確認し、必要なら手動で貼り付けてください"
            }
            input::DeliveryOutcome::ClipboardUnavailable => "クリップボードを使用できません",
            input::DeliveryOutcome::SendFailed(_) => {
                "キー送信に失敗しました。必要なら手動で貼り付けてください"
            }
        };
        let _ = app.emit("user_notice", message);
        state.busy.store(false, Ordering::Release);
    });
}
pub fn run() {
    // Before anything opens the data folders (windows, WebView profile, worker).
    let (moved_settings, moved_models) = migration::migrate_legacy_data();
    let migrated = [moved_settings, moved_models]
        .iter()
        .any(|moved| *moved != migration::Moved::NothingToMove);
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .setup(move |app| {
            if migrated {
                // Shown in the settings window's notice area and its diagnostics tab.
                let _ = app.handle().emit(
                    "user_notice",
                    "以前の設定・辞書・モデルを新しい保存場所へ引き継ぎました",
                );
            }
            app.manage(diagnostics::DiagnosticsStore::default());
            app.manage(candidates::CandidateStore::default());
            app.manage(candidates::ScreenWordStore::default());
            app.manage(install::Installers::default());
            app.manage(settings::SettingsStore::new(
                settings_path(app.handle()).map_err(std::io::Error::other)?,
            ));
            // An uninstall step (for example "uninstall before installing" during an
            // update) removes the Run entry; restore what the saved setting asks for.
            // Release builds only (development builds never register).
            if let Ok(loaded) = app.state::<settings::SettingsStore>().load() {
                let _ = sync_autostart(loaded.into_settings().autostart);
            }
            fit_settings_window(app.handle());
            let handle = app.handle().clone();
            #[cfg(debug_assertions)]
            let probe = std::sync::Arc::new(InputProbeState::default());
            #[cfg(feature = "experimental-audio")]
            let pipeline = if capture_enabled() {
                overlay::create(&handle)?;
                let (pipeline, receiver) = pipeline::PipelineSender::new();
                tauri::async_runtime::spawn(pipeline::run(
                    handle.clone(),
                    receiver,
                    pipeline.gate(),
                    pipeline.cancellation(),
                    pipeline.engine_status(),
                ));
                Some(pipeline)
            } else {
                None
            };
            #[cfg(feature = "experimental-audio")]
            let menu_pipeline = pipeline.clone();
            #[cfg(feature = "experimental-audio")]
            if let Some(pipeline) = &pipeline {
                session_guard::start(app.handle().clone(), pipeline.clone());
            }
            #[cfg(feature = "experimental-audio")]
            app.manage(AudioPipeline(pipeline.clone()));
            // Saved shortcuts if they parse; otherwise the defaults. A pair that another
            // program holds at startup falls back to the defaults below.
            let saved_shortcuts = app
                .state::<settings::SettingsStore>()
                .load()
                .ok()
                .and_then(|loaded| bindings_of(&loaded.into_settings()).ok());
            let defaults =
                bindings_of(&settings::Settings::default()).expect("default shortcuts parse");
            let requested = saved_shortcuts.unwrap_or(defaults);
            app.manage(hotkeys::ShortcutCapture::default());
            let hotkeys = hotkeys::HotkeyService::start(requested, move |event| {
                // A shortcut field is waiting for keys: report the press, do not act on it.
                if handle.state::<hotkeys::ShortcutCapture>().active() {
                    let _ = handle.emit(
                        "shortcut_captured",
                        match event {
                            hotkeys::HotkeyEvent::Record => "record",
                            hotkeys::HotkeyEvent::Repaste => "repaste",
                            hotkeys::HotkeyEvent::Polish => "polish",
                            hotkeys::HotkeyEvent::Compose => "compose",
                            hotkeys::HotkeyEvent::English => "english",
                            hotkeys::HotkeyEvent::Cancel => "cancel",
                        },
                    );
                    return;
                }
                #[cfg(debug_assertions)]
                if std::env::var("VOICEWEAVE_TEST_INPUT").as_deref() == Ok("1") {
                    run_input_probe(handle.clone(), event, probe.clone());
                    return;
                }
                #[cfg(feature = "experimental-audio")]
                if let Some(pipeline) = &pipeline {
                    pipeline.dispatch(&handle, event);
                    return;
                }
                let message = match event {
                    hotkeys::HotkeyEvent::Record
                    | hotkeys::HotkeyEvent::Polish
                    | hotkeys::HotkeyEvent::Compose
                    | hotkeys::HotkeyEvent::English => "録音機能は準備中です",
                    hotkeys::HotkeyEvent::Repaste => "直前の認識結果はありません",
                    hotkeys::HotkeyEvent::Cancel => return,
                };
                let _ = handle.emit("user_notice", message);
            })
            .map_err(std::io::Error::other)?;
            let held = hotkeys.status();
            if !held.covers(requested) && requested != defaults && hotkeys.rebind(defaults).is_ok()
            {
                let _ = app.handle().emit(
                    "user_notice",
                    "保存したショートカットを登録できなかったため、F8 と Ctrl+Alt+V を使います",
                );
            }
            app.manage(hotkeys);
            let settings = MenuItem::with_id(app, "settings", "設定を開く", true, None::<&str>)?;
            #[cfg(feature = "experimental-audio")]
            let capture_enabled = menu_pipeline.is_some();
            #[cfg(not(feature = "experimental-audio"))]
            let capture_enabled = false;
            let cancel = MenuItem::with_id(
                app,
                "cancel_capture",
                "音声入力を取り消す",
                capture_enabled,
                None::<&str>,
            )?;
            let retry = MenuItem::with_id(
                app,
                "retry_engine",
                "認識エンジンを再準備",
                capture_enabled,
                None::<&str>,
            )?;
            let copy_last = MenuItem::with_id(
                app,
                "copy_last_result",
                "直前の結果をコピー",
                capture_enabled,
                None::<&str>,
            )?;
            let quit = MenuItem::with_id(app, "quit", "終了", true, None::<&str>)?;
            let menu = if capture_enabled {
                Menu::with_items(app, &[&settings, &copy_last, &cancel, &retry, &quit])?
            } else {
                Menu::with_items(app, &[&settings, &quit])?
            };
            TrayIconBuilder::new()
                .icon(tray_icon())
                .tooltip("VoiceWeave")
                .menu(&menu)
                .on_menu_event(move |app, event| match event.id.as_ref() {
                    "settings" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    #[cfg(feature = "experimental-audio")]
                    "cancel_capture" => {
                        if let Some(pipeline) = &menu_pipeline {
                            pipeline.cancel(app);
                        }
                    }
                    #[cfg(feature = "experimental-audio")]
                    "retry_engine" => {
                        if let Some(pipeline) = &menu_pipeline {
                            pipeline.retry_engine(app);
                        }
                    }
                    #[cfg(feature = "experimental-audio")]
                    "copy_last_result" => {
                        if let Some(pipeline) = &menu_pipeline {
                            pipeline.copy_last_result(app);
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
                window.state::<hotkeys::ShortcutCapture>().set(false);
            }
        })
        .invoke_handler(tauri::generate_handler![
            answer_memory_prompt,
            probe_worker,
            hotkey_status,
            capture_shortcut,
            polish_models,
            ollama_status,
            ollama_start,
            ollama_install_start,
            ollama_install_cancel,
            ollama_pull_start,
            ollama_pull_cancel,
            save_api_key,
            delete_api_key,
            api_key_status,
            nvidia_gpu_present,
            cloud_models,
            input_probe_enabled,
            development_build,
            retry_engine,
            capture_probe_enabled,
            engine_status,
            diagnostics_snapshot,
            autostart_status,
            list_input_devices,
            preview_dictionary,
            glossary_with_defaults,
            dictionary_candidates,
            remove_dictionary_candidate,
            screen_word_candidates,
            remove_screen_word_candidate,
            cuda_status,
            install_cuda,
            cancel_cuda_install,
            model_setup_status,
            install_model,
            cancel_model_install,
            model_inventory,
            get_settings,
            save_settings
        ])
        .run(tauri::generate_context!())
        .expect("failed to run VoiceWeave");
}

#[cfg(test)]
mod tests {
    use super::hello_version;

    #[test]
    fn rejects_corrupt_worker_reply() {
        assert!(hello_version(&4u32.to_le_bytes()).is_err());
        let json = br#"{"v":1,"kind":"result","request_id":null,"payload_bytes":0}"#;
        let mut bytes = (json.len() as u32).to_le_bytes().to_vec();
        bytes.extend_from_slice(json);
        assert!(hello_version(&bytes).is_err());
    }
}
