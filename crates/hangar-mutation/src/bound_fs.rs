//! Identity-bound filesystem primitives for Gate-3 mutation.
//!
//! Windows is the release target. Every source is opened once with write/delete
//! sharing denied, classified from that handle, hashed through that handle, and
//! renamed or disposed through that same handle. Destination files are created
//! with `CREATE_NEW`. Ancestor directory handles are kept open without delete
//! sharing for the whole operation, so a checked directory cannot be replaced by
//! a junction between proof and mutation.

#[cfg(any(not(windows), test))]
use std::fs;
use std::fs::File;
#[cfg(not(windows))]
use std::fs::OpenOptions;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileStamp {
    pub volume_id: String,
    pub file_id: String,
    pub bytes: u64,
    /// Whole Unix seconds from the bound handle. Legacy journal/manifest rows
    /// did not persist this value, so deserialisation deliberately defaults to
    /// `None`; newly reviewed mutation inputs always carry `Some` and must
    /// match it before any destination-side write.
    #[serde(default)]
    pub modified_unix_seconds: Option<i64>,
}

impl FileStamp {
    pub fn same_object(&self, other: &Self) -> bool {
        self.volume_id == other.volume_id && self.file_id == other.file_id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MoveKind {
    Rename,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RacePoint {
    AfterSourceBoundAndHashed,
    BeforeDestinationCreate,
    AfterDestinationCreated,
    AfterPurgeValidatedBeforeDisposition,
    AfterDirectoryBoundBeforeDisposition,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FaultPoint {
    CreateAfterHandle,
    CopyAfterCreate,
    WriteAfterCreate,
    CancelDeleteOnClose,
}

#[cfg(test)]
type TestHook = std::sync::Arc<dyn Fn(RacePoint, &Path) + Send + Sync + 'static>;

#[cfg(test)]
thread_local! {
    static TEST_HOOK: std::cell::RefCell<Option<TestHook>> = const { std::cell::RefCell::new(None) };
    static TEST_FAULT: std::cell::Cell<Option<FaultPoint>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(crate) fn set_test_hook(hook: Option<TestHook>) {
    TEST_HOOK.with(|slot| *slot.borrow_mut() = hook);
}

#[cfg(test)]
fn set_test_fault(fault: Option<FaultPoint>) {
    TEST_FAULT.with(|slot| slot.set(fault));
}

#[cfg(test)]
pub(crate) fn set_cancel_delete_on_close_fault(enabled: bool) {
    set_test_fault(enabled.then_some(FaultPoint::CancelDeleteOnClose));
}

fn injected_fault(point: RacePoint) -> io::Result<()> {
    #[cfg(test)]
    {
        let expected = match point {
            RacePoint::AfterDestinationCreated => Some(FaultPoint::CopyAfterCreate),
            _ => None,
        };
        if expected.is_some() && TEST_FAULT.with(|slot| slot.get()) == expected {
            return Err(io::Error::other("deterministic post-CREATE_NEW copy fault"));
        }
    }
    let _ = point;
    Ok(())
}

fn injected_write_fault() -> io::Result<()> {
    #[cfg(test)]
    if TEST_FAULT.with(|slot| slot.get()) == Some(FaultPoint::WriteAfterCreate) {
        return Err(io::Error::other(
            "deterministic post-CREATE_NEW write fault",
        ));
    }
    Ok(())
}

fn injected_create_fault() -> io::Result<()> {
    #[cfg(test)]
    if TEST_FAULT.with(|slot| slot.get()) == Some(FaultPoint::CreateAfterHandle) {
        return Err(io::Error::other(
            "deterministic fault after CREATE_NEW returned a handle",
        ));
    }
    Ok(())
}

fn injected_cancel_delete_on_close_fault() -> io::Result<()> {
    #[cfg(test)]
    if TEST_FAULT.with(|slot| slot.get()) == Some(FaultPoint::CancelDeleteOnClose) {
        return Err(io::Error::other(
            "deterministic delete-on-close cancellation fault",
        ));
    }
    Ok(())
}

pub(crate) fn race_hook(point: RacePoint, path: &Path) {
    #[cfg(test)]
    TEST_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow().clone() {
            hook(point, path);
        }
    });
    #[cfg(not(test))]
    {
        let _ = (point, path);
    }
}

#[derive(Debug)]
pub(crate) struct DirectoryGuard {
    stamp: FileStamp,
    #[cfg(windows)]
    handles: Vec<File>,
}

/// Retains both sides of an alias-containment proof for the caller's whole
/// operation. On Windows, none of the reviewed directory ancestors can be
/// renamed/replaced while this value is alive because the handles omit delete
/// sharing.
#[derive(Debug)]
pub(crate) struct ContainmentGuard {
    _source: DirectoryGuard,
    #[cfg(windows)]
    _destination_ancestors: Vec<File>,
    source_volume_id: String,
    destination_volume_id: String,
    available_space_bytes: Option<u64>,
}

impl ContainmentGuard {
    pub(crate) fn same_volume(&self) -> bool {
        self.source_volume_id == self.destination_volume_id
    }

    pub(crate) fn available_space_bytes(&self) -> Option<u64> {
        self.available_space_bytes
    }
}

#[derive(Debug)]
pub(crate) struct BoundFile {
    path: PathBuf,
    file: File,
    stamp: FileStamp,
    _ancestors: DirectoryGuard,
    delete_access: bool,
}

/// Parent-side proof handle for object-archive v2. Unlike `BoundFile`, this
/// deliberately permits named streams: the elevated helper captures and
/// round-trips them before this object can authorize a destructive operation.
/// It still rejects recall/cloud/reparse objects, binds every ancestor, denies
/// write/delete sharing, and verifies the reviewed identity/default stream.
#[derive(Debug)]
pub struct BoundObjectProof {
    path: PathBuf,
    file: File,
    /// Stamp observed on the live handle. For a directory whose children were
    /// removed by this exact batch, its modification time and directory-index
    /// EOF may differ from `authority_stamp`.
    stamp: FileStamp,
    /// Immutable stamp bound to the reviewed row and object_archive/2 proof.
    /// Keeping it separate prevents an internally changed directory mtime from
    /// rewriting the archive authority.
    authority_stamp: FileStamp,
    content_hash: String,
    directory: bool,
    stream_logical_bytes: u64,
    stream_count: u32,
    final_stream_profile_supported: bool,
    internal_directory_time_drift_authorized: bool,
    /// Archive/helper handles retain containment. The final share-zero target
    /// handle deliberately does not: containment is acquired temporarily,
    /// target identity is rebound, then ancestors are released before a parent
    /// directory can itself be opened for final disposition.
    _ancestors: Option<DirectoryGuard>,
    /// Share-zero is tracked independently from `_ancestors`: a helper source
    /// remains share-compatible after its ancestors are deliberately released.
    exclusive: bool,
    #[allow(dead_code)] // exercised by portable/test legacy disposition only
    delete_access: bool,
}

#[derive(Debug)]
pub(crate) struct BoundDirectory {
    path: PathBuf,
    file: File,
    stamp: FileStamp,
    _ancestors: DirectoryGuard,
}

/// Exact Windows disposition mechanism selected for one held-object handle.
///
/// The legacy and extended APIs expose different link counts once disposition
/// is armed, so the mode is part of the proof exchanged with the independent
/// cancellation guardian.  It is process-local evidence only; durable journal
/// rows store [`Self::journal_label`] rather than a raw OS value.
#[cfg(windows)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum WindowsDeleteDispositionMode {
    /// `FileDispositionInfoEx(DELETE | ON_CLOSE)` retains the last pathname
    /// until close, so the armed handle must still report one link.
    ExtendedOnClose,
    /// Legacy `FileDispositionInfo` removes the armed pathname from the live
    /// link count immediately. One safe pre-arm link therefore becomes zero.
    Legacy,
}

#[cfg(windows)]
impl WindowsDeleteDispositionMode {
    pub(crate) fn journal_label(self) -> &'static str {
        match self {
            Self::ExtendedOnClose => "extendedOnClose",
            Self::Legacy => "legacy",
        }
    }

    pub(crate) fn from_journal_label(value: &str) -> Option<Self> {
        match value {
            "extendedOnClose" => Some(Self::ExtendedOnClose),
            "legacy" => Some(Self::Legacy),
            _ => None,
        }
    }
}

/// Medium-integrity proof that the parent can create scratch objects in this
/// exact local directory. The elevated helper duplicates this handle and uses
/// it as NtCreateFile.RootDirectory; it never creates from an absolute scratch
/// pathname.
#[derive(Debug)]
pub struct BoundScratchRoot {
    path: PathBuf,
    file: File,
    stamp: FileStamp,
    _ancestors: DirectoryGuard,
}

#[derive(Debug)]
pub(crate) struct CreatedFile {
    path: PathBuf,
    file: File,
    stamp: FileStamp,
    hash: String,
    delete_on_close_armed: bool,
    _parent: DirectoryGuard,
}

/// Fresh, exact-handle sidecar used only for the guardian's durable close
/// receipt. It starts empty and share-zero. Once the guardian has duplicated
/// the handle, the parent deliberately preserves the pathname across crashes;
/// cleanup is always identity-bound and happens only after terminal commit.
#[cfg(windows)]
#[derive(Debug)]
pub(crate) struct BoundGuardianReceipt {
    created: Option<CreatedFile>,
    cleanup_on_drop: bool,
}

/// Fresh CREATE_NEW container retained by the medium-integrity parent while the
/// helper writes through a duplicated handle. It is deleted through that exact
/// handle on every pre-commit failure.
#[derive(Debug)]
pub struct ObjectArchiveContainer {
    created: Option<CreatedFile>,
}

/// Durable, no-overwrite archive container after parent-side verification and
/// exact-handle promotion from `.partial` to its final name.
#[derive(Debug)]
pub struct CommittedObjectArchive {
    path: PathBuf,
    file: File,
    stamp: FileStamp,
    hash: String,
    _parent: DirectoryGuard,
}

#[derive(Debug)]
pub(crate) struct PreparedMove {
    kind: MoveKind,
    destination_stamp: FileStamp,
    hash: String,
}

impl BoundFile {
    pub(crate) fn open_read(path: &Path) -> io::Result<Self> {
        Self::open(path, false)
    }

    pub(crate) fn open_for_move(path: &Path) -> io::Result<Self> {
        Self::open(path, true)
    }

    fn open(path: &Path, delete_access: bool) -> io::Result<Self> {
        validate_local_mutation_path(path)?;
        let ancestors = guard_existing_parent(path)?;
        let file = platform_open_existing_file(path, delete_access)?;
        let stamp = platform_file_stamp(&file)?;
        Ok(Self {
            path: path.to_path_buf(),
            file,
            stamp,
            _ancestors: ancestors,
            delete_access,
        })
    }

    pub(crate) fn stamp(&self) -> &FileStamp {
        &self.stamp
    }

    pub(crate) fn verify_stamp(&self, expected: &FileStamp) -> io::Result<()> {
        let modified_matches = expected.modified_unix_seconds.is_none()
            || self.stamp.modified_unix_seconds == expected.modified_unix_seconds;
        if self.stamp.same_object(expected)
            && self.stamp.bytes == expected.bytes
            && modified_matches
        {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "file identity changed before mutation: {}",
                    self.path.display()
                ),
            ))
        }
    }

    pub(crate) fn hash(&mut self) -> io::Result<String> {
        hash_open_file(&mut self.file)
    }

    pub(crate) fn read_all(&mut self) -> io::Result<Vec<u8>> {
        use std::io::Read;

        self.file.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        let result = self.file.read_to_end(&mut bytes);
        let _ = self.file.seek(SeekFrom::Start(0));
        result?;
        Ok(bytes)
    }

    pub(crate) fn verify_hash(&mut self, expected: &str) -> io::Result<String> {
        let actual = self.hash()?;
        if actual == expected {
            Ok(actual)
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "file content changed before mutation: {}",
                    self.path.display()
                ),
            ))
        }
    }

    pub(crate) fn prepare_move(
        mut self,
        destination: &Path,
        expected_hash: &str,
    ) -> io::Result<PreparedMove> {
        if !self.delete_access {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the bound source was not opened with delete access",
            ));
        }
        self.verify_hash(expected_hash)?;
        race_hook(RacePoint::AfterSourceBoundAndHashed, &self.path);

        // Gate the destination volume from an already-open ancestor before a
        // missing component is created. Quarantine/restore are metadata-faithful
        // only as a same-volume rename; copy+unlink is disabled until a complete
        // metadata backup/restore format exists.
        let destination_parent =
            guard_or_create_parent_on_volume(destination, &self.stamp.volume_id)?;
        let source_stamp = self.stamp.clone();
        platform_rename_handle_no_replace(
            &self.file,
            &self.path,
            destination,
            &destination_parent,
        )?;
        self.path = destination.to_path_buf();
        self._ancestors = destination_parent;
        let destination_stamp = platform_file_stamp(&self.file)?;
        let destination_hash = self.hash()?;
        if !destination_stamp.same_object(&source_stamp)
            || destination_stamp.bytes != source_stamp.bytes
            || destination_hash != expected_hash
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the renamed handle no longer identifies the reviewed file and bytes",
            ));
        }
        Ok(PreparedMove {
            kind: MoveKind::Rename,
            destination_stamp,
            hash: expected_hash.to_string(),
        })
    }

    pub(crate) fn delete_exact(mut self, expected_hash: &str) -> io::Result<()> {
        if !self.delete_access {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the bound source was not opened with delete access",
            ));
        }
        self.verify_hash(expected_hash)?;
        race_hook(RacePoint::AfterPurgeValidatedBeforeDisposition, &self.path);
        platform_delete_handle(self.file, &self.path, &self.stamp)
    }
}

impl BoundObjectProof {
    /// Bind a regular local file or directory for an object-archive helper
    /// invocation. No filesystem side effect occurs.
    pub fn open_for_archive(
        path: &Path,
        expected_stamp: &FileStamp,
        expected_content_hash: &str,
    ) -> io::Result<Self> {
        Self::open(
            path,
            expected_stamp,
            expected_content_hash,
            false,
            false,
            false,
            false,
        )
    }

    /// Rebind a proof-ready held object for parent-side exact-handle disposal.
    /// Callers must still verify the persisted object_archive/2 semantic proof;
    /// this method alone never grants purge eligibility.
    pub(crate) fn open_for_archive_delete(
        path: &Path,
        expected_stamp: &FileStamp,
        expected_content_hash: &str,
    ) -> io::Result<Self> {
        Self::open(
            path,
            expected_stamp,
            expected_content_hash,
            true,
            false,
            false,
            false,
        )
    }

