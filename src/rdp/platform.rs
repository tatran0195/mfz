//! Windows-only RDP resources. Each guard removes only the state it created.
use std::{io::Write, os::windows::ffi::OsStrExt, path::Path};

use sha2::{Digest, Sha256};
use tracing::{info, warn};
use windows_sys::Win32::{
    Foundation::{ERROR_FILE_NOT_FOUND, ERROR_INSUFFICIENT_BUFFER, ERROR_NOT_FOUND},
    Security::Credentials::{
        CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_MAX_CREDENTIAL_BLOB_SIZE,
        CRED_PERSIST_SESSION, CRED_TYPE_GENERIC,
    },
    Storage::FileSystem::{
        DefineDosDeviceW, GetDriveTypeW, GetLogicalDrives, QueryDosDeviceW,
        DDD_EXACT_MATCH_ON_REMOVE, DDD_NO_BROADCAST_SYSTEM, DDD_RAW_TARGET_PATH,
        DDD_REMOVE_DEFINITION,
    },
};

use super::{
    build_rdp_file_content, find_available_drive_letter, resolve_drives_to_redirect, LaunchOptions,
    RdpProfileOptions,
};
use crate::error::{AppError, Result};

pub(super) fn logical_drives_mask() -> Result<u32> {
    let mask = unsafe { GetLogicalDrives() };
    if mask == 0 {
        Err(std::io::Error::last_os_error().into())
    } else {
        Ok(mask)
    }
}

pub(super) fn is_local_drive(letter: char) -> bool {
    let root: Vec<_> = format!("{letter}:\\")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    // DRIVE_REMOVABLE/FIXED/CDROM/RAMDISK. Exclude unknown/no-root/remote.
    matches!(unsafe { GetDriveTypeW(root.as_ptr()) }, 2 | 3 | 5 | 6)
}

fn query_device(device: &[u16]) -> Result<Option<Vec<u16>>> {
    let mut buffer = vec![0; 1024];
    loop {
        let length =
            unsafe { QueryDosDeviceW(device.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) }
                as usize;
        if length != 0 {
            let end = buffer[..length]
                .iter()
                .position(|ch| *ch == 0)
                .ok_or_else(|| AppError::Rdp("Invalid DOS-device query response".into()))?;
            return Ok(Some(buffer[..end].to_vec()));
        }
        let error = std::io::Error::last_os_error();
        match error.raw_os_error().map(|code| code as u32) {
            Some(ERROR_FILE_NOT_FOUND) => return Ok(None),
            Some(ERROR_INSUFFICIENT_BUFFER) if buffer.len() < 32768 => {
                buffer.resize(buffer.len() * 2, 0)
            }
            _ => return Err(error.into()),
        }
    }
}

struct FolderDriveGuard {
    letter: char,
    device: Vec<u16>,
    target: Vec<u16>,
    active: bool,
}

