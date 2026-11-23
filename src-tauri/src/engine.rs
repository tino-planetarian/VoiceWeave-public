//! A single loaded engine, with rollback when model preparation or persistence fails.
use crate::models::ModelSelection;
use std::future::Future;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineFailure {
    Prepare,
    Termination,
}

pub trait EngineBackend: Send {
    type Handle: Send;
    fn prepare(
        &mut self,
        model: &ModelSelection,
        generation: u64,
    ) -> impl Future<Output = Result<Self::Handle, EngineFailure>> + Send;
    fn stop(
        &mut self,
        handle: Self::Handle,
    ) -> impl Future<Output = Result<(), EngineFailure>> + Send;
    /// The device the prepared handle really runs on, never the one that was requested.
    fn device(_handle: &Self::Handle) -> &'static str {
        "cpu"
    }
    /// Settings the backend was given for a change in progress (beyond the model itself)
    /// go back to what they were: called when the change fails, before the previous
    /// model is loaded again.
    fn rollback_config(&mut self) {}
    /// The change succeeded: what was staged is now the setting.
    fn confirm_config(&mut self) {}
}

pub struct WorkerBackend {
    pub executable: std::path::PathBuf,
    pub args: Vec<String>,
    pub cwd: std::path::PathBuf,
    /// Complete CUDA 12 cuBLAS directory, when GPU recognition is available and wanted.
    pub cuda_dir: Option<std::path::PathBuf>,
    /// The English recognizer's folder, when the English shortcut is on and the model is
    /// installed; loaded next to every prepared worker.
    pub english_dir: Option<std::path::PathBuf>,
    /// The English folder in force before `stage_english`, until the change is confirmed
    /// or rolled back (a failed model switch must not leave the new English setting).
    pub english_before: Option<Option<std::path::PathBuf>>,
}

impl WorkerBackend {
    /// Use `dir` from now on, remembering what was there until `confirm_config`.
    pub fn stage_english(&mut self, dir: Option<std::path::PathBuf>) {
        let before = self.english_dir.clone();
        self.english_before.get_or_insert(before);
        self.english_dir = dir;
    }

    async fn start_worker(&self, generation: u64) -> Result<crate::worker::Worker, EngineFailure> {
        let args: Vec<&str> = self.args.iter().map(String::as_str).collect();
        crate::worker::Worker::start(&self.executable, &args, &self.cwd, generation)
            .await
            .map(|(worker, _)| worker)
            .map_err(|error| {
                if error == crate::worker::WorkerError::Termination {
                    EngineFailure::Termination
                } else {
                    EngineFailure::Prepare
                }
            })
    }

    /// Prepare one worker and prove it with a real (silent) inference before use.
    /// The flag reports a preparation timeout, which usually means memory pressure.
    async fn prepared(
        &self,
        model: &ModelSelection,
        generation: u64,
        cuda_dir: Option<&std::path::Path>,
    ) -> Result<crate::worker::Worker, (EngineFailure, bool)> {
        let mut worker = self
            .start_worker(generation)
            .await
            .map_err(|failure| (failure, false))?;
        let preparation = async {
            match (crate::models::engine(&model.preset), cuda_dir) {
                ("whisper", Some(dir)) => worker.prepare_cuda(&model.directory, dir).await?,
                ("whisper", None) => worker.prepare_cpu(&model.directory).await?,
                (engine, _) => worker.prepare_engine(&model.directory, engine).await?,
            }
            // Readiness includes a real inference, without using the microphone.
            worker
                .transcribe(uuid::Uuid::new_v4(), &[0.0_f32; 16_000])
                .await?;
            // The English recognizer is optional: a problem with it (missing files) must not
            // stop the main one; only a dead worker does.
            if let Some(dir) = self.english_dir.as_deref() {
                use crate::worker::WorkerError;
                match worker.prepare_english(dir).await {
                    Ok(()) | Err(WorkerError::ModelMissing) | Err(WorkerError::Remote(_)) => {}
                    Err(error) => return Err(error),
                }
            }
            Ok::<(), crate::worker::WorkerError>(())
        }
        .await;
        if let Err(error) = preparation {
            let timed_out = error == crate::worker::WorkerError::Timeout;
            return match worker.terminate().await {
                Ok(()) => Err((EngineFailure::Prepare, timed_out)),
                Err(_) => Err((EngineFailure::Termination, timed_out)),
            };
        }
        Ok(worker)
    }
}

