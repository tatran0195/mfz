//! Windows file security belongs at creation, not after writing secret bytes.
//! ReplaceFile preserves an existing destination DACL; a recovery copy protects
//! the original against its documented partial-failure states.
use std::{
    ffi::c_void,
    fs::File,
    io,
    os::windows::{
        ffi::OsStrExt,
        io::{FromRawHandle, OwnedHandle},
    },
    path::Path,
};

use tempfile::{NamedTempFile, TempPath};
use windows_sys::Win32::{
    Foundation::{LocalFree, ERROR_INSUFFICIENT_BUFFER, INVALID_HANDLE_VALUE},
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        },
        GetTokenInformation, TokenUser, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    },
    Storage::FileSystem::{
        CreateFileW, ReplaceFileW, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ,
        FILE_GENERIC_WRITE, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

use crate::error::{AppError, Result};

struct LocalAllocation(*mut c_void);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

fn wide(path: &Path) -> io::Result<Vec<u16>> {
    let absolute = std::path::absolute(path)?;
    let mut path: Vec<_> = absolute.as_os_str().encode_wide().collect();
    if path.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "File path contains NUL",
        ));
    }
    let verbatim: Vec<_> = "\\\\?\\".encode_utf16().collect();
    let unc: Vec<_> = "\\\\".encode_utf16().collect();
    if path.len() >= 259 && !path.starts_with(&verbatim) {
        path = if path.starts_with(&unc) {
            "\\\\?\\UNC\\"
                .encode_utf16()
                .chain(path.into_iter().skip(2))
                .collect()
        } else {
            verbatim.into_iter().chain(path).collect()
        };
    }
    path.push(0);
    Ok(path)
}

fn private_descriptor() -> io::Result<LocalAllocation> {
    let mut token = 0;
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful OpenProcessToken transfers one real owned handle.
    let _token = unsafe { OwnedHandle::from_raw_handle(token as _) };
    let mut length = 0;
    let sized =
        unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut length) };
    if sized != 0
        || io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32)
        || length < std::mem::size_of::<TOKEN_USER>() as u32
        || length > 65536
    {
        return Err(io::Error::other(
            "Cannot obtain bounded Windows token-user information",
        ));
    }
    // usize storage has the alignment required by TOKEN_USER and its SID pointer.
    let mut buffer = vec![0usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
    if unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            length,
            &mut length,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let mut sid = std::ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if sid.is_null() {
        return Err(io::Error::other("Windows returned a null SID string"));
    }
    let _sid = LocalAllocation(sid.cast());
    let mut sid_len = 0;
    while sid_len < 512 && unsafe { *sid.add(sid_len) } != 0 {
        sid_len += 1;
    }
    if sid_len == 512 {
        return Err(io::Error::other(
            "Windows SID string exceeds its supported bound",
        ));
    }
    let sid = String::from_utf16(unsafe { std::slice::from_raw_parts(sid, sid_len) })
        .map_err(|_| io::Error::other("Invalid UTF-16 Windows SID"))?;
    // Explicit process-user SID works for both elevated and filtered tokens.
    // A protected DACL prevents broad parent-directory ACE inheritance. SYSTEM
    // retains access; no Everyone/Users/Administrators grant is added here.
    let sddl: Vec<_> = format!("D:P(A;;FA;;;SY)(A;;FA;;;{sid})")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut descriptor = std::ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if descriptor.is_null() {
        return Err(io::Error::other(
            "Windows returned a null security descriptor",
        ));
    }
    Ok(LocalAllocation(descriptor))
}

pub(super) fn temporary_file(parent: &Path, private: bool, suffix: &str) -> Result<NamedTempFile> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(".mfz-").suffix(suffix);
    if !private {
        return Ok(builder.tempfile_in(parent)?);
    }
    let descriptor = private_descriptor()?;
    Ok(builder.make_in(parent, |path| {
        let path = wide(path)?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0,
            bInheritHandle: 0,
        };
        // The private DACL applies atomically in CreateFileW: no interval exists
        // where another user can open an inherited-public file before tightening.
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                FILE_GENERIC_READ | FILE_GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                &attributes,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL,
                0,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            Ok(unsafe { File::from_raw_handle(handle as _) })
        }
    })?)
}

