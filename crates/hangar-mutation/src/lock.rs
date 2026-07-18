//! File Lock Inspector — best-effort advisory check of whether a file is in use.
//!
//! Lets the executor report a locked file up front instead of failing
//! mid-operation. Best-effort only: it does not identify which process holds the
//! file (that needs the Restart Manager API and is out of scope here), and it
//! treats only a sharing violation as "locked" so read-only files are not
//! misreported.

use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockState {
    Free,
    Locked,
    Missing,
}

/// Windows `ERROR_SHARING_VIOLATION` — another handle holds the file.
#[cfg(windows)]
const SHARING_VIOLATION: i32 = 32;

/// Probe whether `path` can be opened for writing right now.
pub fn inspect_lock(path: &Path) -> LockState {
    if !path.exists() {
        return LockState::Missing;
    }
    match fs::OpenOptions::new().read(true).write(true).open(path) {
        Ok(_) => LockState::Free,
        #[cfg(windows)]
        Err(err) if err.raw_os_error() == Some(SHARING_VIOLATION) => LockState::Locked,
        // Other errors (permissions, read-only) are not a lock; the actual
        // operation will surface them if relevant.
        Err(_) => LockState::Free,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_for_a_writable_file_and_missing_for_absent() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("free.bin");
        fs::write(&file, b"data").unwrap();
        assert_eq!(inspect_lock(&file), LockState::Free);
        assert_eq!(
            inspect_lock(&dir.path().join("nope.bin")),
            LockState::Missing
        );
    }
}
