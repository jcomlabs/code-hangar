//! Privileged Windows object archive v2 foundation.
//!
//! This module is intentionally handle-centric. The elevated helper first
//! duplicates a medium-integrity, already-bound source handle and a CREATE_NEW
//! archive-container handle from the authenticated parent. It then reopens the
//! same source identity with backup/security privilege, captures only an
//! allowlisted BackupRead stream set, restores into a disposable same-volume
//! object with BackupWrite, re-captures it, and requires semantic equality.
//!
//! No function here deletes a user source or opens the application database.
//! Purge remains a parent-side exact-handle disposition after the journal CAS.

use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::FileStamp;

const MAGIC: &[u8; 8] = b"CHOBJV2\0";
const RECORD_BASIC: u8 = 1;
const RECORD_RAW_CHUNK: u8 = 2;
const RECORD_COMMIT: u8 = 0xff;
const RAW_HEADER_BYTES: usize = 20;
const IO_CHUNK_BYTES: usize = 1024 * 1024;
const MAX_STREAM_NAME_BYTES: usize = 64 * 1024;
const MAX_STREAMS: usize = 4096;

#[derive(Debug, Error)]
pub enum ObjectArchiveError {
    #[error("object archive is unsupported: {0}")]
    Unsupported(String),
    #[error("object archive identity proof failed: {0}")]
    Identity(String),
    #[error("object archive format is invalid: {0}")]
    Format(String),
    #[error("object archive scratch cleanup is pending: {0}")]
    ScratchCleanup(String),
    #[error("object archive io failed: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectArchiveProof {
    pub schema: String,
    pub source_stamp: FileStamp,
    pub archive_stamp: FileStamp,
    pub archive_blake3: String,
    pub raw_backup_blake3: String,
    pub semantic_blake3: String,
    pub roundtrip_blake3: String,
    pub stream_count: u32,
    pub security_stream_present: bool,
    pub cleanup_complete: bool,
}

/// Handle-bound inputs for creating a new object-complete archive proof.
pub struct FinalizeObjectArchiveParams<'a> {
    pub parent_pid: u32,
    pub source_parent_handle: u64,
    pub archive_parent_handle: u64,
    pub scratch_root_parent_handle: u64,
    pub expected_archive_path: &'a Path,
    pub source_path: &'a Path,
    pub scratch_leaf: &'a str,
    pub nonce: &'a str,
    pub expected_stamp: &'a FileStamp,
    pub expected_content_hash: &'a str,
    pub expected_scratch_root_stamp: &'a FileStamp,
}

/// Handle-bound inputs for re-verifying an existing object-complete archive.
pub struct VerifyObjectArchiveParams<'a> {
    pub parent_pid: u32,
    pub source_parent_handle: u64,
    pub archive_parent_handle: u64,
    pub scratch_root_parent_handle: u64,
    pub expected_archive_path: &'a Path,
    pub source_path: &'a Path,
    pub scratch_leaf: &'a str,
    pub expected_source_stamp: &'a FileStamp,
    pub expected_content_hash: &'a str,
    pub expected_archive_stamp: &'a FileStamp,
    pub expected_archive_hash: &'a str,
    pub expected_semantic: &'a str,
    pub expected_scratch_root_stamp: &'a FileStamp,
    pub allow_internal_directory_time_drift: bool,
}

#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct BasicObjectInfo {
    directory: bool,
    creation_time: i64,
    last_access_time: i64,
    last_write_time: i64,
    change_time: i64,
    attributes: u32,
    eof: u64,
    link_count: u32,
}

#[cfg(windows)]
#[derive(Debug, Clone)]
struct StreamDescriptor {
    id: u32,
    attributes: u32,
    name: Vec<u8>,
    size: u64,
    ordinal: u32,
    content_blake3: [u8; 32],
}

#[cfg(windows)]
struct CurrentStream {
    id: u32,
    attributes: u32,
    name: Vec<u8>,
    size: u64,
    ordinal: u32,
    remaining: u64,
    hasher: blake3::Hasher,
}

#[cfg(windows)]
enum ParserState {
    Header { bytes: Vec<u8>, needed: usize },
    Data(Box<CurrentStream>),
}

#[cfg(windows)]
struct RawStreamParser {
    state: ParserState,
    descriptors: Vec<StreamDescriptor>,
    security_streams: usize,
}

#[cfg(windows)]
impl RawStreamParser {
    fn new() -> Self {
        Self {
            state: ParserState::Header {
                bytes: Vec::with_capacity(RAW_HEADER_BYTES),
                needed: RAW_HEADER_BYTES,
            },
            descriptors: Vec::new(),
            security_streams: 0,
        }
    }

    fn feed(&mut self, mut input: &[u8]) -> Result<(), ObjectArchiveError> {
        use windows_sys::Win32::Storage::FileSystem::{
            BACKUP_ALTERNATE_DATA, BACKUP_DATA, BACKUP_EA_DATA, BACKUP_SECURITY_DATA,
        };

        while !input.is_empty() {
            match &mut self.state {
                ParserState::Header { bytes, needed } => {
                    let take = (*needed - bytes.len()).min(input.len());
                    bytes.extend_from_slice(&input[..take]);
                    input = &input[take..];
                    if bytes.len() != *needed {
                        continue;
                    }
                    if *needed == RAW_HEADER_BYTES {
                        let name_bytes = u32::from_le_bytes(
                            bytes[16..20].try_into().expect("wire header field"),
                        ) as usize;
                        if !name_bytes.is_multiple_of(2) || name_bytes > MAX_STREAM_NAME_BYTES {
                            return Err(ObjectArchiveError::Format(
                                "BackupRead emitted an invalid stream-name length".into(),
                            ));
                        }
                        *needed = RAW_HEADER_BYTES + name_bytes;
                        if name_bytes != 0 {
                            continue;
                        }
                    }

                    let id = u32::from_le_bytes(bytes[0..4].try_into().expect("stream id"));
                    let attributes =
                        u32::from_le_bytes(bytes[4..8].try_into().expect("stream attrs"));
                    let size = i64::from_le_bytes(bytes[8..16].try_into().expect("stream size"));
                    if size < 0 {
                        return Err(ObjectArchiveError::Format(
                            "BackupRead emitted a negative stream size".into(),
                        ));
                    }
                    if !matches!(
                        id,
                        BACKUP_DATA | BACKUP_ALTERNATE_DATA | BACKUP_EA_DATA | BACKUP_SECURITY_DATA
                    ) {
                        return Err(ObjectArchiveError::Unsupported(format!(
                            "BackupRead stream id {id} is not in the replay allowlist"
                        )));
                    }
                    let name = bytes[RAW_HEADER_BYTES..].to_vec();
                    if id == BACKUP_DATA && !name.is_empty() {
                        return Err(ObjectArchiveError::Format(
                            "unnamed data stream unexpectedly has a name".into(),
                        ));
                    }
                    if id == BACKUP_ALTERNATE_DATA {
                        validate_ads_name(&name)?;
                    }
                    if id == BACKUP_SECURITY_DATA {
                        self.security_streams += 1;
                        if self.security_streams > 1 || size == 0 {
                            return Err(ObjectArchiveError::Format(
                                "object-complete capture requires exactly one non-empty security stream"
                                    .into(),
                            ));
                        }
                    }
                    if self.descriptors.len() >= MAX_STREAMS {
                        return Err(ObjectArchiveError::Unsupported(
                            "object exceeds the bounded stream count".into(),
                        ));
                    }
                    let ordinal = self
                        .descriptors
                        .iter()
                        .filter(|entry| entry.id == id && entry.name == name)
                        .count() as u32;
                    let current = CurrentStream {
                        id,
                        attributes,
                        name,
                        size: size as u64,
                        ordinal,
                        remaining: size as u64,
                        hasher: blake3::Hasher::new(),
                    };
                    bytes.clear();
                    *needed = RAW_HEADER_BYTES;
                    self.state = ParserState::Data(Box::new(current));
                    self.finish_zero_length_stream();
                }
                ParserState::Data(current) => {
                    let take = current.remaining.min(input.len() as u64) as usize;
                    current.hasher.update(&input[..take]);
                    current.remaining -= take as u64;
                    input = &input[take..];
                    if current.remaining == 0 {
                        self.finish_current_stream();
                    }
                }
            }
        }
        Ok(())
    }