pub(super) fn replace_existing(temp: NamedTempFile, destination: &Path) -> Result<()> {
    let destination_wide = wide(destination)?;
    // Reserve a random, private, same-volume recovery filename. ReplaceFileW
    // replaces this owned placeholder with the original destination on commit.
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut backup = temporary_file(parent, true, "")?.into_temp_path();
    let backup_path = backup.to_path_buf();
    let backup_wide = wide(&backup_path)?;
    // ReplaceFileW opens the replacement without sharing. Close its file handle
    // first but retain the TempPath guard for cleanup on pre-commit failure.
    let mut replacement: TempPath = temp.into_temp_path();
    let replacement_wide = wide(&replacement)?;
    let replaced = unsafe {
        ReplaceFileW(
            destination_wide.as_ptr(),
            replacement_wide.as_ptr(),
            backup_wide.as_ptr(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if replaced == 0 {
        let error = io::Error::last_os_error();
        // Never let RAII delete a possibly moved original after a partial API
        // failure. Attempt a no-clobber restore only if the destination vanished.
        backup.disable_cleanup(true);
        let destination_exists = destination.try_exists().map_err(|check| AppError::Config(format!("Windows replacement failed ({error}) and destination inspection failed ({check}); preserve recovery copy {backup_path:?}")))?;
        if !destination_exists {
            match backup.persist_noclobber(destination) {
                Ok(()) => {
                    return Err(AppError::Config(format!(
                        "Windows replacement failed ({error}); the original file was restored"
                    )))
                }
                Err(mut restore) => {
                    restore.path.disable_cleanup(true);
                    return Err(AppError::Config(format!("Windows replacement failed ({error}) and original restore failed ({}); preserve and inspect recovery copy {:?} before retrying", restore.error, backup_path)));
                }
            }
        }
        return Err(AppError::Config(format!("Windows replacement failed ({error}); inspect destination and preserved recovery copy {backup_path:?} before retrying")));
    }
    replacement.disable_cleanup(true); // its old name no longer belongs to us
    backup.close().map_err(|error| AppError::Config(format!("Destination was updated, but old-file backup cleanup could not be confirmed ({error}) at {backup_path:?}; inspect/remove it securely (it may contain the previous secrets)")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Security::{
        Authorization::{
            ConvertSecurityDescriptorToStringSecurityDescriptorW, GetNamedSecurityInfoW,
            SE_FILE_OBJECT,
        },
        GetSecurityDescriptorControl, DACL_SECURITY_INFORMATION, SE_DACL_PROTECTED,
    };

    fn dacl(path: &Path) -> (String, u16) {
        let name = wide(path).unwrap();
        let mut descriptor = std::ptr::null_mut();
        let status = unsafe {
            GetNamedSecurityInfoW(
                name.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut descriptor,
            )
        };
        assert_eq!(status, 0, "GetNamedSecurityInfoW failed: {status}");
        assert!(!descriptor.is_null());
        let _descriptor = LocalAllocation(descriptor);
        let mut control = 0;
        let mut revision = 0;
        assert_ne!(
            unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) },
            0
        );
        let mut text = std::ptr::null_mut();
        assert_ne!(
            unsafe {
                ConvertSecurityDescriptorToStringSecurityDescriptorW(
                    descriptor,
                    1,
                    DACL_SECURITY_INFORMATION,
                    &mut text,
                    std::ptr::null_mut(),
                )
            },
            0
        );
        assert!(!text.is_null());
        let _text = LocalAllocation(text.cast());
        let mut len = 0;
        while unsafe { *text.add(len) } != 0 {
            len += 1;
        }
        (
            String::from_utf16(unsafe { std::slice::from_raw_parts(text, len) }).unwrap(),
            control,
        )
    }

    #[test]
    fn secret_temporary_file_is_private_before_any_content_is_written() {
        let directory = tempfile::tempdir().unwrap();
        let file = temporary_file(directory.path(), true, ".rdp").unwrap();
        assert_eq!(file.path().extension().unwrap(), "rdp");
        let (security, control) = dacl(file.path());
        assert_ne!(control & SE_DACL_PROTECTED, 0);
        assert_eq!(
            security.matches("(A;").count(),
            2,
            "Only SYSTEM and the process user may be granted access: {security}"
        );
        assert!(!security.contains(";;;WD)") && !security.contains(";;;BU)"));
        assert_eq!(file.as_file().metadata().unwrap().len(), 0);
    }

    #[test]
    fn overwrite_preserves_original_dacl_and_removes_owned_recovery_artifacts() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("config.ini");
        std::fs::write(&destination, "original config").unwrap();
        let before = dacl(&destination);
        crate::sys::storage::atomic_write(&destination, b"new config", true, true).unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"new config");
        assert_eq!(dacl(&destination), before);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}
