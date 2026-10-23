//! Durable, versioned JSON snapshot of the download history.
//!
//! Writes go to a temporary file in the same directory, are flushed, and are
//! moved over the previous snapshot with a best-effort durable rename, so an
//! interrupted process never leaves a half-written history in place of the
//! old one. Loading is deliberately forgiving: a truncated snapshot is
//! repaired up to its last complete record and the raw bytes are preserved
//! in a timestamped sidecar before anything overwrites them.

use std::{fs, io, path::Path};

use serde::{Deserialize, Serialize};

use super::service::DownloadDto;

/// Bump when the record shape changes. Version 1 was a bare array of
/// `StoredDownload`; version 2 is the enveloped store below.
pub(super) const STORE_VERSION: u32 = 2;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct StoredDownload {
    pub download: DownloadDto,
    pub destination: String,
    pub encrypted_url: Vec<u8>,
    /// Verified partial-transfer state for interrupted jobs. Absent when the
    /// record predates durable resume or the partial file was never valid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<StoredResume>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct StoredResume {
    pub temp_path: String,
    pub checkpoint: Option<StoredCheckpoint>,
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct StoredCheckpoint {
    pub effective_url: String,
    pub strong_etag: String,
    pub total_bytes: u64,
}

#[derive(Serialize, Deserialize)]
struct StoreFile {
    version: u32,
    downloads: Vec<StoredDownload>,
}

/// What a history load found. `notice` is a user-facing sentence when the
/// snapshot needed repair or could not be read at all.
pub(super) struct LoadResult {
    pub records: Vec<StoredDownload>,
    pub notice: Option<String>,
}

pub(super) fn load(path: &Path) -> io::Result<LoadResult> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(LoadResult {
                records: Vec::new(),
                notice: None,
            })
        }
        Err(error) => {
            return Err(error);
        }
    };

    // Current envelope first, then the version 1 bare array.
    if let Ok(store) = serde_json::from_slice::<StoreFile>(&bytes) {
        if store.version == STORE_VERSION {
            return Ok(LoadResult {
                records: store.downloads,
                notice: None,
            });
        }
    }
    if let Ok(records) = serde_json::from_slice::<Vec<StoredDownload>>(&bytes) {
        return Ok(LoadResult {
            records,
            notice: None,
        });
    }

    // The snapshot is malformed. Try to repair a truncated array up to its
    // last complete record before giving up; keep the raw bytes either way.
    match repair_truncated_array(&bytes) {
        Some(records) => {
            preserve_corrupt_snapshot(path, &bytes)?;
            Ok(LoadResult {
                records,
                notice: Some(
                    "Part of the saved download history was unreadable. Usable records were recovered and a backup of the original file was kept in the Velo data folder.".into(),
                ),
            })
        }
        None => {
            preserve_corrupt_snapshot(path, &bytes)?;
            Ok(LoadResult {
                records: Vec::new(),
                notice: Some(
                    "The saved download history was unreadable, so Velo started with an empty list. A backup of the original file was kept in the Velo data folder.".into(),
                ),
            })
        }
    }
}

/// Recovers the longest prefix of a truncated JSON array that still parses.
/// Returns `None` when the bytes are damaged in a way truncation cannot fix.
fn repair_truncated_array(bytes: &[u8]) -> Option<Vec<StoredDownload>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let trimmed = text.trim_start();
    if !trimmed.starts_with('[') {
        return None;
    }
    let mut end = text.len();
    while let Some(position) = text[..end].rfind('}') {
        let mut repaired = text[..=position].to_string();
        repaired.push(']');
        if let Ok(records) = serde_json::from_str::<Vec<StoredDownload>>(&repaired) {
            return Some(records);
        }
        // Keep shortening the candidate; `position` itself was just tried.
        end = position;
    }
    None
}