    fn finish_zero_length_stream(&mut self) {
        let zero = matches!(&self.state, ParserState::Data(current) if current.remaining == 0);
        if zero {
            self.finish_current_stream();
        }
    }

    fn finish_current_stream(&mut self) {
        let old = std::mem::replace(
            &mut self.state,
            ParserState::Header {
                bytes: Vec::with_capacity(RAW_HEADER_BYTES),
                needed: RAW_HEADER_BYTES,
            },
        );
        if let ParserState::Data(current) = old {
            let current = *current;
            self.descriptors.push(StreamDescriptor {
                id: current.id,
                attributes: current.attributes,
                name: current.name,
                size: current.size,
                ordinal: current.ordinal,
                content_blake3: *current.hasher.finalize().as_bytes(),
            });
        }
    }

    fn finish(self) -> Result<(Vec<StreamDescriptor>, bool), ObjectArchiveError> {
        match self.state {
            ParserState::Header { bytes, needed }
                if bytes.is_empty() && needed == RAW_HEADER_BYTES => {}
            _ => {
                return Err(ObjectArchiveError::Format(
                    "BackupRead ended inside a stream record".into(),
                ))
            }
        }
        if self.security_streams != 1 {
            return Err(ObjectArchiveError::Unsupported(
                "object-complete capture did not yield exactly one security stream".into(),
            ));
        }
        Ok((self.descriptors, true))
    }
}

#[cfg(windows)]
fn validate_ads_name(name: &[u8]) -> Result<(), ObjectArchiveError> {
    if name.len() < 4 || !name.len().is_multiple_of(2) {
        return Err(ObjectArchiveError::Format(
            "alternate data stream has an invalid UTF-16 name".into(),
        ));
    }
    let words = name
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect::<Vec<_>>();
    let text = String::from_utf16(&words).map_err(|_| {
        ObjectArchiveError::Format("alternate data stream name is not valid UTF-16".into())
    })?;
    if !text.starts_with(':') || !text.to_ascii_uppercase().ends_with(":$DATA") {
        return Err(ObjectArchiveError::Unsupported(format!(
            "alternate stream name is outside the replay grammar: {text:?}"
        )));
    }
    Ok(())
}

#[cfg(windows)]
fn basic_payload(info: &BasicObjectInfo) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(52);
    bytes.push(u8::from(info.directory));
    bytes.extend_from_slice(&[0; 3]);
    bytes.extend_from_slice(&info.creation_time.to_le_bytes());
    bytes.extend_from_slice(&info.last_access_time.to_le_bytes());
    bytes.extend_from_slice(&info.last_write_time.to_le_bytes());
    bytes.extend_from_slice(&info.change_time.to_le_bytes());
    bytes.extend_from_slice(&info.attributes.to_le_bytes());
    bytes.extend_from_slice(&info.eof.to_le_bytes());
    bytes.extend_from_slice(&info.link_count.to_le_bytes());
    bytes
}

#[cfg(windows)]
fn parse_basic_payload(bytes: &[u8]) -> Result<BasicObjectInfo, ObjectArchiveError> {
    if bytes.len() != 52 || bytes[1..4] != [0, 0, 0] {
        return Err(ObjectArchiveError::Format(
            "object basic-info record has invalid size/reserved bytes".into(),
        ));
    }
    Ok(BasicObjectInfo {
        directory: bytes[0] != 0,
        creation_time: i64::from_le_bytes(bytes[4..12].try_into().unwrap()),
        last_access_time: i64::from_le_bytes(bytes[12..20].try_into().unwrap()),
        last_write_time: i64::from_le_bytes(bytes[20..28].try_into().unwrap()),
        change_time: i64::from_le_bytes(bytes[28..36].try_into().unwrap()),
        attributes: u32::from_le_bytes(bytes[36..40].try_into().unwrap()),
        eof: u64::from_le_bytes(bytes[40..48].try_into().unwrap()),
        link_count: u32::from_le_bytes(bytes[48..52].try_into().unwrap()),
    })
}

#[cfg(windows)]
fn finish_scratch_cleanup<T>(
    verification: Result<T, ObjectArchiveError>,
    cleanup: std::io::Result<()>,
) -> Result<T, ObjectArchiveError> {
    match (verification, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(cleanup_error)) => Err(ObjectArchiveError::ScratchCleanup(
            cleanup_error.to_string(),
        )),
        (Err(verification_error), Err(cleanup_error)) => {
            Err(ObjectArchiveError::ScratchCleanup(format!(
                "verification failed ({verification_error}); exact-handle cleanup also failed ({cleanup_error})"
            )))
        }
    }
}

#[cfg(windows)]
fn semantic_digest(info: &BasicObjectInfo, descriptors: &[StreamDescriptor]) -> String {
    let mut ordered = descriptors.to_vec();
    ordered.sort_by(|left, right| {
        (left.id, &left.name, left.ordinal).cmp(&(right.id, &right.name, right.ordinal))
    });
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"codehangar-object-semantic-v2\0");
    hasher.update(&basic_payload(info));
    for entry in ordered {
        hasher.update(&entry.id.to_le_bytes());
        hasher.update(&entry.attributes.to_le_bytes());
        hasher.update(&(entry.name.len() as u32).to_le_bytes());
        hasher.update(&entry.name);
        hasher.update(&entry.size.to_le_bytes());
        hasher.update(&entry.ordinal.to_le_bytes());
        hasher.update(&entry.content_blake3);
    }
    hasher.finalize().to_hex().to_string()
}

#[cfg(windows)]
fn directory_semantic_digest_after_internal_child_removal(
    info: &BasicObjectInfo,
    descriptors: &[StreamDescriptor],
) -> String {
    let mut normalized = info.clone();
    normalized.last_write_time = 0;
    normalized.change_time = 0;
    // NTFS exposes directory-index allocation through EndOfFile. Removing the
    // batch's children can shrink it even though directories enumerate no data
    // stream. File EOF is never normalized by this directory-only predicate.
    normalized.eof = 0;
    semantic_digest(&normalized, descriptors)
}

#[cfg(windows)]
fn directory_basic_matches_after_internal_child_removal(
    archived: &BasicObjectInfo,
    current: &BasicObjectInfo,
) -> bool {
    let mut archived = archived.clone();
    let mut current = current.clone();
    archived.last_write_time = 0;
    archived.change_time = 0;
    current.last_write_time = 0;
    current.change_time = 0;
    archived.eof = 0;
    current.eof = 0;
    archived.directory && archived == current
}