    /// Rebind an existing-archive directory after this exact batch removed all
    /// of its planned descendants. Only the reviewed directory timestamps and
    /// NTFS directory-index allocation length may drift; FileId, volume, type,
    /// default hash, stream profile and emptiness remain mandatory. File bytes
    /// never receive this exception. Callers separately prove descendant state.
    #[cfg(windows)]
    pub(crate) fn open_for_archive_delete_allow_directory_time_drift(
        path: &Path,
        expected_stamp: &FileStamp,
        expected_content_hash: &str,
    ) -> io::Result<Self> {
        Self::open(
            path,
            expected_stamp,
            expected_content_hash,
            true,
            false,
            true,
            true,
        )
    }

    /// Rebind an existing-archive directory for helper round-trip after earlier
    /// chunks removed its children. This source handle is deliberately
    /// read-only/share-compatible; DELETE access is acquired only by the final
    /// share-zero rebind after the helper returns.
    #[cfg(windows)]
    pub(crate) fn open_for_archive_allow_directory_time_drift(
        path: &Path,
        expected_stamp: &FileStamp,
        expected_content_hash: &str,
    ) -> io::Result<Self> {
        Self::open(
            path,
            expected_stamp,
            expected_content_hash,
            false,
            false,
            true,
            true,
        )
    }

    /// Mark a helper-verified directory as eligible for the narrowly bounded
    /// child-removal mtime exception. The final detached rebind still proves
    /// emptiness and every immutable field; this marker alone grants nothing.
    #[cfg(windows)]
    pub(crate) fn authorize_internal_directory_time_drift(&mut self) -> io::Result<()> {
        if !self.directory {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "internal directory time drift cannot be authorized for a file",
            ));
        }
        self.internal_directory_time_drift_authorized = true;
        Ok(())
    }

    /// Retain the exact share-compatible source handle used by the elevated
    /// helper, but release its containment chain once every destination handle
    /// is bound. This permits the next lazy capability to target a parent
    /// directory without weakening the helper's exact-handle authority.
    #[cfg(windows)]
    pub(crate) fn release_ancestors_for_helper(mut self) -> io::Result<Self> {
        if self.exclusive || self.delete_access {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "final-delete target cannot be converted back to a helper source",
            ));
        }
        drop(self._ancestors.take());
        Ok(self)
    }

    /// Rebind a held object with no sharing while retaining only its target
    /// handle. Containment is reacquired before the pathname is reopened, the
    /// exact object is revalidated, and the ancestor handles are released before
    /// this value is returned. The final-delete stage uses this only after the
    /// helper has finished with every share-compatible source handle, so a
    /// child never retains an ancestor while a parent target is rebound.
    #[cfg(windows)]
    pub(crate) fn detach_exclusive_target(self) -> io::Result<Self> {
        if self.exclusive {
            return Ok(self);
        }
        let path = self.path.clone();
        let authority_stamp = self.authority_stamp.clone();
        let content_hash = self.content_hash.clone();
        let allow_directory_time_drift = self.internal_directory_time_drift_authorized;
        drop(self);
        Self::open(
            &path,
            &authority_stamp,
            &content_hash,
            true,
            true,
            allow_directory_time_drift,
            false,
        )
    }

    /// Revalidate the already-detached exact target immediately before arming
    /// disposition. No pathname is reopened in the normal helper path. This is
    /// also where a containing directory must prove it is empty after all of
    /// this batch's planned descendants reached a terminal deleted state.
    #[cfg(windows)]
    pub(crate) fn detach_exclusive_for_final_disposition(self) -> io::Result<Self> {
        let mut proof = self.detach_exclusive_target()?;
        let current = platform_file_stamp(&proof.file)?;
        let modified_matches = proof.authority_stamp.modified_unix_seconds.is_none()
            || current.modified_unix_seconds == proof.authority_stamp.modified_unix_seconds;
        let directory_time_drift_allowed =
            proof.internal_directory_time_drift_authorized && proof.directory;
        if !current.same_object(&proof.authority_stamp)
            || (!directory_time_drift_allowed
                && (current.bytes != proof.authority_stamp.bytes || !modified_matches))
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "detached final target no longer matches its immutable archive authority",
            ));
        }
        let content_hash = if proof.directory {
            blake3::hash(&[]).to_hex().to_string()
        } else {
            hash_open_file(&mut proof.file.try_clone()?)?
        };
        if content_hash != proof.content_hash {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "detached final target default stream changed after helper verification",
            ));
        }
        let stream_inventory = platform_archive_stream_inventory(&proof.file)?;
        if !stream_inventory.matches_final_profile(proof.directory) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "detached final target has a named, non-default, or unproved stream profile",
            ));
        }
        if proof.directory {
            require_empty_bound_directory(&proof.file, &proof.path)?;
        }
        proof.stamp = current;
        proof.stream_logical_bytes = stream_inventory.logical_bytes;
        proof.stream_count = stream_inventory.count;
        proof.final_stream_profile_supported = true;
        Ok(proof)
    }

    fn open(
        path: &Path,
        expected_stamp: &FileStamp,
        expected_content_hash: &str,
        delete_access: bool,
        exclusive: bool,
        allow_directory_time_drift: bool,
        require_empty_directory: bool,
    ) -> io::Result<Self> {
        validate_local_mutation_path(path)?;
        let ancestors = guard_existing_parent(path)?;
        let (file, directory) = platform_open_archive_object(path, delete_access, exclusive)?;
        let stamp = platform_file_stamp(&file)?;
        let modified_matches = expected_stamp.modified_unix_seconds.is_none()
            || stamp.modified_unix_seconds == expected_stamp.modified_unix_seconds;
        let directory_time_drift_allowed = allow_directory_time_drift && directory;
        if !stamp.same_object(expected_stamp)
            || (!directory_time_drift_allowed
                && (stamp.bytes != expected_stamp.bytes || !modified_matches))
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "archive source identity changed before capability creation: {}",
                    path.display()
                ),
            ));
        }
        let content_hash = if directory {
            blake3::hash(&[]).to_hex().to_string()
        } else {
            hash_open_file(&mut file.try_clone()?)?
        };
        if content_hash != expected_content_hash {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "archive source default stream changed before capability creation: {}",
                    path.display()
                ),
            ));
        }
        let stream_inventory = platform_archive_stream_inventory(&file)?;
        let final_stream_profile_supported = stream_inventory.matches_final_profile(directory);
        if require_empty_directory {
            if !directory {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "directory-time drift authority resolved to a file",
                ));
            }
            if !final_stream_profile_supported {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "directory-time drift authority refuses named or non-default streams",
                ));
            }
            require_empty_bound_directory(&file, path)?;
        }
        let (stream_logical_bytes, stream_count) =
            (stream_inventory.logical_bytes, stream_inventory.count);
        let retained_ancestors = if exclusive {
            // The share-zero target handle now pins the exact FileId. Releasing
            // containment is intentional so a child proof never prevents its
            // parent directory from acquiring its own final target handle.
            drop(ancestors);
            None
        } else {
            Some(ancestors)
        };
        Ok(Self {
            path: path.to_path_buf(),
            file,
            authority_stamp: expected_stamp.clone(),
            stamp,
            content_hash,
            directory,
            stream_logical_bytes,
            stream_count,
            final_stream_profile_supported,
            internal_directory_time_drift_authorized: allow_directory_time_drift,
            _ancestors: retained_ancestors,
            exclusive,
            delete_access,
        })
    }

    pub fn stamp(&self) -> &FileStamp {
        &self.stamp
    }

    pub(crate) fn authority_stamp(&self) -> &FileStamp {
        &self.authority_stamp
    }

    pub fn content_hash(&self) -> &str {
        &self.content_hash
    }

    pub fn is_directory(&self) -> bool {
        self.directory
    }

    pub fn stream_logical_bytes(&self) -> u64 {
        self.stream_logical_bytes
    }

    pub fn stream_count(&self) -> u32 {
        self.stream_count
    }

    pub(crate) fn matches_final_stream_profile(&self) -> bool {
        self.final_stream_profile_supported
    }

    #[cfg(windows)]
    pub fn raw_handle_value(&self) -> u64 {
        use std::os::windows::io::AsRawHandle;
        self.file.as_raw_handle() as usize as u64
    }

    #[cfg(not(windows))]
    pub fn raw_handle_value(&self) -> u64 {
        0
    }

    #[allow(dead_code)] // portable/test compatibility; Windows production is guardian-mediated
    pub(crate) fn delete_exact(self) -> io::Result<()> {
        if !self.delete_access {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "archive source proof was not opened with delete access",
            ));
        }
        race_hook(RacePoint::AfterPurgeValidatedBeforeDisposition, &self.path);
        #[cfg(windows)]
        {
            platform_dispose_final_object_handle(self.file, &self.stamp)
        }
        #[cfg(not(windows))]
        if self.directory {
            platform_delete_directory_handle(self.file, &self.path, &self.stamp)
        } else {
            platform_delete_handle(self.file, &self.path, &self.stamp)
        }
    }

    /// Validate the exact exclusive handle before a separately authenticated
    /// guardian is allowed to observe an arm attempt.
    #[cfg(windows)]
    pub(crate) fn validate_final_disposition_prearm(&self) -> io::Result<()> {
        validate_final_disposition_profile(&self.file, false, None)
    }

    /// Arm delete-on-close without closing the handle.  The caller must already
    /// have a guardian-bound durable intent and must either prove the profile
    /// and explicitly close, or cancel/retain both handles.
    #[cfg(windows)]
    pub(crate) fn arm_final_disposition(&self) -> io::Result<WindowsDeleteDispositionMode> {
        platform_arm_delete_on_close(&self.file)
    }

    #[cfg(all(test, windows))]
    pub(crate) fn arm_legacy_final_disposition_for_test(
        &self,
    ) -> io::Result<WindowsDeleteDispositionMode> {
        platform_arm_legacy_delete_on_close_for_test(&self.file)
    }

    #[cfg(windows)]
    pub(crate) fn validate_armed_final_disposition(
        &self,
        mode: WindowsDeleteDispositionMode,
    ) -> io::Result<()> {
        validate_final_disposition_profile(&self.file, true, Some(mode))
    }

    #[cfg(windows)]
    pub(crate) fn cancel_final_disposition(
        &self,
        mode: WindowsDeleteDispositionMode,
    ) -> io::Result<()> {
        platform_cancel_delete_on_close_mode(&self.file, mode)
    }

    /// Cancel when the kernel may have accepted an arm but the mode query did
    /// not complete. This is the parent-side counterpart to the guardian's
    /// mode-agnostic crash path.
    #[cfg(windows)]
    pub(crate) fn cancel_final_disposition_unknown_mode(&self) -> io::Result<()> {
        guardian_cancel_delete_on_close(&self.file, None)
    }

    /// Close the final parent handle only after the guardian independently
    /// proved the same profile and acknowledged durable close authorization.
    #[cfg(windows)]
    pub(crate) fn close_proved_final_disposition(self) {
        drop(self);
    }

    /// Last-resort in-process retention when neither peer can prove
    /// cancellation.  The independently running guardian remains the durable
    /// parent-death safety boundary; this leak merely avoids making the current
    /// process the first closer.
    #[cfg(windows)]
    pub(crate) fn retain_unproved_final_disposition(self) {
        std::mem::forget(self);
    }
}

impl BoundDirectory {
    pub(crate) fn open_for_delete(path: &Path) -> io::Result<Self> {
        validate_local_mutation_path(path)?;
        let ancestors = guard_existing_parent(path)?;
        let file = platform_open_existing_directory_for_delete(path)?;
        let stamp = platform_file_stamp(&file)?;
        Ok(Self {
            path: path.to_path_buf(),
            file,
            stamp,
            _ancestors: ancestors,
        })
    }

    pub(crate) fn delete_exact(self) -> io::Result<()> {
        race_hook(RacePoint::AfterDirectoryBoundBeforeDisposition, &self.path);
        platform_delete_directory_handle(self.file, &self.path, &self.stamp)
    }
}

impl BoundScratchRoot {
    pub fn open(path: &Path) -> io::Result<Self> {
        validate_local_mutation_path(path)?;
        let ancestors = guard_existing_parent(path)?;
        let file = platform_open_scratch_root(path)?;
        let stamp = platform_file_stamp(&file)?;
        Ok(Self {
            path: path.to_path_buf(),
            file,
            stamp,
            _ancestors: ancestors,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn stamp(&self) -> &FileStamp {
        &self.stamp
    }

    #[cfg(windows)]
    pub fn raw_handle_value(&self) -> u64 {
        use std::os::windows::io::AsRawHandle;
        self.file.as_raw_handle() as usize as u64
    }

    #[cfg(not(windows))]
    pub fn raw_handle_value(&self) -> u64 {
        0
    }
}

pub fn inspect_local_mutation_file(path: &Path) -> io::Result<(FileStamp, String)> {
    let mut file = BoundFile::open_read(path)?;
    let stamp = file.stamp().clone();
    let hash = file.hash()?;
    Ok((stamp, hash))
}

#[cfg(all(test, windows))]
pub(crate) fn inspect_local_mutation_object_for_test(
    path: &Path,
) -> io::Result<(FileStamp, String, bool)> {
    let (mut file, directory) = platform_open_archive_object(path, false, false)?;
    let stamp = platform_file_stamp(&file)?;
    let hash = if directory {
        blake3::hash(&[]).to_hex().to_string()
    } else {
        hash_open_file(&mut file)?
    };
    Ok((stamp, hash, directory))
}

pub(crate) fn bind_destination_outside_directory(
    destination_root: &Path,
    source_root: &Path,
) -> io::Result<ContainmentGuard> {
    validate_local_mutation_path(destination_root)?;
    validate_local_mutation_path(source_root)?;
    let source = guard_directory_chain(source_root, false, None)?;

    #[cfg(windows)]
    {
        let mut destination_ancestors = Vec::new();
        let mut destination_stamp = None;
        let mut available_space_bytes = None;
        for component in directory_chain_components(destination_root)? {
            let handle = match platform_open_directory(&component) {
                Ok(handle) => handle,
                Err(error) if error.kind() == io::ErrorKind::NotFound => break,
                Err(error) => return Err(error),
            };
            let stamp = platform_file_stamp(&handle)?;
            if stamp.same_object(&source.stamp) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "backup/holding destination aliases the reviewed source directory",
                ));
            }
            available_space_bytes = hangar_fs::available_space_bytes(&component);
            destination_stamp = Some(stamp);
            destination_ancestors.push(handle);
        }
        let destination_stamp = destination_stamp.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "cannot bind a destination-volume ancestor",
            )
        })?;
        Ok(ContainmentGuard {
            source_volume_id: source.stamp.volume_id.clone(),
            destination_volume_id: destination_stamp.volume_id,
            available_space_bytes,
            _source: source,
            _destination_ancestors: destination_ancestors,
        })
    }
    #[cfg(not(windows))]
    {
        let mut destination_stamp = None;
        for component in directory_chain_components(destination_root)? {
            let metadata = match fs::symlink_metadata(&component) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => break,
                Err(error) => return Err(error),
            };
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "backup/holding destination ancestor is not a plain directory",
                ));
            }
            let stamp = fallback_stamp(&metadata);
            if stamp.same_object(&source.stamp) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "backup/holding destination aliases the reviewed source directory",
                ));
            }
            destination_stamp = Some(stamp);
        }
        let destination_stamp = destination_stamp.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "cannot bind a destination-volume ancestor",
            )
        })?;
        Ok(ContainmentGuard {
            source_volume_id: source.stamp.volume_id.clone(),
            destination_volume_id: destination_stamp.volume_id,
            available_space_bytes: None,
            _source: source,
        })
    }
}

