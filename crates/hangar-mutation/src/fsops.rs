//! Shared filesystem move primitive for the mutation executor (gated), used by
//! `restore`. (The `quarantine` executor has an equivalent inline move; the two
//! can be unified in a later cleanup.)
//!
//! Same-volume = atomic rename; cross-volume = copy → blake3-verify → delete
//! source (the source is removed only after its copy verifies byte-identical).

use std::fs;
use std::io;
use std::path::Path;

use thiserror::Error;

use crate::longpath::to_extended;

#[derive(Debug, Error)]
pub(crate) enum FsMoveError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("checksum mismatch moving {path}")]
    ChecksumMismatch { path: String },
    #[error("refusing to overwrite an occupied destination: {path}")]
    DestinationOccupied { path: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MoveStrategy {
    Rename,
    CopyDelete,
}

pub(crate) fn choose_strategy(from: &Path, to: &Path) -> MoveStrategy {
    if same_volume(from, to) {
        MoveStrategy::Rename
    } else {
        MoveStrategy::CopyDelete
    }
}

/// Move `from` to `to`, creating parent directories; returns the moved byte
/// count. Cross-volume moves verify the copy with blake3 before deleting the
/// source. Callers gate reparse/no-follow before calling.
pub(crate) fn move_path(
    from: &Path,
    to: &Path,
    strategy: MoveStrategy,
) -> Result<u64, FsMoveError> {
    // Extended-length (`\\?\`) forms of the source/destination for the actual Win32
    // fs calls, so a >260-char path works regardless of the machine's LongPathsEnabled
    // key. A no-op off Windows and for already-verbatim paths. The journal keeps the
    // original (non-verbatim) `from`/`to`; only these local aliases are verbatim.
    let from_ext = to_extended(from);
    let to_ext = to_extended(to);
    // Immediate no-overwrite guard, mirroring the occupied-destination checks already in the
    // quarantine executor (quarantine.rs) and the backup engine (backup.rs). Both fs::rename
    // and fs::copy silently overwrite an existing destination on Windows, so re-check right
    // before the move to close the TOCTOU window between the caller's earlier
    // destination.exists() check and this point. A file that raced into the slot is refused,
    // never clobbered. Use the extended-length form so the check is accurate for a long path
    // (a bare exists() on a >260 path can wrongly report "absent" without LongPathsEnabled).
    if to_ext.exists() {
        return Err(FsMoveError::DestinationOccupied {
            path: to.to_string_lossy().to_string(),
        });
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(to_extended(parent).as_ref())?;
    }
    let bytes = file_size(&from_ext);
    match strategy {
        MoveStrategy::Rename => {
            fs::rename(from_ext.as_ref(), to_ext.as_ref())?;
        }
        MoveStrategy::CopyDelete => {
            let source_hash = hash_file(&from_ext)?;
            fs::copy(from_ext.as_ref(), to_ext.as_ref())?;
            // Flush the copy to stable storage before we trust it: fs::copy
            // leaves the bytes in the OS write cache, so verifying and then
            // deleting the source without an fsync can lose data on power loss
            // in that window. Spec: copy → fsync → verify → delete source.
            fsync_file(&to_ext)?;
            let dest_hash = hash_file(&to_ext)?;
            if source_hash != dest_hash {
                let _ = fs::remove_file(to_ext.as_ref());
                return Err(FsMoveError::ChecksumMismatch {
                    path: from.to_string_lossy().to_string(),
                });
            }
            fs::remove_file(from_ext.as_ref())?;
        }
    }
    Ok(bytes)
}

fn file_size(path: &Path) -> u64 {
    fs::metadata(to_extended(path).as_ref())
        .map(|meta| meta.len())
        .unwrap_or(0)
}

pub(crate) fn hash_file(path: &Path) -> Result<String, FsMoveError> {
    // Normalize to the extended-length form so hashing a >260-char held/original
    // path succeeds without LongPathsEnabled. Callers pass the journal's ordinary
    // path (e.g. restore hashes the recorded quarantine/original paths); the
    // verbatim alias is used only for the open here.
    let mut hasher = blake3::Hasher::new();
    let mut file = fs::File::open(to_extended(path).as_ref())?;
    io::copy(&mut file, &mut hasher)?;
    Ok(hasher.finalize().to_hex().to_string())
}

/// Flush a just-copied file's contents to stable storage. `fs::copy` leaves the
/// written bytes in the OS write cache, so a verify-then-delete-source sequence
/// can lose the copy on power loss before it is flushed. Re-opens the file with
/// write access because Windows `FlushFileBuffers` (what `sync_all` maps to)
/// requires a writable handle. Shared by the cross-volume copy paths in
/// `quarantine` and `backup` so all three honour copy → fsync → verify.
pub(crate) fn fsync_file(path: &Path) -> io::Result<()> {
    // Extended-length form so a just-copied >260-char file can be reopened to flush
    // without LongPathsEnabled. Shared by the cross-volume copy paths in quarantine
    // and backup, which pass their (possibly long) destination path.
    fs::OpenOptions::new()
        .write(true)
        .open(to_extended(path).as_ref())?
        .sync_all()
}

/// Best-effort same-volume check by canonicalized path prefix (drive on
/// Windows). Canonicalizes the nearest existing ancestor so a not-yet-created
/// destination compares consistently. Junctions/mounts to the same physical
/// volume are not detected; that only changes rename-vs-copy, never correctness.
fn same_volume(a: &Path, b: &Path) -> bool {
    fn volume_key(path: &Path) -> Option<String> {
        let mut probe = path;
        let canonical = loop {
            // Extended-length form so canonicalize resolves a >260-char ancestor
            // (else it fails without LongPathsEnabled and we fall back to a shorter
            // ancestor). Only picks rename-vs-copy, so this never affects correctness.
            if let Ok(canonical) = fs::canonicalize(to_extended(probe).as_ref()) {
                break canonical;
            }
            match probe.parent() {
                Some(parent) => probe = parent,
                None => break path.to_path_buf(),
            }
        };
        match canonical.components().next() {
            Some(std::path::Component::Prefix(prefix)) => {
                Some(prefix.as_os_str().to_string_lossy().to_ascii_uppercase())
            }
            _ => None,
        }
    }
    matches!((volume_key(a), volume_key(b)), (Some(left), Some(right)) if left == right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_path_refuses_an_occupied_destination() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("src.txt");
        let to = dir.path().join("dst.txt");
        fs::write(&from, b"source").unwrap();
        // A file the move would clobber raced into the destination after the caller's
        // earlier exists() check.
        fs::write(&to, b"existing").unwrap();

        let strategy = choose_strategy(&from, &to);
        let result = move_path(&from, &to, strategy);

        assert!(matches!(
            result,
            Err(FsMoveError::DestinationOccupied { .. })
        ));
        // Nothing was overwritten and the source is left intact.
        assert_eq!(fs::read(&to).unwrap(), b"existing");
        assert_eq!(fs::read(&from).unwrap(), b"source");
    }

    #[test]
    fn move_path_moves_to_a_free_destination() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("src.txt");
        let to = dir.path().join("sub").join("dst.txt");
        fs::write(&from, b"payload").unwrap();

        let strategy = choose_strategy(&from, &to);
        let bytes = move_path(&from, &to, strategy).unwrap();

        assert_eq!(bytes, 7);
        assert_eq!(fs::read(&to).unwrap(), b"payload");
        assert!(!from.exists());
    }
}
