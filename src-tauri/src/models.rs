//! Offline selection from app-managed storage and an optional development fallback.
//! Nothing in this module downloads assets or trusts an install manifest as proof.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
pub struct ModelCatalog {
    pub schema_version: u32,
    pub preset: String,
    pub repo_id: String,
    pub revision: String,
    pub license: String,
    pub files: Vec<ModelFile>,
}

#[derive(Deserialize)]
pub struct ModelFile {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}

/// The files a preset's catalog must list, and its license.
fn catalog_contract(preset: &str) -> (&'static [&'static str], &'static str) {
    match preset {
        // NVIDIA Parakeet as published by sherpa-onnx.
        "parakeet-ja" => (&["model.int8.onnx", "tokens.txt"], "cc-by-4.0"),
        ENGLISH_PRESET => (
            &[
                "encoder.int8.onnx",
                "decoder.int8.onnx",
                "joiner.int8.onnx",
                "tokens.txt",
            ],
            "cc-by-4.0",
        ),
        _ => (
            &[
                "config.json",
                "model.bin",
                "preprocessor_config.json",
                "tokenizer.json",
                "vocabulary.json",
            ],
            "mit",
        ),
    }
}

fn valid_catalog(catalog: &ModelCatalog, preset: &str) -> bool {
    let (required, license) = catalog_contract(preset);
    let hex = |text: &str, size| {
        text.len() == size
            && text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    };
    catalog.schema_version == 1
        && catalog.preset == preset
        && catalog.license == license
        && catalog.repo_id.split('/').count() == 2
        && catalog.repo_id.split('/').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
        })
        && hex(&catalog.revision, 40)
        && catalog.files.len() == required.len()
        && required.iter().all(|name| {
            catalog
                .files
                .iter()
                .filter(|file| file.name == *name)
                .count()
                == 1
        })
        && catalog
            .files
            .iter()
            .all(|file| file.bytes > 0 && hex(&file.sha256, 64))
}

fn parse_catalog(json: &str, preset: &str) -> ModelCatalog {
    let catalog = serde_json::from_str(json).expect("packaged model catalog JSON");
    assert!(
        valid_catalog(&catalog, preset),
        "invalid packaged model catalog"
    );
    catalog
}

pub fn standard_catalog() -> &'static ModelCatalog {
    static CATALOG: std::sync::LazyLock<ModelCatalog> = std::sync::LazyLock::new(|| {
        parse_catalog(
            include_str!("../../worker/voiceweave_worker/model_catalog.json"),
            "standard",
        )
    });
    &CATALOG
}

/// large-v3 (not turbo): more accurate in the 2026-10-06 evaluation, about twice the
/// recognition time. Optional; installed only on request.
pub fn accurate_catalog() -> &'static ModelCatalog {
    static CATALOG: std::sync::LazyLock<ModelCatalog> = std::sync::LazyLock::new(|| {
        parse_catalog(
            include_str!("../../worker/voiceweave_worker/model_catalog_accurate.json"),
            "accurate",
        )
    });
    &CATALOG
}

/// Parakeet Japanese (a main recognizer on trial) and English (the English shortcut when
/// the main recognizer is Parakeet). Optional; installed from the settings screen.
fn parakeet_catalog(preset: &str) -> &'static ModelCatalog {
    static JA: std::sync::LazyLock<ModelCatalog> = std::sync::LazyLock::new(|| {
        parse_catalog(
            include_str!("../../worker/voiceweave_worker/model_catalog_parakeet_ja.json"),
            "parakeet-ja",
        )
    });
    static EN: std::sync::LazyLock<ModelCatalog> = std::sync::LazyLock::new(|| {
        parse_catalog(
            include_str!("../../worker/voiceweave_worker/model_catalog_parakeet_en.json"),
            ENGLISH_PRESET,
        )
    });
    if preset == ENGLISH_PRESET {
        &EN
    } else {
        &JA
    }
}