impl EngineBackend for WorkerBackend {
    type Handle = crate::worker::Worker;

    async fn prepare(
        &mut self,
        model: &ModelSelection,
        generation: u64,
    ) -> Result<Self::Handle, EngineFailure> {
        // GPU first when its runtime is installed. A failure here happens before any
        // audio exists, so falling back to CPU with a fresh process cannot repeat input.
        // A timeout is not retried on CPU: it points at memory pressure, where a CPU
        // load would be slower and make things worse.
        // The sherpa-onnx engines run on the CPU only.
        let cuda_dir = self
            .cuda_dir
            .as_deref()
            .filter(|_| crate::models::engine(&model.preset) == "whisper");
        if let Some(dir) = cuda_dir {
            match self.prepared(model, generation, Some(dir)).await {
                Ok(worker) => return Ok(worker),
                Err((failure, true)) | Err((failure @ EngineFailure::Termination, _)) => {
                    return Err(failure)
                }
                Err((EngineFailure::Prepare, false)) => {}
            }
        }
        self.prepared(model, generation, None)
            .await
            .map_err(|(failure, _)| failure)
    }

    fn device(handle: &Self::Handle) -> &'static str {
        handle.device()
    }

    fn rollback_config(&mut self) {
        if let Some(before) = self.english_before.take() {
            self.english_dir = before;
        }
    }

    fn confirm_config(&mut self) {
        self.english_before = None;
    }

    async fn stop(&mut self, handle: Self::Handle) -> Result<(), EngineFailure> {
        handle
            .terminate()
            .await
            .map_err(|_| EngineFailure::Termination)
    }
}

#[derive(Debug)]
pub struct ApplyFailure {
    pub message: String,
    pub restored: bool,
    pub restart_required: bool,
}

pub struct EngineSlot<B: EngineBackend> {
    backend: B,
    active: Option<(ModelSelection, B::Handle)>,
    generation: u64,
    termination_uncertain: bool,
}

