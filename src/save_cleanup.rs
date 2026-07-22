use std::{
    ffi::OsString,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

const SAVE_TEMP_PREFIX: &str = ".ahe-save-";
const SAVE_TEMP_SUFFIX: &str = ".tmp";
const RECOVERY_RECORD_PREFIX: &str = "ahe-save-recovery-";
const MAX_RECOVERY_RECORD_BYTES: u64 = 64 * 1024;

pub struct RecoveryRecord {
    _file: tempfile::NamedTempFile,
}

pub fn create_save_file(parent: &Path) -> io::Result<(tempfile::NamedTempFile, RecoveryRecord)> {
    let parent = fs::canonicalize(parent)?;
    let temporary = tempfile::Builder::new()
        .prefix(SAVE_TEMP_PREFIX)
        .suffix(SAVE_TEMP_SUFFIX)
        .tempfile_in(parent)?;
    let mut record = tempfile::Builder::new()
        .prefix(RECOVERY_RECORD_PREFIX)
        .tempfile_in(std::env::temp_dir())?;
    record.write_all(&encode_path(temporary.path()))?;
    record.as_file_mut().sync_all()?;
    let _ = sync_directory(&std::env::temp_dir());
    #[cfg(not(test))]
    crate::drag_cleanup::register(record.path(), std::time::Duration::ZERO);
    Ok((temporary, RecoveryRecord { _file: record }))
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

pub fn recover_abandoned() {
    let Ok(entries) = fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let record_path = entry.path();
        if !is_recovery_record(&record_path) {
            continue;
        }
        recover_record(&record_path);
    }
}

pub(crate) fn recover_record(record_path: &Path) {
    let target = read_record(record_path);
    if let Some(target) = target.as_deref()
        && is_safe_save_file(target)
    {
        let _ = fs::remove_file(target);
    }
    let _ = fs::remove_file(record_path);
}

fn read_record(record_path: &Path) -> Option<PathBuf> {
    if !is_recovery_record(record_path) {
        return None;
    }
    let metadata = fs::symlink_metadata(record_path).ok()?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_RECOVERY_RECORD_BYTES
    {
        return None;
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(record_path).ok()?.read_to_end(&mut bytes).ok()?;
    decode_path(bytes)
}

pub(crate) fn is_recovery_record(path: &Path) -> bool {
    path.is_absolute()
        && path.parent() == Some(std::env::temp_dir().as_path())
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.starts_with(RECOVERY_RECORD_PREFIX)
                    && name.len() > RECOVERY_RECORD_PREFIX.len()
            })
}

fn is_safe_save_file(path: &Path) -> bool {
    if !path.is_absolute()
        || !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.starts_with(SAVE_TEMP_PREFIX)
                    && name.ends_with(SAVE_TEMP_SUFFIX)
                    && name.len() > SAVE_TEMP_PREFIX.len() + SAVE_TEMP_SUFFIX.len()
            })
    {
        return false;
    }
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
}

#[cfg(unix)]
fn encode_path(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(unix)]
fn decode_path(bytes: Vec<u8>) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    (!bytes.is_empty()).then(|| PathBuf::from(OsString::from_vec(bytes)))
}

#[cfg(windows)]
fn encode_path(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(windows)]
fn decode_path(bytes: Vec<u8>) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    if bytes.is_empty() || !bytes.len().is_multiple_of(2) {
        return None;
    }
    let wide = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();
    Some(PathBuf::from(OsString::from_wide(&wide)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_record_removes_only_ahe_save_files() {
        let destination = tempfile::tempdir().unwrap();
        let (temporary, record) = create_save_file(destination.path()).unwrap();
        let target = temporary.path().to_path_buf();
        let record_path = record._file.path().to_path_buf();
        fs::write(&target, b"partial archive").unwrap();
        let (temporary_file, _) = temporary.keep().unwrap();
        let (record_file, _) = record._file.keep().unwrap();
        drop(temporary_file);
        drop(record_file);

        recover_record(&record_path);
        assert!(!target.exists());
        assert!(!record_path.exists());
    }

    #[test]
    fn malformed_record_cannot_remove_an_unrelated_file() {
        let destination = tempfile::tempdir().unwrap();
        let unrelated = destination.path().join("important.hak");
        fs::write(&unrelated, b"keep me").unwrap();
        let mut record = tempfile::Builder::new()
            .prefix(RECOVERY_RECORD_PREFIX)
            .tempfile_in(std::env::temp_dir())
            .unwrap();
        record.write_all(&encode_path(&unrelated)).unwrap();
        let record_path = record.path().to_path_buf();
        let (record_file, _) = record.keep().unwrap();
        drop(record_file);

        recover_record(&record_path);
        assert_eq!(fs::read(unrelated).unwrap(), b"keep me");
        assert!(!record_path.exists());
    }

    #[test]
    fn path_encoding_round_trips() {
        let path = std::env::temp_dir().join(".ahe-save-roundtrip.tmp");
        assert_eq!(
            decode_path(encode_path(&path)).as_deref(),
            Some(path.as_path())
        );
    }
}