#[cfg(windows)]
mod windows_impl {
    use super::*;
    use std::ffi::{OsStr, OsString};
    use std::fs::File;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};

    use windows_sys::Win32::Foundation::{
        CloseHandle, DuplicateHandle, DUPLICATE_SAME_ACCESS, GENERIC_READ, GENERIC_WRITE, HANDLE,
        INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        BackupRead, BackupWrite, CreateFileW, FileBasicInfo, FileStandardInfo,
        GetFileInformationByHandle, GetFileInformationByHandleEx, GetFinalPathNameByHandleW,
        GetVolumeInformationByHandleW, SetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        DELETE, FILE_ATTRIBUTE_COMPRESSED, FILE_ATTRIBUTE_ENCRYPTED, FILE_ATTRIBUTE_OFFLINE,
        FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS, FILE_ATTRIBUTE_RECALL_ON_OPEN,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_SPARSE_FILE, FILE_BASIC_INFO,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_NO_RECALL, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_NAME_NORMALIZED, FILE_SHARE_READ, FILE_STANDARD_INFO, OPEN_EXISTING, READ_CONTROL,
        VOLUME_NAME_DOS, WRITE_DAC, WRITE_OWNER,
    };
    use windows_sys::Win32::System::SystemServices::ACCESS_SYSTEM_SECURITY;
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, PROCESS_DUP_HANDLE, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    struct ProcessHandle(HANDLE);
    impl Drop for ProcessHandle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    fn duplicate_parent_file(parent_pid: u32, value: u64) -> Result<File, ObjectArchiveError> {
        if value == 0 || value == u64::MAX {
            return Err(ObjectArchiveError::Identity(
                "parent supplied an invalid handle value".into(),
            ));
        }
        let parent = unsafe {
            OpenProcess(
                PROCESS_DUP_HANDLE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                parent_pid,
            )
        };
        if parent.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        let parent = ProcessHandle(parent);
        let mut duplicate = std::ptr::null_mut();
        let duplicated = unsafe {
            DuplicateHandle(
                parent.0,
                value as HANDLE,
                GetCurrentProcess(),
                &mut duplicate,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        };
        if duplicated == 0 || duplicate.is_null() || duplicate == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(unsafe { File::from_raw_handle(duplicate as _) })
    }

    pub(super) fn require_archive_handle_path(
        archive: &File,
        expected_path: &Path,
    ) -> Result<(), ObjectArchiveError> {
        crate::bound_fs::validate_local_mutation_path(expected_path)?;

        // Prove the namespace entry through its own handle. Comparing the
        // caller-supplied spelling directly with GetFinalPathNameByHandleW is
        // not sufficient on Windows: the same path may arrive through a long
        // profile name in one process and its 8.3 alias in another. Opening the
        // authorized entry and comparing both object identity and the two
        // kernel-resolved paths accepts those equivalent spellings without
        // weakening the redirected-handle check.
        let expected = File::open(expected_path)?;
        let archive_stamp = crate::bound_fs::platform_file_stamp(archive)?;
        let expected_stamp = crate::bound_fs::platform_file_stamp(&expected)?;
        if !archive_stamp.same_object(&expected_stamp) {
            return Err(ObjectArchiveError::Identity(
                "duplicated archive handle is not the authorized archive object".into(),
            ));
        }

        let final_path = |file: &File| -> Result<std::path::PathBuf, ObjectArchiveError> {
            // Windows local paths are bounded to 32,767 UTF-16 code units. A
            // fixed stack-independent buffer avoids a second query and ensures the
            // duplicated handle, rather than a newly opened pathname, is proved.
            let mut buffer = vec![0u16; 32_768];
            let written = unsafe {
                GetFinalPathNameByHandleW(
                    file.as_raw_handle() as HANDLE,
                    buffer.as_mut_ptr(),
                    buffer.len() as u32,
                    FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
                )
            };
            if written == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            if written as usize >= buffer.len() {
                return Err(ObjectArchiveError::Identity(
                    "archive handle path exceeds the local path bound".into(),
                ));
            }
            buffer.truncate(written as usize);
            Ok(std::path::PathBuf::from(OsString::from_wide(&buffer)))
        };
        let actual = final_path(archive)?;
        let authorized = final_path(&expected)?;
        let normalize = |path: &Path| {
            let text = path.as_os_str().to_string_lossy().replace('/', "\\");
            text.strip_prefix(r"\\?\")
                .unwrap_or(text.as_ref())
                .trim_end_matches('\\')
                .to_string()
        };
        if !normalize(&actual).eq_ignore_ascii_case(&normalize(&authorized)) {
            return Err(ObjectArchiveError::Identity(
                "duplicated archive handle does not resolve to the authorized archive path".into(),
            ));
        }
        Ok(())
    }

    fn query_basic(file: &File) -> Result<BasicObjectInfo, ObjectArchiveError> {
        let handle = file.as_raw_handle() as HANDLE;
        let mut basic = FILE_BASIC_INFO::default();
        let basic_ok = unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileBasicInfo,
                (&mut basic as *mut FILE_BASIC_INFO).cast(),
                std::mem::size_of::<FILE_BASIC_INFO>() as u32,
            )
        };
        let mut standard = FILE_STANDARD_INFO::default();
        let standard_ok = unsafe {
            GetFileInformationByHandleEx(
                handle,
                FileStandardInfo,
                (&mut standard as *mut FILE_STANDARD_INFO).cast(),
                std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
            )
        };
        let mut identity = BY_HANDLE_FILE_INFORMATION::default();
        let identity_ok = unsafe { GetFileInformationByHandle(handle, &mut identity) };
        if basic_ok == 0 || standard_ok == 0 || identity_ok == 0 || standard.EndOfFile < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let refused = FILE_ATTRIBUTE_REPARSE_POINT
            | FILE_ATTRIBUTE_OFFLINE
            | FILE_ATTRIBUTE_RECALL_ON_OPEN
            | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
            | FILE_ATTRIBUTE_ENCRYPTED
            | FILE_ATTRIBUTE_COMPRESSED
            | FILE_ATTRIBUTE_SPARSE_FILE;
        if basic.FileAttributes & refused != 0 {
            return Err(ObjectArchiveError::Unsupported(format!(
                "object attributes 0x{:x} include reparse/cloud/EFS/compressed/sparse data not yet in the v2 allowlist",
                basic.FileAttributes
            )));
        }
        if identity.nNumberOfLinks != 1 {
            return Err(ObjectArchiveError::Unsupported(format!(
                "hardlink topology is not complete (link count {})",
                identity.nNumberOfLinks
            )));
        }
        Ok(BasicObjectInfo {
            directory: standard.Directory,
            creation_time: basic.CreationTime,
            last_access_time: basic.LastAccessTime,
            last_write_time: basic.LastWriteTime,
            change_time: basic.ChangeTime,
            attributes: basic.FileAttributes,
            eof: standard.EndOfFile as u64,
            link_count: identity.nNumberOfLinks,
        })
    }

    fn require_ntfs(file: &File, label: &str) -> Result<(), ObjectArchiveError> {
        let mut filesystem = [0u16; 32];
        let ok = unsafe {
            GetVolumeInformationByHandleW(
                file.as_raw_handle() as HANDLE,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                filesystem.as_mut_ptr(),
                filesystem.len() as u32,
            )
        };
        if ok == 0 {
            return Err(ObjectArchiveError::Unsupported(format!(
                "cannot prove {label} filesystem: {}",
                std::io::Error::last_os_error()
            )));
        }
        let end = filesystem
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(filesystem.len());
        let name = String::from_utf16(&filesystem[..end]).map_err(|_| {
            ObjectArchiveError::Unsupported(format!("{label} filesystem name is ambiguous"))
        })?;
        if !name.eq_ignore_ascii_case("NTFS") {
            return Err(ObjectArchiveError::Unsupported(format!(
                "{label} filesystem is {name}, not NTFS"
            )));
        }
        Ok(())
    }

    fn hash_default_data(file: &mut File, directory: bool) -> Result<String, ObjectArchiveError> {
        if directory {
            return Ok(blake3::hash(&[]).to_hex().to_string());
        }
        file.seek(SeekFrom::Start(0))?;
        let mut hasher = blake3::Hasher::new();
        std::io::copy(file, &mut hasher)?;
        file.seek(SeekFrom::Start(0))?;
        Ok(hasher.finalize().to_hex().to_string())
    }

    fn privileged_reopen(path: &Path, directory: bool) -> Result<File, ObjectArchiveError> {
        crate::bound_fs::validate_local_mutation_path(path)?;
        let _ancestors = crate::bound_fs::guard_existing_parent(path)?;
        let wide = crate::bound_fs::wide_path(path);
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ | READ_CONTROL | ACCESS_SYSTEM_SECURITY,
                FILE_SHARE_READ,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT
                    | FILE_FLAG_OPEN_NO_RECALL
                    | if directory {
                        FILE_FLAG_BACKUP_SEMANTICS
                    } else {
                        0
                    },
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(unsafe { File::from_raw_handle(handle as _) })
    }

    fn write_record(file: &mut File, kind: u8, payload: &[u8]) -> Result<(), ObjectArchiveError> {
        if payload.len() > IO_CHUNK_BYTES && kind != RECORD_BASIC && kind != RECORD_COMMIT {
            return Err(ObjectArchiveError::Format(
                "archive record exceeded the bounded chunk size".into(),
            ));
        }
        file.write_all(&[kind, 0, 0, 0])?;
        file.write_all(&(payload.len() as u64).to_le_bytes())?;
        file.write_all(payload)?;
        file.write_all(blake3::hash(payload).as_bytes())?;
        Ok(())
    }

    fn read_record(file: &mut File) -> Result<(u8, Vec<u8>), ObjectArchiveError> {
        let mut header = [0u8; 12];
        file.read_exact(&mut header)?;
        if header[1..4] != [0, 0, 0] {
            return Err(ObjectArchiveError::Format(
                "archive record reserved bytes are non-zero".into(),
            ));
        }
        let len = u64::from_le_bytes(header[4..12].try_into().unwrap());
        if len > IO_CHUNK_BYTES as u64 {
            return Err(ObjectArchiveError::Format(
                "archive record exceeds the fixed replay bound".into(),
            ));
        }
        let mut payload = vec![0u8; len as usize];
        file.read_exact(&mut payload)?;
        let mut hash = [0u8; 32];
        file.read_exact(&mut hash)?;
        if blake3::hash(&payload).as_bytes() != &hash {
            return Err(ObjectArchiveError::Format(
                "archive record digest mismatch".into(),
            ));
        }
        Ok((header[0], payload))
    }

    fn capture_to_archive(
        source: &File,
        archive: &mut File,
        basic: &BasicObjectInfo,
        nonce: &str,
        source_stamp: &FileStamp,
    ) -> Result<(String, String, u32, bool), ObjectArchiveError> {
        archive.seek(SeekFrom::Start(0))?;
        let archive_uuid = archive_uuid(nonce);
        let object_uuid = object_uuid(source_stamp);
        archive.write_all(MAGIC)?;
        archive.write_all(&32u32.to_le_bytes())?;
        archive.write_all(&1u32.to_le_bytes())?;
        archive.write_all(&archive_uuid)?;
        archive.write_all(&object_uuid)?;
        write_record(archive, RECORD_BASIC, &basic_payload(basic))?;

        let handle = source.as_raw_handle() as HANDLE;
        let mut context = std::ptr::null_mut();
        let mut raw_hasher = blake3::Hasher::new();
        let mut raw_len = 0u64;
        let mut parser = RawStreamParser::new();
        let mut buffer = vec![0u8; IO_CHUNK_BYTES];
        let capture_result = (|| {
            loop {
                let mut read = 0u32;
                let ok = unsafe {
                    BackupRead(
                        handle,
                        buffer.as_mut_ptr(),
                        buffer.len() as u32,
                        &mut read,
                        0,
                        1,
                        &mut context,
                    )
                };
                if ok == 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                if read == 0 {
                    break;
                }
                let chunk = &buffer[..read as usize];
                raw_len = raw_len.checked_add(read as u64).ok_or_else(|| {
                    ObjectArchiveError::Format("raw stream length overflow".into())
                })?;
                raw_hasher.update(chunk);
                parser.feed(chunk)?;
                write_record(archive, RECORD_RAW_CHUNK, chunk)?;
            }
            Ok::<_, ObjectArchiveError>(())
        })();
        let mut ignored = 0u32;
        let abort_ok = unsafe {
            BackupRead(
                handle,
                std::ptr::null_mut(),
                0,
                &mut ignored,
                1,
                1,
                &mut context,
            )
        };
        capture_result?;
        if abort_ok == 0 {
            return Err(ObjectArchiveError::Io(std::io::Error::last_os_error()));
        }
        let (descriptors, security) = parser.finish()?;
        let raw = raw_hasher.finalize().to_hex().to_string();
        let semantic = semantic_digest(basic, &descriptors);
        let mut commit = Vec::with_capacity(8 + 32 + 32 + 4);
        commit.extend_from_slice(&raw_len.to_le_bytes());
        commit.extend_from_slice(blake3::Hash::from_hex(&raw).unwrap().as_bytes());
        commit.extend_from_slice(blake3::Hash::from_hex(&semantic).unwrap().as_bytes());
        commit.extend_from_slice(&(descriptors.len() as u32).to_le_bytes());
        write_record(archive, RECORD_COMMIT, &commit)?;
        archive.sync_all()?;
        Ok((raw, semantic, descriptors.len() as u32, security))
    }

    fn create_scratch_relative(
        scratch_root: &File,
        leaf: &str,
        directory: bool,
    ) -> Result<File, ObjectArchiveError> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
        };

        #[repr(C)]
        struct UnicodeString {
            length: u16,
            maximum_length: u16,
            buffer: *mut u16,
        }
        #[repr(C)]
        struct ObjectAttributes {
            length: u32,
            root_directory: HANDLE,
            object_name: *mut UnicodeString,
            attributes: u32,
            security_descriptor: *mut core::ffi::c_void,
            security_quality_of_service: *mut core::ffi::c_void,
        }
        #[repr(C)]
        struct IoStatusBlock {
            status: isize,
            information: usize,
        }
        #[link(name = "ntdll")]
        unsafe extern "system" {
            fn NtCreateFile(
                file_handle: *mut HANDLE,
                desired_access: u32,
                object_attributes: *mut ObjectAttributes,
                io_status_block: *mut IoStatusBlock,
                allocation_size: *const i64,
                file_attributes: u32,
                share_access: u32,
                create_disposition: u32,
                create_options: u32,
                ea_buffer: *const core::ffi::c_void,
                ea_length: u32,
            ) -> i32;
            fn RtlNtStatusToDosError(status: i32) -> u32;
        }

        if leaf.is_empty()
            || leaf.len() > 240
            || !leaf.is_ascii()
            || leaf
                .bytes()
                .any(|byte| matches!(byte, b'\\' | b'/' | b':' | 0))
            || matches!(leaf, "." | "..")
        {
            return Err(ObjectArchiveError::Format(
                "scratch leaf is not a bounded plain ASCII name".into(),
            ));
        }
        let mut wide = OsStr::new(leaf).encode_wide().collect::<Vec<_>>();
        let byte_len = wide
            .len()
            .checked_mul(2)
            .and_then(|len| u16::try_from(len).ok())
            .ok_or_else(|| ObjectArchiveError::Format("scratch leaf is too long".into()))?;
        let mut unicode = UnicodeString {
            length: byte_len,
            maximum_length: byte_len,
            buffer: wide.as_mut_ptr(),
        };
        const OBJ_CASE_INSENSITIVE: u32 = 0x40;
        let mut attributes = ObjectAttributes {
            length: std::mem::size_of::<ObjectAttributes>() as u32,
            root_directory: scratch_root.as_raw_handle() as HANDLE,
            object_name: &mut unicode,
            attributes: OBJ_CASE_INSENSITIVE,
            security_descriptor: std::ptr::null_mut(),
            security_quality_of_service: std::ptr::null_mut(),
        };
        let mut io_status = IoStatusBlock {
            status: 0,
            information: 0,
        };
        let mut handle = std::ptr::null_mut();
        const FILE_CREATE: u32 = 2;
        const SYNCHRONIZE: u32 = 0x0010_0000;
        const FILE_DIRECTORY_FILE: u32 = 0x0000_0001;
        const FILE_NON_DIRECTORY_FILE: u32 = 0x0000_0040;
        const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x0000_0020;
        const FILE_OPEN_REPARSE_POINT_NT: u32 = 0x0020_0000;
        const FILE_OPEN_NO_RECALL_NT: u32 = 0x0040_0000;
        const FILE_DELETE_ON_CLOSE: u32 = 0x0000_1000;
        let type_option = if directory {
            FILE_DIRECTORY_FILE
        } else {
            FILE_NON_DIRECTORY_FILE
        };
        let attributes_on_create = if directory {
            FILE_ATTRIBUTE_DIRECTORY
        } else {
            FILE_ATTRIBUTE_NORMAL
        };
        let status = unsafe {
            NtCreateFile(
                &mut handle,
                GENERIC_READ
                    | GENERIC_WRITE
                    | DELETE
                    | WRITE_DAC
                    | WRITE_OWNER
                    | ACCESS_SYSTEM_SECURITY
                    | SYNCHRONIZE,
                &mut attributes,
                &mut io_status,
                std::ptr::null(),
                attributes_on_create,
                0,
                FILE_CREATE,
                type_option
                    | FILE_SYNCHRONOUS_IO_NONALERT
                    | FILE_DELETE_ON_CLOSE
                    | FILE_OPEN_REPARSE_POINT_NT
                    | FILE_OPEN_NO_RECALL_NT,
                std::ptr::null(),
                0,
            )
        };
        if status < 0 || handle.is_null() || handle == INVALID_HANDLE_VALUE {
            if !handle.is_null() && handle != INVALID_HANDLE_VALUE {
                unsafe {
                    windows_sys::Win32::Foundation::CloseHandle(handle);
                }
            }
            let win32 = unsafe { RtlNtStatusToDosError(status) };
            return Err(ObjectArchiveError::Io(std::io::Error::from_raw_os_error(
                win32 as i32,
            )));
        }
        Ok(unsafe { File::from_raw_handle(handle as _) })
    }

    fn write_backup_chunk(
        handle: HANDLE,
        context: &mut *mut core::ffi::c_void,
        mut chunk: &[u8],
    ) -> Result<(), ObjectArchiveError> {
        while !chunk.is_empty() {
            let mut written = 0u32;
            let ok = unsafe {
                BackupWrite(
                    handle,
                    chunk.as_ptr(),
                    chunk.len() as u32,
                    &mut written,
                    0,
                    1,
                    context,
                )
            };
            if ok == 0 || written == 0 || written as usize > chunk.len() {
                return Err(if ok == 0 {
                    std::io::Error::last_os_error().into()
                } else {
                    ObjectArchiveError::Format("BackupWrite made no valid forward progress".into())
                });
            }
            chunk = &chunk[written as usize..];
        }
        Ok(())
    }

    fn restore_archive(
        archive: &mut File,
        scratch: &mut File,
        expected_archive_uuid: Option<[u8; 16]>,
        expected_object_uuid: [u8; 16],
    ) -> Result<(BasicObjectInfo, String, String, u32), ObjectArchiveError> {
        archive.seek(SeekFrom::Start(0))?;
        let mut prefix = [0u8; 48];
        archive.read_exact(&mut prefix)?;
        if &prefix[..8] != MAGIC
            || u32::from_le_bytes(prefix[8..12].try_into().unwrap()) != 32
            || u32::from_le_bytes(prefix[12..16].try_into().unwrap()) != 1
        {
            return Err(ObjectArchiveError::Format(
                "archive v2 prefix is invalid".into(),
            ));
        }
        let archive_uuid: [u8; 16] = prefix[16..32].try_into().expect("fixed archive UUID field");
        let object_uuid: [u8; 16] = prefix[32..48].try_into().expect("fixed object UUID field");
        if archive_uuid == [0; 16]
            || object_uuid != expected_object_uuid
            || expected_archive_uuid.is_some_and(|expected| archive_uuid != expected)
        {
            return Err(ObjectArchiveError::Identity(
                "archive UUID/object identity binding does not match this capability".into(),
            ));
        }
        let (kind, basic_bytes) = read_record(archive)?;
        if kind != RECORD_BASIC {
            return Err(ObjectArchiveError::Format(
                "archive basic-info record is missing".into(),
            ));
        }
        let basic = parse_basic_payload(&basic_bytes)?;
        let handle = scratch.as_raw_handle() as HANDLE;
        let mut context = std::ptr::null_mut();
        let mut replay_raw = blake3::Hasher::new();
        let mut replay_raw_len = 0u64;
        let replay_result = (|| loop {
            let (kind, payload) = read_record(archive)?;
            match kind {
                RECORD_RAW_CHUNK => {
                    replay_raw_len = replay_raw_len
                        .checked_add(payload.len() as u64)
                        .ok_or_else(|| {
                            ObjectArchiveError::Format("archive raw length overflow".into())
                        })?;
                    replay_raw.update(&payload);
                    write_backup_chunk(handle, &mut context, &payload)?;
                }
                RECORD_COMMIT => {
                    if payload.len() != 76 {
                        return Err(ObjectArchiveError::Format(
                            "archive commit record has invalid size".into(),
                        ));
                    }
                    let committed_len = u64::from_le_bytes(payload[..8].try_into().unwrap());
                    let raw = hex_bytes(&payload[8..40]);
                    let semantic = hex_bytes(&payload[40..72]);
                    let count = u32::from_le_bytes(payload[72..76].try_into().unwrap());
                    if committed_len != replay_raw_len
                        || raw != replay_raw.finalize().to_hex().as_str()
                    {
                        return Err(ObjectArchiveError::Format(
                            "archive commit raw length or digest does not match replay records"
                                .into(),
                        ));
                    }
                    let mut trailing = [0u8; 1];
                    if archive.read(&mut trailing)? != 0 {
                        return Err(ObjectArchiveError::Format(
                            "archive contains trailing bytes after its commit".into(),
                        ));
                    }
                    return Ok((raw, semantic, count));
                }
                _ => {
                    return Err(ObjectArchiveError::Format(
                        "archive contains an unexpected record type".into(),
                    ))
                }
            }
        })();
        let mut ignored = 0u32;
        let abort_ok = unsafe {
            BackupWrite(
                handle,
                std::ptr::null(),
                0,
                &mut ignored,
                1,
                1,
                &mut context,
            )
        };
        let (raw, semantic, count) = replay_result?;
        if abort_ok == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let applied = FILE_BASIC_INFO {
            CreationTime: basic.creation_time,
            LastAccessTime: basic.last_access_time,
            LastWriteTime: basic.last_write_time,
            ChangeTime: basic.change_time,
            FileAttributes: basic.attributes,
        };
        let basic_ok = unsafe {
            SetFileInformationByHandle(
                handle,
                FileBasicInfo,
                (&applied as *const FILE_BASIC_INFO).cast(),
                std::mem::size_of::<FILE_BASIC_INFO>() as u32,
            )
        };
        if basic_ok == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        scratch.sync_all()?;
        let actual_basic = query_basic(scratch)?;
        if actual_basic != basic {
            return Err(ObjectArchiveError::Unsupported(
                "roundtrip scratch metadata differs from the archived basic metadata".into(),
            ));
        }
        Ok((actual_basic, raw, semantic, count))
    }

    fn archive_uuid(nonce: &str) -> [u8; 16] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"codehangar/object-archive/archive-uuid/2\0");
        hasher.update(&(nonce.len() as u64).to_le_bytes());
        hasher.update(nonce.as_bytes());
        hasher.finalize().as_bytes()[..16]
            .try_into()
            .expect("fixed digest prefix")
    }

    fn object_uuid(stamp: &FileStamp) -> [u8; 16] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"codehangar/object-archive/object-uuid/2\0");
        hasher.update(&(stamp.volume_id.len() as u64).to_le_bytes());
        hasher.update(stamp.volume_id.as_bytes());
        hasher.update(&(stamp.file_id.len() as u64).to_le_bytes());
        hasher.update(stamp.file_id.as_bytes());
        hasher.finalize().as_bytes()[..16]
            .try_into()
            .expect("fixed digest prefix")
    }

    fn recapture_semantic(
        file: &File,
        basic: &BasicObjectInfo,
    ) -> Result<(String, String, String, u32, bool), ObjectArchiveError> {
        let handle = file.as_raw_handle() as HANDLE;
        let mut context = std::ptr::null_mut();
        let mut parser = RawStreamParser::new();
        let mut raw = blake3::Hasher::new();
        let mut buffer = vec![0u8; IO_CHUNK_BYTES];
        let capture = (|| {
            loop {
                let mut read = 0u32;
                let ok = unsafe {
                    BackupRead(
                        handle,
                        buffer.as_mut_ptr(),
                        buffer.len() as u32,
                        &mut read,
                        0,
                        1,
                        &mut context,
                    )
                };
                if ok == 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                if read == 0 {
                    break;
                }
                let chunk = &buffer[..read as usize];
                raw.update(chunk);
                parser.feed(chunk)?;
            }
            Ok::<_, ObjectArchiveError>(())
        })();
        let mut ignored = 0u32;
        let abort_ok = unsafe {
            BackupRead(
                handle,
                std::ptr::null_mut(),
                0,
                &mut ignored,
                1,
                1,
                &mut context,
            )
        };
        capture?;
        if abort_ok == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let (descriptors, security_present) = parser.finish()?;
        Ok((
            raw.finalize().to_hex().to_string(),
            semantic_digest(basic, &descriptors),
            directory_semantic_digest_after_internal_child_removal(basic, &descriptors),
            descriptors.len() as u32,
            security_present,
        ))
    }

    fn hex_bytes(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            use std::fmt::Write as _;
            write!(&mut out, "{byte:02x}").expect("String write");
        }
        out
    }

    fn hash_file(file: &mut File) -> Result<String, ObjectArchiveError> {
        file.seek(SeekFrom::Start(0))?;
        let mut hasher = blake3::Hasher::new();
        std::io::copy(file, &mut hasher)?;
        file.seek(SeekFrom::Start(0))?;
        Ok(hasher.finalize().to_hex().to_string())
    }

    fn duplicate_and_verify_scratch_root(
        parent_pid: u32,
        parent_handle: u64,
        expected: &FileStamp,
        archive_volume_id: &str,
    ) -> Result<File, ObjectArchiveError> {
        let root = duplicate_parent_file(parent_pid, parent_handle)?;
        let stamp = crate::bound_fs::platform_file_stamp(&root)?;
        let basic = query_basic(&root)?;
        if !basic.directory
            || !stamp.same_object(expected)
            || expected.modified_unix_seconds.is_some()
                && stamp.modified_unix_seconds != expected.modified_unix_seconds
            || stamp.volume_id != archive_volume_id
        {
            return Err(ObjectArchiveError::Identity(
                "duplicated scratch-root handle is not the authorized directory on the archive volume"
                    .into(),
            ));
        }
        require_ntfs(&root, "scratch-root")?;
        Ok(root)
    }

    pub(super) fn verify_existing(
        privilege_guard: &crate::PrivilegeGuard,
        params: VerifyObjectArchiveParams<'_>,
    ) -> Result<ObjectArchiveProof, ObjectArchiveError> {
        let VerifyObjectArchiveParams {
            parent_pid,
            source_parent_handle,
            archive_parent_handle,
            scratch_root_parent_handle,
            expected_archive_path,
            source_path,
            scratch_leaf,
            expected_source_stamp,
            expected_content_hash,
            expected_archive_stamp,
            expected_archive_hash,
            expected_semantic,
            expected_scratch_root_stamp,
            allow_internal_directory_time_drift,
        } = params;
        privilege_guard
            .proof()
            .require_object_complete()
            .map_err(ObjectArchiveError::Unsupported)?;
        crate::bound_fs::validate_local_mutation_path(source_path)?;

        let mut parent_source = duplicate_parent_file(parent_pid, source_parent_handle)?;
        let mut archive = duplicate_parent_file(parent_pid, archive_parent_handle)?;
        require_archive_handle_path(&archive, expected_archive_path)?;
        let parent_stamp = crate::bound_fs::platform_file_stamp(&parent_source)?;
        let source_basic = query_basic(&parent_source)?;
        let source_time_matches = expected_source_stamp.modified_unix_seconds.is_none()
            || parent_stamp.modified_unix_seconds == expected_source_stamp.modified_unix_seconds;
        let directory_time_drift_allowed =
            allow_internal_directory_time_drift && source_basic.directory;
        if !parent_stamp.same_object(expected_source_stamp)
            || (!directory_time_drift_allowed
                && (parent_stamp.bytes != expected_source_stamp.bytes || !source_time_matches))
            || hash_default_data(&mut parent_source, source_basic.directory)?
                != expected_content_hash
        {
            return Err(ObjectArchiveError::Identity(
                "duplicated source no longer matches the persisted archive capability".into(),
            ));
        }

        let archive_stamp = crate::bound_fs::platform_file_stamp(&archive)?;
        if !archive_stamp.same_object(expected_archive_stamp)
            || archive_stamp.bytes != expected_archive_stamp.bytes
            || expected_archive_stamp.modified_unix_seconds.is_some()
                && archive_stamp.modified_unix_seconds
                    != expected_archive_stamp.modified_unix_seconds
            || hash_file(&mut archive)? != expected_archive_hash
        {
            return Err(ObjectArchiveError::Identity(
                "duplicated archive payload no longer matches its persisted identity and digest"
                    .into(),
            ));
        }
        require_ntfs(&parent_source, "source")?;
        require_ntfs(&archive, "archive")?;
        let scratch_root = duplicate_and_verify_scratch_root(
            parent_pid,
            scratch_root_parent_handle,
            expected_scratch_root_stamp,
            &archive_stamp.volume_id,
        )?;

        let privileged = privileged_reopen(source_path, source_basic.directory)?;
        let privileged_stamp = crate::bound_fs::platform_file_stamp(&privileged)?;
        let privileged_basic = query_basic(&privileged)?;
        if !privileged_stamp.same_object(&parent_stamp) || privileged_basic != source_basic {
            return Err(ObjectArchiveError::Identity(
                "privileged source reopen is not the parent-bound object".into(),
            ));
        }

        let mut scratch =
            create_scratch_relative(&scratch_root, scratch_leaf, source_basic.directory)?;
        let scratch_stamp = crate::bound_fs::platform_file_stamp(&scratch)?;
        if scratch_stamp.volume_id != archive_stamp.volume_id {
            return finish_scratch_cleanup(
                Err(ObjectArchiveError::Unsupported(
                    "archive verification scratch is not on the archive volume".into(),
                )),
                crate::bound_fs::platform_dispose_handle(scratch),
            );
        }
        let verify_result = (|| {
            let (restored_basic, raw, semantic, count) =
                restore_archive(&mut archive, &mut scratch, None, object_uuid(&parent_stamp))?;
            if semantic != expected_semantic {
                return Err(ObjectArchiveError::Identity(
                    "archive commit semantic digest differs from the persisted proof".into(),
                ));
            }
            let (
                roundtrip_raw,
                roundtrip_semantic,
                roundtrip_semantic_without_internal_times,
                roundtrip_count,
                roundtrip_security,
            ) = recapture_semantic(&scratch, &restored_basic)?;
            if roundtrip_raw != raw
                || roundtrip_semantic != semantic
                || roundtrip_count != count
                || !roundtrip_security
            {
                return Err(ObjectArchiveError::Unsupported(
                    "existing archive no longer round-trips every supported object stream".into(),
                ));
            }
            Ok::<_, ObjectArchiveError>((
                restored_basic,
                raw,
                semantic,
                roundtrip_semantic_without_internal_times,
                count,
                roundtrip_semantic,
            ))
        })();
        let cleanup = crate::bound_fs::platform_dispose_handle(scratch);
        let (
            restored_basic,
            raw,
            semantic,
            archived_semantic_without_internal_times,
            stream_count,
            roundtrip,
        ) = finish_scratch_cleanup(verify_result, cleanup)?;

        let source_after = crate::bound_fs::platform_file_stamp(&privileged)?;
        let current_basic = query_basic(&privileged)?;
        let (
            current_raw,
            current_semantic,
            current_semantic_without_internal_times,
            current_count,
            current_security,
        ) = recapture_semantic(&privileged, &current_basic)?;
        let source_matches_archive = if allow_internal_directory_time_drift {
            directory_basic_matches_after_internal_child_removal(&restored_basic, &current_basic)
                && current_semantic_without_internal_times
                    == archived_semantic_without_internal_times
                && current_count == stream_count
                && current_security
        } else {
            current_raw == raw
                && current_semantic == semantic
                && current_count == stream_count
                && current_security
        };
        if !source_after.same_object(&parent_stamp)
            || current_basic != source_basic
            || !source_matches_archive
        {
            return Err(ObjectArchiveError::Identity(
                "source object drifted after its archive proof was created".into(),
            ));
        }

        Ok(ObjectArchiveProof {
            schema: "object_archive/2".to_string(),
            source_stamp: source_after,
            archive_stamp,
            archive_blake3: expected_archive_hash.to_string(),
            raw_backup_blake3: raw,
            semantic_blake3: semantic,
            roundtrip_blake3: roundtrip,
            stream_count,
            security_stream_present: true,
            cleanup_complete: true,
        })
    }

    pub(super) fn finalize(
        privilege_guard: &crate::PrivilegeGuard,
        params: FinalizeObjectArchiveParams<'_>,
    ) -> Result<ObjectArchiveProof, ObjectArchiveError> {
        let FinalizeObjectArchiveParams {
            parent_pid,
            source_parent_handle,
            archive_parent_handle,
            scratch_root_parent_handle,
            expected_archive_path,
            source_path,
            scratch_leaf,
            nonce,
            expected_stamp,
            expected_content_hash,
            expected_scratch_root_stamp,
        } = params;
        privilege_guard
            .proof()
            .require_object_complete()
            .map_err(ObjectArchiveError::Unsupported)?;
        crate::bound_fs::validate_local_mutation_path(source_path)?;

        let mut parent_source = duplicate_parent_file(parent_pid, source_parent_handle)?;
        let mut archive = duplicate_parent_file(parent_pid, archive_parent_handle)?;
        require_archive_handle_path(&archive, expected_archive_path)?;
        let parent_stamp = crate::bound_fs::platform_file_stamp(&parent_source)?;
        let basic = query_basic(&parent_source)?;
        if !parent_stamp.same_object(expected_stamp)
            || parent_stamp.bytes != expected_stamp.bytes
            || expected_stamp.modified_unix_seconds.is_some()
                && parent_stamp.modified_unix_seconds != expected_stamp.modified_unix_seconds
        {
            return Err(ObjectArchiveError::Identity(
                "duplicated parent source does not match the capability stamp".into(),
            ));
        }
        if hash_default_data(&mut parent_source, basic.directory)? != expected_content_hash {
            return Err(ObjectArchiveError::Identity(
                "duplicated parent source content differs from the capability".into(),
            ));
        }

        let privileged = privileged_reopen(source_path, basic.directory)?;
        let privileged_stamp = crate::bound_fs::platform_file_stamp(&privileged)?;
        if !privileged_stamp.same_object(&parent_stamp) {
            return Err(ObjectArchiveError::Identity(
                "privileged reopen is not the parent-bound source object".into(),
            ));
        }
        let privileged_basic = query_basic(&privileged)?;
        if privileged_basic != basic {
            return Err(ObjectArchiveError::Identity(
                "source metadata changed before privileged capture".into(),
            ));
        }

        let archive_stamp = crate::bound_fs::platform_file_stamp(&archive)?;
        if archive_stamp.bytes != 0 {
            return Err(ObjectArchiveError::Identity(
                "parent archive handle is not a fresh CREATE_NEW object".into(),
            ));
        }
        require_ntfs(&parent_source, "source")?;
        require_ntfs(&archive, "archive")?;
        let scratch_root = duplicate_and_verify_scratch_root(
            parent_pid,
            scratch_root_parent_handle,
            expected_scratch_root_stamp,
            &archive_stamp.volume_id,
        )?;
        let (raw, semantic, stream_count, security) =
            capture_to_archive(&privileged, &mut archive, &basic, nonce, &parent_stamp)?;

        let mut scratch = create_scratch_relative(&scratch_root, scratch_leaf, basic.directory)?;
        let scratch_stamp = crate::bound_fs::platform_file_stamp(&scratch)?;
        if scratch_stamp.volume_id != archive_stamp.volume_id {
            return finish_scratch_cleanup(
                Err(ObjectArchiveError::Unsupported(
                    "roundtrip scratch is not on the archive destination volume".into(),
                )),
                crate::bound_fs::platform_dispose_handle(scratch),
            );
        }
        let verify_result = (|| {
            let (restored_basic, restored_raw, restored_semantic, restored_count) =
                restore_archive(
                    &mut archive,
                    &mut scratch,
                    Some(archive_uuid(nonce)),
                    object_uuid(&parent_stamp),
                )?;
            if restored_raw != raw
                || restored_semantic != semantic
                || restored_count != stream_count
            {
                return Err(ObjectArchiveError::Format(
                    "archive commit disagrees with captured object proof".into(),
                ));
            }
            let (
                roundtrip_raw,
                roundtrip_semantic,
                _roundtrip_semantic_without_internal_times,
                roundtrip_count,
                roundtrip_security,
            ) = recapture_semantic(&scratch, &restored_basic)?;
            if roundtrip_raw != raw
                || roundtrip_semantic != semantic
                || roundtrip_count != stream_count
                || !roundtrip_security
            {
                return Err(ObjectArchiveError::Unsupported(
                    "BackupWrite roundtrip did not reproduce every allowlisted stream and metadata field"
                        .into(),
                ));
            }
            Ok::<_, ObjectArchiveError>(roundtrip_semantic)
        })();
        // The verification object is always disposed through the exact CREATE_NEW
        // handle. If cleanup fails, the archive is never purge-eligible.
        let cleanup = crate::bound_fs::platform_dispose_handle(scratch);
        let roundtrip = finish_scratch_cleanup(verify_result, cleanup)?;

        // Re-capture the still-parent-bound source immediately before readiness.
        let current_stamp = crate::bound_fs::platform_file_stamp(&privileged)?;
        let current_basic = query_basic(&privileged)?;
        let (
            current_raw,
            current_semantic,
            _current_semantic_without_internal_times,
            current_count,
            current_security,
        ) = recapture_semantic(&privileged, &current_basic)?;
        if !current_stamp.same_object(&parent_stamp)
            || current_basic != basic
            || current_raw != raw
            || current_semantic != semantic
            || current_count != stream_count
            || !current_security
        {
            return Err(ObjectArchiveError::Identity(
                "source object changed between archive capture and purge-readiness proof".into(),
            ));
        }

        archive.sync_all()?;
        let archive_blake3 = hash_file(&mut archive)?;
        let final_archive_stamp = crate::bound_fs::platform_file_stamp(&archive)?;
        Ok(ObjectArchiveProof {
            schema: "object_archive/2".to_string(),
            source_stamp: current_stamp,
            archive_stamp: final_archive_stamp,
            archive_blake3,
            raw_backup_blake3: raw,
            semantic_blake3: semantic,
            roundtrip_blake3: roundtrip,
            stream_count,
            security_stream_present: security,
            cleanup_complete: true,
        })
    }
}

