//! Optional GPU runtime: where it lives and whether it is complete. Installation runs
//! the worker's verified installer through `install`; the recognition worker itself
//! never downloads.
use crate::install::{self, Installers, Which};
use serde::Serialize;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

pub const LIBRARIES: [&str; 2] = ["cublas64_12.dll", "cublasLt64_12.dll"];

/// The managed location the engine reads: `<app local data>/cuda`.
pub fn managed_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_local_data_dir().ok().map(|d| d.join("cuda"))
}

pub fn is_complete(directory: &Path) -> bool {
    LIBRARIES.iter().all(|name| directory.join(name).is_file())
}

#[derive(Clone, Serialize)]
pub struct CudaStatus {
    pub installed: bool,
    pub installing: bool,
    pub supported: bool,
}

pub fn status(app: &AppHandle) -> CudaStatus {
    CudaStatus {
        installed: managed_dir(app).is_some_and(|dir| is_complete(&dir)),
        installing: app.state::<Installers>().cuda.installing(),
        supported: true,
    }
}

/// Download and verify the pinned runtime into `<app local data>/cuda`, then reload
/// the engine so it can move to the GPU.
pub fn start_install(app: &AppHandle) -> Result<(), String> {
    let root = app
        .path()
        .app_local_data_dir()
        .map_err(|_| "保存先を確認できませんでした".to_owned())?;
    let mut arguments: Vec<OsString> = vec!["--root".into(), root.into()];
    // Development only: reuse an already downloaded wheel (it is still hash-checked).
    if cfg!(debug_assertions) {
        if let Some(wheel) = std::env::var_os("VOICEWEAVE_CUDA_WHEEL") {
            arguments.extend(["--wheel".into(), wheel]);
        }
    }
    install::start(app, Which::Cuda, arguments, crate::reload_engine)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completeness_needs_both_libraries() {
        let dir = std::env::temp_dir().join(format!("voiceweave-cuda-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        assert!(!is_complete(&dir));
        std::fs::write(dir.join(LIBRARIES[0]), b"").unwrap();
        assert!(!is_complete(&dir));
        std::fs::write(dir.join(LIBRARIES[1]), b"").unwrap();
        assert!(is_complete(&dir));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
