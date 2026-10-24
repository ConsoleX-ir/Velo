//! Persistent application settings and destination composition.
//!
//! This module is deliberately tauri-free (mirroring `service` and
//! `persistence`) so the default-directory rules can be tested without the
//! webview. It owns two concerns:
//!
//! 1. The durable settings store (`settings.json` in the app-data directory),
//!    which currently holds the user's chosen default download directory.
//! 2. Safe composition of a download destination from a directory and a file
//!    name, so the webview never assembles paths itself and the engine keeps
//!    receiving a single validated absolute destination.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};

use super::persistence;

const SETTINGS_VERSION: u32 = 1;

/// What the settings store can hold today. Bump `SETTINGS_VERSION` and add a
/// `#[serde(default)]` field when the shape grows.
#[derive(Clone, Serialize, Deserialize)]
struct StoredSettings {
    version: u32,
    #[serde(default)]
    default_download_dir: Option<String>,
}

impl Default for StoredSettings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            default_download_dir: None,
        }
    }
}

/// The settings snapshot the webview is allowed to see. Paths are plain
/// user-chosen directories; no secrets live in the store.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettingsDto {
    /// The user's saved default download directory, when one was chosen.
    pub(crate) default_download_dir: Option<String>,
    /// Whether the saved directory still exists and is writable right now.
    pub(crate) stored_dir_valid: bool,
    /// The operating system's Downloads folder, when the platform resolves it.
    pub(crate) os_default_download_dir: Option<String>,
    /// Where new downloads go without a per-download override: the saved
    /// default while it stays valid, otherwise the OS Downloads folder.
    pub(crate) effective_download_dir: Option<String>,
}

/// Command-shaped error so the Tauri shell can pass it to the webview
/// directly, mirroring `service::CommandError`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettingsError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

impl SettingsError {
    fn invalid_directory(message: impl Into<String>) -> Self {
        Self {
            code: "invalid_directory",
            message: message.into(),
        }
    }

    fn store_unavailable() -> Self {
        Self {
            code: "settings_unavailable",
            message: "Velo could not save the settings. The choice applies to this session only."
                .into(),
        }
    }
}

/// Managed state behind the `get_settings` / `set_default_directory` /
/// `clear_default_directory` commands.
pub(crate) struct SettingsState {
    store_path: PathBuf,
    os_default: Option<PathBuf>,
    inner: Mutex<StoredSettings>,
}

impl SettingsState {
    /// Loads the store, falling back to defaults when it is missing or
    /// unreadable. The OS Downloads folder is resolved once at startup.
    pub(crate) fn build(store_path: PathBuf, os_default: Option<PathBuf>) -> Self {
        let stored = load(&store_path).unwrap_or_else(|error| {
            eprintln!("Could not read the Velo settings: {error}");
            StoredSettings::default()
        });
        Self {
            store_path,
            os_default,
            inner: Mutex::new(stored),
        }
    }

    pub(crate) fn get(&self) -> SettingsDto {
        let inner = self.lock();
        self.snapshot(&inner)
    }

    /// Persists a new default download directory after validating it exists,
    /// is a real folder, and accepts writes. Returns the refreshed snapshot.
    pub(crate) fn set_default(&self, directory: &str) -> Result<SettingsDto, SettingsError> {
        let trimmed = directory.trim();
        if !validate_directory(Path::new(trimmed)) {
            return Err(SettingsError::invalid_directory(
                "That folder is not a writable directory. Choose a different location.",
            ));
        }
        let mut inner = self.lock();
        inner.default_download_dir = Some(trimmed.to_owned());
        save(&self.store_path, &inner).map_err(|error| {
            eprintln!("Could not save the Velo settings: {error}");
            SettingsError::store_unavailable()
        })?;
        Ok(self.snapshot(&inner))
    }

    /// Forgets the saved directory so downloads default to the OS folder.
    pub(crate) fn clear_default(&self) -> Result<SettingsDto, SettingsError> {
        let mut inner = self.lock();
        inner.default_download_dir = None;
        save(&self.store_path, &inner).map_err(|error| {
            eprintln!("Could not save the Velo settings: {error}");
            SettingsError::store_unavailable()
        })?;
        Ok(self.snapshot(&inner))
    }

    fn snapshot(&self, inner: &StoredSettings) -> SettingsDto {
        let stored_dir_valid = inner
            .default_download_dir
            .as_deref()
            .map(|dir| validate_directory(Path::new(dir)))
            .unwrap_or(false);
        let effective_download_dir = self.effective_path(inner);
        SettingsDto {
            default_download_dir: inner.default_download_dir.clone(),
            stored_dir_valid,
            os_default_download_dir: self
                .os_default
                .as_deref()
                .map(|path| path.to_string_lossy().into_owned()),
            effective_download_dir,
        }
    }