impl CreatedFile {
    pub(crate) fn stamp(&self) -> &FileStamp {
        &self.stamp
    }

    pub(crate) fn hash(&self) -> &str {
        &self.hash
    }

    fn delete_exact(self) -> io::Result<()> {
        platform_delete_handle(self.file, &self.path, &self.stamp)
    }

    fn cancel_delete_on_close(&mut self) -> io::Result<()> {
        if self.delete_on_close_armed {
            platform_cancel_delete_on_close(&self.file)?;
            self.delete_on_close_armed = false;
        }
        Ok(())
    }
}

#[cfg(windows)]
impl BoundGuardianReceipt {
    pub(crate) fn create_new(path: &Path) -> io::Result<Self> {
        validate_local_mutation_path(path)?;
        let parent = guard_existing_parent(path)?;
        let created = create_new_bound_file(path, parent, false)?;
        Ok(Self {
            created: Some(created),
            cleanup_on_drop: true,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self
            .created
            .as_ref()
            .expect("guardian receipt exists until exact cleanup")
            .path
    }

    pub(crate) fn initial_stamp(&self) -> &FileStamp {
        &self
            .created
            .as_ref()
            .expect("guardian receipt exists until exact cleanup")
            .stamp
    }

    pub(crate) fn raw_handle_value(&self) -> u64 {
        use std::os::windows::io::AsRawHandle as _;
        self.created
            .as_ref()
            .expect("guardian receipt exists until exact cleanup")
            .file
            .as_raw_handle() as usize as u64
    }

    #[cfg(test)]
    pub(crate) fn file_for_test(&self) -> &File {
        &self
            .created
            .as_ref()
            .expect("guardian receipt exists until exact cleanup")
            .file
    }

    /// From this point the sidecar is recovery evidence, so ordinary Rust
    /// unwinding must not erase it. Process termination also leaves it intact.
    pub(crate) fn preserve_for_recovery(&mut self) {
        self.cleanup_on_drop = false;
    }

    pub(crate) fn cleanup_exact(mut self) -> io::Result<()> {
        self.cleanup_on_drop = false;
        self.created
            .take()
            .expect("guardian receipt exists until exact cleanup")
            .delete_exact()
    }
}

#[cfg(windows)]
impl Drop for BoundGuardianReceipt {
    fn drop(&mut self) {
        if !self.cleanup_on_drop {
            return;
        }
        if let Some(created) = self.created.take() {
            let _ = created.delete_exact();
        }
    }
}

/// Rebind and read an exact guardian receipt after both parent and guardian
/// processes are gone. Path substitution, reparse points, ADS, delete-pending
/// state and oversized/torn contents all fail closed.
#[cfg(windows)]
pub(crate) fn read_exact_guardian_receipt(
    path: &Path,
    expected_identity: &FileStamp,
    max_bytes: usize,
) -> io::Result<Vec<u8>> {
    validate_local_mutation_path(path)?;
    let _parent = guard_existing_parent(path)?;
    let mut file = platform_open_existing_file(path, false)?;
    let current = platform_file_stamp(&file)?;
    if !current.same_object(expected_identity) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "guardian receipt pathname no longer resolves to its pre-bound FileId",
        ));
    }
    let length = usize::try_from(current.bytes).map_err(|_| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "guardian receipt length exceeds the platform address range",
        )
    })?;
    if length == 0 || length > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "guardian receipt is empty, torn, or exceeds its bounded format",
        ));
    }
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::with_capacity(length);
    file.take((max_bytes + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() != length || bytes.len() > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "guardian receipt changed while its exact handle was being read",
        ));
    }
    Ok(bytes)
}

#[cfg(windows)]
pub(crate) fn cleanup_exact_guardian_receipt(
    path: &Path,
    expected_identity: &FileStamp,
) -> io::Result<()> {
    validate_local_mutation_path(path)?;
    let _parent = guard_existing_parent(path)?;
    let file = platform_open_existing_file(path, true)?;
    let current = platform_file_stamp(&file)?;
    if !current.same_object(expected_identity) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "guardian receipt cleanup pathname no longer resolves to its pre-bound FileId",
        ));
    }
    platform_delete_handle(file, path, expected_identity)
}

impl ObjectArchiveContainer {
    pub fn create_new(path: &Path) -> io::Result<Self> {
        validate_local_mutation_path(path)?;
        let parent = guard_or_create_parent(path)?;
        // The partial archive is crash-disposable from the instant CREATE_NEW
        // succeeds. Only a fully verified, exact-handle promotion may disarm it.
        let created = create_new_bound_file(path, parent, true)?;
        Ok(Self {
            created: Some(created),
        })
    }

    pub fn path(&self) -> &Path {
        &self
            .created
            .as_ref()
            .expect("archive container is present before commit")
            .path
    }

    pub fn initial_stamp(&self) -> &FileStamp {
        &self
            .created
            .as_ref()
            .expect("archive container is present before commit")
            .stamp
    }

    #[cfg(windows)]
    pub fn raw_handle_value(&self) -> u64 {
        use std::os::windows::io::AsRawHandle;
        self.created
            .as_ref()
            .expect("archive container is present before commit")
            .file
            .as_raw_handle() as usize as u64
    }

    #[cfg(not(windows))]
    pub fn raw_handle_value(&self) -> u64 {
        0
    }

    /// Independently re-hash/rebind the helper result, then promote it by its
    /// exact handle without replacement. The final path is never probed first.
    pub fn verify_and_commit(
        mut self,
        final_path: &Path,
        expected_stamp: &FileStamp,
        expected_hash: &str,
    ) -> io::Result<CommittedObjectArchive> {
        validate_local_mutation_path(final_path)?;
        let mut created = self
            .created
            .take()
            .expect("archive container can only be committed once");
        if let Err(error) = created.file.sync_all() {
            let _ = created.delete_exact();
            return Err(error);
        }
        let current_stamp = match platform_file_stamp(&created.file) {
            Ok(stamp) => stamp,
            Err(error) => {
                let _ = created.delete_exact();
                return Err(error);
            }
        };
        if !current_stamp.same_object(&created.stamp)
            || !current_stamp.same_object(expected_stamp)
            || current_stamp.bytes != expected_stamp.bytes
            || expected_stamp.modified_unix_seconds.is_some()
                && current_stamp.modified_unix_seconds != expected_stamp.modified_unix_seconds
        {
            let _ = created.delete_exact();
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "helper archive result does not match the parent CREATE_NEW object",
            ));
        }
        let hash = match hash_open_file(&mut created.file) {
            Ok(hash) => hash,
            Err(error) => {
                let _ = created.delete_exact();
                return Err(error);
            }
        };
        if hash != expected_hash {
            let _ = created.delete_exact();
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "helper archive result does not match its authenticated digest",
            ));
        }
        let final_parent =
            match guard_or_create_parent_on_volume(final_path, &current_stamp.volume_id) {
                Ok(parent) => parent,
                Err(error) => {
                    let _ = created.delete_exact();
                    return Err(error);
                }
            };
        // Windows refuses to rename a delete-pending file. Disarm only after
        // the bytes, identity, digest and destination parent are all proved.
        // The caller journals this deterministic partial path/stamp before
        // invoking the helper, so the narrow cancel-to-rename crash boundary is
        // recoverable without probing an attacker-selected pathname.
        if let Err(error) = created.cancel_delete_on_close() {
            let _ = created.delete_exact();
            return Err(error);
        }
        if let Err(error) = platform_rename_handle_no_replace(
            &created.file,
            &created.path,
            final_path,
            &final_parent,
        ) {
            let _ = created.delete_exact();
            return Err(error);
        }
        let promoted_stamp = match platform_file_stamp(&created.file) {
            Ok(stamp) => stamp,
            Err(error) => {
                let _ = created.delete_exact();
                return Err(error);
            }
        };
        if !promoted_stamp.same_object(&current_stamp)
            || promoted_stamp.bytes != current_stamp.bytes
        {
            let _ = created.delete_exact();
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "promoted archive no longer identifies the verified CREATE_NEW object",
            ));
        }
        Ok(CommittedObjectArchive {
            path: final_path.to_path_buf(),
            file: created.file,
            stamp: promoted_stamp,
            hash,
            _parent: final_parent,
        })
    }
}

impl Drop for ObjectArchiveContainer {
    fn drop(&mut self) {
        if let Some(created) = self.created.take() {
            let _ = created.delete_exact();
        }
    }
}

impl CommittedObjectArchive {
    /// Rebind an already committed archive payload without a pathname probe.
    /// The caller supplies the immutable journal identity and digest; opening a
    /// same-named replacement never succeeds as the expected archive.
    pub fn open_existing(
        path: &Path,
        expected_stamp: &FileStamp,
        expected_hash: &str,
    ) -> io::Result<Self> {
        validate_local_mutation_path(path)?;
        let parent = guard_existing_parent(path)?;
        let mut file = platform_open_existing_file(path, false)?;
        let stamp = platform_file_stamp(&file)?;
        let modified_matches = expected_stamp.modified_unix_seconds.is_none()
            || stamp.modified_unix_seconds == expected_stamp.modified_unix_seconds;
        if !stamp.same_object(expected_stamp)
            || stamp.bytes != expected_stamp.bytes
            || !modified_matches
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "committed object archive identity no longer matches its journal proof",
            ));
        }
        let hash = hash_open_file(&mut file)?;
        if hash != expected_hash {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "committed object archive digest no longer matches its journal proof",
            ));
        }
        Ok(Self {
            path: path.to_path_buf(),
            file,
            stamp,
            hash,
            _parent: parent,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn stamp(&self) -> &FileStamp {
        &self.stamp
    }

    pub fn hash(&self) -> &str {
        &self.hash
    }

    #[cfg(windows)]
    pub fn raw_handle_value(&self) -> u64 {
        use std::os::windows::io::AsRawHandle;
        self.file.as_raw_handle() as usize as u64
    }

    #[cfg(not(windows))]
    pub fn raw_handle_value(&self) -> u64 {
        0
    }
}

impl PreparedMove {
    pub(crate) fn kind(&self) -> MoveKind {
        self.kind
    }

    pub(crate) fn destination_stamp(&self) -> &FileStamp {
        &self.destination_stamp
    }

    pub(crate) fn hash(&self) -> &str {
        &self.hash
    }

    pub(crate) fn bytes(&self) -> u64 {
        self.destination_stamp.bytes
    }

    pub(crate) fn finalize_source_removal(&mut self) -> io::Result<()> {
        // The legacy content-only copy+unlink route is intentionally absent.
        // Metadata-faithful cross-volume movement is authorized only through
        // object_archive/2; this primitive currently returns rename-only moves.
        debug_assert_eq!(self.kind, MoveKind::Rename);
        Ok(())
    }
}

pub(crate) fn copy_to_new(
    source: &mut BoundFile,
    destination: &Path,
    expected_hash: &str,
) -> io::Result<CreatedFile> {
    source.verify_hash(expected_hash)?;
    race_hook(RacePoint::AfterSourceBoundAndHashed, &source.path);
    let parent = guard_or_create_parent(destination)?;
    let destination_file = create_new_bound_file(destination, parent, false)?;
    copy_bound_file(source, destination_file, expected_hash)
}

pub(crate) fn write_new(destination: &Path, bytes: &[u8]) -> io::Result<FileStamp> {
    let parent = guard_or_create_parent(destination)?;
    let mut created = create_new_bound_file(destination, parent, false)?;
    let result = (|| {
        created.file.write_all(bytes)?;
        injected_write_fault()?;
        created.file.sync_all()?;
        created.stamp = platform_file_stamp(&created.file)?;
        created.hash = hash_open_file(&mut created.file)?;
        Ok(created.stamp.clone())
    })();
    match result {
        Ok(stamp) => Ok(stamp),
        Err(error) => {
            let _ = created.delete_exact();
            Err(error)
        }
    }
}

pub(crate) fn path_matches(
    path: &Path,
    expected_stamp: &FileStamp,
    expected_hash: &str,
) -> io::Result<bool> {
    let mut bound = match BoundFile::open_read(path) {
        Ok(bound) => bound,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let modified_matches = expected_stamp.modified_unix_seconds.is_none()
        || bound.stamp.modified_unix_seconds == expected_stamp.modified_unix_seconds;
    if !bound.stamp.same_object(expected_stamp)
        || bound.stamp.bytes != expected_stamp.bytes
        || !modified_matches
    {
        return Ok(false);
    }
    Ok(bound.hash()? == expected_hash)
}

fn copy_bound_file(
    source: &mut BoundFile,
    mut destination: CreatedFile,
    expected_hash: &str,
) -> io::Result<CreatedFile> {
    let result = (|| {
        source.file.seek(SeekFrom::Start(0))?;
        destination.file.seek(SeekFrom::Start(0))?;
        io::copy(&mut source.file, &mut destination.file)?;
        injected_fault(RacePoint::AfterDestinationCreated)?;
        destination.file.sync_all()?;
        race_hook(RacePoint::AfterDestinationCreated, &destination.path);
        let destination_hash = hash_open_file(&mut destination.file)?;
        let source_hash_after = source.hash()?;
        if destination_hash != expected_hash || source_hash_after != expected_hash {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "source or destination content changed during the bound copy",
            ));
        }
        destination.stamp = platform_file_stamp(&destination.file)?;
        destination.hash = destination_hash;
        Ok(())
    })();
    match result {
        Ok(()) => Ok(destination),
        Err(error) => {
            let _ = destination.delete_exact();
            Err(error)
        }
    }
}

fn hash_open_file(file: &mut File) -> io::Result<String> {
    file.seek(SeekFrom::Start(0))?;
    let mut hasher = blake3::Hasher::new();
    let result = io::copy(file, &mut hasher);
    let _ = file.seek(SeekFrom::Start(0));
    result?;
    Ok(hasher.finalize().to_hex().to_string())
}

pub fn validate_local_mutation_path(path: &Path) -> io::Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "mutation paths must be absolute and cannot contain parent traversal",
        ));
    }
    #[cfg(windows)]
    {
        let path_text = path.as_os_str().to_string_lossy();
        let local = if let Some(local) = path_text.strip_prefix(r"\\?\") {
            let bytes = local.as_bytes();
            let is_verbatim_local_disk = bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], b'\\' | b'/');
            if !is_verbatim_local_disk {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "mutation refuses non-local verbatim namespaces",
                ));
            }
            local
        } else if path_text.starts_with(r"\\") {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "mutation refuses UNC and device namespaces",
            ));
        } else {
            path_text.as_ref()
        };
        // A colon is legal only in the leading drive designator. Any later
        // colon selects an NTFS alternate data stream and must be rejected
        // before the path is opened, created, queried, or canonicalised.
        if local
            .as_bytes()
            .get(2..)
            .is_some_and(|tail| tail.contains(&b':'))
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "mutation refuses alternate-data-stream path syntax",
            ));
        }
        validate_windows_local_drive(local)?;
    }
    Ok(())
}

