use std::{fs, io, path::Path};

use serde::{Deserialize, Serialize};

use super::DownloadDto;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct StoredDownload {
    pub download: DownloadDto,
    pub destination: String,
    pub encrypted_url: Vec<u8>,
}

pub(super) fn load(path: &Path) -> io::Result<Vec<StoredDownload>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

pub(super) fn save(path: &Path, downloads: &[StoredDownload]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "store path has no parent"))?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec(downloads)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    use std::io::Write;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    replace_file(&temporary, path)
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
    fs::rename(temporary, destination)
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
pub(super) fn protect_url(_value: &str) -> io::Result<Vec<u8>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "download URL protection requires Windows DPAPI",
    ))
}

#[cfg(not(windows))]
pub(super) fn unprotect_url(_value: &[u8]) -> io::Result<String> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "download URL protection requires Windows DPAPI",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DownloadDto, UiDownloadState};
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
                state: UiDownloadState::Interrupted,
                bytes_written: 0,
                total_bytes: Some(10),
                speed_bytes_per_second: None,
                eta_seconds: None,
                error: Some("interrupted".into()),
            },
            destination: "C:\\Downloads\\sample.bin".into(),
            encrypted_url: vec![1, 2, 3],
        }
    }

    #[test]
    fn history_snapshot_can_be_replaced_and_loaded() {
        let path = test_path();
        save(&path, &[sample("first")]).unwrap();
        save(&path, &[sample("second")]).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].download.id, "second");
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
