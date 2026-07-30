use super::{Error, OpenOptions, Ordering, Path, PathBuf, Result, TEMP_SEQUENCE, Write, fs, io};

pub(super) struct IndexLock {
    path: PathBuf,
}

impl IndexLock {
    pub(super) fn acquire(index_path: &Path) -> Result<Self> {
        let path = suffix_path(index_path, "lock");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|source| {
                if source.kind() == io::ErrorKind::AlreadyExists {
                    Error::index(index_path, "another writer holds the index lock")
                } else {
                    Error::io(&path, source)
                }
            })?;
        writeln!(file, "{}", std::process::id()).map_err(|source| Error::io(&path, source))?;
        file.sync_all().map_err(|source| Error::io(&path, source))?;
        Ok(Self { path })
    }
}

impl Drop for IndexLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub(super) fn auxiliary_path(path: &Path, suffix: &str) -> PathBuf {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    suffix_path(
        path,
        &format!("{suffix}.{}.{}", std::process::id(), sequence),
    )
}

fn suffix_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(".");
    value.push(suffix);
    PathBuf::from(value)
}

pub(super) fn replace_file(temp: &Path, target: &Path) -> Result<()> {
    if !target.exists() {
        return fs::rename(temp, target).map_err(|source| Error::io(target, source));
    }
    let backup = auxiliary_path(target, "backup");
    fs::rename(target, &backup).map_err(|source| Error::io(target, source))?;
    match fs::rename(temp, target) {
        Ok(()) => {
            let _ = fs::remove_file(backup);
            Ok(())
        }
        Err(source) => {
            let _ = fs::rename(&backup, target);
            Err(Error::io(target, source))
        }
    }
}

#[cfg(unix)]
pub(super) fn encode_path(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(windows)]
pub(super) fn encode_path(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(unix)]
// Windows rejects odd UTF-16 payloads; keeping one fallible decoder signature
// makes the format reader identical on every platform.
#[allow(clippy::unnecessary_wraps)]
pub(super) fn decode_path(bytes: &[u8]) -> std::result::Result<PathBuf, &'static str> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    Ok(PathBuf::from(OsString::from_vec(bytes.to_vec())))
}

#[cfg(windows)]
pub(super) fn decode_path(bytes: &[u8]) -> std::result::Result<PathBuf, &'static str> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    if !bytes.len().is_multiple_of(2) {
        return Err("Windows root path has an odd byte length");
    }
    let wide = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();
    Ok(PathBuf::from(OsString::from_wide(&wide)))
}