#[cfg(windows)]
fn validate_windows_local_drive(local_path: &str) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;

    let bytes = local_path.as_bytes();
    if bytes.len() < 3
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || !matches!(bytes[2], b'\\' | b'/')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "mutation requires an absolute local drive path",
        ));
    }
    let root = format!("{}:\\", bytes[0] as char);
    let wide = std::ffi::OsStr::new(&root)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let drive_type = unsafe { GetDriveTypeW(wide.as_ptr()) };
    if !windows_drive_type_is_local(drive_type) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "mutation refuses mapped network, remote, optical, or unknown drives",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn windows_drive_type_is_local(drive_type: u32) -> bool {
    use windows_sys::Win32::System::WindowsProgramming::{
        DRIVE_FIXED, DRIVE_RAMDISK, DRIVE_REMOVABLE,
    };

    matches!(drive_type, DRIVE_FIXED | DRIVE_REMOVABLE | DRIVE_RAMDISK)
}

pub(crate) fn guard_existing_parent(path: &Path) -> io::Result<DirectoryGuard> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "mutation path has no parent")
    })?;
    guard_directory_chain(parent, false, None)
}

fn guard_or_create_parent(path: &Path) -> io::Result<DirectoryGuard> {
    validate_local_mutation_path(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "destination has no parent"))?;
    guard_directory_chain(parent, true, None)
}

fn guard_or_create_parent_on_volume(
    path: &Path,
    expected_volume: &str,
) -> io::Result<DirectoryGuard> {
    validate_local_mutation_path(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "destination has no parent"))?;
    guard_directory_chain(parent, true, Some(expected_volume))
}

fn directory_chain_components(path: &Path) -> io::Result<Vec<PathBuf>> {
    let mut components = Vec::new();
    let mut cursor = path;
    loop {
        if !cursor.as_os_str().is_empty() {
            components.push(cursor.to_path_buf());
        }
        match cursor.parent() {
            Some(parent) if parent != cursor && !parent.as_os_str().is_empty() => cursor = parent,
            _ => break,
        }
    }
    components.reverse();
    if components.len() > 256 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "mutation path exceeds the bounded ancestor depth",
        ));
    }
    Ok(components)
}

#[cfg(windows)]
fn guard_directory_chain(
    path: &Path,
    create: bool,
    required_volume: Option<&str>,
) -> io::Result<DirectoryGuard> {
    let components = directory_chain_components(path)?;

    let mut handles = Vec::with_capacity(components.len());
    let mut volume_id = None;
    let mut last_stamp = None;
    for component in components {
        let handle = match platform_open_directory(&component) {
            Ok(handle) => handle,
            Err(error) if create && error.kind() == io::ErrorKind::NotFound => {
                match platform_create_directory(&component) {
                    Ok(()) => {}
                    Err(create_error) if create_error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(create_error) => return Err(create_error),
                }
                platform_open_directory(&component)?
            }
            Err(error) => return Err(error),
        };
        let stamp = platform_file_stamp(&handle)?;
        if required_volume.is_some_and(|required| required != stamp.volume_id.as_str()) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "cross-volume quarantine/restore is disabled until metadata-complete backup exists",
            ));
        }
        if let Some(expected_volume) = &volume_id {
            if expected_volume != &stamp.volume_id {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "mutation ancestor chain crosses a mount or redirected volume",
                ));
            }
        } else {
            volume_id = Some(stamp.volume_id.clone());
        }
        last_stamp = Some(stamp);
        handles.push(handle);
    }
    Ok(DirectoryGuard {
        stamp: last_stamp.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "empty mutation ancestor chain")
        })?,
        handles,
    })
}

#[cfg(windows)]
fn platform_create_directory(path: &Path) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;

    let wide = wide_path(path);
    let created = unsafe { CreateDirectoryW(wide.as_ptr(), std::ptr::null()) };
    if created == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn guard_directory_chain(
    path: &Path,
    create: bool,
    required_volume: Option<&str>,
) -> io::Result<DirectoryGuard> {
    // Prove the nearest existing ancestor is on the source volume before
    // create_dir_all can create a destination component. This is the portable
    // fallback for the Windows handle-chain implementation above.
    if let Some(required) = required_volume {
        let mut existing = path;
        loop {
            match fs::symlink_metadata(existing) {
                Ok(metadata) => {
                    if metadata.file_type().is_symlink() || !metadata.is_dir() {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "mutation destination ancestor is not a plain directory",
                        ));
                    }
                    if fallback_stamp(&metadata).volume_id != required {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "cross-volume quarantine/restore is disabled until metadata-complete backup exists",
                        ));
                    }
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    existing = existing.parent().ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidInput,
                            "destination has no existing local ancestor",
                        )
                    })?;
                }
                Err(error) => return Err(error),
            }
        }
    }
    if create {
        fs::create_dir_all(path)?;
    }
    let mut cursor = Some(path);
    while let Some(current) = cursor {
        let metadata = fs::symlink_metadata(current)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "mutation ancestor is not a plain directory",
            ));
        }
        cursor = current
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty());
    }
    let metadata = fs::metadata(path)?;
    let stamp = fallback_stamp(&metadata);
    Ok(DirectoryGuard { stamp })
}

#[cfg(windows)]
fn platform_open_directory(path: &Path) -> io::Result<File> {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{GENERIC_READ, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_NO_RECALL,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    let wide = wide_path(path);
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_OPEN_NO_RECALL,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_handle(handle as _) };
    validate_windows_handle(&file, true, false)?;
    Ok(file)
}

#[cfg(windows)]
fn platform_open_existing_directory_for_delete(path: &Path) -> io::Result<File> {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{GENERIC_READ, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, DELETE, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_NO_RECALL,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, OPEN_EXISTING,
    };

    let wide = wide_path(path);
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | DELETE,
            FILE_SHARE_READ,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_OPEN_NO_RECALL,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_handle(handle as _) };
    validate_windows_handle(&file, true, false)?;
    Ok(file)
}

#[cfg(windows)]
fn platform_open_scratch_root(path: &Path) -> io::Result<File> {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FileStandardInfo, GetFileInformationByHandleEx, FILE_ADD_FILE,
        FILE_ADD_SUBDIRECTORY, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_NO_RECALL,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY, FILE_SHARE_READ, FILE_STANDARD_INFO,
        OPEN_EXISTING, READ_CONTROL,
    };

    let wide = wide_path(path);
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_LIST_DIRECTORY | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY | READ_CONTROL,
            FILE_SHARE_READ,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_OPEN_NO_RECALL,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "parent cannot bind writable scratch root: {}",
                io::Error::last_os_error()
            ),
        ));
    }
    let file = unsafe { File::from_raw_handle(handle as _) };
    // Attribute/reparse/cloud checks are identical to the legacy directory
    // guard; stream inventory is irrelevant because this root is not archived.
    let mut standard = FILE_STANDARD_INFO::default();
    let standard_ok = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileStandardInfo,
            (&mut standard as *mut FILE_STANDARD_INFO).cast(),
            std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
        )
    };
    if standard_ok == 0 || !standard.Directory || standard.DeletePending {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "scratch root handle is not a stable directory",
        ));
    }
    validate_windows_archive_root_attributes(&file)?;
    Ok(file)
}

#[cfg(not(windows))]
fn platform_open_scratch_root(path: &Path) -> io::Result<File> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "scratch root is not a plain directory",
        ));
    }
    OpenOptions::new().read(true).open(path)
}

#[cfg(windows)]
fn validate_windows_archive_root_attributes(file: &File) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileAttributeTagInfo, GetFileInformationByHandleEx, FILE_ATTRIBUTE_TAG_INFO,
    };
    let mut attributes = FILE_ATTRIBUTE_TAG_INFO::default();
    let ok = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle() as _,
            FileAttributeTagInfo,
            (&mut attributes as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    };
    if ok == 0
        || !windows_handle_attributes_are_local(attributes.FileAttributes, attributes.ReparseTag)
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "scratch root is reparse, offline, recall-on-access, cloud-backed, or ambiguous",
        ));
    }
    Ok(())
}

#[cfg(not(windows))]
fn platform_open_existing_directory_for_delete(path: &Path) -> io::Result<File> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "mutation cleanup target is not a plain directory",
        ));
    }
    OpenOptions::new().read(true).open(path)
}

#[cfg(windows)]
fn platform_open_existing_file(path: &Path, delete_access: bool) -> io::Result<File> {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{GENERIC_READ, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, DELETE, FILE_FLAG_OPEN_NO_RECALL, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_SHARE_READ, OPEN_EXISTING,
    };

    let wide = wide_path(path);
    let access = GENERIC_READ | if delete_access { DELETE } else { 0 };
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            access,
            FILE_SHARE_READ,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_OPEN_NO_RECALL,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_handle(handle as _) };
    validate_windows_handle(&file, false, false)?;
    Ok(file)
}

#[cfg(not(windows))]
fn platform_open_existing_file(path: &Path, _delete_access: bool) -> io::Result<File> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "mutation source is not a plain file",
        ));
    }
    OpenOptions::new().read(true).open(path)
}

#[cfg(windows)]
fn platform_open_archive_object(
    path: &Path,
    delete_access: bool,
    exclusive: bool,
) -> io::Result<(File, bool)> {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{GENERIC_READ, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FileAttributeTagInfo, FileStandardInfo, GetFileInformationByHandleEx, DELETE,
        FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_NO_RECALL,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_STANDARD_INFO, OPEN_EXISTING,
        READ_CONTROL,
    };

    let wide = wide_path(path);
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | READ_CONTROL | if delete_access { DELETE } else { 0 },
            if exclusive { 0 } else { FILE_SHARE_READ },
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_OPEN_NO_RECALL,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_handle(handle as _) };
    let mut attributes = FILE_ATTRIBUTE_TAG_INFO::default();
    let attributes_ok = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileAttributeTagInfo,
            (&mut attributes as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    };
    if attributes_ok == 0
        || !windows_handle_attributes_are_local(attributes.FileAttributes, attributes.ReparseTag)
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "archive proof refuses offline, recall-on-access, cloud-backed, linked, or unknown reparse handles",
        ));
    }
    let mut standard = FILE_STANDARD_INFO::default();
    let standard_ok = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileStandardInfo,
            (&mut standard as *mut FILE_STANDARD_INFO).cast(),
            std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
        )
    };
    if standard_ok == 0 || standard.DeletePending {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "archive proof cannot establish object type or object is delete-pending",
        ));
    }
    Ok((file, standard.Directory))
}

#[cfg(not(windows))]
fn platform_open_archive_object(
    path: &Path,
    _delete_access: bool,
    _exclusive: bool,
) -> io::Result<(File, bool)> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !(metadata.is_file() || metadata.is_dir()) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "archive proof source is not a plain file or directory",
        ));
    }
    Ok((OpenOptions::new().read(true).open(path)?, metadata.is_dir()))
}

#[derive(Debug)]
struct ObjectStreamInventory {
    logical_bytes: u64,
    count: u32,
    default_data_count: u32,
    has_non_default: bool,
}

impl ObjectStreamInventory {
    fn matches_final_profile(&self, directory: bool) -> bool {
        if directory {
            // A plain directory has no FileStreamInfo entries. Any entry is a
            // named stream or another unsupported stream kind.
            self.count == 0
        } else {
            // A plain file must enumerate exactly the unnamed data stream.
            // ERROR_HANDLE_EOF for a file is deliberately not accepted.
            self.count == 1 && self.default_data_count == 1 && !self.has_non_default
        }
    }
}

#[cfg(windows)]
fn require_empty_bound_directory(file: &File, _path: &Path) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{ERROR_HANDLE_EOF, ERROR_NO_MORE_FILES};
    use windows_sys::Win32::Storage::FileSystem::{
        FileIdBothDirectoryInfo, FileIdBothDirectoryRestartInfo, GetFileInformationByHandleEx,
        FILE_ID_BOTH_DIR_INFO,
    };

    // Enumerate through the already-bound share-zero target. Reopening the path
    // here would both self-conflict and reintroduce a pathname TOCTOU window.
    const BUFFER_BYTES: usize = 64 * 1024;
    const MAX_PAGES: usize = 64;
    let handle = file.as_raw_handle() as _;
    let mut restart = true;
    for _ in 0..MAX_PAGES {
        let words = BUFFER_BYTES.div_ceil(std::mem::size_of::<usize>());
        let mut storage = vec![0usize; words];
        let class = if restart {
            FileIdBothDirectoryRestartInfo
        } else {
            FileIdBothDirectoryInfo
        };
        restart = false;
        let ok = unsafe {
            GetFileInformationByHandleEx(
                handle,
                class,
                storage.as_mut_ptr().cast(),
                (words * std::mem::size_of::<usize>()) as u32,
            )
        };
        if ok == 0 {
            let error = io::Error::last_os_error();
            if matches!(
                error.raw_os_error().map(|code| code as u32),
                Some(ERROR_NO_MORE_FILES) | Some(ERROR_HANDLE_EOF)
            ) {
                return Ok(());
            }
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("cannot enumerate the bound final directory handle: {error}"),
            ));
        }

        let buffer_len = storage.len() * std::mem::size_of::<usize>();
        let base = storage.as_ptr().cast::<u8>();
        let header_len = std::mem::offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
        let mut offset = 0usize;
        loop {
            if offset
                .checked_add(header_len)
                .is_none_or(|end| end > buffer_len)
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "bound directory enumeration buffer is ambiguous",
                ));
            }
            let info = unsafe { &*(base.add(offset).cast::<FILE_ID_BOTH_DIR_INFO>()) };
            let name_bytes = info.FileNameLength as usize;
            let entry_span = if info.NextEntryOffset == 0 {
                buffer_len - offset
            } else {
                info.NextEntryOffset as usize
            };
            if !name_bytes.is_multiple_of(std::mem::size_of::<u16>())
                || entry_span < header_len
                || name_bytes > entry_span - header_len
                || offset
                    .checked_add(entry_span)
                    .is_none_or(|end| end > buffer_len)
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "bound directory enumeration entry is ambiguous",
                ));
            }
            let name = unsafe {
                std::slice::from_raw_parts(
                    base.add(offset + header_len).cast::<u16>(),
                    name_bytes / std::mem::size_of::<u16>(),
                )
            };
            let name = String::from_utf16(name).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "bound directory child name is not valid UTF-16",
                )
            })?;
            if name != "." && name != ".." {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!("final directory contains an unknown or undisposed child {name:?}"),
                ));
            }
            if info.NextEntryOffset == 0 {
                break;
            }
            if !(info.NextEntryOffset as usize).is_multiple_of(std::mem::align_of::<usize>()) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "bound directory enumeration chain is misaligned",
                ));
            }
            offset = offset
                .checked_add(info.NextEntryOffset as usize)
                .ok_or_else(|| io::Error::other("bound directory offset overflow"))?;
        }
    }
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "bound directory emptiness exceeded the bounded enumeration page count",
    ))
}