impl FolderDriveGuard {
    fn mount(folder: &Path) -> Result<Self> {
        let canonical = folder.canonicalize()?;
        if !canonical.is_dir() {
            return Err(AppError::Rdp("Shared folder is not a directory".into()));
        }
        let letter = find_available_drive_letter()?;
        let device: Vec<_> = format!("{letter}:").encode_utf16().chain(Some(0)).collect();
        if query_device(&device)?.is_some() {
            return Err(AppError::Rdp("Virtual drive letter was claimed concurrently; retry without altering that mapping".into()));
        }
        let path: Vec<_> = canonical.as_os_str().encode_wide().collect();
        let extended_unc: Vec<_> = "\\\\?\\UNC\\".encode_utf16().collect();
        let extended: Vec<_> = "\\\\?\\".encode_utf16().collect();
        let unc: Vec<_> = "\\\\".encode_utf16().collect();
        let (prefix, tail) = if path.starts_with(&extended_unc) {
            ("\\??\\UNC\\", &path[extended_unc.len()..])
        } else if path.starts_with(&extended) {
            ("\\??\\", &path[extended.len()..])
        } else if path.starts_with(&unc) {
            ("\\??\\UNC\\", &path[unc.len()..])
        } else {
            ("\\??\\", path.as_slice())
        };
        let target: Vec<_> = prefix
            .encode_utf16()
            .chain(tail.iter().copied())
            .chain(Some(0))
            .collect();
        if unsafe {
            DefineDosDeviceW(
                DDD_RAW_TARGET_PATH | DDD_NO_BROADCAST_SYSTEM,
                device.as_ptr(),
                target.as_ptr(),
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        let guard = Self {
            letter,
            device,
            target,
            active: true,
        };
        if query_device(&guard.device)?.as_deref() != Some(&guard.target[..guard.target.len() - 1])
        {
            return Err(AppError::Rdp("Virtual drive mapping changed during creation; no unrelated mapping will be removed".into()));
        }
        info!("Shared folder mapped to {}:", letter);
        Ok(guard)
    }

    fn release(&mut self) -> Result<()> {
        if !self.active {
            return Ok(());
        }
        // EXACT_MATCH + RAW_TARGET is essential: never delete another process's
        // mapping just because it subsequently uses the same drive letter.
        if unsafe {
            DefineDosDeviceW(
                DDD_REMOVE_DEFINITION
                    | DDD_EXACT_MATCH_ON_REMOVE
                    | DDD_RAW_TARGET_PATH
                    | DDD_NO_BROADCAST_SYSTEM,
                self.device.as_ptr(),
                self.target.as_ptr(),
            )
        } == 0
        {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_FILE_NOT_FOUND as i32) {
                return Err(error.into());
            }
        }
        self.active = false;
        Ok(())
    }
}

impl Drop for FolderDriveGuard {
    fn drop(&mut self) {
        if let Err(error) = self.release() {
            warn!("Owned folder mapping cleanup failed: {error}");
        }
    }
}

struct ReadCredential(*mut CREDENTIALW);
impl ReadCredential {
    fn read(target: &[u16]) -> Result<Option<Self>> {
        let mut credential = std::ptr::null_mut();
        if unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) } != 0 {
            if credential.is_null() {
                return Err(AppError::Rdp(
                    "Credential read returned a null buffer".into(),
                ));
            }
            Ok(Some(Self(credential)))
        } else {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_NOT_FOUND as i32) {
                Ok(None)
            } else {
                Err(error.into())
            }
        }
    }
}
impl Drop for ReadCredential {
    fn drop(&mut self) {
        unsafe {
            CredFree(self.0.cast());
        }
    }
}

// SAFETY: Credential API strings are documented NUL-terminated and the owned
// CredReadW buffer remains live for the whole read. Never dereference a null.
unsafe fn read_wide(value: *const u16) -> Option<String> {
    if value.is_null() {
        return None;
    }
    let mut len = 0;
    while unsafe { *value.add(len) } != 0 {
        len += 1;
    }
    String::from_utf16(unsafe { std::slice::from_raw_parts(value, len) }).ok()
}