    fn effective_path(&self, inner: &StoredSettings) -> Option<String> {
        match &inner.default_download_dir {
            Some(dir) if validate_directory(Path::new(dir)) => Some(dir.clone()),
            _ => self
                .os_default
                .as_deref()
                .map(|path| path.to_string_lossy().into_owned()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, StoredSettings> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn load(path: &Path) -> std::io::Result<StoredSettings> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(StoredSettings::default());
        }
        Err(error) => return Err(error),
    };
    let stored = serde_json::from_slice::<StoredSettings>(&bytes)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if stored.version != SETTINGS_VERSION {
        // Unknown future shape: keep nothing rather than guess. The user can
        // pick the directory again; downloads are unaffected.
        return Ok(StoredSettings::default());
    }
    Ok(stored)
}

fn save(path: &Path, stored: &StoredSettings) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec(stored)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    persistence::write_atomic(path, &bytes)
}

/// Acceptance probe for a candidate default directory: absolute, a real
/// directory, and writable (proven by creating and removing a uniquely named
/// marker file rather than trusting metadata alone).
pub(crate) fn validate_directory(path: &Path) -> bool {
    if path.as_os_str().is_empty() || !path.is_absolute() || !path.is_dir() {
        return false;
    }
    let probe = path.join(format!(".velo-write-probe-{}", std::process::id()));
    match fs::File::create(&probe) {
        Ok(file) => {
            drop(file);
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Characters Windows forbids in file names. Rejected on every platform so a
/// name typed into Velo behaves identically wherever the file lands.
const FORBIDDEN_NAME_CHARS: [char; 9] = ['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// Why composing a destination failed. `InvalidFileName` carries the
/// user-facing remedy.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum JoinDestinationError {
    InvalidDirectory,
    InvalidFileName(&'static str),
}

/// Composes the final download destination from a directory and a bare file
/// name. The name must be a single safe component: no separators, no parent
/// components, no Windows-reserved device names. The engine re-validates the
/// joined path and enforces its own no-overwrite policy afterwards.
pub(crate) fn join_destination(
    directory: &str,
    file_name: &str,
) -> Result<PathBuf, JoinDestinationError> {
    let dir = Path::new(directory.trim());
    if dir.as_os_str().is_empty() || !dir.is_absolute() {
        return Err(JoinDestinationError::InvalidDirectory);
    }
    let name = file_name.trim();
    if name.is_empty() || name == "." || name == ".." {
        return Err(JoinDestinationError::InvalidFileName(
            "Enter a file name for this download.",
        ));
    }
    if name
        .chars()
        .any(|character| FORBIDDEN_NAME_CHARS.contains(&character) || character.is_control())
    {
        return Err(JoinDestinationError::InvalidFileName(
            "The file name contains characters Windows does not allow.",
        ));
    }
    if name.ends_with('.') {
        return Err(JoinDestinationError::InvalidFileName(
            "The file name cannot end with a dot.",
        ));
    }
    if is_reserved_windows_name(name) {
        return Err(JoinDestinationError::InvalidFileName(
            "The file name uses a reserved Windows device name.",
        ));
    }
    Ok(dir.join(name))
}

/// `CON`, `AUX`, `COM1`… are device names on Windows even with an extension.
fn is_reserved_windows_name(name: &str) -> bool {
    const RESERVED_STEMS: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem = name.split('.').next().unwrap_or("");
    RESERVED_STEMS.contains(&stem.to_ascii_uppercase().as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_ID: AtomicU64 = AtomicU64::new(0);

    fn test_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "velo-settings-{}-{}",
            std::process::id(),
            TEST_ID.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn store_path(dir: &Path) -> PathBuf {
        dir.join("settings.json")
    }

    #[test]
    fn chosen_default_directory_persists_across_restart() {
        let dir = test_dir();
        let path = store_path(&dir);
        let state = SettingsState::build(path.clone(), None);
        let snapshot = state.set_default(dir.to_string_lossy().as_ref()).unwrap();
        assert_eq!(
            snapshot.default_download_dir.as_deref(),
            Some(dir.to_string_lossy().as_ref())
        );
        assert!(snapshot.stored_dir_valid);
        assert_eq!(
            snapshot.effective_download_dir.as_deref(),
            Some(dir.to_string_lossy().as_ref())
        );

        // A fresh state (the next launch) reads the same choice.
        let restarted = SettingsState::build(path, None);
        let snapshot = restarted.get();
        assert_eq!(
            snapshot.default_download_dir.as_deref(),
            Some(dir.to_string_lossy().as_ref())
        );
        assert!(snapshot.stored_dir_valid);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_store_starts_with_the_os_default() {
        let dir = test_dir();
        let os_default = dir.join("system-downloads");
        let state = SettingsState::build(store_path(&dir), Some(os_default.clone()));
        let snapshot = state.get();
        assert!(snapshot.default_download_dir.is_none());
        assert_eq!(
            snapshot.os_default_download_dir.as_deref(),
            Some(os_default.to_string_lossy().as_ref())
        );
        assert_eq!(
            snapshot.effective_download_dir.as_deref(),
            Some(os_default.to_string_lossy().as_ref())
        );
        assert_eq!(
            state.get().effective_download_dir.as_deref(),
            Some(os_default.to_string_lossy().as_ref())
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unusable_directories_are_rejected() {
        let dir = test_dir();
        let state = SettingsState::build(store_path(&dir), None);
        assert!(state.set_default("relative/path").is_err());
        assert!(state.set_default("").is_err());
        assert!(state
            .set_default(
                dir.join("definitely-missing-velo")
                    .to_string_lossy()
                    .as_ref()
            )
            .is_err());
        // A regular file is not a directory.
        let file = dir.join("plain.txt");
        fs::write(&file, b"velo").unwrap();
        assert!(state.set_default(file.to_string_lossy().as_ref()).is_err());
        // Nothing was persisted by the rejected attempts.
        assert!(state.get().default_download_dir.is_none());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_invalid_saved_directory_falls_back_to_the_os_default() {
        let dir = test_dir();
        let path = store_path(&dir);
        let broken = StoredSettings {
            version: SETTINGS_VERSION,
            default_download_dir: Some(dir.join("gone-velo").to_string_lossy().into_owned()),
        };
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, serde_json::to_vec(&broken).unwrap()).unwrap();

        let os_default = dir.join("system-downloads");
        let state = SettingsState::build(path, Some(os_default.clone()));
        let snapshot = state.get();
        assert!(!snapshot.stored_dir_valid);
        assert_eq!(
            snapshot.effective_download_dir.as_deref(),
            Some(os_default.to_string_lossy().as_ref())
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn clearing_the_default_returns_to_the_os_folder() {
        let dir = test_dir();
        let os_default = dir.join("system-downloads");
        let state = SettingsState::build(store_path(&dir), Some(os_default.clone()));
        state.set_default(dir.to_string_lossy().as_ref()).unwrap();
        let snapshot = state.clear_default().unwrap();
        assert!(snapshot.default_download_dir.is_none());
        assert_eq!(
            snapshot.effective_download_dir.as_deref(),
            Some(os_default.to_string_lossy().as_ref())
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_corrupt_settings_store_starts_empty_instead_of_failing() {
        let dir = test_dir();
        let path = store_path(&dir);
        fs::write(&path, b"{ not json").unwrap();
        let state = SettingsState::build(path, None);
        assert!(state.get().default_download_dir.is_none());
        // The next successful save replaces the broken file.
        state.set_default(dir.to_string_lossy().as_ref()).unwrap();
        assert!(state.get().stored_dir_valid);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn join_composes_safe_destinations() {
        let dir = test_dir();
        let joined = join_destination(dir.to_string_lossy().as_ref(), "setup.exe").unwrap();
        assert_eq!(joined, dir.join("setup.exe"));
        // Surrounding whitespace is trimmed, not preserved: the result is a
        // Windows-safe name with no trailing dot or space.
        let joined = join_destination(
            format!("{} ", dir.to_string_lossy()).as_str(),
            " archive.tar.gz ",
        )
        .unwrap();
        assert_eq!(joined, dir.join("archive.tar.gz"));
        assert_eq!(
            join_destination(dir.to_string_lossy().as_ref(), "name ").unwrap(),
            dir.join("name")
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn join_rejects_traversal_separators_and_unsafe_names() {
        let dir = test_dir();
        let directory = dir.to_string_lossy().as_ref().to_owned();
        for bad in [
            "../escape.bin",
            "..\\escape.bin",
            "a/b.bin",
            "a\\b.bin",
            "C:oops.bin",
            "..",
            ".",
            "",
            "  ",
            "name.",
            "con",
            "Con.txt",
            "nul.zip",
            "com1.dat",
        ] {
            assert!(
                join_destination(&directory, bad).is_err(),
                "expected {bad:?} to be rejected"
            );
        }
        assert_eq!(
            join_destination("relative/dir", "file.bin"),
            Err(JoinDestinationError::InvalidDirectory)
        );
        assert_eq!(
            join_destination("", "file.bin"),
            Err(JoinDestinationError::InvalidDirectory)
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