#[cfg(not(windows))]
fn require_empty_bound_directory(_file: &File, path: &Path) -> io::Result<()> {
    match std::fs::read_dir(path)?.next() {
        None => Ok(()),
        Some(Ok(entry)) => Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("final directory contains child {:?}", entry.file_name()),
        )),
        Some(Err(error)) => Err(error),
    }
}

#[cfg(windows)]
fn platform_archive_stream_inventory(file: &File) -> io::Result<ObjectStreamInventory> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{
        ERROR_HANDLE_EOF, ERROR_INSUFFICIENT_BUFFER, ERROR_MORE_DATA,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        FileStreamInfo, GetFileInformationByHandleEx, FILE_STREAM_INFO,
    };

    const INITIAL_BYTES: usize = 4 * 1024;
    const MAX_BYTES: usize = 1024 * 1024;
    let handle = file.as_raw_handle() as _;
    let mut buffer_bytes = INITIAL_BYTES;
    let storage = loop {
        let words = buffer_bytes.div_ceil(std::mem::size_of::<usize>());
        let mut storage = vec![0usize; words];
        let ok = unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileStreamInfo,
                storage.as_mut_ptr().cast(),
                (words * std::mem::size_of::<usize>()) as u32,
            )
        };
        if ok != 0 {
            break storage;
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error().map(|code| code as u32) == Some(ERROR_HANDLE_EOF) {
            return Ok(ObjectStreamInventory {
                logical_bytes: 0,
                count: 0,
                default_data_count: 0,
                has_non_default: false,
            });
        }
        if matches!(
            error.raw_os_error().map(|code| code as u32),
            Some(ERROR_INSUFFICIENT_BUFFER) | Some(ERROR_MORE_DATA)
        ) && buffer_bytes < MAX_BYTES
        {
            buffer_bytes = (buffer_bytes * 2).min(MAX_BYTES);
            continue;
        }
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("cannot inventory archive-source streams: {error}"),
        ));
    };

    let buffer_len = storage.len() * std::mem::size_of::<usize>();
    let base = storage.as_ptr().cast::<u8>();
    let header_len = std::mem::offset_of!(FILE_STREAM_INFO, StreamName);
    let mut offset = 0usize;
    let mut total = 0u64;
    let mut count = 0u32;
    let mut default_data_count = 0u32;
    let mut has_non_default = false;
    loop {
        if offset
            .checked_add(header_len)
            .is_none_or(|end| end > buffer_len)
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "ambiguous archive-source stream enumeration buffer",
            ));
        }
        let info = unsafe { &*(base.add(offset).cast::<FILE_STREAM_INFO>()) };
        let name_bytes = info.StreamNameLength as usize;
        let entry_span = if info.NextEntryOffset == 0 {
            buffer_len - offset
        } else {
            info.NextEntryOffset as usize
        };
        if !name_bytes.is_multiple_of(2)
            || entry_span < header_len
            || name_bytes > entry_span - header_len
            || offset
                .checked_add(entry_span)
                .is_none_or(|end| end > buffer_len)
            || info.StreamSize < 0
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "ambiguous archive-source stream entry",
            ));
        }
        let name = unsafe {
            std::slice::from_raw_parts(
                base.add(offset + header_len).cast::<u16>(),
                name_bytes / std::mem::size_of::<u16>(),
            )
        };
        let name = String::from_utf16(name).map_err(|_| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "ambiguous archive-source stream name encoding",
            )
        })?;
        if name == "::$DATA" {
            default_data_count = default_data_count
                .checked_add(1)
                .ok_or_else(|| io::Error::other("archive-source default stream count overflow"))?;
        } else {
            has_non_default = true;
        }
        total = total
            .checked_add(info.StreamSize as u64)
            .ok_or_else(|| io::Error::other("archive-source stream size overflow"))?;
        count = count
            .checked_add(1)
            .ok_or_else(|| io::Error::other("archive-source stream count overflow"))?;
        if count > 4096 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "archive-source stream count exceeds the supported bound",
            ));
        }
        if info.NextEntryOffset == 0 {
            break;
        }
        if !(info.NextEntryOffset as usize).is_multiple_of(std::mem::align_of::<usize>()) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "archive-source stream chain is misaligned",
            ));
        }
        offset = offset
            .checked_add(info.NextEntryOffset as usize)
            .ok_or_else(|| io::Error::other("archive-source stream offset overflow"))?;
    }
    Ok(ObjectStreamInventory {
        logical_bytes: total,
        count,
        default_data_count,
        has_non_default,
    })
}

#[cfg(not(windows))]
fn platform_archive_stream_inventory(file: &File) -> io::Result<ObjectStreamInventory> {
    let metadata = file.metadata()?;
    let directory = metadata.is_dir();
    Ok(ObjectStreamInventory {
        logical_bytes: metadata.len(),
        count: u32::from(!directory),
        default_data_count: u32::from(!directory),
        has_non_default: false,
    })
}

#[cfg(windows)]
fn create_new_bound_file(
    path: &Path,
    parent: DirectoryGuard,
    delete_on_close: bool,
) -> io::Result<CreatedFile> {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, CREATE_NEW, DELETE, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OPEN_NO_RECALL,
        FILE_FLAG_OPEN_REPARSE_POINT,
    };

    race_hook(RacePoint::BeforeDestinationCreate, path);
    let wide = wide_path(path);
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE | DELETE,
            0,
            std::ptr::null(),
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_OPEN_NO_RECALL,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        let error = io::Error::last_os_error();
        return Err(map_destination_occupied(path, error));
    }
    let file = unsafe { File::from_raw_handle(handle as _) };
    if delete_on_close {
        if let Err(error) = platform_arm_delete_on_close(&file) {
            best_effort_delete_created_handle(file, path);
            return Err(error);
        }
    }
    let stamp = match (|| {
        injected_create_fault()?;
        validate_windows_handle(&file, false, delete_on_close)?;
        platform_file_stamp(&file)
    })() {
        Ok(stamp) => stamp,
        Err(error) => {
            best_effort_delete_created_handle(file, path);
            return Err(error);
        }
    };
    Ok(CreatedFile {
        path: path.to_path_buf(),
        file,
        stamp,
        hash: String::new(),
        delete_on_close_armed: delete_on_close,
        _parent: parent,
    })
}

#[cfg(not(windows))]
fn create_new_bound_file(
    path: &Path,
    parent: DirectoryGuard,
    delete_on_close: bool,
) -> io::Result<CreatedFile> {
    race_hook(RacePoint::BeforeDestinationCreate, path);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)?;
    let stamp = match (|| {
        injected_create_fault()?;
        platform_file_stamp(&file)
    })() {
        Ok(stamp) => stamp,
        Err(error) => {
            best_effort_delete_created_handle(file, path);
            return Err(error);
        }
    };
    Ok(CreatedFile {
        path: path.to_path_buf(),
        file,
        stamp,
        hash: String::new(),
        // Non-Windows builds never execute object_archive/2 in production. The
        // Drop guard remains best-effort for portable tests.
        delete_on_close_armed: delete_on_close,
        _parent: parent,
    })
}

#[cfg(windows)]
fn validate_windows_handle(
    file: &File,
    expect_directory: bool,
    allow_delete_pending: bool,
) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileAttributeTagInfo, FileStandardInfo, GetFileInformationByHandleEx,
        FILE_ATTRIBUTE_TAG_INFO, FILE_STANDARD_INFO,
    };

    let handle = file.as_raw_handle() as _;
    let mut attributes = FILE_ATTRIBUTE_TAG_INFO::default();
    let attribute_ok = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileAttributeTagInfo,
            &mut attributes as *mut FILE_ATTRIBUTE_TAG_INFO as *mut _,
            std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    };
    if attribute_ok == 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "cannot prove mutation handle attributes: {}",
                io::Error::last_os_error()
            ),
        ));
    }
    if !windows_handle_attributes_are_local(attributes.FileAttributes, attributes.ReparseTag) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "mutation refuses offline, recall-on-access, cloud-backed, linked, or unknown reparse handles",
        ));
    }

    let mut standard = FILE_STANDARD_INFO::default();
    let standard_ok = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileStandardInfo,
            &mut standard as *mut FILE_STANDARD_INFO as *mut _,
            std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
        )
    };
    if standard_ok == 0 {
        return Err(io::Error::last_os_error());
    }
    if (!allow_delete_pending && standard.DeletePending) || standard.Directory != expect_directory {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "mutation handle has the wrong type or is delete-pending",
        ));
    }
    validate_windows_plain_stream_profile(file, expect_directory)?;
    Ok(())
}

#[cfg(windows)]
fn validate_windows_plain_stream_profile(file: &File, directory: bool) -> io::Result<()> {
    let inventory = platform_archive_stream_inventory(file)?;
    if inventory.matches_final_profile(directory) {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            if directory {
                format!(
                    "mutation refuses directory stream inventory ({} FileStreamInfo entries; expected zero)",
                    inventory.count
                )
            } else {
                format!(
                    "mutation refuses file stream inventory ({} entries, {} exact default streams, named/non-default={})",
                    inventory.count, inventory.default_data_count, inventory.has_non_default
                )
            },
        ))
    }
}

#[cfg(windows)]
fn windows_handle_attributes_are_local(attributes: u32, reparse_tag: u32) -> bool {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
        FILE_ATTRIBUTE_RECALL_ON_OPEN, FILE_ATTRIBUTE_REPARSE_POINT,
    };

    let unsafe_attributes = FILE_ATTRIBUTE_REPARSE_POINT
        | FILE_ATTRIBUTE_OFFLINE
        | FILE_ATTRIBUTE_RECALL_ON_OPEN
        | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS;
    reparse_tag == 0 && attributes & unsafe_attributes == 0
}

#[cfg(windows)]
pub(crate) fn platform_file_stamp(file: &File) -> io::Result<FileStamp> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileStandardInfo, GetFileInformationByHandle, GetFileInformationByHandleEx,
        BY_HANDLE_FILE_INFORMATION, FILE_STANDARD_INFO,
    };

    let handle = file.as_raw_handle() as _;
    // Use the same volume/file-index representation as hangar-fs and the DB:
    // decimal volume serial + decimal 64-bit file index. This lets an identity
    // reviewed during planning be compared directly with the mutation handle.
    let mut id = BY_HANDLE_FILE_INFORMATION::default();
    let id_ok = unsafe { GetFileInformationByHandle(handle, &mut id) };
    if id_ok == 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "cannot prove mutation file identity: {}",
                io::Error::last_os_error()
            ),
        ));
    }
    let file_index = ((id.nFileIndexHigh as u64) << 32) | id.nFileIndexLow as u64;
    let mut standard = FILE_STANDARD_INFO::default();
    let standard_ok = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileStandardInfo,
            &mut standard as *mut FILE_STANDARD_INFO as *mut _,
            std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
        )
    };
    if standard_ok == 0 || standard.EndOfFile < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(FileStamp {
        volume_id: id.dwVolumeSerialNumber.to_string(),
        file_id: file_index.to_string(),
        bytes: standard.EndOfFile as u64,
        modified_unix_seconds: filetime_to_unix_seconds(
            id.ftLastWriteTime.dwHighDateTime,
            id.ftLastWriteTime.dwLowDateTime,
        ),
    })
}

#[cfg(windows)]
fn filetime_to_unix_seconds(high: u32, low: u32) -> Option<i64> {
    const WINDOWS_TO_UNIX_EPOCH_100NS: u64 = 116_444_736_000_000_000;
    let ticks = ((high as u64) << 32) | low as u64;
    ticks
        .checked_sub(WINDOWS_TO_UNIX_EPOCH_100NS)
        .and_then(|ticks| i64::try_from(ticks / 10_000_000).ok())
}

#[cfg(not(windows))]
pub(crate) fn platform_file_stamp(file: &File) -> io::Result<FileStamp> {
    Ok(fallback_stamp(&file.metadata()?))
}

#[cfg(all(unix, not(windows)))]
fn fallback_stamp(metadata: &fs::Metadata) -> FileStamp {
    use std::os::unix::fs::MetadataExt;
    FileStamp {
        volume_id: metadata.dev().to_string(),
        file_id: metadata.ino().to_string(),
        bytes: metadata.len(),
        modified_unix_seconds: metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|duration| i64::try_from(duration.as_secs()).ok()),
    }
}

#[cfg(not(any(unix, windows)))]
fn fallback_stamp(metadata: &fs::Metadata) -> FileStamp {
    FileStamp {
        volume_id: "fallback".to_string(),
        file_id: format!("{}", metadata.len()),
        bytes: metadata.len(),
        modified_unix_seconds: metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
            .and_then(|duration| i64::try_from(duration.as_secs()).ok()),
    }
}

#[cfg(windows)]
pub(crate) fn platform_rename_handle_no_replace(
    file: &File,
    _source: &Path,
    destination: &Path,
    parent: &DirectoryGuard,
) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileRenameInfo, FileRenameInfoEx, SetFileInformationByHandle, FILE_RENAME_INFO,
    };

    // SetFileInformationByHandle accepts an absolute DOS destination when
    // RootDirectory is NULL. The separately held destination-parent handle is
    // still the ancestor proof/anti-replacement lock; using the absolute name
    // avoids filesystem-dependent rejection of a relative RootDirectory form.
    let destination_name = crate::longpath::to_extended(destination);
    let wide: Vec<u16> = destination_name.as_os_str().encode_wide().collect();
    if wide.is_empty() || wide.len() > (u32::MAX as usize / 2) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "rename destination leaf is invalid",
        ));
    }
    let byte_len = wide.len() * std::mem::size_of::<u16>();
    let buffer_len = std::mem::size_of::<FILE_RENAME_INFO>()
        + byte_len.saturating_sub(std::mem::size_of::<u16>());
    let words = buffer_len.div_ceil(std::mem::size_of::<usize>());
    let mut storage = vec![0usize; words];
    let info = storage.as_mut_ptr() as *mut FILE_RENAME_INFO;
    let _parent_guard = parent
        .handles
        .last()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing destination parent"))?;
    unsafe {
        (*info).Anonymous.Flags = 0;
        (*info).RootDirectory = std::ptr::null_mut();
        (*info).FileNameLength = byte_len as u32;
        std::ptr::copy_nonoverlapping(wide.as_ptr(), (*info).FileName.as_mut_ptr(), wide.len());
    }
    let handle = file.as_raw_handle() as _;
    let ex_ok = unsafe {
        SetFileInformationByHandle(handle, FileRenameInfoEx, info.cast(), buffer_len as u32)
    };
    if ex_ok != 0 {
        return Ok(());
    }
    unsafe {
        (*info).Anonymous.ReplaceIfExists = false;
    }
    let fallback_ok = unsafe {
        SetFileInformationByHandle(handle, FileRenameInfo, info.cast(), buffer_len as u32)
    };
    if fallback_ok == 0 {
        Err(map_destination_occupied(
            destination,
            io::Error::last_os_error(),
        ))
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn map_destination_occupied(path: &Path, error: io::Error) -> io::Error {
    if error.kind() == io::ErrorKind::AlreadyExists
        || matches!(error.raw_os_error(), Some(80 | 183))
    {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("destination is occupied: {}", path.display()),
        )
    } else {
        error
    }
}