/// Copies the unreadable snapshot next to itself before any later save can
/// replace it, so user data is never silently destroyed.
fn preserve_corrupt_snapshot(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default();
    let sidecar = path.with_extension(format!("json.corrupt-{stamp}"));
    fs::write(&sidecar, bytes)
}

pub(super) fn save(path: &Path, downloads: &[StoredDownload]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "store path has no parent"))?;
    fs::create_dir_all(parent)?;
    let bytes = serde_json::to_vec(&StoreFile {
        version: STORE_VERSION,
        downloads: downloads.to_vec(),
    })
    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    write_atomic(path, &bytes)
}

/// Serializes `bytes` into a synced temporary file next to `path`, then
/// durably replaces `path` with it. Shared by the download-history snapshot
/// and the settings store so both survive an interrupted process.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temporary = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("store")
    ));
    let saved = write_snapshot(&temporary, bytes);
    if saved.is_err() {
        let _ = fs::remove_file(&temporary);
        return saved;
    }
    replace_file(&temporary, path)
}

fn write_snapshot(temporary: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut options = fs::OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // 0o600: the history is private to the running user account.
        options.mode(0o600);
    }
    let mut file = options
        .create(true)
        .truncate(true)
        .write(true)
        .open(temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    Ok(())
}

#[cfg(windows)]
fn replace_file(temporary: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let temporary: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let result = unsafe {
        MoveFileExW(
            temporary.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_file(temporary: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(temporary, destination)?;
    // The rename is only durable once the directory entry itself is synced.
    if let Some(parent) = destination.parent() {
        if let Ok(dir) = fs::File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

#[cfg(windows)]
pub(super) fn protect_url(value: &str) -> io::Result<Vec<u8>> {
    use std::{mem::zeroed, ptr, slice};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB},
    };

    let bytes = value.as_bytes();
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(bytes.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "URL is too long"))?,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let mut output: CRYPT_INTEGER_BLOB = unsafe { zeroed() };
    let result = unsafe {
        CryptProtectData(
            &input,
            ptr::null(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    let protected =
        unsafe { slice::from_raw_parts(output.pbData, output.cbData as usize) }.to_vec();
    unsafe { LocalFree(output.pbData.cast()) };
    Ok(protected)
}

#[cfg(windows)]
pub(super) fn unprotect_url(value: &[u8]) -> io::Result<String> {
    use std::{mem::zeroed, ptr, slice};
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        },
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(value.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "stored URL is too long"))?,
        pbData: value.as_ptr().cast_mut(),
    };
    let mut output: CRYPT_INTEGER_BLOB = unsafe { zeroed() };
    let result = unsafe {
        CryptUnprotectData(
            &input,
            ptr::null_mut(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if result == 0 {
        return Err(io::Error::last_os_error());
    }
    let bytes = unsafe { slice::from_raw_parts(output.pbData, output.cbData as usize) };
    let decoded = String::from_utf8(bytes.to_vec())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error));
    unsafe { LocalFree(output.pbData.cast()) };
    decoded
}

#[cfg(not(windows))]
#[allow(dead_code)]
pub(super) fn protect_url(_value: &str) -> io::Result<Vec<u8>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "download URL protection requires Windows DPAPI",
    ))
}

#[cfg(not(windows))]
#[allow(dead_code)]
pub(super) fn unprotect_url(_value: &[u8]) -> io::Result<String> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "download URL protection requires Windows DPAPI",
    ))
}

/// Test-only stand-in so service-level tests can exercise history restoration
/// without DPAPI. Production always uses DPAPI on Windows.
#[cfg(test)]
pub(crate) mod test_protection {
    use std::io;

    pub(crate) fn protect(value: &str) -> io::Result<Vec<u8>> {
        Ok(value.as_bytes().to_vec())
    }

