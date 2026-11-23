//! Start at Windows sign-in through the per-user Run key. Written only when the user
//! saves the setting, and read back so the screen shows what Windows will really do.
//! Development builds never register themselves: their executable is a build output.
use std::path::Path;

pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
pub const VALUE_NAME: &str = "VoiceWeave";

/// The registry value could not be read or changed. No detail: it may contain paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegistryError;

/// One registry value; a trait so the policy is testable without touching Windows.
pub trait RunEntry {
    fn read(&self) -> Result<Option<String>, RegistryError>;
    fn write(&self, command: &str) -> Result<(), RegistryError>;
    fn remove(&self) -> Result<(), RegistryError>;
}

/// The command Windows runs at sign-in. Quoted, since install paths contain spaces.
pub fn command_line(executable: &Path) -> String {
    format!("\"{}\"", executable.display())
}

/// Whether Windows will start exactly this executable at sign-in.
pub fn is_registered(entry: &impl RunEntry, executable: &Path) -> Result<bool, RegistryError> {
    Ok(entry.read()?.as_deref() == Some(command_line(executable).as_str()))
}

/// Make the Run entry match `enabled` and report what is registered afterwards.
/// Another program's value under our name is replaced only when enabling, and an
/// entry for a different executable path counts as "not registered for this one".
pub fn apply(
    entry: &impl RunEntry,
    enabled: bool,
    executable: &Path,
) -> Result<bool, RegistryError> {
    if enabled {
        entry.write(&command_line(executable))?;
    } else if entry.read()?.is_some() {
        entry.remove()?;
    }
    is_registered(entry, executable)
}

#[cfg(windows)]
pub struct WindowsRunEntry {
    subkey: Vec<u16>,
    value: Vec<u16>,
}

#[cfg(windows)]
impl WindowsRunEntry {
    pub fn current_user() -> Self {
        Self::at(RUN_KEY, VALUE_NAME)
    }

    fn at(subkey: &str, value: &str) -> Self {
        let wide = |text: &str| text.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        Self {
            subkey: wide(subkey),
            value: wide(value),
        }
    }
}

#[cfg(windows)]
impl RunEntry for WindowsRunEntry {
    fn read(&self) -> Result<Option<String>, RegistryError> {
        use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
        use windows_sys::Win32::System::Registry::{
            RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ,
        };
        let mut buffer = vec![0_u16; 1024];
        let mut bytes = (buffer.len() * 2) as u32;
        // SAFETY: NUL-terminated names; the buffer size is passed in bytes.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                self.subkey.as_ptr(),
                self.value.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        match status {
            ERROR_SUCCESS => {
                let len = (bytes as usize / 2).saturating_sub(1).min(buffer.len());
                Ok(Some(String::from_utf16_lossy(&buffer[..len])))
            }
            ERROR_FILE_NOT_FOUND => Ok(None),
            _ => Err(RegistryError),
        }
    }

    fn write(&self, command: &str) -> Result<(), RegistryError> {
        use windows_sys::Win32::Foundation::ERROR_SUCCESS;
        use windows_sys::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
        let data: Vec<u16> = command.encode_utf16().chain(Some(0)).collect();
        // SAFETY: NUL-terminated names and data; the size is in bytes including NUL.
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                self.subkey.as_ptr(),
                self.value.as_ptr(),
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        };
        (status == ERROR_SUCCESS).then_some(()).ok_or(RegistryError)
    }

    fn remove(&self) -> Result<(), RegistryError> {
        use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
        use windows_sys::Win32::System::Registry::{RegDeleteKeyValueW, HKEY_CURRENT_USER};
        // SAFETY: NUL-terminated names.
        let status = unsafe {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, self.subkey.as_ptr(), self.value.as_ptr())
        };
        matches!(status, ERROR_SUCCESS | ERROR_FILE_NOT_FOUND)
            .then_some(())
            .ok_or(RegistryError)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::path::PathBuf;

    #[derive(Default)]
    struct FakeEntry {
        value: RefCell<Option<String>>,
        fail_write: bool,
    }

    impl RunEntry for FakeEntry {
        fn read(&self) -> Result<Option<String>, RegistryError> {
            Ok(self.value.borrow().clone())
        }
        fn write(&self, command: &str) -> Result<(), RegistryError> {
            if self.fail_write {
                return Err(RegistryError);
            }
            *self.value.borrow_mut() = Some(command.to_owned());
            Ok(())
        }
        fn remove(&self) -> Result<(), RegistryError> {
            *self.value.borrow_mut() = None;
            Ok(())
        }
    }

    fn exe() -> PathBuf {
        PathBuf::from(r"C:\Program Files\VoiceWeave\VoiceWeave.exe")
    }

    #[test]
    fn enabling_registers_a_quoted_path_and_disabling_removes_it() {
        let entry = FakeEntry::default();
        assert_eq!(apply(&entry, true, &exe()), Ok(true));
        assert_eq!(
            entry.value.borrow().as_deref(),
            Some(r#""C:\Program Files\VoiceWeave\VoiceWeave.exe""#)
        );
        assert_eq!(apply(&entry, false, &exe()), Ok(false));
        assert_eq!(*entry.value.borrow(), None);
    }

    #[test]
    fn an_entry_for_another_path_is_not_ours_and_a_failed_write_is_reported() {
        let entry = FakeEntry::default();
        *entry.value.borrow_mut() = Some(r#""D:\old\VoiceWeave.exe""#.to_owned());
        assert_eq!(is_registered(&entry, &exe()), Ok(false));
        let failing = FakeEntry {
            fail_write: true,
            ..FakeEntry::default()
        };
        assert_eq!(apply(&failing, true, &exe()), Err(RegistryError));
    }

    /// Writes and removes a value under a private test key in HKCU. Run explicitly:
    /// `cargo test -- --ignored real_registry_round_trip`.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn real_registry_round_trip() {
        let subkey = format!(r"Software\VoiceWeaveTest\{}", uuid::Uuid::new_v4());
        let entry = WindowsRunEntry::at(&subkey, "VoiceWeave");
        assert_eq!(entry.read(), Ok(None));
        assert_eq!(apply(&entry, true, &exe()), Ok(true));
        assert_eq!(apply(&entry, false, &exe()), Ok(false));
        assert_eq!(entry.read(), Ok(None));
        let wide: Vec<u16> = r"Software\VoiceWeaveTest"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // SAFETY: NUL-terminated name of the private test tree.
        unsafe {
            windows_sys::Win32::System::Registry::RegDeleteTreeW(
                windows_sys::Win32::System::Registry::HKEY_CURRENT_USER,
                wide.as_ptr(),
            );
        }
    }
}