#[cfg(not(windows))]
fn platform_rename_handle_no_replace(
    file: &File,
    source: &Path,
    destination: &Path,
    _parent: &DirectoryGuard,
) -> io::Result<()> {
    let source_stamp = platform_file_stamp(file)?;
    fs::hard_link(source, destination)?;
    let destination_stamp = fallback_stamp(&fs::metadata(destination)?);
    if !source_stamp.same_object(&destination_stamp) {
        let _ = fs::remove_file(destination);
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "non-Windows no-replace rename could not bind the source identity",
        ));
    }
    let current = BoundFile::open_read(source)?;
    if !current.stamp.same_object(&source_stamp) {
        let _ = fs::remove_file(destination);
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "non-Windows source identity changed before unlink",
        ));
    }
    fs::remove_file(source)
}

#[cfg(windows)]
fn platform_delete_handle(file: File, _path: &Path, expected: &FileStamp) -> io::Result<()> {
    let current = platform_file_stamp(&file)?;
    if !current.same_object(expected) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "bound delete identity changed",
        ));
    }
    platform_dispose_handle(file)
}

#[cfg(windows)]
pub(crate) fn platform_dispose_handle(file: File) -> io::Result<()> {
    platform_arm_delete_on_close(&file)?;

    // Generic exact-handle cleanup is also used for object_archive/2 scratch
    // objects, whose whole purpose is to round-trip named streams. Do not apply
    // the conservative final-removal profile here; only held-object disposition
    // goes through `platform_dispose_final_object_handle` below.
    drop(file);
    Ok(())
}

#[cfg(windows)]
#[allow(dead_code)] // retained for the portable/test compatibility method above
fn platform_dispose_final_object_handle(file: File, expected: &FileStamp) -> io::Result<()> {
    let current = platform_file_stamp(&file)?;
    if !current.same_object(expected) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "bound final-disposition identity changed",
        ));
    }

    // The pre-arm proof catches an already-present hardlink/ADS. It cannot be
    // the only proof because those namespace changes are allowed while this
    // handle is merely open; the mode-specific post-arm proof below closes that
    // interval after NTFS starts rejecting new links/stream opens.
    validate_final_disposition_profile(&file, false, None)?;
    let disposition_mode = match platform_set_delete_on_close(&file) {
        Ok(mode) => mode,
        Err(arm_error) => {
            if let Err(cancel_error) = platform_cancel_delete_on_close(&file) {
                // Last in-process containment only. Forgetting prevents this return
                // from closing a possibly armed handle, but process crash/shutdown
                // will still close it. The caller must already have journaled
                // `unprovedFinalProfile` so recovery never converts path absence
                // after this dual failure into a proved deletion.
                std::mem::forget(file);
                return Err(io::Error::other(format!(
                    "final disposition could not arm/prove delete-pending ({arm_error}) or cancel it; the exact handle was retained only for the current process, leaving crash/shutdown as a residual physical risk: {cancel_error}"
                )));
            }
            return Err(arm_error);
        }
    };

    // The helper's archive proof deliberately happens before the medium-integrity
    // parent disposes the held object. Merely retaining a no-write/no-delete-share
    // handle does not freeze NTFS namespace metadata: CreateHardLinkW and a new ADS
    // can both succeed against another stream while that handle is open. Close the
    // race by making this exact file object delete-pending first. New hardlinks and
    // stream opens are then refused by NTFS; while that state is active, re-query the
    // SAME handle and require the conservative final-removal profile:
    //   * delete-pending really is active;
    //   * the mode-specific link count still proves one pre-arm pathname; and
    //   * the only stream is the unnamed/default data stream.
    // Any mismatch is cancelled through the exact handle and must never be reported
    // as a successful disposition.
    if let Err(validation_error) =
        validate_final_disposition_profile(&file, true, Some(disposition_mode))
    {
        if let Err(cancel_error) = platform_cancel_delete_on_close_mode(&file, disposition_mode) {
            // Dropping a handle which may still be delete-pending would immediately
            // turn a failed validation into the deletion it refused. Forgetting is
            // only last-resort in-process containment: crash/shutdown still closes
            // the handle and can physically remove raced, unarchived ADS. The
            // durable `unprovedFinalProfile` marker prevents a false success claim,
            // but does not resolve that dual-fault physical-loss residual.
            std::mem::forget(file);
            return Err(io::Error::other(format!(
                "final disposition proof failed ({validation_error}) and delete-on-close could not be cancelled; the exact handle was retained only for the current process, leaving crash/shutdown as a residual physical risk: {cancel_error}"
            )));
        }
        return Err(validation_error);
    }
    drop(file);
    Ok(())
}

#[cfg(windows)]
pub(crate) fn validate_final_disposition_profile(
    file: &File,
    require_delete_pending: bool,
    disposition_mode: Option<WindowsDeleteDispositionMode>,
) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileStandardInfo, GetFileInformationByHandle, GetFileInformationByHandleEx,
        BY_HANDLE_FILE_INFORMATION, FILE_STANDARD_INFO,
    };

    let handle = file.as_raw_handle() as _;
    let mut standard = FILE_STANDARD_INFO::default();
    let standard_ok = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileStandardInfo,
            (&mut standard as *mut FILE_STANDARD_INFO).cast(),
            std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
        )
    };
    if standard_ok == 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "cannot prove delete-pending disposition state: {}",
                io::Error::last_os_error()
            ),
        ));
    }
    if standard.DeletePending != require_delete_pending {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            if require_delete_pending {
                "exact handle did not enter delete-pending state"
            } else {
                "exact handle was already delete-pending before final disposition"
            },
        ));
    }

    let mut identity = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(handle, &mut identity) } == 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "cannot prove final hardlink topology: {}",
                io::Error::last_os_error()
            ),
        ));
    }
    let expected_links = match disposition_mode {
        None => 1,
        Some(WindowsDeleteDispositionMode::ExtendedOnClose) => 1,
        Some(WindowsDeleteDispositionMode::Legacy) => 0,
    };
    if identity.nNumberOfLinks != expected_links {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "final disposition refuses incomplete hardlink topology (link count {}, expected {expected_links} for {disposition_mode:?})",
                identity.nNumberOfLinks,
            ),
        ));
    }

    // This intentionally narrows final disposition, not archive capture. The v2
    // archive may preserve ADS, but the parent cannot prove that an already-open
    // ADS writer did not change a stream after the helper's final recapture. The
    // safe release profile therefore keeps any named/non-default stream held.
    validate_windows_plain_stream_profile(file, standard.Directory)
}

#[cfg(windows)]
fn platform_set_delete_on_close(file: &File) -> io::Result<WindowsDeleteDispositionMode> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileDispositionInfo, FileDispositionInfoEx, SetFileInformationByHandle,
        FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_ON_CLOSE, FILE_DISPOSITION_INFO,
        FILE_DISPOSITION_INFO_EX,
    };

    let handle = file.as_raw_handle() as _;
    let extended = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_ON_CLOSE,
    };
    let ex_ok = unsafe {
        SetFileInformationByHandle(
            handle,
            FileDispositionInfoEx,
            (&extended as *const FILE_DISPOSITION_INFO_EX).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        )
    };
    if ex_ok == 0 {
        let legacy = FILE_DISPOSITION_INFO { DeleteFile: true };
        let legacy_ok = unsafe {
            SetFileInformationByHandle(
                handle,
                FileDispositionInfo,
                (&legacy as *const FILE_DISPOSITION_INFO).cast(),
                std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        };
        if legacy_ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(WindowsDeleteDispositionMode::Legacy)
    } else {
        Ok(WindowsDeleteDispositionMode::ExtendedOnClose)
    }
}

#[cfg(windows)]
fn platform_delete_pending(file: &File) -> io::Result<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileStandardInfo, GetFileInformationByHandleEx, FILE_STANDARD_INFO,
    };

    let handle = file.as_raw_handle() as _;
    let mut standard = FILE_STANDARD_INFO::default();
    let queried = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileStandardInfo,
            (&mut standard as *mut FILE_STANDARD_INFO).cast(),
            std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
        )
    };
    if queried == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(standard.DeletePending)
}

#[cfg(windows)]
fn platform_arm_delete_on_close(file: &File) -> io::Result<WindowsDeleteDispositionMode> {
    let disposition_mode = platform_set_delete_on_close(file)?;
    if !platform_delete_pending(file)? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "exact archive handle could not be made crash-disposable",
        ));
    }
    Ok(disposition_mode)
}

#[cfg(all(test, windows))]
fn platform_arm_legacy_delete_on_close_for_test(
    file: &File,
) -> io::Result<WindowsDeleteDispositionMode> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FileDispositionInfo, SetFileInformationByHandle, FILE_DISPOSITION_INFO,
    };

    let legacy = FILE_DISPOSITION_INFO { DeleteFile: true };
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle() as _,
            FileDispositionInfo,
            (&legacy as *const FILE_DISPOSITION_INFO).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if !platform_delete_pending(file)? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "legacy test disposition did not enter delete-pending state",
        ));
    }
    Ok(WindowsDeleteDispositionMode::Legacy)
}

#[cfg(windows)]
fn platform_cancel_delete_on_close(file: &File) -> io::Result<()> {
    platform_cancel_delete_on_close_with_preference(file, None)
}

#[cfg(windows)]
pub(crate) fn platform_cancel_delete_on_close_mode(
    file: &File,
    disposition_mode: WindowsDeleteDispositionMode,
) -> io::Result<()> {
    platform_cancel_delete_on_close_with_preference(file, Some(disposition_mode))
}

/// Guardian-side cancellation accepts a missing preferred mode because the
/// parent can die in the narrow interval after the kernel arm but before it
/// durably records which API succeeded. Both API forms are tried and the exact
/// duplicated handle must finally report DeletePending=false.
#[cfg(windows)]
pub(crate) fn guardian_cancel_delete_on_close(
    file: &File,
    preferred_mode: Option<WindowsDeleteDispositionMode>,
) -> io::Result<()> {
    if !platform_delete_pending(file)? {
        return Ok(());
    }
    platform_cancel_delete_on_close_with_preference(file, preferred_mode)
}

#[cfg(all(test, windows))]
pub(crate) fn guardian_delete_pending(file: &File) -> io::Result<bool> {
    platform_delete_pending(file)
}

#[cfg(windows)]
fn platform_cancel_delete_on_close_with_preference(
    file: &File,
    preferred_mode: Option<WindowsDeleteDispositionMode>,
) -> io::Result<()> {
    injected_cancel_delete_on_close_fault()?;
    let attempts = match preferred_mode {
        Some(WindowsDeleteDispositionMode::Legacy) => [
            WindowsDeleteDispositionMode::Legacy,
            WindowsDeleteDispositionMode::ExtendedOnClose,
            WindowsDeleteDispositionMode::Legacy,
        ],
        Some(WindowsDeleteDispositionMode::ExtendedOnClose) | None => [
            WindowsDeleteDispositionMode::ExtendedOnClose,
            WindowsDeleteDispositionMode::Legacy,
            WindowsDeleteDispositionMode::ExtendedOnClose,
        ],
    };
    let mut last_error = None;
    for mode in attempts {
        match platform_cancel_delete_on_close_once(file, mode) {
            Ok(()) => match platform_delete_pending(file) {
                Ok(false) => return Ok(()),
                Ok(true) => {
                    last_error = Some(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        format!("{mode:?} cancellation left the exact handle delete-pending"),
                    ));
                }
                Err(error) => last_error = Some(error),
            },
            Err(error) => last_error = Some(error),
        }
        std::thread::yield_now();
    }
    Err(io::Error::other(format!(
        "delete-on-close cancellation failed after mode-aware fallback/retry: {}",
        last_error.unwrap_or_else(|| io::Error::other("no cancellation attempt completed"))
    )))
}

#[cfg(windows)]
fn platform_cancel_delete_on_close_once(
    file: &File,
    mode: WindowsDeleteDispositionMode,
) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileDispositionInfo, FileDispositionInfoEx, SetFileInformationByHandle,
        FILE_DISPOSITION_INFO, FILE_DISPOSITION_INFO_EX,
    };

    let handle = file.as_raw_handle() as _;
    let cancelled = match mode {
        WindowsDeleteDispositionMode::ExtendedOnClose => {
            let extended = FILE_DISPOSITION_INFO_EX { Flags: 0 };
            unsafe {
                SetFileInformationByHandle(
                    handle,
                    FileDispositionInfoEx,
                    (&extended as *const FILE_DISPOSITION_INFO_EX).cast(),
                    std::mem::size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
                )
            }
        }
        WindowsDeleteDispositionMode::Legacy => {
            let legacy = FILE_DISPOSITION_INFO { DeleteFile: false };
            unsafe {
                SetFileInformationByHandle(
                    handle,
                    FileDispositionInfo,
                    (&legacy as *const FILE_DISPOSITION_INFO).cast(),
                    std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
                )
            }
        }
    };
    if cancelled == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn platform_cancel_delete_on_close(_file: &File) -> io::Result<()> {
    Ok(())
}

fn best_effort_delete_created_handle(file: File, path: &Path) {
    #[cfg(windows)]
    {
        // The handle came directly from CREATE_NEW with no sharing, so it is the
        // exact object this call created even if attribute/identity initialization
        // failed. Disposition through it cannot target a raced path replacement.
        let _ = platform_dispose_handle(file);
        let _ = path;
    }
    #[cfg(not(windows))]
    {
        // The portable fallback cannot dispose by handle. Re-read both identities
        // while the parent guard is retained and only unlink a matching pathname.
        if let Ok(stamp) = platform_file_stamp(&file) {
            let _ = platform_delete_handle(file, path, &stamp);
        }
    }
}

#[cfg(windows)]
fn platform_delete_directory_handle(
    file: File,
    path: &Path,
    expected: &FileStamp,
) -> io::Result<()> {
    platform_delete_handle(file, path, expected)
}

