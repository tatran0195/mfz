//! Small filesystem primitives shared by configuration and secret-bearing exports.
//! A mutation lock must cover the READ as well as the atomic replacement.
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use crate::error::{AppError, Result};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;

pub(crate) fn temporary_file(
    parent: &Path,
    private: bool,
    suffix: &str,
) -> Result<tempfile::NamedTempFile> {
    #[cfg(windows)]
    {
        platform::temporary_file(parent, private, suffix)
    }
    #[cfg(not(windows))]
    {
        // tempfile creates 0600 files atomically on Unix, even for ordinary data.
        let _ = private;
        Ok(tempfile::Builder::new()
            .prefix(".mfz-")
            .suffix(suffix)
            .tempfile_in(parent)?)
    }
}

pub(crate) const MAX_PROFILE_BYTES: u64 = 1024 * 1024;

pub(crate) fn read_profile(path: &Path) -> Result<String> {
    if !fs::metadata(path)?.is_file() {
        return Err(AppError::Config("Profile must be a regular file".into()));
    }
    let mut content = String::new();
    File::open(path)?
        .take(MAX_PROFILE_BYTES + 1)
        .read_to_string(&mut content)?;
    if content.len() as u64 > MAX_PROFILE_BYTES {
        return Err(AppError::Config(
            "Profile exceeds the 1 MiB size limit".into(),
        ));
    }
    Ok(content)
}

/// Locks a stable sibling inode. Never unlink this lock file: another process
/// could otherwise lock a different inode while a previous lock is still held.
pub(crate) struct MutationLock {
    _file: File,
}

impl MutationLock {
    pub(crate) fn acquire(path: &Path) -> Result<Self> {
        let name = path
            .file_name()
            .ok_or_else(|| AppError::Config("A file path is required".into()))?;
        let mut lock_name = name.to_os_string();
        lock_name.push(".lock");
        let lock_path = path.with_file_name(lock_name);
        reject_non_regular(&lock_path)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(lock_path)?;
        file.try_lock().map_err(|error| {
            AppError::Config(format!(
                "Cannot acquire the file mutation lock (another operation may be active): {error}"
            ))
        })?;
        Ok(Self { _file: file })
    }
}

fn reject_non_regular(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if !meta.file_type().is_file() => Err(AppError::Config(
            "Refusing to replace a symlink or non-regular file".into(),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn atomic_write(
    path: &Path,
    contents: &[u8],
    overwrite: bool,
    private: bool,
) -> Result<()> {
    reject_non_regular(path)?;
    if let Ok(meta) = fs::metadata(path) {
        if meta.permissions().readonly() {
            return Err(AppError::Config("Destination file is read-only".into()));
        }
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temp = temporary_file(parent, private, "")?;
    temp.write_all(contents)?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    #[cfg(windows)]
    if overwrite && path.try_exists()? {
        return platform::replace_existing(temp, path);
    }
    if overwrite && !cfg!(windows) {
        temp.persist(path)
            .map_err(|error| AppError::Io(error.error))?;
    } else {
        temp.persist_noclobber(path)
            .map_err(|error| AppError::Io(error.error))?;
    }
    Ok(())
}

/// Keep configuration search opt-in: an explicitly supplied missing path must
/// not silently select a similarly named file from another directory.
pub(crate) fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}