/// The pinned catalog for a preset that has one.
pub fn catalog(preset: &str) -> Option<&'static ModelCatalog> {
    match preset {
        "standard" => Some(standard_catalog()),
        "accurate" => Some(accurate_catalog()),
        "parakeet-ja" | ENGLISH_PRESET => Some(parakeet_catalog(preset)),
        _ => None,
    }
}

/// Presets the settings screen lists, in display order.
pub const PRESETS: [&str; 4] = ["standard", "accurate", "dev-tiny", "parakeet-ja"];

/// Candidates on trial (development only): listed once their folder is in place, each
/// in a folder of its own name under the model root.
pub const TRIAL_PRESETS: [&str; 1] = ["parakeet-ja"];

/// The English recognizer (Parakeet English, a transducer), loaded next to the main one
/// when the English shortcut is on. Not a preset: it is never the main recognizer.
pub const ENGLISH_PRESET: &str = "parakeet-en";

/// The recognizer a preset runs on: Whisper (faster-whisper), or Parakeet on sherpa-onnx
/// (worker `sherpa_engine.py`, CPU only).
pub fn engine(preset: &str) -> &'static str {
    match preset {
        "parakeet-ja" => "nemo_ctc",
        ENGLISH_PRESET => "nemo_transducer",
        _ => "whisper",
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelSelection {
    pub preset: String,
    pub directory: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelError {
    Unsupported,
    Missing,
    Invalid,
    Io,
}

impl ModelError {
    pub fn message(self) -> &'static str {
        match self {
            Self::Unsupported => "この認識プリセットはまだ利用できません",
            Self::Missing => "選択した認識モデルが未導入です",
            Self::Invalid => "認識モデルのファイル検査に失敗しました",
            Self::Io => "認識モデルのファイルを読み取れませんでした",
        }
    }
}

#[derive(Serialize)]
pub struct ModelAvailability {
    pub preset: &'static str,
    pub files_present: bool,
    pub location: Option<&'static str>,
}

pub struct ModelRoots {
    pub managed: PathBuf,
    /// Release callers must leave this empty. Never migrate these files implicitly.
    pub development: Option<PathBuf>,
}

impl ModelRoots {
    pub fn resolve(&self, preset: &str) -> Result<ModelSelection, ModelError> {
        let managed = select(&self.managed, preset)?;
        reject_links(&managed.directory)?;
        match fs::symlink_metadata(&managed.directory) {
            Ok(_) => return Ok(managed),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(ModelError::Io),
        }
        if let Some(root) = &self.development {
            return select(root, preset);
        }
        Ok(managed)
    }

    /// The presets, then the English recognizer (not a preset: listed for its install state).
    pub fn inventory(&self) -> Vec<ModelAvailability> {
        PRESETS
            .into_iter()
            .chain([ENGLISH_PRESET])
            .map(|preset| {
                let selection = self.resolve(preset);
                let location = selection.as_ref().ok().map(|selection| {
                    if selection.directory.starts_with(&self.managed) {
                        "managed"
                    } else {
                        "development"
                    }
                });
                ModelAvailability {
                    preset,
                    files_present: selection
                        .and_then(|selection| check_required_files(&selection))
                        .is_ok(),
                    location,
                }
            })
            .collect()
    }
}

fn reject_links(path: &Path) -> Result<(), ModelError> {
    if !path.is_absolute() {
        return Err(ModelError::Invalid);
    }
    for component in path.ancestors() {
        let metadata = match fs::symlink_metadata(component) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(ModelError::Io),
        };
        if metadata.file_type().is_symlink() {
            return Err(ModelError::Invalid);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes()
                & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
                != 0
            {
                return Err(ModelError::Invalid);
            }
        }
    }
    Ok(())
}