    pub(crate) fn unprotect(value: &[u8]) -> io::Result<String> {
        String::from_utf8(value.to_vec())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::{DownloadDto, UiDownloadState};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_path() -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("velo-history-{}-{nonce}.json", std::process::id()))
    }

    fn sample(id: &str) -> StoredDownload {
        StoredDownload {
            download: DownloadDto {
                id: id.into(),
                generation: 1,
                revision: 1,
                order: 1,
                file_name: "sample.bin".into(),
                destination: "C:\\Downloads\\sample.bin".into(),
                state: UiDownloadState::Interrupted,
                bytes_written: 0,
                total_bytes: Some(10),
                speed_bytes_per_second: None,
                eta_seconds: None,
                error: Some("interrupted".into()),
            },
            destination: "C:\\Downloads\\sample.bin".into(),
            encrypted_url: vec![1, 2, 3],
            resume: None,
        }
    }

    #[test]
    fn history_snapshot_can_be_replaced_and_loaded() {
        let path = test_path();
        save(&path, &[sample("first")]).unwrap();
        save(&path, &[sample("second")]).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.records.len(), 1);
        assert_eq!(loaded.records[0].download.id, "second");
        assert!(loaded.notice.is_none());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn version_one_bare_array_snapshots_still_load() {
        let path = test_path();
        let legacy = serde_json::to_vec(&vec![sample("legacy")]).unwrap();
        fs::write(&path, &legacy).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.records.len(), 1);
        assert_eq!(loaded.records[0].download.id, "legacy");
        assert!(loaded.notice.is_none());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn truncated_array_is_repaired_up_to_the_last_complete_record() {
        let path = test_path();
        let mut bytes = serde_json::to_vec(&vec![sample("whole"), sample("cut")]).unwrap();
        // Simulate an interrupted write: keep the first record complete but
        // truncate inside the second one.
        let text = String::from_utf8(bytes.clone()).unwrap();
        let last_record_start = text.rfind("{\"download\"").unwrap();
        let first_record_end = text[1..last_record_start].rfind('}').unwrap() + 2;
        bytes.truncate(first_record_end);
        fs::write(&path, &bytes).unwrap();
        let stem = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap()
            .to_owned();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.records.len(), 1);
        assert_eq!(loaded.records[0].download.id, "whole");
        assert!(loaded.notice.is_some());
        // The original bytes survive in a sidecar.
        let sidecars: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|candidate| {
                candidate
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with(stem.as_str()) && name.contains(".corrupt-")
                    })
            })
            .collect();
        assert_eq!(sidecars.len(), 1);
        for sidecar in sidecars {
            fs::remove_file(sidecar).unwrap();
        }
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn unreadable_history_is_backed_up_and_reported_instead_of_discarded() {
        let path = test_path();
        fs::write(&path, b"{not json at all").unwrap();
        let stem = path
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap()
            .to_owned();
        let loaded = load(&path).unwrap();
        assert!(loaded.records.is_empty());
        assert!(loaded.notice.is_some());
        let sidecars: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|candidate| {
                candidate
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with(stem.as_str()) && name.contains(".corrupt-")
                    })
            })
            .collect();
        assert_eq!(sidecars.len(), 1);
        assert_eq!(fs::read(&sidecars[0]).unwrap(), b"{not json at all");
        fs::remove_file(sidecars[0].clone()).unwrap();
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn saved_snapshots_are_versioned_envelopes() {
        let path = test_path();
        save(&path, &[sample("enveloped")]).unwrap();
        let bytes = fs::read(&path).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["version"], STORE_VERSION);
        assert!(value["downloads"].is_array());
        fs::remove_file(path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn saved_snapshot_permissions_are_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt;
        let path = test_path();
        save(&path, &[sample("private")]).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        fs::remove_file(path).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn url_round_trips_through_current_windows_account_protection() {
        let url = "https://example.test/file?token=private";
        let encrypted = protect_url(url).unwrap();
        assert_ne!(encrypted, url.as_bytes());
        assert_eq!(unprotect_url(&encrypted).unwrap(), url);
    }
}
