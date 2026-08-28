//! Strict, one-shot protocol for the local elevated object-backup helper.
//!
//! The command line selects only a random named pipe, parent PID and nonce. All
//! filesystem authority travels inside this authenticated, typed capability
//! after both peers have verified pipe/process identity. There is deliberately
//! no generic command, shell string, arbitrary copy verb or reusable token.

use std::path::{Path, PathBuf};

use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::{bound_fs, FileStamp};

pub const PROTOCOL_SCHEMA: &str = "codehangar/elevated-object-helper/2";
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
pub const MAX_CAPABILITY_LIFETIME_SECONDS: i64 = 120;
pub const FRAME_PREFIX_BYTES: usize = 4;
pub const FRAME_HEADER_BYTES: usize = 96;
pub const FRAME_MAC_BYTES: usize = 32;
pub const FRAME_MIN_DECLARED_BYTES: usize = FRAME_HEADER_BYTES + FRAME_MAC_BYTES;
pub const FRAME_WIRE_VERSION: u8 = 1;
pub const OBJECT_ARCHIVE_DIRECTORY_NAME: &str = ".codehangar-object-archive-v2";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParentBinding {
    pub pid: u32,
    pub session_id: u32,
    pub process_started_100ns: u64,
    /// SHA-256 of the release-signed Code Hangar executable independently
    /// compared with the offline-signed release identity manifest.
    pub image_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectedObject {
    pub path: PathBuf,
    /// Raw handle value in the bound parent process. The helper duplicates it
    /// from the already-verified parent with DuplicateHandle before reopening
    /// privileged metadata, preventing an elevated arbitrary-file reader.
    pub parent_handle_value: u64,
    pub stamp: FileStamp,
    pub content_blake3: String,
    /// Required for purge/round-trip; absent only while ObjectBackupV2 is
    /// creating the first complete proof for this exact object.
    pub semantic_blake3: Option<String>,
    /// Narrow existing-archive exception for a directory whose original
    /// object_archive/2 proof remains authoritative after this exact batch
    /// removed every planned descendant. The helper may normalize only the
    /// directory last-write/change times and its NTFS directory-index EOF;
    /// identity, type, other metadata and every stream remain exact. File EOF
    /// never receives this exception.
    #[serde(default)]
    pub allow_internal_directory_time_drift: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectedScratchRoot {
    /// Audit label and synthetic-fixture boundary only. The helper never uses
    /// this pathname to create an object; creation is relative to the duplicated
    /// parent directory handle below.
    pub path: PathBuf,
    pub parent_handle_value: u64,
    pub stamp: FileStamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ElevatedCapability {
    ObjectBackupV2 {
        source: ExpectedObject,
        parent_archive_handle_value: u64,
        /// Exact parent-created CREATE_NEW object that the duplicated handle
        /// must identify. It is derived from the transport nonce and global
        /// capability index before UAC, so a raw handle cannot redirect the
        /// privileged BackupWrite destination.
        archive_path: PathBuf,
        scratch_root: ExpectedScratchRoot,
        scratch_leaf: String,
    },
    RoundtripVerify {
        source: ExpectedObject,
        parent_archive_handle_value: u64,
        /// Exact committed object_archive/2 path already bound by the journal.
        /// The helper proves that the duplicated archive handle resolves here
        /// before reading or restoring any archive bytes.
        archive_path: PathBuf,
        expected_archive_stamp: FileStamp,
        expected_archive_blake3: String,
        scratch_root: ExpectedScratchRoot,
        scratch_leaf: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElevatedRequest {
    pub schema: String,
    /// 256-bit hex nonce also used in the one-shot pipe name.
    pub nonce: String,
    pub issued_at_unix_seconds: i64,
    pub expires_at_unix_seconds: i64,
    pub parent: ParentBinding,
    /// Opaque v2 plan fingerprint rebuilt by the non-elevated backend.
    pub plan_fingerprint: String,
    pub operation_id: i64,
    /// Digest of the committed DB capability row/operation intent.
    pub journal_capability_blake3: String,
    /// Unsigned development builds may only set this for synthetic temp
    /// fixtures. Production purge rejects it and release builds reject unsigned
    /// peers independently of this field.
    #[serde(default)]
    pub synthetic_test: bool,
    /// A bounded batch authorized by one UAC consent and one journal intent.
    /// Capabilities are executed in order and each produces a per-item result.
    pub capabilities: Vec<ElevatedCapability>,
}

/// Hard upper bound for one UAC-backed operation. Capabilities are transported
/// in authenticated chunks, so this is an operation safety limit rather than a
/// frame-size limit. The non-elevated preview must enforce the same bound before
/// asking for confirmation or launching UAC.
pub const MAX_CAPABILITIES_PER_INVOCATION: usize = 131_072;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElevatedSuccess {
    pub schema: String,
    pub nonce: String,
    pub operation_id: i64,
    pub helper_image_sha256: String,
    pub privilege_bitmap: u32,
    pub items: Vec<ElevatedItemResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "item_result", rename_all = "snake_case")]
pub enum ElevatedItemResult {
    Ready(Box<ElevatedObjectResult>),
    Blocked {
        capability_index: u32,
        code: String,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElevatedObjectResult {
    pub capability_index: u32,
    pub proof_schema: String,
    pub source_before: FileStamp,
    pub source_after: FileStamp,
    /// Identity of the exact parent-created archive handle after the helper
    /// has flushed the complete object_archive/2 payload. The medium-integrity
    /// parent must compare this authenticated stamp with its still-open
    /// CREATE_NEW handle before promoting the archive or authorizing deletion.
    pub archive_stamp: Option<FileStamp>,
    pub archive_blake3: Option<String>,
    pub raw_backup_blake3: Option<String>,
    pub semantic_blake3: Option<String>,
    pub roundtrip_blake3: Option<String>,
    pub stream_count: Option<u32>,
    pub security_stream_present: bool,
    pub cleanup_complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ElevatedFailure {
    pub schema: String,
    pub nonce: String,
    pub operation_id: i64,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ElevatedResponse {
    Success(ElevatedSuccess),
    Failure(ElevatedFailure),
}

impl ElevatedRequest {
    pub fn validate(
        &self,
        now_unix_seconds: i64,
        expected_nonce: &str,
        expected_parent_pid: u32,
        expected_parent_session: u32,
        synthetic_root: Option<&Path>,
    ) -> Result<(), String> {
        self.validate_capability_slice(
            now_unix_seconds,
            expected_nonce,
            expected_parent_pid,
            expected_parent_session,
            synthetic_root,
            0,
        )
    }

    /// Validate one authenticated streaming slice using its immutable global
    /// capability offset. Scratch leaf authority is nonce *and global-index*
    /// bound, so replaying a valid chunk at another position is rejected.
    pub fn validate_capability_slice(
        &self,
        now_unix_seconds: i64,
        expected_nonce: &str,
        expected_parent_pid: u32,
        expected_parent_session: u32,
        synthetic_root: Option<&Path>,
        base_index: u32,
    ) -> Result<(), String> {
        if self.schema != PROTOCOL_SCHEMA {
            return Err("unsupported elevated-helper protocol schema".to_string());
        }
        require_hex(&self.nonce, 64, "nonce")?;
        if self.nonce != expected_nonce {
            return Err("elevated-helper nonce mismatch".to_string());
        }
        if self.parent.pid == 0
            || self.parent.pid != expected_parent_pid
            || self.parent.session_id != expected_parent_session
            || self.parent.process_started_100ns == 0
        {
            return Err("elevated-helper parent process binding mismatch".to_string());
        }
        require_hex(&self.parent.image_sha256, 64, "parent image SHA-256")?;
        if self.operation_id <= 0 {
            return Err("elevated-helper capability has no committed operation id".to_string());
        }
        if self.plan_fingerprint.len() != 67
            || !self.plan_fingerprint.starts_with("v2:")
            || !self.plan_fingerprint[3..]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("elevated-helper capability has no v2 plan fingerprint".to_string());
        }
        require_hex(
            &self.journal_capability_blake3,
            64,
            "journal capability digest",
        )?;
        if self.issued_at_unix_seconds > now_unix_seconds
            || self.expires_at_unix_seconds < now_unix_seconds
            || self.expires_at_unix_seconds - self.issued_at_unix_seconds
                > MAX_CAPABILITY_LIFETIME_SECONDS
        {
            return Err("elevated-helper capability is expired or has an invalid lifetime".into());
        }

        let global_end = (base_index as usize)
            .checked_add(self.capabilities.len())
            .ok_or_else(|| "elevated-helper capability index overflow".to_string())?;
        if self.capabilities.is_empty() || global_end > MAX_CAPABILITIES_PER_INVOCATION {
            return Err("elevated-helper capability batch has an invalid size".to_string());
        }
        for (index, capability) in self.capabilities.iter().enumerate() {
            let global_index = base_index
                .checked_add(
                    u32::try_from(index)
                        .map_err(|_| "elevated-helper capability index overflow".to_string())?,
                )
                .ok_or_else(|| "elevated-helper capability index overflow".to_string())?;
            let required_leaf = scratch_leaf_for_capability(&self.nonce, global_index);
            match capability {
                ElevatedCapability::ObjectBackupV2 {
                    source,
                    parent_archive_handle_value,
                    archive_path,
                    scratch_root,
                    scratch_leaf,
                } => {
                    validate_expected_object(source, false)?;
                    if source.allow_internal_directory_time_drift {
                        return Err(
                            "new object archive cannot authorize pre-capture directory time drift"
                                .to_string(),
                        );
                    }
                    require_parent_handle(*parent_archive_handle_value, "archive")?;
                    validate_scratch_root(scratch_root)?;
                    bound_fs::validate_local_mutation_path(archive_path)
                        .map_err(|error| error.to_string())?;
                    if archive_path
                        != &archive_path_for_capability(
                            &scratch_root.path,
                            &self.nonce,
                            global_index,
                        )
                    {
                        return Err(
                            "new archive path is not scratch-root/nonce/index bound".to_string()
                        );
                    }
                    if scratch_leaf != &required_leaf {
                        return Err("scratch leaf is not nonce/index bound".to_string());
                    }
                }
                ElevatedCapability::RoundtripVerify {
                    source,
                    parent_archive_handle_value,
                    archive_path,
                    expected_archive_stamp,
                    expected_archive_blake3,
                    scratch_root,
                    scratch_leaf,
                } => {
                    validate_expected_object(source, true)?;
                    require_parent_handle(*parent_archive_handle_value, "archive")?;
                    if expected_archive_stamp.volume_id.is_empty()
                        || expected_archive_stamp.file_id.is_empty()
                    {
                        return Err("roundtrip archive has no object identity".to_string());
                    }
                    require_hex(expected_archive_blake3, 64, "archive digest")?;
                    validate_scratch_root(scratch_root)?;
                    validate_committed_archive_path(archive_path, &scratch_root.path)?;
                    if scratch_leaf != &required_leaf {
                        return Err("scratch leaf is not nonce/index bound".to_string());
                    }
                }
            }
        }

        if self.synthetic_test {
            let root = synthetic_root.ok_or_else(|| {
                "synthetic elevated capability is disabled in this process".to_string()
            })?;
            bound_fs::validate_local_mutation_path(root).map_err(|error| error.to_string())?;
            for path in self.authorized_paths() {
                if !crate::backup::is_inside(path, root) {
                    return Err("synthetic capability escaped its explicit fixture root".into());
                }
            }
        }
        Ok(())
    }

    /// Validate the immutable, handle-neutral authorization projection used by
    /// the streamed launcher before UAC. Raw handle values are intentionally
    /// ignored here because a lazy batch does not create them until its chunk
    /// is about to be sent. The helper still runs `validate_capability_slice`
    /// on every materialized chunk and duplicates/validates each real handle.
    pub(crate) fn validate_authorization_slice(
        &self,
        now_unix_seconds: i64,
        expected_nonce: &str,
        expected_parent_pid: u32,
        expected_parent_session: u32,
        synthetic_root: Option<&Path>,
        base_index: u32,
    ) -> Result<(), String> {
        let mut authorization = self.clone();
        for capability in &mut authorization.capabilities {
            neutralize_capability_handles(capability, 1);
        }
        authorization.validate_capability_slice(
            now_unix_seconds,
            expected_nonce,
            expected_parent_pid,
            expected_parent_session,
            synthetic_root,
            base_index,
        )
    }

    fn authorized_paths(&self) -> Vec<&Path> {
        self.capabilities
            .iter()
            .flat_map(|capability| match capability {
                ElevatedCapability::ObjectBackupV2 {
                    source,
                    archive_path,
                    scratch_root,
                    ..
                }
                | ElevatedCapability::RoundtripVerify {
                    source,
                    archive_path,
                    scratch_root,
                    ..
                } => vec![
                    source.path.as_path(),
                    archive_path.as_path(),
                    scratch_root.path.as_path(),
                ],
            })
            .collect()
    }
}

/// Replace process-local handle values while preserving every durable field
/// that is bound to the preview/journal. A zero replacement is the canonical
/// wire commitment; a non-zero replacement is used solely to reuse strict
/// structural validation before the real handles exist.
pub(crate) fn neutralize_capability_handles(capability: &mut ElevatedCapability, replacement: u64) {
    match capability {
        ElevatedCapability::ObjectBackupV2 {
            source,
            parent_archive_handle_value,
            scratch_root,
            ..
        }
        | ElevatedCapability::RoundtripVerify {
            source,
            parent_archive_handle_value,
            scratch_root,
            ..
        } => {
            source.parent_handle_value = replacement;
            *parent_archive_handle_value = replacement;
            scratch_root.parent_handle_value = replacement;
        }
    }
}

fn validate_scratch_root(root: &ExpectedScratchRoot) -> Result<(), String> {
    bound_fs::validate_local_mutation_path(&root.path).map_err(|error| error.to_string())?;
    require_parent_handle(root.parent_handle_value, "scratch root")?;
    if root.stamp.volume_id.is_empty() || root.stamp.file_id.is_empty() {
        return Err("scratch root capability has no volume/file identity".to_string());
    }
    Ok(())
}

pub fn scratch_leaf_for_capability(nonce: &str, index: u32) -> String {
    format!(".codehangar-roundtrip-{nonce}-{index:08x}.tmp")
}

/// Exact partial archive path authorized for one new-object capability. The
/// random one-shot transport nonce makes the name unguessable across runs; the
/// global index prevents two chunks in the same confirmed batch from sharing a
/// destination.
pub fn archive_path_for_capability(scratch_root: &Path, nonce: &str, index: u32) -> PathBuf {
    scratch_root
        .join(OBJECT_ARCHIVE_DIRECTORY_NAME)
        .join(format!(".codehangar-archive-{nonce}-{index:08x}.partial"))
}

fn validate_committed_archive_path(path: &Path, scratch_root: &Path) -> Result<(), String> {
    bound_fs::validate_local_mutation_path(path).map_err(|error| error.to_string())?;
    let expected_parent = scratch_root.join(OBJECT_ARCHIVE_DIRECTORY_NAME);
    if path.parent() != Some(expected_parent.as_path()) {
        return Err("committed archive is not a direct child of its bound archive root".into());
    }
    let leaf = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "committed archive has no bounded UTF-8 leaf".to_string())?;
    let entry = leaf
        .strip_prefix("entry-")
        .and_then(|value| value.strip_suffix(".chobj"))
        .ok_or_else(|| "committed archive leaf does not match object_archive/2".to_string())?;
    if entry.len() != 16 || !entry.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("committed archive leaf has an invalid entry identity".into());
    }
    Ok(())
}

fn validate_expected_object(object: &ExpectedObject, require_semantic: bool) -> Result<(), String> {
    bound_fs::validate_local_mutation_path(&object.path).map_err(|error| error.to_string())?;
    if object.stamp.volume_id.is_empty() || object.stamp.file_id.is_empty() {
        return Err("object capability has no volume/file identity".to_string());
    }
    require_parent_handle(object.parent_handle_value, "source")?;
    require_hex(&object.content_blake3, 64, "object content digest")?;
    match (&object.semantic_blake3, require_semantic) {
        (Some(value), _) => require_hex(value, 64, "object semantic digest"),
        (None, true) => Err("operation requires an object-complete semantic digest".to_string()),
        (None, false) => Ok(()),
    }
}

fn require_parent_handle(value: u64, label: &str) -> Result<(), String> {
    let exceeds_architecture = std::mem::size_of::<windows_sys::Win32::Foundation::HANDLE>() == 4
        && value > u32::MAX as u64;
    if value == 0 || value == u64::MAX || exceeds_architecture {
        Err(format!("{label} parent handle value is invalid"))
    } else {
        Ok(())
    }
}

fn require_hex(value: &str, len: usize, label: &str) -> Result<(), String> {
    if value.len() == len && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(format!("{label} has an invalid encoding"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameRole {
    ParentRequest = 1,
    HelperResponse = 2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameContext {
    pub role: FrameRole,
    pub sequence: u64,
    /// 128-bit operation UUID encoded as exactly 32 hex characters.
    pub operation_uuid: String,
    /// 256-bit invocation nonce encoded as exactly 64 hex characters.
    pub nonce: String,
    /// For a request this is the payload digest. For a response it is the
    /// already-validated parent request digest, binding both directions.
    pub request_blake3: String,
}

pub fn encode_authenticated<T: Serialize>(
    value: &T,
    key: &[u8; 32],
    context: &FrameContext,
) -> Result<Vec<u8>, String> {
    let payload = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    if payload.len() > MAX_FRAME_BYTES.saturating_sub(FRAME_HEADER_BYTES + FRAME_MAC_BYTES) {
        return Err("elevated-helper frame exceeds the fixed limit".to_string());
    }
    let operation_uuid = decode_fixed_hex::<16>(&context.operation_uuid, "operation UUID")?;
    let nonce = decode_fixed_hex::<32>(&context.nonce, "frame nonce")?;
    let request_digest = decode_fixed_hex::<32>(&context.request_blake3, "request digest")?;
    if context.sequence == 0 {
        return Err("elevated-helper frame sequence must be non-zero".to_string());
    }
    if context.role == FrameRole::ParentRequest
        && blake3::hash(&payload).as_bytes() != &request_digest
    {
        return Err("parent request digest does not match its payload".to_string());
    }
    let mut authenticated = Vec::with_capacity(FRAME_HEADER_BYTES + payload.len());
    authenticated.push(FRAME_WIRE_VERSION);
    authenticated.push(context.role as u8);
    authenticated.extend_from_slice(&[0, 0]);
    authenticated.extend_from_slice(&context.sequence.to_le_bytes());
    authenticated.extend_from_slice(&operation_uuid);
    authenticated.extend_from_slice(&nonce);
    authenticated.extend_from_slice(&request_digest);
    authenticated.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    authenticated.extend_from_slice(&payload);
    let mut hasher = blake3::Hasher::new_keyed(key);
    hasher.update(PROTOCOL_SCHEMA.as_bytes());
    hasher.update(&authenticated);
    let mac = hasher.finalize();
    debug_assert_eq!(authenticated.len(), FRAME_HEADER_BYTES + payload.len());
    let frame_len = authenticated.len() + FRAME_MAC_BYTES;
    let mut frame = Vec::with_capacity(frame_len + FRAME_PREFIX_BYTES);
    frame.extend_from_slice(&(frame_len as u32).to_le_bytes());
    frame.extend_from_slice(&authenticated);
    frame.extend_from_slice(mac.as_bytes());
    Ok(frame)
}

pub fn decode_authenticated<T: DeserializeOwned>(
    frame: &[u8],
    key: &[u8; 32],
    expected: &FrameContext,
) -> Result<T, String> {
    if frame.len() < FRAME_PREFIX_BYTES + FRAME_MIN_DECLARED_BYTES
        || frame.len() > MAX_FRAME_BYTES + FRAME_PREFIX_BYTES
    {
        return Err("elevated-helper frame has an invalid length".to_string());
    }
    let declared = u32::from_le_bytes(frame[..4].try_into().expect("four bytes")) as usize;
    if declared != frame.len() - FRAME_PREFIX_BYTES || declared < FRAME_MIN_DECLARED_BYTES {
        return Err("elevated-helper frame length prefix mismatch".to_string());
    }
    let authenticated_end = frame.len() - FRAME_MAC_BYTES;
    let authenticated = &frame[FRAME_PREFIX_BYTES..authenticated_end];
    if authenticated[0] != FRAME_WIRE_VERSION
        || authenticated[1] != expected.role as u8
        || authenticated[2..4] != [0, 0]
    {
        return Err("elevated-helper frame role/version mismatch".to_string());
    }
    let sequence = u64::from_le_bytes(authenticated[4..12].try_into().unwrap());
    let operation_uuid = decode_fixed_hex::<16>(&expected.operation_uuid, "operation UUID")?;
    let nonce = decode_fixed_hex::<32>(&expected.nonce, "frame nonce")?;
    let request_digest = decode_fixed_hex::<32>(&expected.request_blake3, "request digest")?;
    if sequence != expected.sequence
        || authenticated[12..28] != operation_uuid
        || authenticated[28..60] != nonce
        || authenticated[60..92] != request_digest
    {
        return Err("elevated-helper frame context/replay binding mismatch".to_string());
    }
    let payload_len = u32::from_le_bytes(authenticated[92..96].try_into().unwrap()) as usize;
    if FRAME_HEADER_BYTES.checked_add(payload_len) != Some(authenticated.len()) {
        return Err("elevated-helper payload length mismatch".to_string());
    }
    let payload = &authenticated[FRAME_HEADER_BYTES..];
    if expected.role == FrameRole::ParentRequest
        && blake3::hash(payload).as_bytes() != &request_digest
    {
        return Err("parent request payload digest mismatch".to_string());
    }
    let supplied_mac = &frame[authenticated_end..];
    let mut hasher = blake3::Hasher::new_keyed(key);
    hasher.update(PROTOCOL_SCHEMA.as_bytes());
    hasher.update(authenticated);
    let expected_mac = hasher.finalize();
    let different = expected_mac
        .as_bytes()
        .iter()
        .zip(supplied_mac)
        .fold(0u8, |difference, (left, right)| difference | (left ^ right));
    if different != 0 {
        return Err("elevated-helper frame authentication failed".to_string());
    }
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

fn decode_fixed_hex<const N: usize>(value: &str, label: &str) -> Result<[u8; N], String> {
    if value.len() != N * 2 {
        return Err(format!("{label} has an invalid encoding"));
    }
    let mut out = [0u8; N];
    for (index, pair) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let text = std::str::from_utf8(pair).map_err(|_| format!("{label} is not ASCII"))?;
        out[index] =
            u8::from_str_radix(text, 16).map_err(|_| format!("{label} has an invalid encoding"))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(root: &Path) -> ElevatedRequest {
        ElevatedRequest {
            schema: PROTOCOL_SCHEMA.to_string(),
            nonce: "ab".repeat(32),
            issued_at_unix_seconds: 100,
            expires_at_unix_seconds: 160,
            parent: ParentBinding {
                pid: 42,
                session_id: 7,
                process_started_100ns: 99,
                image_sha256: "cd".repeat(32),
            },
            plan_fingerprint: format!("v2:{}", "ef".repeat(32)),
            operation_id: 11,
            journal_capability_blake3: "01".repeat(32),
            synthetic_test: true,
            capabilities: vec![ElevatedCapability::ObjectBackupV2 {
                source: ExpectedObject {
                    path: root.join("source.bin"),
                    parent_handle_value: 0x1234,
                    stamp: FileStamp {
                        volume_id: "1".to_string(),
                        file_id: "2".to_string(),
                        bytes: 3,
                        modified_unix_seconds: Some(4),
                    },
                    content_blake3: "23".repeat(32),
                    semantic_blake3: None,
                    allow_internal_directory_time_drift: false,
                },
                parent_archive_handle_value: 0x5678,
                archive_path: archive_path_for_capability(
                    &root.join("backup/scratch"),
                    &"ab".repeat(32),
                    0,
                ),
                scratch_root: ExpectedScratchRoot {
                    path: root.join("backup/scratch"),
                    parent_handle_value: 0x7890,
                    stamp: FileStamp {
                        volume_id: "1".to_string(),
                        file_id: "scratch".to_string(),
                        bytes: 0,
                        modified_unix_seconds: Some(4),
                    },
                },
                scratch_leaf: scratch_leaf_for_capability(&"ab".repeat(32), 0),
            }],
        }
    }

    #[test]
    fn typed_capability_is_short_lived_parent_bound_and_rooted() {
        let root = tempfile::tempdir().unwrap();
        let request = request(root.path());
        request
            .validate(120, &"ab".repeat(32), 42, 7, Some(root.path()))
            .unwrap();

        let mut escaped = request.clone();
        if let ElevatedCapability::ObjectBackupV2 { scratch_root, .. } =
            &mut escaped.capabilities[0]
        {
            scratch_root.path = root.path().parent().unwrap().join("escaped");
        }
        assert!(escaped
            .validate(120, &"ab".repeat(32), 42, 7, Some(root.path()))
            .is_err());

        let mut redirected_archive = request.clone();
        if let ElevatedCapability::ObjectBackupV2 { archive_path, .. } =
            &mut redirected_archive.capabilities[0]
        {
            *archive_path = root.path().join("backup/scratch/attacker.partial");
        }
        assert!(redirected_archive
            .validate(120, &"ab".repeat(32), 42, 7, Some(root.path()))
            .is_err());

        let mut stale = request;
        stale.expires_at_unix_seconds = 119;
        assert!(stale
            .validate(120, &"ab".repeat(32), 42, 7, Some(root.path()))
            .is_err());
    }

    #[test]
    fn streamed_slice_binds_scratch_leaf_to_global_index() {
        let root = tempfile::tempdir().unwrap();
        let nonce = "ab".repeat(32);
        let mut slice = request(root.path());
        if let ElevatedCapability::ObjectBackupV2 {
            archive_path,
            scratch_root,
            scratch_leaf,
            ..
        } = &mut slice.capabilities[0]
        {
            *archive_path = archive_path_for_capability(&scratch_root.path, &nonce, 64);
            *scratch_leaf = scratch_leaf_for_capability(&nonce, 64);
        }
        slice
            .validate_capability_slice(120, &nonce, 42, 7, Some(root.path()), 64)
            .unwrap();
        assert!(slice
            .validate_capability_slice(120, &nonce, 42, 7, Some(root.path()), 0)
            .is_err());

        let overflow_base = u32::try_from(MAX_CAPABILITIES_PER_INVOCATION).unwrap();
        assert!(slice
            .validate_capability_slice(120, &nonce, 42, 7, Some(root.path()), overflow_base,)
            .is_err());
    }

    #[test]
    fn handle_neutral_authorization_validates_before_lazy_materialization() {
        let root = tempfile::tempdir().unwrap();
        let nonce = "ab".repeat(32);
        let mut template = request(root.path());
        neutralize_capability_handles(&mut template.capabilities[0], 0);

        assert!(template
            .validate_capability_slice(120, &nonce, 42, 7, Some(root.path()), 0)
            .is_err());
        template
            .validate_authorization_slice(120, &nonce, 42, 7, Some(root.path()), 0)
            .unwrap();

        if let ElevatedCapability::ObjectBackupV2 { source, .. } = &mut template.capabilities[0] {
            source.stamp.file_id.clear();
        }
        assert!(template
            .validate_authorization_slice(120, &nonce, 42, 7, Some(root.path()), 0)
            .is_err());
    }

    #[test]
    fn existing_archive_must_be_a_bound_object_archive_v2_child() {
        let root = tempfile::tempdir().unwrap();
        let nonce = "ab".repeat(32);
        let mut request = request(root.path());
        let (mut source, scratch_root, scratch_leaf) = match &request.capabilities[0] {
            ElevatedCapability::ObjectBackupV2 {
                source,
                scratch_root,
                scratch_leaf,
                ..
            } => (source.clone(), scratch_root.clone(), scratch_leaf.clone()),
            ElevatedCapability::RoundtripVerify { .. } => unreachable!(),
        };
        source.semantic_blake3 = Some("45".repeat(32));
        request.capabilities[0] = ElevatedCapability::RoundtripVerify {
            source,
            parent_archive_handle_value: 0x5678,
            archive_path: scratch_root
                .path
                .join(OBJECT_ARCHIVE_DIRECTORY_NAME)
                .join("entry-0000000000000001.chobj"),
            expected_archive_stamp: FileStamp {
                volume_id: "1".to_string(),
                file_id: "archive".to_string(),
                bytes: 1,
                modified_unix_seconds: Some(4),
            },
            expected_archive_blake3: "67".repeat(32),
            scratch_root,
            scratch_leaf,
        };
        request
            .validate(120, &nonce, 42, 7, Some(root.path()))
            .unwrap();

        if let ElevatedCapability::RoundtripVerify { archive_path, .. } =
            &mut request.capabilities[0]
        {
            archive_path.set_file_name("unexpected.chobj");
        }
        assert!(request
            .validate(120, &nonce, 42, 7, Some(root.path()))
            .is_err());
    }

    #[test]
    fn directory_time_drift_flag_is_roundtrip_only_and_semantic_bound() {
        let root = tempfile::tempdir().unwrap();
        let nonce = "ab".repeat(32);
        let mut request = request(root.path());
        if let ElevatedCapability::ObjectBackupV2 { source, .. } = &mut request.capabilities[0] {
            source.allow_internal_directory_time_drift = true;
        }
        assert!(request
            .validate(120, &nonce, 42, 7, Some(root.path()))
            .unwrap_err()
            .contains("pre-capture directory time drift"));

        let (mut source, scratch_root, scratch_leaf) = match &request.capabilities[0] {
            ElevatedCapability::ObjectBackupV2 {
                source,
                scratch_root,
                scratch_leaf,
                ..
            } => (source.clone(), scratch_root.clone(), scratch_leaf.clone()),
            ElevatedCapability::RoundtripVerify { .. } => unreachable!(),
        };
        source.semantic_blake3 = Some("45".repeat(32));
        request.capabilities[0] = ElevatedCapability::RoundtripVerify {
            source,
            parent_archive_handle_value: 0x5678,
            archive_path: scratch_root
                .path
                .join(OBJECT_ARCHIVE_DIRECTORY_NAME)
                .join("entry-0000000000000001.chobj"),
            expected_archive_stamp: FileStamp {
                volume_id: "1".to_string(),
                file_id: "archive".to_string(),
                bytes: 1,
                modified_unix_seconds: Some(4),
            },
            expected_archive_blake3: "67".repeat(32),
            scratch_root,
            scratch_leaf,
        };
        request
            .validate(120, &nonce, 42, 7, Some(root.path()))
            .unwrap();

        if let ElevatedCapability::RoundtripVerify { source, .. } = &mut request.capabilities[0] {
            source.semantic_blake3 = None;
        }
        assert!(request
            .validate(120, &nonce, 42, 7, Some(root.path()))
            .is_err());
    }

    #[test]
    fn authenticated_frame_rejects_tampering_and_trailing_bytes() {
        let root = tempfile::tempdir().unwrap();
        let request = request(root.path());
        let key = [9u8; 32];
        let payload = serde_json::to_vec(&request).unwrap();
        let context = FrameContext {
            role: FrameRole::ParentRequest,
            sequence: 1,
            operation_uuid: "12".repeat(16),
            nonce: "ab".repeat(32),
            request_blake3: blake3::hash(&payload).to_hex().to_string(),
        };
        let frame = encode_authenticated(&request, &key, &context).unwrap();
        let decoded: ElevatedRequest = decode_authenticated(&frame, &key, &context).unwrap();
        assert_eq!(decoded, request);

        let mut tampered = frame.clone();
        tampered[12] ^= 1;
        assert!(decode_authenticated::<ElevatedRequest>(&tampered, &key, &context).is_err());

        let mut trailing = frame.clone();
        trailing.push(0);
        assert!(decode_authenticated::<ElevatedRequest>(&trailing, &key, &context).is_err());

        let mut replay_context = context;
        replay_context.sequence = 2;
        assert!(decode_authenticated::<ElevatedRequest>(&frame, &key, &replay_context).is_err());
    }

    #[test]
    fn protocol_has_no_delete_or_purge_capability() {
        let serialized =
            serde_json::to_string(&request(tempfile::tempdir().unwrap().path())).unwrap();
        assert!(!serialized.contains("purge"));
        assert!(!serialized.contains("delete"));
    }
}
