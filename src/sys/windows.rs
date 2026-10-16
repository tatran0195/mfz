//! Small, checked Windows platform helpers. No ambient executable/path search.
use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf};

use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;

use crate::error::{AppError, Result};

pub(crate) fn system_directory() -> Result<PathBuf> {
    let mut buffer = vec![0u16; 260];
    loop {
        // SAFETY: buffer is writable for the length supplied to the API.
        let length =
            unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
        if length == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if length < buffer.len() {
            return Ok(PathBuf::from(OsString::from_wide(&buffer[..length])));
        }
        if length > 32768 {
            return Err(AppError::Config(
                "Windows system path exceeds the supported limit".into(),
            ));
        }
        buffer.resize(length + 1, 0);
    }
}

pub(crate) fn desktop_directory() -> Result<PathBuf> {
    use windows_sys::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_Desktop, SHGetKnownFolderPath, KF_FLAG_DONT_VERIFY},
    };
    let mut path = std::ptr::null_mut();
    let status = unsafe {
        SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DONT_VERIFY as u32, 0, &mut path)
    };
    if status < 0 || path.is_null() {
        if !path.is_null() {
            unsafe {
                CoTaskMemFree(path.cast());
            }
        }
        return Err(AppError::Shell(format!(
            "Cannot resolve Windows Desktop Known Folder (HRESULT {status:#x})"
        )));
    }
    let mut len = 0;
    // SAFETY: successful API result is a CoTaskMem-allocated NUL-terminated
    // UTF-16 path. Copy before freeing it, including unpaired UTF-16 if present.
    while unsafe { *path.add(len) } != 0 {
        len += 1;
    }
    let directory = PathBuf::from(OsString::from_wide(unsafe {
        std::slice::from_raw_parts(path, len)
    }));
    unsafe {
        CoTaskMemFree(path.cast());
    }
    Ok(directory)
}
