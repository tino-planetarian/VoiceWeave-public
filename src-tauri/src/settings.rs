use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const CURRENT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Settings {
    pub schema_version: u32,
    pub revision: u64,
    pub shortcuts: Shortcuts,
    pub recognition: RecognitionSettings,
    pub input_device: InputDevice,
    pub autostart: bool,
    pub dictionary: DictionarySettings,
    /// Remove standalone hesitations ("えーと、"). On by default since the 2026-10-04
    /// joint test on real speech; files written before the option read as on.
    #[serde(default = "enabled")]
    pub remove_fillers: bool,
    /// Kanji numbers with a unit written as digits ("三十分" → "30分"). On for a trial
    /// (2026-11-03); files written before the option read as on.
    #[serde(default = "enabled")]
    pub tidy_numbers: bool,
    /// A restatement within one utterance keeps the corrected item ("3時、いや4時" → "4時").
    #[serde(default = "enabled")]
    pub fix_restatements: bool,
    /// Email-style drafts from a local model (design: docs/POLISH_MODE_DESIGN.md).
    /// Off until the user turns it on; older files read as off.
    #[serde(default)]
    pub polish: PolishSettings,
    /// How 、/。 are added where fast speech left none. Default: the rules, as before.
    #[serde(default)]
    pub punctuation: PunctuationSettings,
    /// Recognize while the user speaks, so the wait after stop does not grow with the
    /// length of the recording (docs/STREAMING_DESIGN.md). On unless turned off.
    #[serde(default = "enabled")]
    pub streaming: bool,
    /// Split long transcripts into paragraphs before topic words (rules; only blank lines
    /// are added). On unless turned off.
    #[serde(default = "enabled")]
    pub paragraphs: bool,
    /// Read uncommon words from the window being dictated into, as hints for the
    /// recognizer and the drafting model. Off unless turned on.
    #[serde(default)]
    pub screen_context: bool,
    /// Show the parts recognized while speaking in the recording indicator. Off unless
    /// turned on; needs `streaming`.
    #[serde(default)]
    pub live_preview: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PunctuationSettings {
    /// "rules" (built-in, default) or "local_ai" (Ollama; marks only, checked).
    pub mode: String,
    pub model: String,
    /// While the PC is short of memory, punctuate with the rules and take the model off
    /// (src/memory_gate.rs). Files written before the option read as on.
    #[serde(default = "enabled")]
    pub auto_rules_when_low_memory: bool,
}

impl Default for PunctuationSettings {
    fn default() -> Self {
        Self {
            mode: "rules".into(),
            // Best balance with the recognition model resident (2026-10-06): 0.87 s, F1 0.75.
            model: "gemma3:4b".into(),
            auto_rules_when_low_memory: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PolishSettings {
    pub enabled: bool,
    pub shortcut: String,
    pub model: String,
    /// Loopback only; checked again before every connection.
    pub endpoint: String,
    /// Paste the faithful transcript below the draft.
    pub append_original: bool,
    /// Which model writes the draft: "local" (Ollama on this PC), "openai", "anthropic",
    /// or "compare" (both cloud services at once, both drafts pasted, for testing).
    #[serde(default = "local_provider")]
    pub provider: String,
    #[serde(default = "default_openai_model")]
    pub openai_model: String,
    #[serde(default = "default_anthropic_model")]
    pub anthropic_model: String,
    /// What the speaker usually talks about, given to the drafting model so that terms of
    /// their field are written correctly (for example 衝 in astronomy).
    #[serde(default = "default_profile")]
    pub profile: String,
    /// When the chosen cloud service is slow, ask the other one (if its key is saved) and
    /// use whichever answers first.
    #[serde(default = "enabled")]
    pub fallback: bool,
    /// Compose mode: write a text from a spoken request (what to say, in any order),
    /// with the same drafting service. Held only while polishing is on too.
    #[serde(default = "enabled")]
    pub compose_enabled: bool,
    #[serde(default = "default_compose_shortcut")]
    pub compose_shortcut: String,
    /// Use text the user copied (an email to answer) as reference in compose mode.
    #[serde(default = "enabled")]
    pub compose_clipboard: bool,
}

pub const DEFAULT_COMPOSE_SHORTCUT: &str = "Shift+F9";

fn default_compose_shortcut() -> String {
    DEFAULT_COMPOSE_SHORTCUT.into()
}

fn default_profile() -> String {
    String::new()
}

fn local_provider() -> String {
    "local".into()
}

fn default_openai_model() -> String {
    "gpt-6-luna".into()
}

fn default_anthropic_model() -> String {
    "claude-haiku-5-5".into()
}

pub const DEFAULT_POLISH_SHORTCUT: &str = "F9";

impl Default for PolishSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            shortcut: DEFAULT_POLISH_SHORTCUT.into(),
            model: crate::polish::DEFAULT_MODEL.into(),
            endpoint: crate::polish::DEFAULT_ENDPOINT.into(),
            append_original: true,
            provider: local_provider(),
            openai_model: default_openai_model(),
            anthropic_model: default_anthropic_model(),
            profile: default_profile(),
            fallback: true,
            compose_enabled: true,
            compose_shortcut: default_compose_shortcut(),
            compose_clipboard: true,
        }
    }
}

fn enabled() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Shortcuts {
    pub record: String,
    pub repaste: String,
    /// Record in English (Parakeet English on this PC, English on Groq in the cloud mode).
    /// Off unless turned on: the English model uses about 1 GB of memory while loaded.
    #[serde(default)]
    pub english_enabled: bool,
    #[serde(default = "default_english_shortcut")]
    pub english: String,
}

fn default_english_shortcut() -> String {
    crate::shortcut::DEFAULT_ENGLISH.into()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecognitionSettings {
    pub preset: String,
    /// Where speech is recognized: "local" (this PC, the default; nothing leaves it) or
    /// "cloud" (the recording is sent to Groq; for PCs without a usable GPU). A main
    /// switch chosen once per PC, not per recording.
    #[serde(default = "local_mode")]
    pub mode: String,
    /// Cloud recognition only: also send the dictionary's words and the registered words
    /// found on screen to Groq as hints (they can include names that were not spoken).
    /// Off unless the user turns it on; files written before the option read as off.
    #[serde(default)]
    pub cloud_hints: bool,
}

fn local_mode() -> String {
    "local".into()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum InputDevice {
    #[serde(rename = "system_default")]
    SystemDefault,
    #[serde(rename = "endpoint")]
    Endpoint { id: String, display_name: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DictionarySettings {
    pub entries: Vec<DictionaryEntry>,
    /// Terms of the user's fields (astronomy, planetariums …), one text the user edits.
    #[serde(default = "crate::glossary::default_text")]
    pub glossary: String,
}

impl Default for DictionarySettings {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            glossary: crate::glossary::default_text(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DictionaryEntry {
    pub id: String,
    pub term: String,
    pub aliases: Vec<String>,
    pub enabled: bool,
    /// When this term is the right one (for example "惑星が太陽と反対側に来る天文現象"),
    /// for the drafting model; empty when not needed.
    #[serde(default)]
    pub note: String,
    /// Shown to the recognizer as a hint before it listens. Off for terms it would
    /// otherwise write where they were never said (an unusual title) or confuse with a
    /// common word ("Claude" for "クラウド"); aliases and the note still apply.
    #[serde(default = "enabled")]
    pub hint: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LoadResult {
    Defaults(Settings),
    Loaded(Settings),
    RecoveredDefaults { settings: Settings, backup: PathBuf },
}

impl LoadResult {
    pub fn into_settings(self) -> Settings {
        match self {
            Self::Defaults(settings)
            | Self::Loaded(settings)
            | Self::RecoveredDefaults { settings, .. } => settings,
        }
    }
}

/// Serialize in-process file access, including concurrent Tauri commands.
pub struct SettingsStore {
    path: PathBuf,
    access: std::sync::Mutex<()>,
}

impl SettingsStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            access: std::sync::Mutex::new(()),
        }
    }

    pub fn load(&self) -> Result<LoadResult, SettingsError> {
        let _lock = self
            .access
            .lock()
            .map_err(|_| SettingsError::Io("settings lock failed".into()))?;
        load(&self.path)
    }

    pub fn save(
        &self,
        settings: &Settings,
        expected_revision: u64,
    ) -> Result<Settings, SettingsError> {
        let _lock = self
            .access
            .lock()
            .map_err(|_| SettingsError::Io("settings lock failed".into()))?;
        save(&self.path, settings, expected_revision)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum SettingsError {
    Io(String),
    Invalid(String),
    UnsupportedSchema(u64),
    RevisionConflict { expected: u64, actual: u64 },
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(message) | Self::Invalid(message) => formatter.write_str(message),
            Self::UnsupportedSchema(version) => {
                write!(formatter, "unsupported settings schema: {version}")
            }
            Self::RevisionConflict { expected, actual } => {
                write!(
                    formatter,
                    "settings changed (expected revision {expected}, actual {actual})"
                )
            }
        }
    }
}

impl std::error::Error for SettingsError {}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            revision: 0,
            shortcuts: Shortcuts {
                record: "F8".into(),
                repaste: "Ctrl+Alt+V".into(),
                english_enabled: false,
                english: default_english_shortcut(),
            },
            recognition: RecognitionSettings {
                preset: "standard".into(),
                mode: local_mode(),
                cloud_hints: false,
            },
            input_device: InputDevice::SystemDefault,
            autostart: false,
            dictionary: DictionarySettings::default(),
            remove_fillers: true,
            tidy_numbers: true,
            fix_restatements: true,
            polish: PolishSettings::default(),
            punctuation: PunctuationSettings::default(),
            streaming: true,
            paragraphs: true,
            screen_context: false,
            live_preview: false,
        }
    }
}

impl Settings {
    pub fn validate(&self) -> Result<(), SettingsError> {
        if self.schema_version != CURRENT_SCHEMA_VERSION {
            return Err(SettingsError::UnsupportedSchema(self.schema_version.into()));
        }
        if self.shortcuts.record.trim().is_empty() || self.shortcuts.repaste.trim().is_empty() {
            return Err(SettingsError::Invalid("shortcut cannot be empty".into()));
        }
        if self.shortcuts.english_enabled && self.shortcuts.english.trim().is_empty() {
            return Err(SettingsError::Invalid(
                "英語のショートカットが空です".into(),
            ));
        }
        if !matches!(self.recognition.mode.as_str(), "local" | "cloud") {
            return Err(SettingsError::Invalid(
                "認識の方式の設定が正しくありません".into(),
            ));
        }
        if self.recognition.preset.trim().is_empty() {
            return Err(SettingsError::Invalid(
                "recognition preset cannot be empty".into(),
            ));
        }
        if let InputDevice::Endpoint { id, display_name } = &self.input_device {
            let printable = |text: &str| !text.chars().any(char::is_control);
            if id.trim().is_empty()
                || id.chars().count() > 512
                || display_name.chars().count() > 256
                || !printable(id)
                || !printable(display_name)
            {
                return Err(SettingsError::Invalid(
                    "マイクの指定が正しくありません".into(),
                ));
            }
        }
        if self.dictionary.entries.len() > 200 {
            return Err(SettingsError::Invalid("辞書の登録は200件までです".into()));
        }
        for entry in &self.dictionary.entries {
            if entry.id.trim().is_empty()
                || entry.term.is_empty()
                || entry.term.chars().count() > 100
            {
                return Err(SettingsError::Invalid(
                    "辞書の語が空か、100文字を超えています".into(),
                ));
            }
            if entry
                .aliases
                .iter()
                .any(|alias| alias.is_empty() || alias.chars().count() > 100)
            {
                return Err(SettingsError::Invalid(
                    "辞書の誤認識の書き方が空か、100文字を超えています".into(),
                ));
            }
        }
        let polish = &self.polish;
        if polish.model.trim().is_empty()
            || polish.model.chars().count() > 100
            || polish.model.chars().any(char::is_control)
        {
            return Err(SettingsError::Invalid(
                "整形に使うモデル名が空か、100文字を超えています".into(),
            ));
        }
        let model_name_ok = |name: &str| {
            !name.trim().is_empty()
                && name.chars().count() <= 100
                && !name.chars().any(char::is_control)
        };
        if polish.profile.chars().count() > 300
            || polish.profile.chars().any(|c| c.is_control() && c != '\n')
        {
            return Err(SettingsError::Invalid(
                "整形のAIに伝える「話し手の分野」は300文字までです".into(),
            ));
        }
        if self.dictionary.glossary.chars().count() > crate::glossary::MAX_CHARS {
            return Err(SettingsError::Invalid(
                "分野の用語集は4000文字までです".into(),
            ));
        }
        if self.dictionary.entries.iter().any(|entry| {
            entry.note.chars().count() > 100 || entry.note.chars().any(char::is_control)
        }) {
            return Err(SettingsError::Invalid(
                "辞書の「使う場面」は100文字までです".into(),
            ));
        }
        if !matches!(
            polish.provider.as_str(),
            "local" | "openai" | "anthropic" | "compare"
        ) || !model_name_ok(&polish.openai_model)
            || !model_name_ok(&polish.anthropic_model)
        {
            return Err(SettingsError::Invalid(
                "整形に使うAIの設定が正しくありません".into(),
            ));
        }
        if crate::polish::loopback_endpoint(&polish.endpoint)
            .is_err_and(|error| error == crate::polish::PolishError::NotLocal)
        {
            return Err(SettingsError::Invalid(
                crate::polish::PolishError::NotLocal.status().into(),
            ));
        }
        if !matches!(self.punctuation.mode.as_str(), "rules" | "local_ai")
            || self.punctuation.model.trim().is_empty()
            || self.punctuation.model.chars().count() > 100
            || self.punctuation.model.chars().any(char::is_control)
        {
            return Err(SettingsError::Invalid(
                "句読点の設定が正しくありません".into(),
            ));
        }
        if polish.enabled && polish.shortcut.trim().is_empty() {
            return Err(SettingsError::Invalid(
                "整形のショートカットが空です".into(),
            ));
        }
        if polish.enabled && polish.compose_enabled && polish.compose_shortcut.trim().is_empty() {
            return Err(SettingsError::Invalid(
                "作文のショートカットが空です".into(),
            ));
        }
        if let Some(alias) = crate::dictionary::conflicting_alias(&self.dictionary.entries) {
            return Err(SettingsError::Invalid(format!(
                "「{alias}」が別々の語の別表記として登録されています"
            )));
        }
        Ok(())
    }
}

fn read_settings(path: &Path) -> Result<Settings, SettingsError> {
    let bytes = fs::read(path).map_err(|error| SettingsError::Io(error.to_string()))?;
    let document: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| SettingsError::Invalid(error.to_string()))?;
    // A future schema can have entirely different fields. Check the version
    // before deserializing today's shape, so that it is never called corrupt.
    if let Some(version) = document
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
    {
        if version != u64::from(CURRENT_SCHEMA_VERSION) {
            return Err(SettingsError::UnsupportedSchema(version));
        }
    }
    let settings: Settings = serde_json::from_value(document)
        .map_err(|error| SettingsError::Invalid(error.to_string()))?;
    settings.validate()?;
    Ok(settings)
}

pub fn load(path: &Path) -> Result<LoadResult, SettingsError> {
    if !path
        .try_exists()
        .map_err(|error| SettingsError::Io(error.to_string()))?
    {
        return Ok(LoadResult::Defaults(Settings::default()));
    }
    match read_settings(path) {
        Ok(settings) => Ok(LoadResult::Loaded(settings)),
        Err(SettingsError::Invalid(_)) => {
            let backup = corrupted_backup_path(path);
            fs::rename(path, &backup).map_err(|error| SettingsError::Io(error.to_string()))?;
            Ok(LoadResult::RecoveredDefaults {
                settings: Settings::default(),
                backup,
            })
        }
        Err(error) => Err(error),
    }
}

pub fn save(
    path: &Path,
    settings: &Settings,
    expected_revision: u64,
) -> Result<Settings, SettingsError> {
    settings.validate()?;
    let actual_revision = if path
        .try_exists()
        .map_err(|error| SettingsError::Io(error.to_string()))?
    {
        read_settings(path)?.revision
    } else {
        0
    };
    if actual_revision != expected_revision {
        return Err(SettingsError::RevisionConflict {
            expected: expected_revision,
            actual: actual_revision,
        });
    }
    let mut next = settings.clone();
    next.schema_version = CURRENT_SCHEMA_VERSION;
    next.revision = actual_revision.saturating_add(1);
    let parent = path
        .parent()
        .ok_or_else(|| SettingsError::Io("settings path has no parent".into()))?;
    fs::create_dir_all(parent).map_err(|error| SettingsError::Io(error.to_string()))?;
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(&next)
        .map_err(|error| SettingsError::Invalid(error.to_string()))?;
    fs::write(&temporary, bytes).map_err(|error| SettingsError::Io(error.to_string()))?;
    replace_file(&temporary, path).map_err(|error| SettingsError::Io(error.to_string()))?;
    Ok(next)
}

fn corrupted_backup_path(path: &Path) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    path.with_extension(format!("corrupt-{stamp}.json"))
}

fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        };
        let source: Vec<u16> = source
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let destination: Vec<u16> = destination
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let result = unsafe {
            MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if result == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        fs::rename(source, destination)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("voiceweave-settings-{name}-{}", std::process::id()))
    }

    #[test]
    fn files_written_before_the_filler_option_still_load_with_it_on() {
        let old = r#"{"schema_version":1,"revision":3,"shortcuts":{"record":"F8","repaste":"Ctrl+Alt+V"},"recognition":{"preset":"standard"},"input_device":{"kind":"system_default"},"autostart":false,"dictionary":{"entries":[]}}"#;
        let settings: Settings = serde_json::from_str(old).expect("old file parses");
        assert!(settings.remove_fillers);
        assert!(settings.validate().is_ok());
        assert!(Settings::default().remove_fillers);
        assert!(settings.tidy_numbers && settings.fix_restatements);
    }

    #[test]
    fn a_selected_microphone_must_have_a_usable_id_and_name() {
        let with = |id: &str, display_name: &str| Settings {
            input_device: InputDevice::Endpoint {
                id: id.into(),
                display_name: display_name.into(),
            },
            ..Settings::default()
        };
        assert!(with("wasapi:{0.0.1.00000000}.{abc}", "マイク (MU900)")
            .validate()
            .is_ok());
        for invalid in [
            with("", "name"),
            with("   ", "name"),
            with(&"x".repeat(513), "name"),
            with("id", &"n".repeat(257)),
            with("id\u{0}", "name"),
            with("id", "line\nbreak"),
        ] {
            assert!(invalid.validate().is_err());
        }
    }

    #[test]
    fn missing_file_returns_defaults_without_writing() {
        let path = temp_path("missing.json");
        let _ = fs::remove_file(&path);
        assert_eq!(load(&path), Ok(LoadResult::Defaults(Settings::default())));
    }

    #[test]
    fn concurrent_saves_cannot_both_commit_the_same_revision() {
        let path =
            std::env::temp_dir().join(format!("voiceweave-settings-{}.json", uuid::Uuid::new_v4()));
        let store = std::sync::Arc::new(SettingsStore::new(path.clone()));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let store = store.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    store.save(&Settings::default(), 0)
                })
            })
            .collect();
        let results: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Err(SettingsError::RevisionConflict { .. })))
                .count(),
            1
        );
        assert_eq!(store.load().unwrap().into_settings().revision, 1);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn save_increments_revision_and_round_trips() {
        let path = temp_path("roundtrip.json");
        let _ = fs::remove_file(&path);
        let saved = save(&path, &Settings::default(), 0).expect("save");
        assert_eq!(saved.revision, 1);
        assert_eq!(load(&path), Ok(LoadResult::Loaded(saved)));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn stale_revision_is_rejected_without_overwriting() {
        let path = temp_path("revision.json");
        let _ = fs::remove_file(&path);
        save(&path, &Settings::default(), 0).expect("initial save");
        assert_eq!(
            save(&path, &Settings::default(), 0),
            Err(SettingsError::RevisionConflict {
                expected: 0,
                actual: 1
            })
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn corrupt_file_is_moved_before_defaults_are_returned() {
        let path = temp_path("corrupt.json");
        let _ = fs::remove_file(&path);
        fs::write(&path, b"not-json").expect("write corrupt settings");
        let LoadResult::RecoveredDefaults { backup, .. } = load(&path).expect("recover") else {
            panic!("expected recovery");
        };
        assert!(!path.exists());
        assert!(backup.is_file());
        let _ = fs::remove_file(backup);
    }

    #[test]
    fn unknown_schema_is_not_moved_or_overwritten() {
        let path = temp_path("future.json");
        let _ = fs::remove_file(&path);
        fs::write(&path, br#"{"schema_version":99,"revision":4,"shortcuts":{"record":"F8","repaste":"Ctrl+Alt+V"},"recognition":{"preset":"standard"},"input_device":{"kind":"system_default"},"autostart":false,"dictionary":{"entries":[]}}"#).expect("write future settings");
        assert_eq!(load(&path), Err(SettingsError::UnsupportedSchema(99)));
        assert!(path.is_file());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn future_schema_with_changed_fields_is_preserved_on_load_and_save() {
        let path =
            std::env::temp_dir().join(format!("voiceweave-future-{}.json", uuid::Uuid::new_v4()));
        let original = br#"{"schema_version":4294967296,"new_preferences":{"mode":"future"}}"#;
        fs::write(&path, original).unwrap();
        assert_eq!(
            load(&path),
            Err(SettingsError::UnsupportedSchema(4294967296))
        );
        assert_eq!(
            save(&path, &Settings::default(), 0),
            Err(SettingsError::UnsupportedSchema(4294967296))
        );
        assert_eq!(fs::read(&path).unwrap(), original);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn file_read_errors_do_not_move_the_original_path() {
        let path = std::env::temp_dir().join(format!(
            "voiceweave-settings-directory-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir(&path).unwrap();
        assert!(matches!(load(&path), Err(SettingsError::Io(_))));
        assert!(path.is_dir());
        fs::remove_dir(path).unwrap();
    }

    #[test]
    fn recognition_mode_defaults_to_this_pc_and_only_known_modes_are_saved() {
        let old: RecognitionSettings = serde_json::from_str(r#"{"preset":"standard"}"#).unwrap();
        assert_eq!(old.mode, "local");
        let mut settings = Settings::default();
        settings.recognition.mode = "cloud".into();
        assert!(settings.validate().is_ok());
        settings.recognition.mode = "elsewhere".into();
        assert!(settings.validate().is_err());
    }

    #[test]
    fn the_english_shortcut_is_off_by_default_and_needs_a_key_when_on() {
        // Files written before the option read as off, with the default key.
        let old: Shortcuts =
            serde_json::from_str(r#"{"record":"F8","repaste":"Ctrl+Alt+V"}"#).unwrap();
        assert!(!old.english_enabled);
        assert_eq!(old.english, "Shift+F8");
        let mut settings = Settings::default();
        settings.shortcuts.english = "".into();
        assert!(settings.validate().is_ok());
        settings.shortcuts.english_enabled = true;
        assert!(settings.validate().is_err());
        settings.shortcuts.english = "Shift+F8".into();
        assert!(settings.validate().is_ok());
    }
}
