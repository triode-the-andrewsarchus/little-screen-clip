//! Atomic Linux rename primitives; callers own source/destination admission.
use std::{
    ffi::{CString, OsStr},
    os::unix::ffi::OsStrExt,
    path::Path,
};

/// Renames without overwriting an existing destination.
///
/// Callers must perform ordinary-source and destination safety admission and
/// retain any required pins; this syscall primitive supplies no such admission.
///
/// # Errors
/// Returns invalid-name or kernel rename errors, including an occupied target.
pub fn rename_noreplace(source: &Path, destination: &Path) -> std::io::Result<()> {
    rename_at_noreplace(
        libc::AT_FDCWD,
        source.as_os_str(),
        libc::AT_FDCWD,
        destination.as_os_str(),
    )
}

/// Renames between borrowed directory descriptors without overwriting.
///
/// Callers retain and validate the directory handles and both names. Absolute
/// paths and `AT_FDCWD` retain normal Linux semantics; this is not a sandbox.
///
/// # Errors
/// Returns invalid-name or kernel rename errors, including an occupied target.
pub fn rename_at_noreplace(
    source_fd: i32,
    source: &OsStr,
    destination_fd: i32,
    destination: &OsStr,
) -> std::io::Result<()> {
    let source = CString::new(source.as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let destination = CString::new(destination.as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    // SAFETY: both names are NUL-terminated and each descriptor is borrowed from
    // a live directory or is AT_FDCWD. Linux refuses any existing destination.
    let result = unsafe {
        libc::renameat2(
            source_fd,
            source.as_ptr(),
            destination_fd,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