pub fn select(root: &Path, preset: &str) -> Result<ModelSelection, ModelError> {
    let directory = match preset {
        "standard" | "accurate" => root
            .join(preset)
            .join(&catalog(preset).expect("pinned preset").revision),
        "dev-tiny" => root.join("dev-tiny"),
        // Installed from the settings screen: <preset>/<revision>, like the presets above.
        // Copied by hand (the copy script, the development folder): the files directly in
        // <preset>.
        "parakeet-ja" | ENGLISH_PRESET => {
            let folder = root.join(preset);
            let pinned = folder.join(&catalog(preset).expect("pinned preset").revision);
            if pinned.is_dir() {
                pinned
            } else {
                folder
            }
        }
        _ => return Err(ModelError::Unsupported),
    };
    Ok(ModelSelection {
        preset: preset.to_owned(),
        directory,
    })
}

pub fn inventory(root: &Path) -> Vec<ModelAvailability> {
    PRESETS
        .into_iter()
        .chain([ENGLISH_PRESET])
        .map(|preset| ModelAvailability {
            preset,
            location: None,
            files_present: select(root, preset)
                .and_then(|selection| check_required_files(&selection))
                .is_ok(),
        })
        .collect()
}

fn regular_file(path: &Path) -> Result<u64, ModelError> {
    reject_links(path)?;
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ModelError::Missing
        } else {
            ModelError::Io
        }
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() == 0 {
        return Err(ModelError::Invalid);
    }
    Ok(metadata.len())
}

/// The model's files are in place (no hashing: for the quick checks; `verify` before use).
pub fn installed(selection: &ModelSelection) -> bool {
    check_required_files(selection).is_ok()
}

fn check_required_files(selection: &ModelSelection) -> Result<(), ModelError> {
    reject_links(&selection.directory)?;
    if !selection.directory.is_absolute() {
        return Err(ModelError::Invalid);
    }
    let metadata = fs::symlink_metadata(&selection.directory).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ModelError::Missing
        } else {
            ModelError::Io
        }
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ModelError::Invalid);
    }
    if let Some(catalog) = catalog(&selection.preset) {
        for file in &catalog.files {
            regular_file(&selection.directory.join(&file.name))?;
        }
        return Ok(());
    }
    for name in ["config.json", "model.bin", "tokenizer.json"] {
        regular_file(&selection.directory.join(name))?;
    }
    if !selection.directory.join("vocabulary.json").exists() {
        regular_file(&selection.directory.join("vocabulary.txt"))?;
    } else {
        regular_file(&selection.directory.join("vocabulary.json"))?;
    }
    Ok(())
}

fn verify_files(directory: &Path, files: &[(&str, u64, &str)]) -> Result<(), ModelError> {
    let mut buffer = [0_u8; 64 * 1024];
    for &(name, bytes, expected_hash) in files {
        let path = directory.join(name);
        if regular_file(&path)? != bytes {
            return Err(ModelError::Invalid);
        }
        let mut file = File::open(path).map_err(|_| ModelError::Io)?;
        let mut digest = Sha256::new();
        loop {
            let count = file.read(&mut buffer).map_err(|_| ModelError::Io)?;
            if count == 0 {
                break;
            }
            digest.update(&buffer[..count]);
        }
        if format!("{:x}", digest.finalize()) != expected_hash {
            return Err(ModelError::Invalid);
        }
    }
    Ok(())
}

