use std::fs::File;
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::AsRawFd;

use codex_hashline_transaction::TransactionFileSystemError;

const EXT_SUPER_MAGIC: libc::c_long = 0xef53;
const TMPFS_MAGIC: libc::c_long = 0x0102_1994;
const FS_CASEFOLD_FL: libc::c_long = 0x4000_0000;

pub(super) fn ensure_byte_exact_directory(
    directory: &File,
) -> Result<(), TransactionFileSystemError> {
    let mut stat = MaybeUninit::<libc::statfs>::uninit();
    // SAFETY: stat points to writable storage and directory owns a live descriptor.
    let result = unsafe { libc::fstatfs(directory.as_raw_fd(), stat.as_mut_ptr()) };
    if result != 0 {
        return Err(platform_error(io::Error::last_os_error()));
    }
    // SAFETY: successful fstatfs initialized stat.
    let filesystem_type = unsafe { stat.assume_init() }.f_type;
    match filesystem_type {
        TMPFS_MAGIC => Ok(()),
        EXT_SUPER_MAGIC => ensure_ext_directory_is_case_sensitive(directory),
        _ => Err(TransactionFileSystemError::Unsupported {
            capability: "byte-exact transaction path keys",
            reason: format!(
                "Linux filesystem type {filesystem_type:#x} has no proven byte-exact lookup adapter"
            ),
        }),
    }
}

fn ensure_ext_directory_is_case_sensitive(
    directory: &File,
) -> Result<(), TransactionFileSystemError> {
    let mut flags: libc::c_long = 0;
    // SAFETY: the request only reads flags into a correctly sized output value.
    let result = unsafe { libc::ioctl(directory.as_raw_fd(), libc::FS_IOC_GETFLAGS, &mut flags) };
    if result != 0 {
        return Err(platform_error(io::Error::last_os_error()));
    }
    if flags & FS_CASEFOLD_FL != 0 {
        return Err(TransactionFileSystemError::Unsupported {
            capability: "byte-exact transaction path keys",
            reason: "the selected directory enables case-insensitive lookup".to_string(),
        });
    }
    Ok(())
}

fn platform_error(error: io::Error) -> TransactionFileSystemError {
    TransactionFileSystemError::Platform {
        operation: "inspect path semantics",
        reason: error.to_string(),
    }
}
