//! One-time move of the user's data from the folders named after the old identifier
//! ("local.whisperkey.app", before 0.2.0) to the current one. Settings, dictionary and
//! diagnostics live under %APPDATA%; models, the GPU runtime and the WebView profile
//! under %LOCALAPPDATA%. Each item is renamed on the same drive (instant, nothing is
//! copied) before any window or worker opens these folders. Nothing that already exists
//! under the new name is ever overwritten.
use std::path::Path;
use std::time::Duration;

/// Must equal `identifier` in tauri.conf.json (checked by a test).
pub const IDENTIFIER: &str = "local.voiceweave.app";
pub const LEGACY_IDENTIFIER: &str = "local.whisperkey.app";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Moved {
    /// Nothing under the old name.
    NothingToMove,
    /// Everything under the old name is now under the new one.
    Moved,
    /// Some items stayed: already present under the new name, or still in use (for
    /// example by WebView helper processes of an old instance that is shutting down).
    Partial,
}

/// Rename with a few short retries: helper processes of a just-closed instance can hold
/// a file for a moment.
fn rename_with_retry(from: &Path, to: &Path) -> bool {
    for attempt in 0..10 {
        if std::fs::rename(from, to).is_ok() {
            return true;
        }
        if attempt < 9 {
            std::thread::sleep(Duration::from_millis(300));
        }
    }
    false
}

/// Move every item of `old` into `new` that `new` does not have yet, then remove `old`
/// if it became empty.
pub fn move_folder(old: &Path, new: &Path) -> Moved {
    if !old.is_dir() {
        return Moved::NothingToMove;
    }
    if !new.exists() && rename_with_retry(old, new) {
        return Moved::Moved;
    }
    if std::fs::create_dir_all(new).is_err() {
        return Moved::Partial;
    }
    let Ok(entries) = std::fs::read_dir(old) else {
        return Moved::Partial;
    };
    let mut left = false;
    for entry in entries.flatten() {
        let target = new.join(entry.file_name());
        if target.exists() || !rename_with_retry(&entry.path(), &target) {
            left = true;
        }
    }
    if left {
        Moved::Partial
    } else {
        let _ = std::fs::remove_dir(old);
        Moved::Moved
    }
}

/// Move both data folders. Returns (roaming, local).
pub fn migrate_legacy_data() -> (Moved, Moved) {
    let mut results = [Moved::NothingToMove; 2];
    for (slot, variable) in results.iter_mut().zip(["APPDATA", "LOCALAPPDATA"]) {
        if let Some(base) = std::env::var_os(variable) {
            let base = Path::new(&base);
            *slot = move_folder(&base.join(LEGACY_IDENTIFIER), &base.join(IDENTIFIER));
        }
    }
    (results[0], results[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_matches_the_bundle_configuration() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(config["identifier"], IDENTIFIER);
        assert_ne!(IDENTIFIER, LEGACY_IDENTIFIER);
    }

    fn scratch() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("voiceweave-migrate-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn an_old_folder_alone_is_moved_whole() {
        let root = scratch();
        let (old, new) = (root.join("old"), root.join("new"));
        assert_eq!(move_folder(&old, &new), Moved::NothingToMove);
        std::fs::create_dir_all(old.join("models")).unwrap();
        std::fs::write(old.join("settings.json"), b"{}").unwrap();
        assert_eq!(move_folder(&old, &new), Moved::Moved);
        assert!(!old.exists());
        assert_eq!(std::fs::read(new.join("settings.json")).unwrap(), b"{}");
        assert!(new.join("models").is_dir());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn missing_items_join_an_existing_new_folder_without_overwriting() {
        // As happened on 2026-10-06: the new folder already held a fresh WebView profile.
        let root = scratch();
        let (old, new) = (root.join("old"), root.join("new"));
        std::fs::create_dir_all(old.join("models")).unwrap();
        std::fs::write(old.join("models").join("model.bin"), b"weights").unwrap();
        std::fs::create_dir_all(old.join("EBWebView")).unwrap();
        std::fs::write(old.join("EBWebView").join("old"), b"old").unwrap();
        std::fs::create_dir_all(new.join("EBWebView")).unwrap();
        std::fs::write(new.join("EBWebView").join("new"), b"new").unwrap();

        assert_eq!(move_folder(&old, &new), Moved::Partial);
        assert_eq!(
            std::fs::read(new.join("models/model.bin")).unwrap(),
            b"weights"
        );
        // The existing item is untouched and the old one stays where it was.
        assert!(new.join("EBWebView/new").exists() && !new.join("EBWebView/old").exists());
        assert!(old.join("EBWebView/old").exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