/// Execute one object-complete archive finalization inside the authenticated,
/// elevated helper process. `source_parent_handle` and
/// `archive_parent_handle` are duplicated from the verified parent process;
/// neither is accepted from an unauthenticated command line.
#[cfg(windows)]
pub fn finalize_object_archive_v2(
    privilege_guard: &crate::PrivilegeGuard,
    params: FinalizeObjectArchiveParams<'_>,
) -> Result<ObjectArchiveProof, ObjectArchiveError> {
    windows_impl::finalize(privilege_guard, params)
}

/// Rebind and round-trip an already committed object_archive/2 payload. This is
/// the last elevated read-only proof before the medium-integrity parent may
/// commit delete intent and dispose the exact held object handle.
#[cfg(windows)]
pub fn verify_object_archive_v2(
    privilege_guard: &crate::PrivilegeGuard,
    params: VerifyObjectArchiveParams<'_>,
) -> Result<ObjectArchiveProof, ObjectArchiveError> {
    windows_impl::verify_existing(privilege_guard, params)
}

#[cfg(not(windows))]
pub fn finalize_object_archive_v2(
    _privilege_guard: &crate::PrivilegeGuard,
    _params: FinalizeObjectArchiveParams<'_>,
) -> Result<ObjectArchiveProof, ObjectArchiveError> {
    Err(ObjectArchiveError::Unsupported(
        "object archive v2 requires the Windows elevated helper".to_string(),
    ))
}