struct CredentialGuard {
    target: Vec<u16>,
    marker: String,
    username: String,
    blob_hash: [u8; 32],
    active: bool,
}
impl CredentialGuard {
    fn create(host: &str, port: u16, username: &str, password: &str) -> Result<Self> {
        let target: Vec<_> = format!("TERMSRV/{host}:{port}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // No destructive overwrite, broad host-only delete, or process-argument
        // secret. Existing user credentials remain completely untouched.
        if ReadCredential::read(&target)?.is_some() {
            return Err(AppError::Rdp("A TERMSRV credential already exists for this endpoint; omit the stored RDP password and use mstsc's prompt, or manage that credential explicitly".into()));
        }
        let marker = format!("MFZ temporary credential {}", uuid::Uuid::new_v4());
        let mut comment: Vec<_> = marker.encode_utf16().chain(Some(0)).collect();
        let mut user: Vec<_> = username.encode_utf16().chain(Some(0)).collect();
        let mut target = target;
        let mut blob: Vec<_> = password.encode_utf16().flat_map(u16::to_le_bytes).collect();
        if blob.len() > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize {
            return Err(AppError::Rdp(
                "RDP password exceeds the Windows credential blob limit".into(),
            ));
        }
        let blob_hash = Sha256::digest(&blob).into();
        let mut credential: CREDENTIALW = unsafe { std::mem::zeroed() };
        credential.Type = CRED_TYPE_GENERIC;
        credential.TargetName = target.as_mut_ptr();
        credential.Comment = comment.as_mut_ptr();
        credential.UserName = user.as_mut_ptr();
        credential.Persist = CRED_PERSIST_SESSION;
        credential.CredentialBlobSize = blob.len() as u32;
        credential.CredentialBlob = blob.as_mut_ptr();
        let written = unsafe { CredWriteW(&credential, 0) };
        let write_error = (written == 0).then(std::io::Error::last_os_error);
        // Best-effort clearing of this temporary copy; not a promise that all
        // original CLI/String/OS copies of a supplied secret have been erased.
        blob.fill(0);
        if let Some(error) = write_error {
            return Err(error.into());
        }
        let guard = Self {
            target,
            marker,
            username: username.to_owned(),
            blob_hash,
            active: true,
        };
        if !guard.still_owned()? {
            return Err(AppError::Rdp(
                "Temporary RDP credential failed ownership/readback verification".into(),
            ));
        }
        Ok(guard)
    }

    fn still_owned(&self) -> Result<bool> {
        let Some(current) = ReadCredential::read(&self.target)? else {
            return Ok(false);
        };
        let credential = unsafe { &*current.0 };
        if unsafe { read_wide(credential.Comment) }.as_deref() != Some(&self.marker)
            || unsafe { read_wide(credential.UserName) }.as_deref() != Some(&self.username)
        {
            return Ok(false);
        }
        let len = credential.CredentialBlobSize as usize;
        if len > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize
            || (len != 0 && credential.CredentialBlob.is_null())
        {
            return Ok(false);
        }
        let blob = if len == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(credential.CredentialBlob, len) }
        };
        Ok(<[u8; 32]>::from(Sha256::digest(blob)) == self.blob_hash)
    }

    fn release(&mut self) -> Result<()> {
        if !self.active {
            return Ok(());
        }
        if self.still_owned()?
            && unsafe { CredDeleteW(self.target.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        self.active = false;
        Ok(())
    }
}
impl Drop for CredentialGuard {
    fn drop(&mut self) {
        if let Err(error) = self.release() {
            warn!("Owned RDP credential cleanup failed: {error}");
        }
    }
}

pub(super) async fn launch(opts: LaunchOptions<'_>) -> Result<()> {
    let executable = crate::windows::system_directory()?.join("mstsc.exe");
    if !executable.is_file() {
        return Err(AppError::Rdp(
            "The Windows system mstsc.exe was not found".into(),
        ));
    }
    let mut folder = opts
        .shared_folder
        .map(FolderDriveGuard::mount)
        .transpose()?;
    let drives = resolve_drives_to_redirect(
        opts.shared_drives,
        folder.as_ref().map(|guard| guard.letter),
    )?;
    let contents = build_rdp_file_content(&RdpProfileOptions {
        host: opts.host,
        port: opts.port,
        username: opts.username,
        fullscreen: opts.fullscreen,
        drives_to_redirect: drives.as_deref(),
        optimization: opts.optimization,
    })?;
    let mut profile = crate::storage::temporary_file(&std::env::temp_dir(), true, ".rdp")?;
    profile.write_all(contents.as_bytes())?;
    profile.flush()?;
    // Close the write handle so mstsc.exe can open and read the profile without
    // sharing or lock collisions, while retaining the TempPath RAII cleanup guard.
    let profile = profile.into_temp_path();
    let mut credentials = match (opts.username, opts.password) {
        (Some(user), Some(password)) => Some(CredentialGuard::create(
            opts.host, opts.port, user, password,
        )?),
        _ => None,
    };
    // Awaiting the actual owned process keeps resources alive. Cancellation
    // kills our child and drops both guards; it never kills an unrelated mstsc.
    let mut child = tokio::process::Command::new(executable)
        .arg(&profile)
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| AppError::Rdp(format!("Cannot start system mstsc.exe: {error}")))?;
    let status = child.wait().await?;
    let credential_cleanup = credentials
        .as_mut()
        .map(CredentialGuard::release)
        .transpose();
    let folder_cleanup = folder.as_mut().map(FolderDriveGuard::release).transpose();
    if !status.success() {
        return Err(AppError::Rdp(format!(
            "mstsc exited unsuccessfully: {status}"
        )));
    }
    credential_cleanup?;
    folder_cleanup?;
    Ok(())
}