#[cfg(not(windows))]
fn platform_delete_handle(file: File, path: &Path, expected: &FileStamp) -> io::Result<()> {
    let current_handle = platform_file_stamp(&file)?;
    let current_path = BoundFile::open_read(path)?;
    if !current_handle.same_object(expected) || !current_path.stamp.same_object(expected) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "non-Windows delete identity changed",
        ));
    }
    fs::remove_file(path)
}

#[cfg(not(windows))]
fn platform_delete_directory_handle(
    file: File,
    path: &Path,
    expected: &FileStamp,
) -> io::Result<()> {
    let current_handle = platform_file_stamp(&file)?;
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "non-Windows cleanup target changed type",
        ));
    }
    let current_path = fallback_stamp(&metadata);
    if !current_handle.same_object(expected) || !current_path.same_object(expected) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "non-Windows cleanup identity changed",
        ));
    }
    fs::remove_dir(path)
}

#[cfg(windows)]
pub(crate) fn wide_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    crate::longpath::to_extended(path)
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    fn bound_final_object(path: &Path) -> BoundObjectProof {
        let (mut file, directory) = platform_open_archive_object(path, false, false).unwrap();
        let stamp = platform_file_stamp(&file).unwrap();
        let hash = if directory {
            blake3::hash(&[]).to_hex().to_string()
        } else {
            hash_open_file(&mut file).unwrap()
        };
        drop(file);
        BoundObjectProof::open_for_archive_delete(path, &stamp, &hash).unwrap()
    }

    #[cfg(windows)]
    fn bound_helper_object(path: &Path) -> BoundObjectProof {
        let (mut file, directory) = platform_open_archive_object(path, false, false).unwrap();
        let stamp = platform_file_stamp(&file).unwrap();
        let hash = if directory {
            blake3::hash(&[]).to_hex().to_string()
        } else {
            hash_open_file(&mut file).unwrap()
        };
        drop(file);
        BoundObjectProof::open_for_archive(path, &stamp, &hash)
            .unwrap()
            .release_ancestors_for_helper()
            .unwrap()
    }

    #[test]
    fn file_stamp_object_identity_ignores_size_but_full_verify_does_not() {
        let a = FileStamp {
            volume_id: "v".to_string(),
            file_id: "i".to_string(),
            bytes: 1,
            modified_unix_seconds: Some(1),
        };
        let mut b = a.clone();
        b.bytes = 2;
        assert!(a.same_object(&b));
        assert_ne!(a, b);
    }

    #[test]
    fn create_initialization_failure_removes_the_exact_new_object() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("create-fault.bin");
        set_test_fault(Some(FaultPoint::CreateAfterHandle));
        let result = write_new(&destination, b"never committed");
        set_test_fault(None);
        assert!(result.is_err());
        assert!(!destination.exists());
    }

    #[test]
    fn copy_failure_after_create_removes_the_exact_new_object() {
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("source.bin");
        let destination = dir.path().join("copy-fault.bin");
        fs::write(&source_path, b"reviewed source").unwrap();
        let mut source = BoundFile::open_read(&source_path).unwrap();
        let hash = source.hash().unwrap();
        set_test_fault(Some(FaultPoint::CopyAfterCreate));
        let result = copy_to_new(&mut source, &destination, &hash);
        set_test_fault(None);
        assert!(result.is_err());
        assert!(!destination.exists());
        assert_eq!(fs::read(source_path).unwrap(), b"reviewed source");
    }

    #[test]
    fn write_failure_after_create_removes_the_exact_new_object() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("write-fault.bin");
        set_test_fault(Some(FaultPoint::WriteAfterCreate));
        let result = write_new(&destination, b"partially written");
        set_test_fault(None);
        assert!(result.is_err());
        assert!(!destination.exists());
    }

    #[test]
    fn archive_container_promotes_exact_create_new_object_without_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let partial = dir.path().join("object.partial");
        let final_path = dir.path().join("object.chobj");
        let mut container = ObjectArchiveContainer::create_new(&partial).unwrap();
        let created = container.created.as_mut().unwrap();
        created.file.write_all(b"object archive v2").unwrap();
        created.file.sync_all().unwrap();
        let expected_stamp = platform_file_stamp(&created.file).unwrap();
        let expected_hash = hash_open_file(&mut created.file).unwrap();
        let committed = container
            .verify_and_commit(&final_path, &expected_stamp, &expected_hash)
            .unwrap();
        assert_eq!(committed.hash(), expected_hash);
        assert_eq!(committed.stamp(), &expected_stamp);
        let committed_path = committed.path().to_path_buf();
        // The archive authority deliberately retains a no-sharing proof handle.
        // Close it before reopening the pathname as an ordinary reader.
        drop(committed);
        assert_eq!(fs::read(committed_path).unwrap(), b"object archive v2");
        assert!(!partial.exists());

        let raced_partial = dir.path().join("raced.partial");
        let occupied_final = dir.path().join("occupied.chobj");
        fs::write(&occupied_final, b"occupant").unwrap();
        let mut raced = ObjectArchiveContainer::create_new(&raced_partial).unwrap();
        let created = raced.created.as_mut().unwrap();
        created.file.write_all(b"must be cleaned").unwrap();
        created.file.sync_all().unwrap();
        let stamp = platform_file_stamp(&created.file).unwrap();
        let hash = hash_open_file(&mut created.file).unwrap();
        assert!(raced
            .verify_and_commit(&occupied_final, &stamp, &hash)
            .is_err());
        assert_eq!(fs::read(occupied_final).unwrap(), b"occupant");
        assert!(!raced_partial.exists());
    }

    #[cfg(windows)]
    #[test]
    fn abandoned_archive_partial_is_removed_by_its_create_handle_close() {
        let dir = tempfile::tempdir().unwrap();
        let partial = dir.path().join("abandoned.partial");
        let mut container = ObjectArchiveContainer::create_new(&partial).unwrap();
        let mut created = container.created.take().unwrap();
        created.file.write_all(b"unproved archive bytes").unwrap();
        created.file.sync_all().unwrap();

        // Bypass ObjectArchiveContainer::drop's explicit cleanup. Closing the
        // CREATE_NEW file object alone models the OS closing handles after a
        // parent/helper crash and must still remove the partial.
        drop(created);
        drop(container);
        assert!(!partial.exists());
    }

    #[test]
    fn local_path_validation_rejects_parent_traversal_before_touch() {
        let dir = tempfile::tempdir().unwrap();
        let unsafe_path = dir.path().join("safe").join("..").join("outside.bin");
        assert_eq!(
            validate_local_mutation_path(&unsafe_path)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(!dir.path().join("outside.bin").exists());
    }

    #[cfg(windows)]
    #[test]
    fn local_path_validation_rejects_remote_and_device_namespaces_without_probing() {
        for path in [
            Path::new(r"\\server\share\must-not-probe.bin"),
            Path::new(r"\\?\UNC\server\share\must-not-probe.bin"),
            Path::new(r"\\.\C:\must-not-probe.bin"),
            Path::new(r"\\?\GLOBALROOT\Device\HarddiskVolume1\must-not-probe.bin"),
        ] {
            assert!(
                validate_local_mutation_path(path).is_err(),
                "{}",
                path.display()
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn local_path_validation_rejects_ads_syntax_before_touch() {
        let dir = tempfile::tempdir().unwrap();
        let ordinary = dir.path().join("ordinary.bin");
        fs::write(&ordinary, b"ordinary").unwrap();
        let ads = PathBuf::from(format!("{}:secret", ordinary.display()));
        assert_eq!(
            validate_local_mutation_path(&ads).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(!PathBuf::from(format!("{}:secret", ordinary.display())).exists());
    }

    #[cfg(windows)]
    #[test]
    fn bound_handles_reject_real_file_and_directory_ads() {
        use std::io::Write as _;

        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.bin");
        let directory = dir.path().join("folder");
        fs::write(&source, b"ordinary").unwrap();
        fs::create_dir(&directory).unwrap();

        for ads in [
            format!("{}:secret", source.display()),
            format!("{}:secret", directory.display()),
        ] {
            let mut stream = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(ads)
                .unwrap();
            stream
                .write_all(b"metadata not represented by manifest")
                .unwrap();
        }

        let file_error = BoundFile::open_read(&source).unwrap_err();
        assert_eq!(file_error.kind(), io::ErrorKind::PermissionDenied);
        assert!(file_error.to_string().contains("stream"));

        let directory_error = BoundDirectory::open_for_delete(&directory).unwrap_err();
        assert_eq!(directory_error.kind(), io::ErrorKind::PermissionDenied);
        assert!(directory_error.to_string().contains("stream"));
        assert!(source.exists());
        assert!(directory.exists());
    }

    #[cfg(windows)]
    #[test]
    fn object_archive_proof_separately_binds_and_inventories_ads() {
        use std::io::Write as _;

        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.bin");
        let directory = dir.path().join("folder");
        fs::write(&source, b"ordinary").unwrap();
        fs::create_dir(&directory).unwrap();
        for ads in [
            format!("{}:secret", source.display()),
            format!("{}:secret", directory.display()),
        ] {
            let mut stream = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(ads)
                .unwrap();
            stream.write_all(b"preserved ADS payload").unwrap();
        }

        let (file, _) = platform_open_archive_object(&source, false, false).unwrap();
        let file_stamp = platform_file_stamp(&file).unwrap();
        let file_hash = hash_open_file(&mut file.try_clone().unwrap()).unwrap();
        drop(file);
        let file_proof =
            BoundObjectProof::open_for_archive(&source, &file_stamp, &file_hash).unwrap();
        assert!(!file_proof.is_directory());
        assert!(file_proof.stream_count() >= 2);
        assert!(file_proof.stream_logical_bytes() > file_stamp.bytes);

        let (directory_file, _) = platform_open_archive_object(&directory, false, false).unwrap();
        let directory_stamp = platform_file_stamp(&directory_file).unwrap();
        drop(directory_file);
        let directory_proof = BoundObjectProof::open_for_archive(
            &directory,
            &directory_stamp,
            blake3::hash(&[]).to_hex().as_ref(),
        )
        .unwrap();
        assert!(directory_proof.is_directory());
        assert!(directory_proof.stream_count() >= 1);
        assert!(directory_proof.stream_logical_bytes() > 0);

        // The legacy/content-only primitive remains fail-closed for the same
        // source: archive support did not relax its global ADS invariant.
        assert!(BoundFile::open_read(&source).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn final_disposition_refuses_preexisting_hardlink_before_arming() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.bin");
        let raced_link = dir.path().join("raced-link.bin");
        fs::write(&source, b"must remain through both links").unwrap();
        let proof = bound_final_object(&source);

        // NTFS permits this even though the exact source handle is already open
        // with no write/delete sharing. This is the race the final post-arm proof
        // must close.
        // `std::fs::hard_link` is the Rust standard library's CreateHardLinkW
        // path on Windows. Before delete-pending it succeeds on this held object.
        fs::hard_link(&source, &raced_link).unwrap();
        let error = proof.delete_exact().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains("hardlink topology"));
        assert_eq!(
            fs::read(&source).unwrap(),
            b"must remain through both links"
        );
        assert_eq!(
            fs::read(&raced_link).unwrap(),
            b"must remain through both links"
        );
    }

    #[cfg(windows)]
    #[test]
    fn final_disposition_post_arm_proof_catches_a_hardlink_won_after_precheck() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.bin");
        let raced_link = dir.path().join("raced-link.bin");
        fs::write(&source, b"must remain through both links").unwrap();
        let proof = bound_final_object(&source);

        validate_final_disposition_profile(&proof.file, false, None).unwrap();
        fs::hard_link(&source, &raced_link).unwrap();
        let disposition_mode = platform_arm_delete_on_close(&proof.file).unwrap();
        let error = validate_final_disposition_profile(&proof.file, true, Some(disposition_mode))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains("hardlink topology"));
        // Extended disposition retains both names (2, expected 1); legacy
        // disposition removes the armed name from the live count (1, expected 0).
        // Either mode therefore detects the raced second link on the same handle.
        platform_cancel_delete_on_close_mode(&proof.file, disposition_mode).unwrap();
        drop(proof);

        assert_eq!(
            fs::read(&source).unwrap(),
            b"must remain through both links"
        );
        assert_eq!(
            fs::read(&raced_link).unwrap(),
            b"must remain through both links"
        );
    }

    #[cfg(windows)]
    #[test]
    fn delete_pending_blocks_late_hardlink_and_ads_creation_on_the_bound_object() {
        use std::io::Write as _;

        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.bin");
        let late_link = dir.path().join("late-link.bin");
        let late_ads = PathBuf::from(format!("{}:late", source.display()));
        fs::write(&source, b"reviewed bytes").unwrap();
        let proof = bound_final_object(&source);

        let disposition_mode = platform_arm_delete_on_close(&proof.file).unwrap();
        // The matching CreateHardLinkW path must now be refused.
        assert!(fs::hard_link(&source, &late_link).is_err());
        assert!(std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&late_ads)
            .and_then(|mut stream| stream.write_all(b"late metadata"))
            .is_err());
        validate_final_disposition_profile(&proof.file, true, Some(disposition_mode)).unwrap();
        platform_cancel_delete_on_close_mode(&proof.file, disposition_mode).unwrap();
        drop(proof);

        assert_eq!(fs::read(&source).unwrap(), b"reviewed bytes");
        assert!(!late_link.exists());
        assert!(fs::read(&late_ads).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn cancel_failure_fault_keeps_the_exact_handle_delete_pending_until_retried() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.bin");
        fs::write(&source, b"reviewed bytes").unwrap();
        let proof = bound_final_object(&source);

        let disposition_mode = platform_arm_delete_on_close(&proof.file).unwrap();
        set_test_fault(Some(FaultPoint::CancelDeleteOnClose));
        let error =
            platform_cancel_delete_on_close_mode(&proof.file, disposition_mode).unwrap_err();
        set_test_fault(None);
        assert!(error.to_string().contains("cancellation fault"));
        validate_final_disposition_profile(&proof.file, true, Some(disposition_mode)).unwrap();

        // The deterministic fault models the dangerous branch without leaking the
        // handle from the test process. A real caller cannot treat the earlier error
        // as preservation; only a proved retry returns the object to a safe state.
        platform_cancel_delete_on_close_mode(&proof.file, disposition_mode).unwrap();
        validate_final_disposition_profile(&proof.file, false, None).unwrap();
        drop(proof);
        assert_eq!(fs::read(&source).unwrap(), b"reviewed bytes");
    }

    #[cfg(windows)]
    #[test]
    fn exclusive_final_rebind_refuses_an_existing_compatible_reader() {
        use std::os::windows::fs::OpenOptionsExt as _;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };

        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.bin");
        fs::write(&source, b"reviewed bytes").unwrap();
        let (stamp, hash) = inspect_local_mutation_file(&source).unwrap();
        let proof = BoundObjectProof::open_for_archive_delete(&source, &stamp, &hash).unwrap();
        let reader = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(&source)
            .unwrap();

        let error = proof.detach_exclusive_for_final_disposition().unwrap_err();
        assert!(matches!(error.raw_os_error(), Some(32 | 33)));
        drop(reader);

        let proof = BoundObjectProof::open_for_archive_delete(&source, &stamp, &hash).unwrap();
        let exclusive = proof.detach_exclusive_for_final_disposition().unwrap();
        exclusive.validate_final_disposition_prearm().unwrap();
        drop(exclusive);
        assert_eq!(fs::read(&source).unwrap(), b"reviewed bytes");
    }

    #[cfg(windows)]
    #[test]
    fn detached_share_zero_child_and_parent_handles_coexist_without_error_32() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let child = root.join("child.bin");
        fs::create_dir(&root).unwrap();
        fs::write(&child, b"bottom-up topology").unwrap();

        // Direct-chunk topology: both helper sources are share-compatible and
        // ancestor-free, and the child permits the helper's privileged read
        // reopen. Final target rebinds then coexist without error 32.
        let child_helper = bound_helper_object(&child);
        let helper_reopen = platform_open_archive_object(&child, false, false)
            .unwrap()
            .0;
        drop(helper_reopen);
        let root_helper = bound_helper_object(&root);
        let child_final = child_helper.detach_exclusive_target().unwrap();
        let root_final = root_helper.detach_exclusive_target().unwrap();
        child_final.validate_final_disposition_prearm().unwrap();
        root_final.validate_final_disposition_prearm().unwrap();
        assert_ne!(
            child_final.raw_handle_value(),
            root_final.raw_handle_value()
        );
        drop(child_final);
        drop(root_final);

        // Production direct-chunk order: keep the parent's helper source,
        // acquire and dispose the child target first, then permit only that
        // internally caused directory drift before acquiring the empty parent
        // target. This exercises the real bottom-up transition, not only the
        // ability to hold two different FILE_OBJECTs simultaneously.
        let direct_root = dir.path().join("direct-root");
        let direct_child = direct_root.join("child.bin");
        fs::create_dir(&direct_root).unwrap();
        fs::write(&direct_child, b"direct chunk").unwrap();
        let child_helper = bound_helper_object(&direct_child);
        let mut root_helper = bound_helper_object(&direct_root);
        child_helper
            .detach_exclusive_for_final_disposition()
            .unwrap()
            .delete_exact()
            .unwrap();
        assert!(!direct_child.exists());
        root_helper
            .authorize_internal_directory_time_drift()
            .unwrap();
        root_helper
            .detach_exclusive_for_final_disposition()
            .unwrap()
            .delete_exact()
            .unwrap();
        assert!(!direct_root.exists());

        // Split-chunk topology: the child proof is fully released before the
        // parent is rebound later. This must use the same detached primitive.
        let child_final = bound_helper_object(&child)
            .detach_exclusive_target()
            .unwrap();
        child_final.validate_final_disposition_prearm().unwrap();
        drop(child_final);
        let root_final = bound_helper_object(&root)
            .detach_exclusive_target()
            .unwrap();
        root_final.validate_final_disposition_prearm().unwrap();
        drop(root_final);
    }

    #[cfg(windows)]
    #[test]
    fn production_empty_directory_proof_enumerates_the_exclusive_bound_handle() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty");
        fs::create_dir(&empty).unwrap();
        let mut proof = bound_final_object(&empty);
        proof.authorize_internal_directory_time_drift().unwrap();

        // `detach` opens the target share-zero before checking emptiness. This
        // succeeds only if production enumerates that handle, never the path.
        let exclusive = proof.detach_exclusive_for_final_disposition().unwrap();
        exclusive.validate_final_disposition_prearm().unwrap();
        drop(exclusive);

        let nonempty = dir.path().join("nonempty");
        fs::create_dir(&nonempty).unwrap();
        fs::write(nonempty.join("unknown.bin"), b"unknown child").unwrap();
        let mut proof = bound_final_object(&nonempty);
        proof.authorize_internal_directory_time_drift().unwrap();
        let error = proof.detach_exclusive_for_final_disposition().unwrap_err();
        assert!(error.to_string().contains("unknown or undisposed child"));
        assert_ne!(error.raw_os_error(), Some(32));
    }

    #[cfg(windows)]
    #[test]
    fn guardian_receipt_reader_rejects_a_reparse_path_before_fileid_trust() {
        use std::os::windows::fs::symlink_file;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("receipt-target.bin");
        let redirected = dir.path().join("receipt-reparse.bin");
        fs::write(&target, b"synthetic durable receipt").unwrap();
        let target_file = platform_open_existing_file(&target, false).unwrap();
        let stamp = platform_file_stamp(&target_file).unwrap();
        drop(target_file);

        match symlink_file(&target, &redirected) {
            Ok(()) => {
                let error = read_exact_guardian_receipt(&redirected, &stamp, 4096).unwrap_err();
                assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
                assert!(error.to_string().contains("reparse"));
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::PermissionDenied | io::ErrorKind::Unsupported
                ) || error.raw_os_error() == Some(1314) =>
            {
                // Windows may disable unprivileged symlink creation. The exact
                // production attribute predicate used by the receipt reader is
                // still deterministic and must reject the symlink reparse tag.
                use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
                assert!(!windows_handle_attributes_are_local(
                    FILE_ATTRIBUTE_REPARSE_POINT,
                    0xA000_000C,
                ));
            }
            Err(error) => panic!("unexpected symlink fixture failure: {error}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn guardian_duplicate_cancels_after_parent_drop_and_parent_cancel_fault() {
        use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _};
        use windows_sys::Win32::Foundation::{
            DuplicateHandle, DUPLICATE_SAME_ACCESS, HANDLE, INVALID_HANDLE_VALUE,
        };
        use windows_sys::Win32::System::Threading::GetCurrentProcess;

        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.bin");
        let raced_link = dir.path().join("raced-link.bin");
        fs::write(&source, b"must survive parent death").unwrap();
        let (stamp, hash) = inspect_local_mutation_file(&source).unwrap();
        let proof = BoundObjectProof::open_for_archive_delete(&source, &stamp, &hash)
            .unwrap()
            .detach_exclusive_for_final_disposition()
            .unwrap();
        proof.validate_final_disposition_prearm().unwrap();

        let mut duplicate = std::ptr::null_mut();
        let duplicated = unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                proof.file.as_raw_handle() as HANDLE,
                GetCurrentProcess(),
                &mut duplicate,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        };
        assert_ne!(duplicated, 0);
        assert!(!duplicate.is_null() && duplicate != INVALID_HANDLE_VALUE);
        let guardian = unsafe { File::from_raw_handle(duplicate as _) };
        assert_eq!(platform_file_stamp(&guardian).unwrap(), stamp);

        // CreateHardLinkW can win despite the exclusive default-stream handle;
        // the same-handle post-arm proof must reject it.
        fs::hard_link(&source, &raced_link).unwrap();
        let mode = proof.arm_final_disposition().unwrap();
        assert!(proof.validate_armed_final_disposition(mode).is_err());

        set_test_fault(Some(FaultPoint::CancelDeleteOnClose));
        assert!(proof.cancel_final_disposition(mode).is_err());
        set_test_fault(None);

        // Model abrupt parent termination: its armed handle closes, but the
        // independently duplicated guardian handle keeps the FILE_OBJECT live.
        drop(proof);
        guardian_cancel_delete_on_close(&guardian, Some(mode)).unwrap();
        assert!(!guardian_delete_pending(&guardian).unwrap());
        drop(guardian);

        assert_eq!(fs::read(&source).unwrap(), b"must survive parent death");
        assert_eq!(fs::read(&raced_link).unwrap(), b"must survive parent death");
    }

    #[cfg(windows)]
    #[test]
    fn final_disposition_cancels_when_an_ads_writer_wins_the_pre_arm_race() {
        use std::io::Write as _;
        use std::os::windows::fs::OpenOptionsExt as _;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };

        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.bin");
        let raced_ads = PathBuf::from(format!("{}:raced", source.display()));
        fs::write(&source, b"reviewed bytes").unwrap();
        let proof = bound_final_object(&source);

        // Model the exact pre-arm interval: the initial same-handle profile is
        // clean, then another stream wins the race before disposition is armed.
        validate_final_disposition_profile(&proof.file, false, None).unwrap();
        let mut writer = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(&raced_ads)
            .unwrap();
        writer.write_all(b"before delete-pending").unwrap();

        let disposition_mode = platform_arm_delete_on_close(&proof.file).unwrap();
        // An already-open stream handle can remain writable even though NTFS now
        // refuses new stream opens. That is why re-checking only the default stream
        // bytes is insufficient and the post-arm stream inventory is mandatory.
        writer.write_all(b"; after delete-pending").unwrap();
        let error = validate_final_disposition_profile(&proof.file, true, Some(disposition_mode))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains("stream"));
        platform_cancel_delete_on_close_mode(&proof.file, disposition_mode).unwrap();
        drop(proof);
        drop(writer);

        assert_eq!(fs::read(&source).unwrap(), b"reviewed bytes");
        assert_eq!(
            fs::read(&raced_ads).unwrap(),
            b"before delete-pending; after delete-pending"
        );
    }

    #[cfg(windows)]
    #[test]
    fn final_disposition_keeps_named_stream_objects_but_deletes_plain_files_and_directories() {
        use std::io::Write as _;

        let dir = tempfile::tempdir().unwrap();
        let with_ads = dir.path().join("with-ads.bin");
        let ads = PathBuf::from(format!("{}:preserved", with_ads.display()));
        fs::write(&with_ads, b"default stream").unwrap();
        let mut stream = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&ads)
            .unwrap();
        stream.write_all(b"named stream").unwrap();
        drop(stream);

        let error = bound_final_object(&with_ads).delete_exact().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains("stream"));
        assert_eq!(fs::read(&with_ads).unwrap(), b"default stream");
        assert_eq!(fs::read(&ads).unwrap(), b"named stream");

        let directory_with_ads = dir.path().join("directory-with-ads");
        fs::create_dir(&directory_with_ads).unwrap();
        let directory_ads = PathBuf::from(format!("{}:preserved", directory_with_ads.display()));
        let mut stream = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&directory_ads)
            .unwrap();
        stream.write_all(b"directory named stream").unwrap();
        drop(stream);

        let error = bound_final_object(&directory_with_ads)
            .delete_exact()
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(error.to_string().contains("stream"));
        assert!(directory_with_ads.is_dir());
        assert_eq!(fs::read(&directory_ads).unwrap(), b"directory named stream");

        let plain = dir.path().join("plain.bin");
        fs::write(&plain, b"plain").unwrap();
        bound_final_object(&plain).delete_exact().unwrap();
        assert!(!plain.exists());

        let empty_directory = dir.path().join("empty-directory");
        fs::create_dir(&empty_directory).unwrap();
        bound_final_object(&empty_directory).delete_exact().unwrap();
        assert!(!empty_directory.exists());
    }

    #[cfg(windows)]
    #[test]
    fn object_archive_scratch_cleanup_still_disposes_an_object_with_ads() {
        use std::io::Write as _;

        let dir = tempfile::tempdir().unwrap();
        let scratch = dir.path().join("roundtrip-scratch.bin");
        let ads = PathBuf::from(format!("{}:roundtrip", scratch.display()));
        fs::write(&scratch, b"restored default stream").unwrap();
        let mut stream = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&ads)
            .unwrap();
        stream.write_all(b"restored named stream").unwrap();
        drop(stream);

        let (scratch_handle, directory) =
            platform_open_archive_object(&scratch, true, false).unwrap();
        assert!(!directory);
        platform_dispose_handle(scratch_handle).unwrap();
        assert!(!scratch.exists());
        assert!(fs::read(&ads).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn mapped_remote_drive_types_are_not_local_mutation_drives() {
        use windows_sys::Win32::System::WindowsProgramming::{
            DRIVE_CDROM, DRIVE_FIXED, DRIVE_NO_ROOT_DIR, DRIVE_RAMDISK, DRIVE_REMOTE,
            DRIVE_REMOVABLE, DRIVE_UNKNOWN,
        };

        assert!(windows_drive_type_is_local(DRIVE_FIXED));
        assert!(windows_drive_type_is_local(DRIVE_REMOVABLE));
        assert!(windows_drive_type_is_local(DRIVE_RAMDISK));
        for drive_type in [DRIVE_REMOTE, DRIVE_CDROM, DRIVE_UNKNOWN, DRIVE_NO_ROOT_DIR] {
            assert!(!windows_drive_type_is_local(drive_type));
        }
    }

    #[cfg(windows)]
    #[test]
    fn cloud_recall_and_reparse_attributes_are_rejected_individually() {
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
            FILE_ATTRIBUTE_RECALL_ON_OPEN, FILE_ATTRIBUTE_REPARSE_POINT,
        };

        assert!(windows_handle_attributes_are_local(0, 0));
        for attributes in [
            FILE_ATTRIBUTE_REPARSE_POINT,
            FILE_ATTRIBUTE_OFFLINE,
            FILE_ATTRIBUTE_RECALL_ON_OPEN,
            FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
        ] {
            assert!(!windows_handle_attributes_are_local(attributes, 0));
        }
        assert!(!windows_handle_attributes_are_local(0, 0xA000_000C));
    }

    #[cfg(windows)]
    #[test]
    fn create_new_refuses_a_destination_raced_in_at_the_hook() {
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("source.bin");
        let destination = dir.path().join("destination.bin");
        fs::write(&source_path, b"source").unwrap();
        let mut source = BoundFile::open_read(&source_path).unwrap();
        let hash = source.hash().unwrap();
        let raced = destination.clone();
        set_test_hook(Some(std::sync::Arc::new(move |point, _| {
            if point == RacePoint::BeforeDestinationCreate {
                fs::write(&raced, b"raced occupant").unwrap();
            }
        })));
        let result = copy_to_new(&mut source, &destination, &hash);
        set_test_hook(None);
        assert!(result.is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"raced occupant");
    }

    #[cfg(windows)]
    #[test]
    fn bound_source_denies_path_replacement_after_hash() {
        let dir = tempfile::tempdir().unwrap();
        let source_path = dir.path().join("source.bin");
        let destination = dir.path().join("destination.bin");
        let displaced = dir.path().join("displaced.bin");
        fs::write(&source_path, b"reviewed").unwrap();
        let mut source = BoundFile::open_for_move(&source_path).unwrap();
        let hash = source.hash().unwrap();
        let raced_source = source_path.clone();
        let raced_displaced = displaced.clone();
        set_test_hook(Some(std::sync::Arc::new(move |point, _| {
            if point == RacePoint::AfterSourceBoundAndHashed {
                assert!(fs::rename(&raced_source, &raced_displaced).is_err());
                assert!(!raced_displaced.exists());
            }
        })));
        let prepared = source.prepare_move(&destination, &hash).unwrap();
        set_test_hook(None);
        assert_eq!(prepared.kind(), MoveKind::Rename);
        assert_eq!(fs::read(destination).unwrap(), b"reviewed");
    }
}