#[cfg(not(windows))]
pub fn verify_object_archive_v2(
    _privilege_guard: &crate::PrivilegeGuard,
    _params: VerifyObjectArchiveParams<'_>,
) -> Result<ObjectArchiveProof, ObjectArchiveError> {
    Err(ObjectArchiveError::Unsupported(
        "object archive v2 verification requires the Windows elevated helper".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn basic_record_roundtrip_includes_change_time_and_rejects_the_old_layout() {
        let basic = BasicObjectInfo {
            directory: false,
            creation_time: 11,
            last_access_time: 22,
            last_write_time: 33,
            change_time: 44,
            attributes: 0x20,
            eof: 55,
            link_count: 1,
        };
        let encoded = basic_payload(&basic);
        assert_eq!(encoded.len(), 52);
        assert_eq!(parse_basic_payload(&encoded).unwrap(), basic);
        assert!(parse_basic_payload(&encoded[..44]).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn directory_archive_child_removal_exception_is_typed_and_metadata_bounded() {
        let archived = BasicObjectInfo {
            directory: true,
            creation_time: 11,
            last_access_time: 22,
            last_write_time: 33,
            change_time: 44,
            attributes: 0x10,
            eof: 0,
            link_count: 1,
        };
        let mut internally_changed = archived.clone();
        internally_changed.last_write_time = 333;
        internally_changed.change_time = 444;
        assert!(directory_basic_matches_after_internal_child_removal(
            &archived,
            &internally_changed
        ));
        assert_eq!(
            directory_semantic_digest_after_internal_child_removal(&archived, &[]),
            directory_semantic_digest_after_internal_child_removal(&internally_changed, &[])
        );

        internally_changed.eof = 24_576;
        assert!(directory_basic_matches_after_internal_child_removal(
            &archived,
            &internally_changed
        ));

        for mutate in [
            |value: &mut BasicObjectInfo| value.directory = false,
            |value: &mut BasicObjectInfo| value.creation_time += 1,
            |value: &mut BasicObjectInfo| value.last_access_time += 1,
            |value: &mut BasicObjectInfo| value.attributes ^= 0x20,
            |value: &mut BasicObjectInfo| value.link_count += 1,
        ] {
            let mut changed = internally_changed.clone();
            mutate(&mut changed);
            assert!(!directory_basic_matches_after_internal_child_removal(
                &archived, &changed
            ));
        }
    }

    #[cfg(windows)]
    #[test]
    fn cleanup_failure_is_never_masked_by_a_verification_failure() {
        let verification = Err::<(), _>(ObjectArchiveError::Identity("drift".into()));
        let cleanup = Err(std::io::Error::other("still present"));
        let error = finish_scratch_cleanup(verification, cleanup).unwrap_err();
        assert!(matches!(error, ObjectArchiveError::ScratchCleanup(_)));
        let detail = error.to_string();
        assert!(detail.contains("drift"));
        assert!(detail.contains("still present"));
    }

    #[cfg(windows)]
    #[test]
    fn duplicated_archive_handle_must_resolve_to_the_authorized_path() {
        let dir = tempfile::tempdir().unwrap();
        let authorized = dir.path().join("authorized.chobj");
        let redirected = dir.path().join("redirected.chobj");
        std::fs::write(&authorized, b"archive").unwrap();
        std::fs::write(&redirected, b"other").unwrap();
        let file = std::fs::File::open(&authorized).unwrap();

        windows_impl::require_archive_handle_path(&file, &authorized).unwrap();
        assert!(windows_impl::require_archive_handle_path(&file, &redirected).is_err());
    }

    #[test]
    fn non_windows_or_medium_process_cannot_fabricate_object_complete_proof() {
        let dir = tempfile::tempdir().unwrap();
        let stamp = FileStamp {
            volume_id: "fixture".to_string(),
            file_id: "fixture".to_string(),
            bytes: 0,
            modified_unix_seconds: None,
        };
        match crate::enable_object_backup_privileges() {
            Err(_) => {}
            Ok(privilege_guard) => {
                let archive_path = dir.path().join("archive.chobj");
                let source_path = dir.path().join("source.bin");
                let nonce = "ab".repeat(32);
                let empty_hash = blake3::hash(&[]).to_hex();
                let result = finalize_object_archive_v2(
                    &privilege_guard,
                    FinalizeObjectArchiveParams {
                        parent_pid: std::process::id(),
                        source_parent_handle: 1,
                        archive_parent_handle: 2,
                        scratch_root_parent_handle: 3,
                        expected_archive_path: &archive_path,
                        source_path: &source_path,
                        scratch_leaf: ".codehangar-roundtrip-test-00000000.tmp",
                        nonce: &nonce,
                        expected_stamp: &stamp,
                        expected_content_hash: empty_hash.as_ref(),
                        expected_scratch_root_stamp: &stamp,
                    },
                );
                assert!(result.is_err());
            }
        }
    }
}