pub fn verify(selection: &ModelSelection) -> Result<(), ModelError> {
    if !matches!(
        selection.preset.as_str(),
        "standard" | "accurate" | "dev-tiny" | "external"
    ) && !TRIAL_PRESETS.contains(&selection.preset.as_str())
        && selection.preset != ENGLISH_PRESET
    {
        return Err(ModelError::Unsupported);
    }
    check_required_files(selection)?;
    if let Some(catalog) = catalog(&selection.preset) {
        let files: Vec<_> = catalog
            .files
            .iter()
            .map(|file| (file.name.as_str(), file.bytes, file.sha256.as_str()))
            .collect();
        verify_files(&selection.directory, &files)?;
    }
    // tiny and explicit development overrides have no approved hash catalog yet.
    // File presence is not a claim that these weights have a verified provenance.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packaged_catalog_rejects_unsafe_metadata() {
        let source = include_str!("../../worker/voiceweave_worker/model_catalog.json");
        assert!(valid_catalog(standard_catalog(), "standard"));
        assert!(valid_catalog(accurate_catalog(), "accurate"));
        // A catalog only counts for the preset it names.
        assert!(!valid_catalog(accurate_catalog(), "standard"));
        for mutation in ["schema", "revision", "hash", "path", "duplicate", "zero"] {
            let mut catalog: ModelCatalog = serde_json::from_str(source).unwrap();
            match mutation {
                "schema" => catalog.schema_version = 2,
                "revision" => catalog.revision = "main".into(),
                "hash" => catalog.files[0].sha256 = "unverified".into(),
                "path" => catalog.files[0].name = "../config.json".into(),
                "duplicate" => catalog.files[1].name = catalog.files[0].name.clone(),
                "zero" => catalog.files[0].bytes = 0,
                _ => unreachable!(),
            }
            assert!(!valid_catalog(&catalog, "standard"), "{mutation}");
        }
    }

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let directory =
                std::env::temp_dir().join(format!("voiceweave-model-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&directory).unwrap();
            Self(directory)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            assert_eq!(
                self.0.canonicalize().unwrap().parent().unwrap(),
                std::env::temp_dir().canonicalize().unwrap()
            );
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn selection_is_pinned_and_unknown_presets_cannot_be_paths() {
        let root = Fixture::new();
        assert_eq!(
            select(&root.0, "standard").unwrap().directory,
            root.0.join("standard").join(&standard_catalog().revision)
        );
        assert_eq!(select(&root.0, "../dev-tiny"), Err(ModelError::Unsupported));
    }

    #[test]
    fn missing_or_partial_models_are_not_available() {
        let root = Fixture::new();
        assert!(inventory(&root.0).iter().all(|model| !model.files_present));
        let model = select(&root.0, "dev-tiny").unwrap();
        fs::create_dir(&model.directory).unwrap();
        fs::write(model.directory.join("model.bin"), b"not enough").unwrap();
        assert_eq!(verify(&model), Err(ModelError::Missing));
    }

    #[test]
    fn same_size_corruption_fails_hash_validation() {
        let root = Fixture::new();
        let digest = format!("{:x}", Sha256::digest(b"correct"));
        let files = [("model.bin", 7, digest.as_str())];
        fs::write(root.0.join("model.bin"), b"correct").unwrap();
        assert_eq!(verify_files(&root.0, &files), Ok(()));
        fs::write(root.0.join("model.bin"), b"corrupt").unwrap();
        assert_eq!(verify_files(&root.0, &files), Err(ModelError::Invalid));
    }

    #[test]
    fn empty_assets_and_directory_instead_of_file_are_rejected() {
        let root = Fixture::new();
        fs::write(root.0.join("empty"), b"").unwrap();
        assert_eq!(
            regular_file(&root.0.join("empty")),
            Err(ModelError::Invalid)
        );
        assert_eq!(regular_file(&root.0), Err(ModelError::Invalid));
    }

    #[test]
    fn managed_model_takes_precedence_and_damage_never_falls_back() {
        let root = Fixture::new();
        let roots = ModelRoots {
            managed: root.0.join("managed"),
            development: Some(root.0.join("development")),
        };
        let dev = select(roots.development.as_ref().unwrap(), "dev-tiny").unwrap();
        fs::create_dir_all(&dev.directory).unwrap();
        for name in [
            "config.json",
            "model.bin",
            "tokenizer.json",
            "vocabulary.json",
        ] {
            fs::write(dev.directory.join(name), b"synthetic").unwrap();
        }
        assert_eq!(roots.resolve("dev-tiny"), Ok(dev));
        assert!(roots.inventory()[2].files_present);
        assert_eq!(roots.inventory()[2].location, Some("development"));
        let managed = select(&roots.managed, "dev-tiny").unwrap();
        fs::create_dir_all(&managed.directory).unwrap();
        assert_eq!(roots.resolve("dev-tiny"), Ok(managed.clone()));
        assert_eq!(verify(&managed), Err(ModelError::Missing));
        assert!(!roots.inventory()[2].files_present);
        assert_eq!(roots.inventory()[2].location, Some("managed"));
    }

    #[test]
    fn unpublished_staging_is_ignored_and_resolution_creates_nothing() {
        let root = Fixture::new();
        let roots = ModelRoots {
            managed: root.0.join("managed"),
            development: None,
        };
        let selected = roots.resolve("standard").unwrap();
        assert!(!roots.managed.exists());
        let staging = roots.managed.join("standard/.install-unpublished");
        fs::create_dir_all(staging).unwrap();
        assert_eq!(roots.resolve("standard"), Ok(selected));
        assert!(!roots.inventory()[0].files_present);
        assert_eq!(roots.resolve("unknown"), Err(ModelError::Unsupported));
    }

    #[test]
    fn invalid_managed_destination_blocks_development_fallback() {
        let root = Fixture::new();
        let roots = ModelRoots {
            managed: root.0.join("managed"),
            development: Some(root.0.join("development")),
        };
        let managed = select(&roots.managed, "standard").unwrap();
        fs::create_dir_all(managed.directory.parent().unwrap()).unwrap();
        fs::write(&managed.directory, b"not a model directory").unwrap();
        assert_eq!(roots.resolve("standard"), Ok(managed.clone()));
        assert_eq!(verify(&managed), Err(ModelError::Invalid));
        assert!(!roots.inventory()[0].files_present);
    }

    #[cfg(windows)]
    #[test]
    fn managed_junction_is_rejected_instead_of_falling_back() {
        let root = Fixture::new();
        let target = root.0.join("target");
        fs::create_dir(&target).unwrap();
        let junction = root.0.join("managed");
        let result = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&target)
            .output()
            .unwrap();
        assert!(result.status.success());
        let roots = ModelRoots {
            managed: junction.clone(),
            development: Some(root.0.join("development")),
        };
        let result = roots.resolve("standard");
        fs::remove_dir(junction).unwrap(); // Only the junction, before fixture cleanup.
        assert_eq!(result, Err(ModelError::Invalid));
        assert!(target.exists());
    }

    #[test]
    fn sherpa_presets_use_their_own_engine_and_files() {
        assert_eq!(engine("standard"), "whisper");
        assert_eq!(engine("parakeet-ja"), "nemo_ctc");
        assert_eq!(engine(ENGLISH_PRESET), "nemo_transducer");
        let root = Fixture::new();
        let model = select(&root.0, "parakeet-ja").unwrap();
        assert_eq!(model.directory, root.0.join("parakeet-ja"));
        fs::create_dir_all(&model.directory).unwrap();
        assert!(!installed(&model));
        fs::write(model.directory.join("model.int8.onnx"), b"onnx").unwrap();
        fs::write(model.directory.join("tokens.txt"), b"tokens").unwrap();
        // In place, but not the pinned release: usable only after the hashes match.
        assert!(installed(&model));
        assert_eq!(verify(&model), Err(ModelError::Invalid));
    }

    #[test]
    fn parakeet_installed_from_the_settings_screen_is_preferred_to_a_copied_one() {
        let root = Fixture::new();
        let revision = &catalog("parakeet-ja").unwrap().revision;
        assert_eq!(
            select(&root.0, "parakeet-ja").unwrap().directory,
            root.0.join("parakeet-ja")
        );
        fs::create_dir_all(root.0.join("parakeet-ja").join(revision)).unwrap();
        assert_eq!(
            select(&root.0, "parakeet-ja").unwrap().directory,
            root.0.join("parakeet-ja").join(revision)
        );
        assert_eq!(catalog(ENGLISH_PRESET).unwrap().license, "cc-by-4.0");
        assert!(inventory(&root.0)
            .iter()
            .any(|model| model.preset == ENGLISH_PRESET));
    }
}