impl<B: EngineBackend> EngineSlot<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            active: None,
            generation: 0,
            termination_uncertain: false,
        }
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn selection(&self) -> Option<&ModelSelection> {
        self.active.as_ref().map(|(model, _)| model)
    }

    pub fn device(&self) -> Option<&'static str> {
        self.active.as_ref().map(|(_, handle)| B::device(handle))
    }

    pub fn handle_mut(&mut self) -> Option<&mut B::Handle> {
        self.active.as_mut().map(|(_, handle)| handle)
    }

    pub async fn clear(&mut self) -> Result<(), EngineFailure> {
        if self.termination_uncertain {
            return Err(EngineFailure::Termination);
        }
        self.generation += 1;
        if let Some((_, handle)) = self.active.take() {
            if self.backend.stop(handle).await.is_err() {
                self.termination_uncertain = true;
                return Err(EngineFailure::Termination);
            }
        }
        Ok(())
    }

    async fn load(&mut self, model: ModelSelection) -> Result<(), EngineFailure> {
        self.generation += 1;
        let handle = match self.backend.prepare(&model, self.generation).await {
            Ok(handle) => handle,
            Err(error) => {
                self.termination_uncertain = error == EngineFailure::Termination;
                return Err(error);
            }
        };
        self.active = Some((model, handle));
        Ok(())
    }

    async fn restore(&mut self, previous: Option<ModelSelection>) -> bool {
        if self.clear().await.is_err() {
            return false;
        }
        match previous {
            Some(model) => self.load(model).await.is_ok(),
            None => false,
        }
    }

    /// The caller verifies files and validates revisions before entering this transaction.
    /// Persistence runs only after prepare succeeds. No two handles are loaded together.
    pub async fn apply<T>(
        &mut self,
        model: ModelSelection,
        commit: impl FnOnce() -> Result<T, String> + Send,
    ) -> Result<T, ApplyFailure> {
        let previous = self.selection().cloned();
        if self.clear().await.is_err() {
            self.backend.rollback_config();
            return Err(ApplyFailure {
                message:
                    "以前の認識エンジンの終了を確認できませんでした。アプリを再起動してください"
                        .into(),
                restored: false,
                restart_required: true,
            });
        }
        if let Err(error) = self.load(model).await {
            // The previous model is loaded again with the previous settings.
            self.backend.rollback_config();
            let restored = error != EngineFailure::Termination && self.restore(previous).await;
            return Err(ApplyFailure {
                message: "選択した認識モデルを準備できませんでした".into(),
                restored,
                restart_required: self.termination_uncertain,
            });
        }
        match commit() {
            Ok(value) => {
                self.backend.confirm_config();
                Ok(value)
            }
            Err(message) => {
                self.backend.rollback_config();
                let restored = self.restore(previous).await;
                Err(ApplyFailure {
                    message,
                    restored,
                    restart_required: self.termination_uncertain,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Trace {
        events: Vec<String>,
        loaded: usize,
        /// The extra setting each preparation was given.
        configs: Vec<u32>,
    }
    struct FakeBackend {
        trace: Arc<Mutex<Trace>>,
        fail: Vec<String>,
        stop_failure: bool,
        /// Stands for a setting beyond the model (the English folder).
        config: u32,
        config_before: Option<u32>,
    }
    impl FakeBackend {
        fn stage(&mut self, config: u32) {
            self.config_before.get_or_insert(self.config);
            self.config = config;
        }
    }
    impl EngineBackend for FakeBackend {
        type Handle = String;
        fn rollback_config(&mut self) {
            if let Some(before) = self.config_before.take() {
                self.config = before;
            }
        }
        fn confirm_config(&mut self) {
            self.config_before = None;
        }
        async fn prepare(
            &mut self,
            model: &ModelSelection,
            generation: u64,
        ) -> Result<String, EngineFailure> {
            let mut trace = self.trace.lock().unwrap();
            assert_eq!(trace.loaded, 0, "models must never overlap");
            trace.configs.push(self.config);
            trace
                .events
                .push(format!("prepare:{}:{generation}", model.preset));
            if self.fail.contains(&model.preset) {
                return Err(EngineFailure::Prepare);
            }
            trace.loaded += 1;
            Ok(model.preset.clone())
        }
        async fn stop(&mut self, handle: String) -> Result<(), EngineFailure> {
            let mut trace = self.trace.lock().unwrap();
            trace.events.push(format!("stop:{handle}"));
            trace.loaded -= 1;
            if self.stop_failure {
                Err(EngineFailure::Termination)
            } else {
                Ok(())
            }
        }
    }
    fn selection(preset: &str) -> ModelSelection {
        ModelSelection {
            preset: preset.into(),
            directory: std::env::temp_dir().join(preset),
        }
    }
    fn slot(fail: &[&str]) -> EngineSlot<FakeBackend> {
        EngineSlot::new(FakeBackend {
            trace: Arc::new(Mutex::new(Trace::default())),
            fail: fail.iter().map(|preset| (*preset).into()).collect(),
            stop_failure: false,
            config: 0,
            config_before: None,
        })
    }

    #[tokio::test]
    async fn a_failed_change_brings_back_the_previous_extra_setting() {
        // A failed preparation: the previous model is loaded again with the old setting.
        let mut engine = slot(&["broken"]);
        engine.apply(selection("old"), || Ok(())).await.unwrap();
        engine.backend.stage(1);
        assert!(engine
            .apply(selection("broken"), || -> Result<(), String> {
                unreachable!()
            })
            .await
            .is_err());
        assert_eq!(engine.backend.config, 0);
        assert_eq!(
            engine.backend.trace.lock().unwrap().configs.last(),
            Some(&0)
        );

        // A failed save: same.
        engine.backend.fail.clear();
        engine.backend.stage(2);
        assert!(engine
            .apply(selection("new"), || -> Result<(), String> {
                Err("disk unavailable".into())
            })
            .await
            .is_err());
        assert_eq!(engine.backend.config, 0);
        assert_eq!(
            engine.backend.trace.lock().unwrap().configs.last(),
            Some(&0)
        );

        // A success keeps the new setting.
        engine.backend.stage(3);
        engine.apply(selection("new"), || Ok(())).await.unwrap();
        assert_eq!(engine.backend.config, 3);
        assert_eq!(engine.backend.config_before, None);
    }

    #[tokio::test]
    async fn switching_stops_old_model_and_commits_after_prepare() {
        let mut engine = slot(&[]);
        engine.apply(selection("old"), || Ok(())).await.unwrap();
        let trace = engine.backend.trace.clone();
        let previous_generation = engine.generation();
        engine
            .apply(selection("new"), || {
                assert!(trace
                    .lock()
                    .unwrap()
                    .events
                    .last()
                    .unwrap()
                    .starts_with("prepare:new:"));
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(engine.selection().unwrap().preset, "new");
        assert!(engine.generation() > previous_generation);
    }

    #[tokio::test]
    async fn preparation_failure_preserves_persisted_choice_and_restores_old() {
        let mut engine = slot(&["broken"]);
        engine.apply(selection("old"), || Ok(())).await.unwrap();
        let previous_generation = engine.generation();
        let failure = engine
            .apply(selection("broken"), || -> Result<(), String> {
                panic!("failed preparation must not save settings")
            })
            .await
            .unwrap_err();
        assert!(failure.restored);
        assert_eq!(engine.selection().unwrap().preset, "old");
        assert!(engine.generation() > previous_generation);
    }

    #[tokio::test]
    async fn failed_save_rolls_back_loaded_model() {
        let mut engine = slot(&[]);
        engine.apply(selection("old"), || Ok(())).await.unwrap();
        let failure = engine
            .apply(selection("new"), || -> Result<(), String> {
                Err("disk unavailable".into())
            })
            .await
            .unwrap_err();
        assert!(failure.restored);
        assert_eq!(failure.message, "disk unavailable");
        assert_eq!(engine.selection().unwrap().preset, "old");
    }

    #[tokio::test]
    async fn failed_rollback_exposes_unavailable_state() {
        let mut engine = slot(&[]);
        engine.apply(selection("old"), || Ok(())).await.unwrap();
        engine.backend.fail = vec!["old".into(), "new".into()];
        assert!(
            !engine
                .apply(selection("new"), || Ok(()))
                .await
                .unwrap_err()
                .restored
        );
        assert!(engine.handle_mut().is_none());
    }

    #[tokio::test]
    async fn unconfirmed_termination_never_starts_another_model() {
        let mut engine = slot(&[]);
        engine.apply(selection("old"), || Ok(())).await.unwrap();
        engine.backend.stop_failure = true;
        assert!(
            !engine
                .apply(selection("new"), || Ok(()))
                .await
                .unwrap_err()
                .restored
        );
        assert_eq!(
            engine.backend.trace.lock().unwrap().events.last().unwrap(),
            "stop:old"
        );
        let event_count = engine.backend.trace.lock().unwrap().events.len();
        engine.backend.stop_failure = false;
        assert!(engine.apply(selection("new"), || Ok(())).await.is_err());
        assert_eq!(
            engine.backend.trace.lock().unwrap().events.len(),
            event_count
        );
    }
}
