//! Authenticated Windows transports for the object-archive helper.
//!
//! The executable is deliberately not a general service. Its elevated archive
//! mode and non-elevated final-disposition guardian mode each use a random,
//! first-instance, local-only named pipe, mutually verify the signed release
//! processes, and only then exchange an ephemeral session key. Filesystem
//! paths, handles, operation verbs and the session key never appear on the
//! command line.

use std::ffi::OsString;
use std::ops::Range;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::{
    ElevatedCapability, ElevatedItemResult, ElevatedRequest, ElevatedResponse, FileStamp,
    ParentBinding,
};

/// Release invocations require an external, offline-signed release manifest and
/// a valid offline Authenticode chain for both images. The manifest contains the
/// hashes of the already-signed final binaries; only its long-lived RSA-PSS
/// trust root is embedded at compile time. This deliberately avoids impossible
/// self-referential image pins.
pub const RELEASE_IDENTITY_REQUIREMENT: &str =
    "release helper requires Authenticode plus a release manifest signed by the embedded offline trust root";

pub const RELEASE_MANIFEST_FILE_NAME: &str = "code-hangar-release-manifest.json";
pub const RELEASE_MANIFEST_SCHEMA: &str = "codehangar/release-identity/1";

/// Bound for the current resident-batch launcher. Every capability carries
/// three parent-process handle references (source, archive and scratch root),
/// and callers must keep their owning handles alive until the matching helper
/// response. This compatibility bound applies only to `invoke_elevated_helper`;
/// the production final-removal path uses the lazy provider and the larger wire
/// ceiling without splitting one confirmed batch into multiple UAC prompts.
pub const MAX_RESIDENT_CAPABILITIES_PER_INVOCATION: usize = 4_096;

/// One lazily materialized capability slice. `guard` owns every source,
/// archive and scratch handle referenced by `capabilities`; the transport keeps
/// it alive through the authenticated helper reply and hands it to
/// `consume_chunk` before any handle can be dropped.
pub(crate) struct MaterializedCapabilityChunk<Guard> {
    pub capabilities: Vec<ElevatedCapability>,
    pub guard: Guard,
}

/// Parent-side source for a large, single-UAC batch. Durable handle-neutral
/// templates live in `ElevatedRequest.capabilities`; this provider opens only
/// the current bounded slice and consumes its result while those exact handles
/// remain alive.
pub(crate) trait LazyElevatedCapabilityBatch {
    type Guard;

    fn total_capabilities(&self) -> usize;

    fn materialize_chunk(
        &mut self,
        range: Range<usize>,
        nonce: &str,
    ) -> Result<MaterializedCapabilityChunk<Self::Guard>, ElevatedTransportError>;

    fn consume_chunk(
        &mut self,
        start_index: usize,
        guard: Self::Guard,
        results: &[ElevatedItemResult],
    ) -> Result<(), ElevatedTransportError>;

    /// True only after the provider has latched a stop at one of its own
    /// atomic disposition boundaries. The transport never polls the external
    /// stop source directly.
    fn stop_stream_requested(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReleaseInstallationVerification {
    pub release_id: String,
    pub manifest_sha256: String,
    pub parent_sha256: String,
    pub helper_sha256: String,
}

#[derive(Debug, Error)]
pub enum ElevatedTransportError {
    #[error("elevated-helper transport is unsupported: {0}")]
    Unsupported(String),
    #[error("elevated-helper identity proof failed: {0}")]
    Identity(String),
    #[error("elevated-helper protocol failed: {0}")]
    Protocol(String),
    #[error("elevated-helper launch failed: {0}")]
    Launch(String),
    #[error("elevated-helper transport timed out: {0}")]
    Timeout(String),
    #[error("elevated-helper io failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Durable process evidence captured only after both sides of the local pipe
/// have verified the signed release identity and the guardian has duplicated
/// and rebound the exact held-object handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DispositionGuardianIdentity {
    pub pid: u32,
    pub process_started_100ns: u64,
    pub image_sha256: String,
    pub nonce_digest: String,
    pub receipt: GuardianReceiptAuthority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GuardianReceiptAuthority {
    pub path: PathBuf,
    pub initial_stamp: FileStamp,
    /// DPAPI ciphertext encoded as lowercase hex. The 256-bit MAC key is never
    /// written to the journal or formatted through `Debug`/logs.
    pub protected_key_hex: String,
}

#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GuardianCloseReceiptExpectation {
    pub operation_id: i64,
    pub batch_item_id: i64,
    pub nonce_digest: String,
    pub guardian_pid: u32,
    pub guardian_started_100ns: u64,
    pub guardian_image_sha256: String,
    pub target_stamp: FileStamp,
    pub disposition_mode: crate::bound_fs::WindowsDeleteDispositionMode,
}

/// Complete parent-side binding required before a disposition guardian may be
/// launched. Keeping these fields together prevents call sites from silently
/// omitting one of the durable object, process-session or receipt authorities.
#[cfg(windows)]
pub(crate) struct DispositionGuardianLaunch<'a> {
    pub helper_path: &'a Path,
    pub operation_uuid: &'a str,
    pub guardian_nonce: &'a str,
    pub operation_id: i64,
    pub batch_item_id: i64,
    pub parent_handle_value: u64,
    pub expected_stamp: &'a FileStamp,
    pub receipt_path: &'a Path,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GuardianCancelOutcome {
    /// The guardian queried the duplicated handle and proved DeletePending is
    /// false. Both peers may now close their handles without deleting bytes.
    CancelledSafe,
    /// Cancellation was not proved. The guardian keeps the duplicated handle
    /// alive and retries after the parent pipe closes.
    Retaining,
}

/// Recovery-side observation of the exact guardian process identity that was
/// durably bound before disposition. A PID by itself is never liveness proof:
/// the process start time and signed-image hash must match as well.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExactGuardianLiveness {
    Alive,
    Terminated,
}

#[cfg(windows)]
pub(crate) use windows_impl::DispositionGuardian;

#[cfg(windows)]
pub(crate) fn exact_guardian_liveness(
    pid: u32,
    process_started_100ns: u64,
    image_sha256: &str,
) -> Result<ExactGuardianLiveness, ElevatedTransportError> {
    windows_impl::exact_guardian_liveness(pid, process_started_100ns, image_sha256)
}

#[cfg(windows)]
pub(crate) fn verify_guardian_close_receipt(
    authority: &GuardianReceiptAuthority,
    expected: &GuardianCloseReceiptExpectation,
) -> Result<(), ElevatedTransportError> {
    windows_impl::verify_guardian_close_receipt(authority, expected)
}

#[cfg(all(test, windows))]
pub(crate) fn create_guardian_receipt_fixture(
    path: &Path,
    expected: &GuardianCloseReceiptExpectation,
    durably_write: bool,
) -> Result<GuardianReceiptAuthority, ElevatedTransportError> {
    windows_impl::create_guardian_receipt_fixture(path, expected, durably_write)
}

/// Launch the separately signed helper image as a non-elevated, no-path
/// cancellation guardian and bind it to the exact parent handle before any
/// delete disposition is armed.
#[cfg(windows)]
pub(crate) fn launch_disposition_guardian(
    launch: DispositionGuardianLaunch<'_>,
) -> Result<DispositionGuardian, ElevatedTransportError> {
    windows_impl::launch_disposition_guardian(launch)
}

/// Build the exact binding a caller must place in an `ElevatedRequest`.
/// Constructing the binding is read-only; release identity enforcement happens
/// again at invocation time immediately before UAC.
#[cfg(windows)]
pub fn current_parent_binding() -> Result<ParentBinding, ElevatedTransportError> {
    windows_impl::current_parent_binding()
}

#[cfg(not(windows))]
pub fn current_parent_binding() -> Result<ParentBinding, ElevatedTransportError> {
    Err(ElevatedTransportError::Unsupported(
        "the object helper transport is Windows-only".to_string(),
    ))
}

/// Launch one release helper and exchange one authenticated capability batch.
///
/// `request.nonce` must be a caller-generated 256-bit CSPRNG value already
/// persisted with the journal intent. The transport validates and uses those
/// exact bytes for the pipe, authenticated frames and nonce/index-derived
/// scratch paths; replacing it here would make crash recovery unable to prove
/// which artifacts belong to the invocation. `operation_uuid` is a 128-bit UUID
/// encoded as exactly 32 hexadecimal characters.
///
/// This API is deliberately a resident-batch API: the caller owns every raw
/// handle represented in `request.capabilities` until this function returns.
/// It rejects more than [`MAX_RESIDENT_CAPABILITIES_PER_INVOCATION`] before
/// creating a pipe or displaying UAC. Large final-removal batches use the lazy
/// API below, which commits stable identities separately from process-local
/// handle values.
#[cfg(windows)]
pub fn invoke_elevated_helper(
    request: ElevatedRequest,
    operation_uuid: &str,
    helper_path: &Path,
) -> Result<ElevatedResponse, ElevatedTransportError> {
    windows_impl::invoke(request, operation_uuid, helper_path)
}

/// Stream a large immutable batch through one helper/UAC session while keeping
/// at most one bounded capability chunk resident. Template capabilities may
/// contain zero raw handles; every other field is committed before UAC and the
/// materialized projection must match exactly before it leaves the parent.
#[cfg(windows)]
pub(crate) fn invoke_elevated_helper_lazy<Batch: LazyElevatedCapabilityBatch>(
    request_templates: ElevatedRequest,
    operation_uuid: &str,
    helper_path: &Path,
    batch: &mut Batch,
) -> Result<ElevatedResponse, ElevatedTransportError> {
    windows_impl::invoke_lazy(request_templates, operation_uuid, helper_path, batch)
}

/// Packaging gate to run after both executables have received their final
/// Authenticode signatures and before those exact bytes are bundled. It checks
/// the offline-root signature, both manifest hashes and both Authenticode
/// chains without network retrieval.
#[cfg(windows)]
pub fn verify_release_installation(
    directory: &Path,
) -> Result<ReleaseInstallationVerification, ElevatedTransportError> {
    windows_impl::verify_release_installation(directory)
}

#[cfg(not(windows))]
pub fn verify_release_installation(
    _directory: &Path,
) -> Result<ReleaseInstallationVerification, ElevatedTransportError> {
    Err(ElevatedTransportError::Unsupported(
        "release installation verification is Windows-only".to_string(),
    ))
}

#[cfg(not(windows))]
pub fn invoke_elevated_helper(
    _request: ElevatedRequest,
    _operation_uuid: &str,
    _helper_path: &Path,
) -> Result<ElevatedResponse, ElevatedTransportError> {
    Err(ElevatedTransportError::Unsupported(
        "the object helper transport is Windows-only".to_string(),
    ))
}

#[cfg(not(windows))]
pub(crate) fn invoke_elevated_helper_lazy<Batch: LazyElevatedCapabilityBatch>(
    _request_templates: ElevatedRequest,
    _operation_uuid: &str,
    _helper_path: &Path,
    _batch: &mut Batch,
) -> Result<ElevatedResponse, ElevatedTransportError> {
    Err(ElevatedTransportError::Unsupported(
        "the object helper transport is Windows-only".to_string(),
    ))
}

/// Entry point used only by the dedicated `code-hangar-elevated` binary.
/// Exactly the three value pairs `--pipe`, `--parent-pid` and `--nonce` are
/// accepted. Any extra flag is rejected before opening a pipe.
#[cfg(windows)]
pub fn run_elevated_helper_cli<I>(args: I) -> Result<(), ElevatedTransportError>
where
    I: IntoIterator<Item = OsString>,
{
    windows_impl::run_cli(args)
}

#[cfg(not(windows))]
pub fn run_elevated_helper_cli<I>(_args: I) -> Result<(), ElevatedTransportError>
where
    I: IntoIterator<Item = OsString>,
{
    Err(ElevatedTransportError::Unsupported(
        "the object helper transport is Windows-only".to_string(),
    ))
}

#[cfg(windows)]
mod windows_impl {
    use std::ffi::{OsStr, OsString};
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::time::Duration;
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde::{de::DeserializeOwned, Deserialize, Serialize};
    use sha2::{Digest, Sha256};
    use windows_sys::Win32::Foundation::{
        CloseHandle, DuplicateHandle, GetLastError, LocalFree, DUPLICATE_SAME_ACCESS,
        ERROR_BROKEN_PIPE, ERROR_INVALID_PARAMETER, ERROR_IO_PENDING, ERROR_MORE_DATA,
        ERROR_NO_DATA, ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED, FILETIME, GENERIC_READ,
        GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::Cryptography::{
        BCryptCloseAlgorithmProvider, BCryptDestroyKey, BCryptGenRandom, BCryptImportKeyPair,
        BCryptOpenAlgorithmProvider, BCryptVerifySignature, CryptProtectData, CryptUnprotectData,
        BCRYPT_ALG_HANDLE, BCRYPT_KEY_HANDLE, BCRYPT_PAD_PSS, BCRYPT_PSS_PADDING_INFO,
        BCRYPT_RSAPUBLIC_BLOB, BCRYPT_RSAPUBLIC_MAGIC, BCRYPT_RSA_ALGORITHM,
        BCRYPT_SHA256_ALGORITHM, BCRYPT_USE_SYSTEM_PREFERRED_RNG, CRYPTPROTECT_UI_FORBIDDEN,
        CRYPT_INTEGER_BLOB,
    };
    use windows_sys::Win32::Security::WinTrust::{
        WinVerifyTrust, WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0,
        WINTRUST_FILE_INFO, WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_FILE, WTD_REVOKE_NONE,
        WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE,
    };
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
        TOKEN_USER,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FlushFileBuffers, ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE,
        FILE_FLAG_OPEN_NO_RECALL, FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_OVERLAPPED,
        FILE_SHARE_READ, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, SYNCHRONIZE,
    };
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeClientProcessId,
        GetNamedPipeServerProcessId, PeekNamedPipe, SetNamedPipeHandleState, WaitNamedPipeW,
        PIPE_READMODE_MESSAGE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_MESSAGE, PIPE_WAIT,
    };
    use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows_sys::Win32::System::Threading::{
        CreateEventW, GetCurrentProcess, GetCurrentProcessId, GetProcessId, GetProcessTimes,
        OpenProcess, OpenProcessToken, QueryFullProcessImageNameW, TerminateProcess,
        WaitForMultipleObjects, WaitForSingleObject, CREATE_BREAKAWAY_FROM_JOB, CREATE_NO_WINDOW,
        PROCESS_DUP_HANDLE, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows_sys::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    use crate::{
        archive_path_for_capability, decode_authenticated, enable_object_backup_privileges,
        encode_authenticated, finalize_object_archive_v2, scratch_leaf_for_capability,
        verify_object_archive_v2, ElevatedCapability, ElevatedFailure, ElevatedItemResult,
        ElevatedObjectResult, ElevatedRequest, ElevatedResponse, ElevatedSuccess, FileStamp,
        FinalizeObjectArchiveParams, FrameContext, FrameRole, ParentBinding, PrivilegeGuard,
        VerifyObjectArchiveParams, FRAME_HEADER_BYTES, FRAME_MAC_BYTES, FRAME_MIN_DECLARED_BYTES,
        FRAME_PREFIX_BYTES, FRAME_WIRE_VERSION, MAX_CAPABILITIES_PER_INVOCATION,
        MAX_CAPABILITY_LIFETIME_SECONDS, MAX_FRAME_BYTES, PROTOCOL_SCHEMA,
        REQUIRED_OBJECT_PRIVILEGES,
    };

    use super::{
        DispositionGuardianIdentity, DispositionGuardianLaunch, ElevatedTransportError,
        ExactGuardianLiveness, GuardianCancelOutcome, GuardianCloseReceiptExpectation,
        GuardianReceiptAuthority, LazyElevatedCapabilityBatch, MaterializedCapabilityChunk,
        ReleaseInstallationVerification, RELEASE_IDENTITY_REQUIREMENT, RELEASE_MANIFEST_FILE_NAME,
        RELEASE_MANIFEST_SCHEMA,
    };

    const PIPE_PREFIX: &str = r"\\.\pipe\codehangar-elevated-";
    const GUARDIAN_PIPE_PREFIX: &str = r"\\.\pipe\codehangar-disposition-guardian-";
    const HELLO_MAGIC: &[u8; 8] = b"CHHLOv1\0";
    const KEY_MAGIC: &[u8; 8] = b"CHKEYv1\0";
    const CONNECT_TIMEOUT_MS: u32 = 120_000;
    const IO_TIMEOUT_MS: u32 = 120_000;
    const EXIT_TIMEOUT_MS: u32 = 10_000;
    const PIPE_BUFFER_BYTES: u32 = (MAX_FRAME_BYTES + FRAME_PREFIX_BYTES) as u32;
    const STREAM_SCHEMA: &str = "codehangar/elevated-stream/2";
    const GUARDIAN_SCHEMA: &str = "codehangar/final-disposition-guardian/1";
    const GUARDIAN_RECEIPT_SCHEMA: &str = "codehangar/final-disposition-receipt/1";
    const GUARDIAN_RECEIPT_MAX_BYTES: usize = 8 * 1024;
    const GUARDIAN_SELECTOR: &str = "--final-disposition-guardian";
    const GUARDIAN_RETRY_LIMIT: usize = 8;
    const MAX_CHUNK_CAPABILITIES: usize = 64;
    const MAX_CHUNK_PAYLOAD_BYTES: usize = 512 * 1024;
    const RELEASE_ROOT_RSA_PUBLIC_BLOB_HEX: Option<&str> =
        option_env!("CODE_HANGAR_RELEASE_ROOT_RSA_PUBLIC_BLOB_HEX");

    #[derive(Clone, Copy)]
    enum ReleaseRole {
        Parent,
        Helper,
    }

    impl ReleaseRole {
        fn label(self) -> &'static str {
            match self {
                Self::Parent => "parent",
                Self::Helper => "helper",
            }
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ReleaseImageEntry {
        file_name: String,
        sha256: String,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ReleaseManifest {
        schema: String,
        release_id: String,
        parent: ReleaseImageEntry,
        helper: ReleaseImageEntry,
        signature_rsa_pss_sha256: String,
    }

    struct ReleaseManifestProof {
        manifest: ReleaseManifest,
        digest: [u8; 32],
        directory: PathBuf,
        _file: File,
    }

    impl ReleaseManifestProof {
        fn image(&self, role: ReleaseRole) -> &ReleaseImageEntry {
            match role {
                ReleaseRole::Parent => &self.manifest.parent,
                ReleaseRole::Helper => &self.manifest.helper,
            }
        }
    }

    #[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct GuardianReceiptKey([u8; 32]);

    impl std::fmt::Debug for GuardianReceiptKey {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("GuardianReceiptKey([REDACTED])")
        }
    }

    impl Drop for GuardianReceiptKey {
        fn drop(&mut self) {
            self.0.fill(0);
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct GuardianReceiptKeyContext {
        schema: String,
        operation_id: i64,
        batch_item_id: i64,
        nonce_digest: String,
        guardian_pid: u32,
        guardian_started_100ns: u64,
        guardian_image_sha256: String,
        target_stamp: FileStamp,
        receipt_volume_id: String,
        receipt_file_id: String,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct GuardianCloseReceiptPayload {
        schema: String,
        operation_id: i64,
        batch_item_id: i64,
        nonce_digest: String,
        guardian_pid: u32,
        guardian_started_100ns: u64,
        guardian_image_sha256: String,
        target_stamp: FileStamp,
        receipt_volume_id: String,
        receipt_file_id: String,
        disposition_mode: crate::bound_fs::WindowsDeleteDispositionMode,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct GuardianCloseReceipt {
        payload: GuardianCloseReceiptPayload,
        mac_blake3: String,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "command", rename_all = "camelCase", deny_unknown_fields)]
    enum GuardianCommand {
        Bind {
            schema: String,
            operation_id: i64,
            batch_item_id: i64,
            parent_handle_value: u64,
            expected_stamp: FileStamp,
            receipt_handle_value: u64,
            expected_receipt_stamp: FileStamp,
            receipt_key: GuardianReceiptKey,
        },
        ArmAuthorized {
            schema: String,
            batch_item_id: i64,
        },
        ProveArmed {
            schema: String,
            batch_item_id: i64,
            mode: crate::bound_fs::WindowsDeleteDispositionMode,
        },
        Cancel {
            schema: String,
            batch_item_id: i64,
            preferred_mode: Option<crate::bound_fs::WindowsDeleteDispositionMode>,
        },
        CloseAuthorized {
            schema: String,
            batch_item_id: i64,
        },
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "reply", rename_all = "camelCase", deny_unknown_fields)]
    enum GuardianReply {
        HandleBound {
            schema: String,
            batch_item_id: i64,
            guardian_pid: u32,
            guardian_started_100ns: u64,
            guardian_image_sha256: String,
        },
        ArmReady {
            schema: String,
            batch_item_id: i64,
        },
        FinalProfileProvedHeld {
            schema: String,
            batch_item_id: i64,
            mode: crate::bound_fs::WindowsDeleteDispositionMode,
        },
        CancelledSafe {
            schema: String,
            batch_item_id: i64,
        },
        CancellationPendingRetained {
            schema: String,
            batch_item_id: i64,
            message: String,
        },
        HandleClosed {
            schema: String,
            batch_item_id: i64,
        },
        Refused {
            schema: String,
            batch_item_id: i64,
            message: String,
        },
    }

    impl GuardianReply {
        fn require_common(&self, batch_item_id: i64) -> Result<(), ElevatedTransportError> {
            let (schema, returned_item_id) = match self {
                Self::HandleBound {
                    schema,
                    batch_item_id,
                    ..
                }
                | Self::ArmReady {
                    schema,
                    batch_item_id,
                }
                | Self::FinalProfileProvedHeld {
                    schema,
                    batch_item_id,
                    ..
                }
                | Self::CancelledSafe {
                    schema,
                    batch_item_id,
                }
                | Self::CancellationPendingRetained {
                    schema,
                    batch_item_id,
                    ..
                }
                | Self::HandleClosed {
                    schema,
                    batch_item_id,
                }
                | Self::Refused {
                    schema,
                    batch_item_id,
                    ..
                } => (schema, *batch_item_id),
            };
            if schema != GUARDIAN_SCHEMA || returned_item_id != batch_item_id {
                return Err(ElevatedTransportError::Protocol(
                    "guardian reply is not bound to the durable batch item".to_string(),
                ));
            }
            Ok(())
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum GuardianPhase {
        Bound,
        ArmAuthorized,
        ProfileProved,
        CancellationPending,
    }

    /// Owns the guardian's duplicated target handle and makes every ordinary
    /// Rust exit fail closed once the parent has been told it may arm final
    /// disposition.
    ///
    /// After `ArmReady` is delivered the parent can make the shared FILE_OBJECT
    /// delete-pending before the guardian receives another frame.  A broken
    /// pipe, serialization error, early `?`, or unwind must therefore never
    /// drop the duplicate directly.  Until either cancellation is proved or a
    /// durable close receipt authorizes the final close, `Drop` retries
    /// cancellation without a timeout and only then allows the OS handle to
    /// close.
    struct GuardianTargetHandle {
        file: File,
        parent_process: OwnedHandle,
        arm_may_be_active: bool,
        future_arm_excluded: bool,
        preferred_mode: Option<crate::bound_fs::WindowsDeleteDispositionMode>,
    }

    impl GuardianTargetHandle {
        fn new(file: File, parent_process: OwnedHandle) -> Self {
            Self {
                file,
                parent_process,
                arm_may_be_active: false,
                future_arm_excluded: false,
                preferred_mode: None,
            }
        }

        fn file(&self) -> &File {
            &self.file
        }

        /// Set before attempting to send `ArmReady`: delivery can be
        /// successful even when the subsequent transport observation is not.
        fn authorize_arm(&mut self) {
            self.arm_may_be_active = true;
        }

        fn observe_mode(&mut self, mode: crate::bound_fs::WindowsDeleteDispositionMode) {
            self.preferred_mode = Some(mode);
            // An authenticated ProveArmed frame means the signed parent has
            // completed its only arm attempt. After this point a proved
            // DeletePending=false cannot race a future arm.
            self.future_arm_excluded = true;
        }

        fn exclude_future_arm(&mut self) {
            self.future_arm_excluded = true;
        }

        fn cancellation_proved(&mut self) {
            self.arm_may_be_active = false;
        }

        fn close_authorized_by_durable_receipt(&mut self) {
            self.arm_may_be_active = false;
        }

        fn retain_until_cancelled(&mut self) {
            if !self.arm_may_be_active {
                return;
            }
            if self.future_arm_excluded {
                guardian_retain_until_cancelled(&self.file, self.preferred_mode);
            } else {
                // ArmReady may have reached the parent even if the guardian's
                // write/reply observation failed. A momentary
                // DeletePending=false is therefore not terminal while that
                // exact parent is alive and may still perform its one arm.
                guardian_retain_until_parent_exit_and_cancelled(
                    &self.file,
                    self.preferred_mode,
                    self.parent_process.raw(),
                );
            }
            self.cancellation_proved();
        }
    }

    impl Drop for GuardianTargetHandle {
        fn drop(&mut self) {
            self.retain_until_cancelled();
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct StreamRequestHeader {
        protocol_schema: String,
        nonce: String,
        issued_at_unix_seconds: i64,
        expires_at_unix_seconds: i64,
        parent: ParentBinding,
        plan_fingerprint: String,
        operation_id: i64,
        journal_capability_blake3: String,
        synthetic_test: bool,
    }

    impl StreamRequestHeader {
        fn from_request(request: &ElevatedRequest) -> Self {
            Self {
                protocol_schema: request.schema.clone(),
                nonce: request.nonce.clone(),
                issued_at_unix_seconds: request.issued_at_unix_seconds,
                expires_at_unix_seconds: request.expires_at_unix_seconds,
                parent: request.parent.clone(),
                plan_fingerprint: request.plan_fingerprint.clone(),
                operation_id: request.operation_id,
                journal_capability_blake3: request.journal_capability_blake3.clone(),
                synthetic_test: request.synthetic_test,
            }
        }

        fn request_with(&self, capabilities: Vec<ElevatedCapability>) -> ElevatedRequest {
            ElevatedRequest {
                schema: self.protocol_schema.clone(),
                nonce: self.nonce.clone(),
                issued_at_unix_seconds: self.issued_at_unix_seconds,
                expires_at_unix_seconds: self.expires_at_unix_seconds,
                parent: self.parent.clone(),
                plan_fingerprint: self.plan_fingerprint.clone(),
                operation_id: self.operation_id,
                journal_capability_blake3: self.journal_capability_blake3.clone(),
                synthetic_test: self.synthetic_test,
                capabilities,
            }
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ChunkCommitment {
        start_index: u32,
        item_count: u32,
        capability_blake3: String,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct StreamBegin {
        schema: String,
        operation_uuid: String,
        header: StreamRequestHeader,
        total_capabilities: u32,
        chunks: Vec<ChunkCommitment>,
        batch_blake3: String,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct StreamChunk {
        schema: String,
        batch_blake3: String,
        chunk_index: u32,
        start_index: u32,
        capabilities: Vec<ElevatedCapability>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct StreamEnd {
        schema: String,
        batch_blake3: String,
        total_capabilities: u32,
        chunk_count: u32,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct StreamStop {
        schema: String,
        batch_blake3: String,
        processed_capabilities: u32,
        processed_chunks: u32,
    }

    /// Every post-begin parent message is explicitly typed and authenticated.
    /// `Stop` is a clean terminal command, not a truncated `End` and not a
    /// helper failure.
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum StreamCommand {
        Chunk(StreamChunk),
        Stop(StreamStop),
        End(StreamEnd),
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "stream_result", rename_all = "snake_case")]
    enum StreamReply {
        BeginReady {
            schema: String,
            batch_blake3: String,
            helper_image_sha256: String,
            privilege_bitmap: u32,
        },
        ChunkResults {
            schema: String,
            batch_blake3: String,
            chunk_index: u32,
            start_index: u32,
            items: Vec<ElevatedItemResult>,
        },
        EndReady {
            schema: String,
            batch_blake3: String,
            total_capabilities: u32,
            chunk_count: u32,
        },
        StopReady {
            schema: String,
            batch_blake3: String,
            processed_capabilities: u32,
            processed_chunks: u32,
        },
        Failure(ElevatedFailure),
    }

    #[derive(Debug, Clone)]
    struct PlannedChunk {
        start: usize,
        end: usize,
        digest: String,
    }

    #[derive(Clone)]
    enum IdentityPolicy {
        Release,
        #[cfg(test)]
        SyntheticFixture {
            expected_image: PathBuf,
        },
    }

    struct OwnedHandle(HANDLE);

    impl OwnedHandle {
        fn new(handle: HANDLE, label: &str) -> Result<Self, ElevatedTransportError> {
            if handle.is_null() || handle == INVALID_HANDLE_VALUE {
                Err(last_io(label))
            } else {
                Ok(Self(handle))
            }
        }

        fn raw(&self) -> HANDLE {
            self.0
        }
    }

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }

    struct LocalAllocation(*mut core::ffi::c_void);

    impl Drop for LocalAllocation {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    LocalFree(self.0);
                }
            }
        }
    }

    struct PipeServer {
        handle: OwnedHandle,
    }

    impl PipeServer {
        fn create(pipe_name: &str) -> Result<Self, ElevatedTransportError> {
            let sid = current_user_sid()?;
            let sddl = format!("D:P(A;;GA;;;SY)(A;;GA;;;{sid})");
            let sddl_wide = wide(&sddl);
            let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
            let converted = unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl_wide.as_ptr(),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    std::ptr::null_mut(),
                )
            };
            if converted == 0 || descriptor.is_null() {
                return Err(last_io("cannot build the one-shot pipe DACL"));
            }
            let _descriptor = LocalAllocation(descriptor);
            let security = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor,
                bInheritHandle: 0,
            };
            let name = wide(pipe_name);
            let handle = unsafe {
                CreateNamedPipeW(
                    name.as_ptr(),
                    PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE | FILE_FLAG_OVERLAPPED,
                    PIPE_TYPE_MESSAGE
                        | PIPE_READMODE_MESSAGE
                        | PIPE_WAIT
                        | PIPE_REJECT_REMOTE_CLIENTS,
                    1,
                    PIPE_BUFFER_BYTES,
                    PIPE_BUFFER_BYTES,
                    CONNECT_TIMEOUT_MS,
                    &security,
                )
            };
            Ok(Self {
                handle: OwnedHandle::new(handle, "cannot create the one-shot named pipe")?,
            })
        }

        fn connect(&self, child_process: Option<HANDLE>) -> Result<(), ElevatedTransportError> {
            let event = OwnedHandle::new(
                unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) },
                "cannot create named-pipe connect event",
            )?;
            let mut overlapped = OVERLAPPED {
                hEvent: event.raw(),
                ..Default::default()
            };
            let connected = unsafe { ConnectNamedPipe(self.handle.raw(), &mut overlapped) };
            if connected != 0 {
                return Ok(());
            }
            match unsafe { GetLastError() } {
                ERROR_PIPE_CONNECTED => Ok(()),
                ERROR_IO_PENDING => {
                    let wait = if let Some(process) = child_process {
                        let handles = [event.raw(), process];
                        unsafe {
                            WaitForMultipleObjects(
                                handles.len() as u32,
                                handles.as_ptr(),
                                0,
                                CONNECT_TIMEOUT_MS,
                            )
                        }
                    } else {
                        unsafe { WaitForSingleObject(event.raw(), CONNECT_TIMEOUT_MS) }
                    };
                    if wait == WAIT_TIMEOUT {
                        cancel_and_drain(self.handle.raw(), &mut overlapped);
                        return Err(ElevatedTransportError::Timeout(
                            "waiting for the one-shot helper connection".to_string(),
                        ));
                    }
                    if child_process.is_some() && wait == WAIT_OBJECT_0 + 1 {
                        cancel_and_drain(self.handle.raw(), &mut overlapped);
                        return Err(ElevatedTransportError::Launch(
                            "elevated helper exited before connecting".to_string(),
                        ));
                    }
                    if wait != WAIT_OBJECT_0 {
                        cancel_and_drain(self.handle.raw(), &mut overlapped);
                        return Err(last_io("waiting for the helper pipe connection failed"));
                    }
                    let mut transferred = 0u32;
                    let finished = unsafe {
                        GetOverlappedResult(self.handle.raw(), &overlapped, &mut transferred, 0)
                    };
                    if finished == 0 {
                        Err(last_io("completing the helper pipe connection failed"))
                    } else {
                        Ok(())
                    }
                }
                _ => Err(last_io("cannot connect the one-shot helper pipe")),
            }
        }

        fn write_message(&self, bytes: &[u8]) -> Result<(), ElevatedTransportError> {
            overlapped_write(self.handle.raw(), bytes)
        }

        fn read_message(&self, limit: usize) -> Result<Vec<u8>, ElevatedTransportError> {
            overlapped_read_framed(self.handle.raw(), limit)
        }

        fn read_exact_message(&self, bytes: usize) -> Result<Vec<u8>, ElevatedTransportError> {
            let (message, more) = overlapped_read_piece(self.handle.raw(), bytes)?;
            if message.len() != bytes || more {
                return Err(ElevatedTransportError::Protocol(
                    "handshake message has an invalid message boundary".to_string(),
                ));
            }
            Ok(message)
        }

        fn ensure_no_queued_message(&self, label: &str) -> Result<(), ElevatedTransportError> {
            ensure_no_queued_message(self.handle.raw(), label)
        }
    }

    impl Drop for PipeServer {
        fn drop(&mut self) {
            unsafe {
                DisconnectNamedPipe(self.handle.raw());
            }
        }
    }

    struct ChildGuard {
        process: OwnedHandle,
        finished: bool,
    }

    impl ChildGuard {
        fn new(process: HANDLE) -> Result<Self, ElevatedTransportError> {
            Ok(Self {
                process: OwnedHandle::new(process, "ShellExecuteEx returned no helper process")?,
                finished: false,
            })
        }

        fn raw(&self) -> HANDLE {
            self.process.raw()
        }

        fn require_clean_exit(&mut self) -> Result<(), ElevatedTransportError> {
            let wait = unsafe { WaitForSingleObject(self.raw(), EXIT_TIMEOUT_MS) };
            if wait != WAIT_OBJECT_0 {
                unsafe {
                    TerminateProcess(self.raw(), 0x4348_0001);
                    WaitForSingleObject(self.raw(), EXIT_TIMEOUT_MS);
                }
                return Err(ElevatedTransportError::Protocol(
                    "one-shot helper did not exit after its response".to_string(),
                ));
            }
            self.finished = true;
            Ok(())
        }
    }

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            if !self.finished && unsafe { WaitForSingleObject(self.raw(), 0) } == WAIT_TIMEOUT {
                unsafe {
                    TerminateProcess(self.raw(), 0x4348_0002);
                }
            }
        }
    }

    struct ProcessIdentity {
        _process: Option<OwnedHandle>,
        _image_file: File,
        pid: u32,
        session_id: u32,
        started_100ns: u64,
        image_path: PathBuf,
        image_sha256: String,
        user_sid: String,
    }

    /// Owns every resource from process creation until the authenticated bind
    /// has produced a complete controller. Any `?` in that interval must close
    /// the pipe, wait for the unarmed guardian to release its duplicate receipt
    /// handle, and only then allow exact cleanup of the unjournaled sidecar.
    struct PendingDispositionGuardian {
        pipe: Option<PipeServer>,
        child: Option<Child>,
        receipt: Option<crate::bound_fs::BoundGuardianReceipt>,
    }

    impl PendingDispositionGuardian {
        fn pipe(&self) -> &PipeServer {
            self.pipe
                .as_ref()
                .expect("pending guardian retains its pipe until promotion")
        }

        fn child(&self) -> &Child {
            self.child
                .as_ref()
                .expect("pending guardian retains its process until promotion")
        }

        fn receipt(&self) -> &crate::bound_fs::BoundGuardianReceipt {
            self.receipt
                .as_ref()
                .expect("pending guardian retains its receipt until promotion")
        }

        #[allow(clippy::too_many_arguments)]
        fn promote(
            mut self,
            key: [u8; 32],
            operation_uuid: &str,
            nonce: &str,
            batch_item_id: i64,
            identity: DispositionGuardianIdentity,
        ) -> DispositionGuardian {
            DispositionGuardian {
                pipe: self.pipe.take(),
                _child: self
                    .child
                    .take()
                    .expect("pending guardian process exists at promotion"),
                receipt: self.receipt.take(),
                receipt_journaled: false,
                key,
                operation_uuid: operation_uuid.to_string(),
                nonce: nonce.to_string(),
                sequence: 3,
                batch_item_id,
                identity,
            }
        }
    }

    impl Drop for PendingDispositionGuardian {
        fn drop(&mut self) {
            drop(self.pipe.take());
            let exited = self.child.as_mut().is_none_or(|child| {
                let wait = unsafe {
                    WaitForSingleObject(child.as_raw_handle() as HANDLE, EXIT_TIMEOUT_MS)
                };
                if wait == WAIT_OBJECT_0 {
                    let _ = child.try_wait();
                    true
                } else {
                    false
                }
            });
            if !exited {
                // Never race pathname cleanup against a still-running process
                // which may own a share-zero duplicate. The target is not armed
                // before promotion, so this is a bounded orphan-sidecar residual
                // rather than a data-loss tradeoff.
                if let Some(receipt) = self.receipt.as_mut() {
                    receipt.preserve_for_recovery();
                }
            }
        }
    }

    /// Parent-side controller for one no-path, exact-handle cancellation
    /// guardian. Dropping this value never terminates the child: closing the
    /// pipe is itself the fail-closed signal that makes the guardian cancel or
    /// retain the duplicated handle independently.
    pub(crate) struct DispositionGuardian {
        pipe: Option<PipeServer>,
        // Keeping the process handle open pins the process identity used by the
        // handshake. Dropping `Child` does not terminate the process on Windows.
        _child: Child,
        receipt: Option<crate::bound_fs::BoundGuardianReceipt>,
        receipt_journaled: bool,
        key: [u8; 32],
        operation_uuid: String,
        nonce: String,
        sequence: u64,
        batch_item_id: i64,
        identity: DispositionGuardianIdentity,
    }

    impl DispositionGuardian {
        pub(crate) fn identity(&self) -> &DispositionGuardianIdentity {
            &self.identity
        }

        pub(crate) fn mark_receipt_journaled(&mut self) {
            if let Some(receipt) = self.receipt.as_mut() {
                receipt.preserve_for_recovery();
            }
            self.receipt_journaled = true;
        }

        /// Remove the exact pre-bound sidecar only after deletion or safe
        /// cancellation is durably terminal. Waiting for the guardian process
        /// first proves its duplicate receipt handle is gone; a timeout leaves
        /// the sidecar intact for startup recovery rather than guessing cleanup.
        pub(crate) fn cleanup_receipt_after_terminal_state(
            &mut self,
        ) -> Result<(), ElevatedTransportError> {
            // Closing the authenticated channel is the guardian's fail-closed
            // shutdown signal. It also prevents a later frame from racing the
            // terminal receipt cleanup while we wait for every duplicated
            // handle in the guardian process to disappear.
            drop(self.pipe.take());
            match unsafe {
                WaitForSingleObject(self._child.as_raw_handle() as HANDLE, EXIT_TIMEOUT_MS)
            } {
                WAIT_OBJECT_0 => {}
                WAIT_TIMEOUT => {
                    return Err(ElevatedTransportError::Timeout(
                        "guardian did not exit before receipt cleanup".to_string(),
                    ));
                }
                WAIT_FAILED => return Err(last_io("cannot wait for guardian receipt cleanup")),
                status => {
                    return Err(ElevatedTransportError::Protocol(format!(
                        "guardian receipt cleanup wait returned unexpected status {status}"
                    )));
                }
            }
            let _ = self._child.try_wait().map_err(ElevatedTransportError::Io)?;
            self.receipt
                .take()
                .ok_or_else(|| {
                    ElevatedTransportError::Protocol(
                        "guardian receipt handle is unavailable for cleanup".to_string(),
                    )
                })?
                .cleanup_exact()
                .map_err(ElevatedTransportError::Io)
        }

        pub(crate) fn arm_authorized(&mut self) -> Result<(), ElevatedTransportError> {
            let reply = self.roundtrip(&GuardianCommand::ArmAuthorized {
                schema: GUARDIAN_SCHEMA.to_string(),
                batch_item_id: self.batch_item_id,
            })?;
            match reply {
                GuardianReply::ArmReady { .. } => Ok(()),
                GuardianReply::Refused { message, .. } => {
                    Err(ElevatedTransportError::Protocol(message))
                }
                _ => Err(ElevatedTransportError::Protocol(
                    "guardian did not acknowledge arm authorization".to_string(),
                )),
            }
        }

        pub(crate) fn prove_armed(
            &mut self,
            mode: crate::bound_fs::WindowsDeleteDispositionMode,
        ) -> Result<(), ElevatedTransportError> {
            let reply = self.roundtrip(&GuardianCommand::ProveArmed {
                schema: GUARDIAN_SCHEMA.to_string(),
                batch_item_id: self.batch_item_id,
                mode,
            })?;
            match reply {
                GuardianReply::FinalProfileProvedHeld { mode: returned, .. }
                    if returned == mode =>
                {
                    Ok(())
                }
                GuardianReply::Refused { message, .. } => {
                    Err(ElevatedTransportError::Protocol(message))
                }
                _ => Err(ElevatedTransportError::Protocol(
                    "guardian did not independently prove the armed profile".to_string(),
                )),
            }
        }

        pub(crate) fn cancel(
            &mut self,
            preferred_mode: Option<crate::bound_fs::WindowsDeleteDispositionMode>,
        ) -> Result<GuardianCancelOutcome, ElevatedTransportError> {
            let reply = self.roundtrip(&GuardianCommand::Cancel {
                schema: GUARDIAN_SCHEMA.to_string(),
                batch_item_id: self.batch_item_id,
                preferred_mode,
            })?;
            match reply {
                GuardianReply::CancelledSafe { .. } => Ok(GuardianCancelOutcome::CancelledSafe),
                GuardianReply::CancellationPendingRetained { .. } => {
                    Ok(GuardianCancelOutcome::Retaining)
                }
                GuardianReply::Refused { message, .. } => {
                    Err(ElevatedTransportError::Protocol(message))
                }
                _ => Err(ElevatedTransportError::Protocol(
                    "guardian returned an invalid cancellation state".to_string(),
                )),
            }
        }

        pub(crate) fn close_authorized(&mut self) -> Result<(), ElevatedTransportError> {
            let reply = self.roundtrip(&GuardianCommand::CloseAuthorized {
                schema: GUARDIAN_SCHEMA.to_string(),
                batch_item_id: self.batch_item_id,
            })?;
            match reply {
                GuardianReply::HandleClosed { .. } => {}
                GuardianReply::Refused { message, .. } => {
                    return Err(ElevatedTransportError::Protocol(message));
                }
                _ => {
                    return Err(ElevatedTransportError::Protocol(
                        "guardian did not acknowledge exact-handle close".to_string(),
                    ));
                }
            }
            // `HandleClosed` is authenticated evidence that the duplicate was
            // closed after its final same-handle proof. Process-exit polling is
            // deliberately not another authority gate: an observation timeout
            // after this ACK would leave the parent as the sole armed holder and
            // could recreate the exact cancel-failure/parent-death hazard that
            // the guardian removes.
            Ok(())
        }

        fn roundtrip(
            &mut self,
            command: &GuardianCommand,
        ) -> Result<GuardianReply, ElevatedTransportError> {
            let pipe = self.pipe.as_ref().ok_or_else(|| {
                ElevatedTransportError::Protocol(
                    "guardian control channel is already closed".to_string(),
                )
            })?;
            let reply: GuardianReply = parent_roundtrip(
                pipe,
                &self.key,
                &self.operation_uuid,
                &self.nonce,
                self.sequence,
                command,
            )?;
            self.sequence = self.sequence.checked_add(2).ok_or_else(|| {
                ElevatedTransportError::Protocol("guardian sequence overflow".to_string())
            })?;
            reply.require_common(self.batch_item_id)?;
            Ok(reply)
        }
    }

    impl Drop for DispositionGuardian {
        fn drop(&mut self) {
            // Field drop order alone is unsafe here: the parent receipt handle
            // could otherwise attempt CREATE_NEW sidecar cleanup while the
            // guardian still owns its duplicated share-zero handle. Disconnect
            // first and, only for a pre-journal sidecar, wait for the guardian
            // to finish before allowing `BoundGuardianReceipt::drop` to erase
            // the exact file.
            drop(self.pipe.take());
            if self.receipt_journaled || self.receipt.is_none() {
                return;
            }

            let wait = unsafe {
                WaitForSingleObject(self._child.as_raw_handle() as HANDLE, EXIT_TIMEOUT_MS)
            };
            if wait == WAIT_OBJECT_0 {
                let _ = self._child.try_wait();
                return;
            }

            // A guardian that has not exited may still own either duplicated
            // handle. Never guess by deleting through the pathname. Preserving
            // an unjournaled empty sidecar is preferable to weakening the
            // target guardian; ordinary pre-arm EOF should exit immediately.
            if let Some(receipt) = self.receipt.as_mut() {
                receipt.preserve_for_recovery();
            }
        }
    }

    pub(super) fn current_parent_binding() -> Result<ParentBinding, ElevatedTransportError> {
        let identity = inspect_process(unsafe { GetCurrentProcessId() }, None)?;
        Ok(ParentBinding {
            pid: identity.pid,
            session_id: identity.session_id,
            process_started_100ns: identity.started_100ns,
            image_sha256: identity.image_sha256,
        })
    }

    fn assign_dynamic_capability_fields(request: &mut ElevatedRequest) {
        assign_dynamic_capability_fields_slice(&mut request.capabilities, &request.nonce, 0);
    }

    fn validated_persisted_nonce(
        request: &ElevatedRequest,
    ) -> Result<String, ElevatedTransportError> {
        require_hex(&request.nonce, 64, "persisted transport nonce")?;
        Ok(request.nonce.clone())
    }

    fn assign_dynamic_capability_fields_slice(
        capabilities: &mut [ElevatedCapability],
        nonce: &str,
        start_index: usize,
    ) {
        for (offset, capability) in capabilities.iter_mut().enumerate() {
            let global_index = (start_index + offset) as u32;
            match capability {
                ElevatedCapability::ObjectBackupV2 {
                    archive_path,
                    scratch_root,
                    scratch_leaf,
                    ..
                } => {
                    *archive_path =
                        archive_path_for_capability(&scratch_root.path, nonce, global_index);
                    *scratch_leaf = scratch_leaf_for_capability(nonce, global_index);
                }
                ElevatedCapability::RoundtripVerify { scratch_leaf, .. } => {
                    *scratch_leaf = scratch_leaf_for_capability(nonce, global_index);
                }
            }
        }
    }

    fn plan_stream(
        request: &ElevatedRequest,
        operation_uuid: &str,
    ) -> Result<(StreamBegin, Vec<PlannedChunk>), ElevatedTransportError> {
        let header = StreamRequestHeader::from_request(request);
        let mut chunks = Vec::new();
        let mut start = 0usize;
        while start < request.capabilities.len() {
            let mut end = start;
            let mut estimated = 512usize;
            while end < request.capabilities.len() && end - start < MAX_CHUNK_CAPABILITIES {
                let encoded = serde_json::to_vec(&request.capabilities[end])
                    .map_err(|error| ElevatedTransportError::Protocol(error.to_string()))?;
                if encoded.len() + 1024 > MAX_CHUNK_PAYLOAD_BYTES {
                    return Err(ElevatedTransportError::Protocol(format!(
                        "capability {end} cannot fit in one authenticated stream chunk"
                    )));
                }
                if end > start && estimated + encoded.len() + 16 > MAX_CHUNK_PAYLOAD_BYTES {
                    break;
                }
                estimated += encoded.len() + 16;
                end += 1;
            }
            if end == start {
                return Err(ElevatedTransportError::Protocol(
                    "stream planner made no capability progress".to_string(),
                ));
            }
            let digest = capability_chunk_digest(start, &request.capabilities[start..end])?;
            chunks.push(PlannedChunk { start, end, digest });
            start = end;
        }
        let commitments = chunks
            .iter()
            .map(|chunk| ChunkCommitment {
                start_index: chunk.start as u32,
                item_count: (chunk.end - chunk.start) as u32,
                capability_blake3: chunk.digest.clone(),
            })
            .collect::<Vec<_>>();
        let batch_blake3 = stream_batch_digest(
            operation_uuid,
            &header,
            request.capabilities.len(),
            &commitments,
        )?;
        let begin = StreamBegin {
            schema: STREAM_SCHEMA.to_string(),
            operation_uuid: operation_uuid.to_string(),
            header,
            total_capabilities: request.capabilities.len() as u32,
            chunks: commitments,
            batch_blake3,
        };
        let begin_bytes = serde_json::to_vec(&begin)
            .map_err(|error| ElevatedTransportError::Protocol(error.to_string()))?;
        if begin_bytes.len() + FRAME_MIN_DECLARED_BYTES > MAX_FRAME_BYTES {
            return Err(ElevatedTransportError::Protocol(
                "stream begin commitment cannot fit in one authenticated frame".to_string(),
            ));
        }
        Ok((begin, chunks))
    }

    fn capability_chunk_digest(
        start: usize,
        capabilities: &[ElevatedCapability],
    ) -> Result<String, ElevatedTransportError> {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"codehangar/elevated-stream/chunk/2-handle-neutral\0");
        hasher.update(&(start as u64).to_le_bytes());
        hasher.update(&(capabilities.len() as u64).to_le_bytes());
        for capability in capabilities {
            let mut authorization = capability.clone();
            crate::elevated_protocol::neutralize_capability_handles(&mut authorization, 0);
            let encoded = serde_json::to_vec(&authorization)
                .map_err(|error| ElevatedTransportError::Protocol(error.to_string()))?;
            hasher.update(&(encoded.len() as u64).to_le_bytes());
            hasher.update(&encoded);
        }
        Ok(hasher.finalize().to_hex().to_string())
    }

    fn stream_batch_digest(
        operation_uuid: &str,
        header: &StreamRequestHeader,
        total: usize,
        commitments: &[ChunkCommitment],
    ) -> Result<String, ElevatedTransportError> {
        let header = serde_json::to_vec(header)
            .map_err(|error| ElevatedTransportError::Protocol(error.to_string()))?;
        let chunks = serde_json::to_vec(commitments)
            .map_err(|error| ElevatedTransportError::Protocol(error.to_string()))?;
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"codehangar/elevated-stream/batch/1\0");
        hasher.update(&(operation_uuid.len() as u64).to_le_bytes());
        hasher.update(operation_uuid.as_bytes());
        hasher.update(&(header.len() as u64).to_le_bytes());
        hasher.update(&header);
        hasher.update(&(total as u64).to_le_bytes());
        hasher.update(&(chunks.len() as u64).to_le_bytes());
        hasher.update(&chunks);
        Ok(hasher.finalize().to_hex().to_string())
    }

    fn parent_roundtrip<Request, Response>(
        pipe: &PipeServer,
        key: &[u8; 32],
        operation_uuid: &str,
        nonce: &str,
        request_sequence: u64,
        request: &Request,
    ) -> Result<Response, ElevatedTransportError>
    where
        Request: Serialize,
        Response: DeserializeOwned,
    {
        let payload = serde_json::to_vec(request)
            .map_err(|error| ElevatedTransportError::Protocol(error.to_string()))?;
        let request_digest = blake3::hash(&payload).to_hex().to_string();
        let request_context = FrameContext {
            role: FrameRole::ParentRequest,
            sequence: request_sequence,
            operation_uuid: operation_uuid.to_string(),
            nonce: nonce.to_string(),
            request_blake3: request_digest.clone(),
        };
        let frame = encode_authenticated(request, key, &request_context)
            .map_err(ElevatedTransportError::Protocol)?;
        pipe.write_message(&frame)?;
        let response_frame = pipe.read_message(MAX_FRAME_BYTES + FRAME_PREFIX_BYTES)?;
        pipe.ensure_no_queued_message("helper sent an out-of-order response frame")?;
        let response_context = FrameContext {
            role: FrameRole::HelperResponse,
            sequence: request_sequence.checked_add(1).ok_or_else(|| {
                ElevatedTransportError::Protocol("stream sequence overflow".to_string())
            })?,
            operation_uuid: operation_uuid.to_string(),
            nonce: nonce.to_string(),
            request_blake3: request_digest,
        };
        decode_authenticated(&response_frame, key, &response_context)
            .map_err(ElevatedTransportError::Protocol)
    }

    fn helper_receive<Request>(
        pipe: HANDLE,
        key: &[u8; 32],
        nonce: &str,
        expected_sequence: u64,
        expected_operation_uuid: Option<&str>,
    ) -> Result<(Request, FrameContext), ElevatedTransportError>
    where
        Request: DeserializeOwned,
    {
        let frame = sync_read_message(pipe, MAX_FRAME_BYTES + FRAME_PREFIX_BYTES)?;
        ensure_no_queued_message(pipe, "parent queued an out-of-order request frame")?;
        let context = parent_context_from_frame(&frame, nonce, expected_sequence)?;
        require_stream_operation_uuid(&context, expected_operation_uuid)?;
        let request = decode_authenticated(&frame, key, &context)
            .map_err(ElevatedTransportError::Protocol)?;
        Ok((request, context))
    }

    fn require_stream_operation_uuid(
        context: &FrameContext,
        expected: Option<&str>,
    ) -> Result<(), ElevatedTransportError> {
        if expected.is_some_and(|value| context.operation_uuid != value) {
            Err(ElevatedTransportError::Protocol(
                "stream frame changed the authenticated begin operation UUID".to_string(),
            ))
        } else {
            Ok(())
        }
    }

    fn helper_send<Response>(
        pipe: HANDLE,
        key: &[u8; 32],
        request_context: &FrameContext,
        response: &Response,
    ) -> Result<(), ElevatedTransportError>
    where
        Response: Serialize,
    {
        let context = FrameContext {
            role: FrameRole::HelperResponse,
            sequence: request_context.sequence.checked_add(1).ok_or_else(|| {
                ElevatedTransportError::Protocol("stream sequence overflow".to_string())
            })?,
            operation_uuid: request_context.operation_uuid.clone(),
            nonce: request_context.nonce.clone(),
            request_blake3: request_context.request_blake3.clone(),
        };
        let frame = encode_authenticated(response, key, &context)
            .map_err(ElevatedTransportError::Protocol)?;
        sync_write_message(pipe, &frame)
    }

    fn finish_helper(
        pipe: &PipeServer,
        child: &mut ChildGuard,
    ) -> Result<(), ElevatedTransportError> {
        if unsafe { FlushFileBuffers(pipe.handle.raw()) } == 0 {
            let error = unsafe { GetLastError() };
            if !is_closed_pipe_error(error) {
                return Err(last_io("cannot flush the one-shot pipe"));
            }
        }
        unsafe {
            DisconnectNamedPipe(pipe.handle.raw());
        }
        child.require_clean_exit()
    }

    struct ResidentBatch {
        capabilities: Vec<ElevatedCapability>,
    }

    impl LazyElevatedCapabilityBatch for ResidentBatch {
        type Guard = ();

        fn total_capabilities(&self) -> usize {
            self.capabilities.len()
        }

        fn materialize_chunk(
            &mut self,
            range: std::ops::Range<usize>,
            _nonce: &str,
        ) -> Result<MaterializedCapabilityChunk<Self::Guard>, ElevatedTransportError> {
            Ok(MaterializedCapabilityChunk {
                capabilities: self.capabilities[range].to_vec(),
                guard: (),
            })
        }

        fn consume_chunk(
            &mut self,
            _start_index: usize,
            _guard: Self::Guard,
            _results: &[ElevatedItemResult],
        ) -> Result<(), ElevatedTransportError> {
            Ok(())
        }
    }

    pub(super) fn invoke(
        request: ElevatedRequest,
        operation_uuid: &str,
        helper_path: &Path,
    ) -> Result<ElevatedResponse, ElevatedTransportError> {
        validate_resident_batch_capacity(request.capabilities.len())?;
        let mut batch = ResidentBatch {
            capabilities: request.capabilities.clone(),
        };
        invoke_with_batch(request, operation_uuid, helper_path, &mut batch, true)
    }

    pub(super) fn invoke_lazy<Batch: LazyElevatedCapabilityBatch>(
        request_templates: ElevatedRequest,
        operation_uuid: &str,
        helper_path: &Path,
        batch: &mut Batch,
    ) -> Result<ElevatedResponse, ElevatedTransportError> {
        invoke_with_batch(request_templates, operation_uuid, helper_path, batch, false)
    }

    fn invoke_with_batch<Batch: LazyElevatedCapabilityBatch>(
        mut request: ElevatedRequest,
        operation_uuid: &str,
        helper_path: &Path,
        batch: &mut Batch,
        require_resident_handles_before_uac: bool,
    ) -> Result<ElevatedResponse, ElevatedTransportError> {
        if request.synthetic_test {
            return Err(ElevatedTransportError::Identity(
                "synthetic fixture mode is not exposed by the release launcher".to_string(),
            ));
        }
        if request.capabilities.is_empty()
            || request.capabilities.len() > MAX_CAPABILITIES_PER_INVOCATION
            || batch.total_capabilities() != request.capabilities.len()
        {
            return Err(ElevatedTransportError::Protocol(
                "lazy elevated batch/template count is outside the protocol bound".to_string(),
            ));
        }
        require_hex(operation_uuid, 32, "operation UUID")?;
        let nonce = validated_persisted_nonce(&request)?;
        assign_dynamic_capability_fields(&mut request);

        let parent = inspect_process(unsafe { GetCurrentProcessId() }, None)?;
        crate::bound_fs::validate_local_mutation_path(helper_path)
            .map_err(|error| ElevatedTransportError::Identity(error.to_string()))?;
        let release = load_release_manifest(helper_path)?;
        verify_identity(
            &parent,
            ReleaseRole::Parent,
            &IdentityPolicy::Release,
            Some(&release),
        )?;
        require_parent_binding(&request.parent, &parent)?;

        let helper_file = open_image_file(helper_path)?;
        let helper_stamp = crate::bound_fs::platform_file_stamp(&helper_file)
            .map_err(ElevatedTransportError::Io)?;
        let helper_hash = hash_sha256(&helper_file)?;
        verify_release_identity(helper_path, &helper_hash, ReleaseRole::Helper, &release)?;

        let admitted_at_unix_seconds = now_unix_seconds()?;
        let validation = if require_resident_handles_before_uac {
            request.validate(
                admitted_at_unix_seconds,
                &nonce,
                parent.pid,
                parent.session_id,
                None,
            )
        } else {
            request.validate_authorization_slice(
                admitted_at_unix_seconds,
                &nonce,
                parent.pid,
                parent.session_id,
                None,
                0,
            )
        };
        validation.map_err(ElevatedTransportError::Protocol)?;
        // Planning and size refusal happen before creating the pipe or showing
        // UAC. Large immutable batches are streamed through one helper session;
        // they are never silently split into multiple elevation prompts.
        let (begin, planned_chunks) = plan_stream(&request, operation_uuid)?;

        let pipe_name = pipe_name(&nonce);
        let pipe = PipeServer::create(&pipe_name)?;
        let mut child = launch_helper(helper_path, &pipe_name, parent.pid, &nonce)?;
        pipe.connect(Some(child.raw()))?;

        let mut client_pid = 0u32;
        if unsafe { GetNamedPipeClientProcessId(pipe.handle.raw(), &mut client_pid) } == 0
            || client_pid == 0
            || client_pid != unsafe { GetProcessId(child.raw()) }
        {
            return Err(ElevatedTransportError::Identity(
                "named-pipe client is not the process returned by ShellExecuteExW".to_string(),
            ));
        }
        let helper = inspect_process(client_pid, Some(child.raw()))?;
        if helper.session_id != parent.session_id || helper.user_sid != parent.user_sid {
            return Err(ElevatedTransportError::Identity(
                "elevated helper is not in the parent user/session".to_string(),
            ));
        }
        if !helper_stamp.same_object(
            &crate::bound_fs::platform_file_stamp(&helper._image_file)
                .map_err(ElevatedTransportError::Io)?,
        ) {
            return Err(ElevatedTransportError::Identity(
                "elevated process image is not the selected helper object".to_string(),
            ));
        }
        verify_identity(
            &helper,
            ReleaseRole::Helper,
            &IdentityPolicy::Release,
            Some(&release),
        )?;
        if helper.image_sha256 != helper_hash {
            return Err(ElevatedTransportError::Identity(
                "selected helper changed between validation and launch".to_string(),
            ));
        }

        let hello = pipe.read_exact_message(HELLO_MAGIC.len() + 64)?;
        validate_hello(&hello, &nonce, &release.digest)?;

        let key = random_key()?;
        pipe.write_message(&key_message(&nonce, &release.digest, &key)?)?;
        let mut sequence = 1u64;
        let begin_reply: StreamReply =
            parent_roundtrip(&pipe, &key, operation_uuid, &nonce, sequence, &begin)?;
        sequence += 2;
        let (helper_image_sha256, privilege_bitmap) = match begin_reply {
            StreamReply::BeginReady {
                schema,
                batch_blake3,
                helper_image_sha256,
                privilege_bitmap,
            } if schema == STREAM_SCHEMA
                && batch_blake3 == begin.batch_blake3
                && helper_image_sha256 == helper.image_sha256
                && privilege_bitmap == REQUIRED_OBJECT_PRIVILEGES =>
            {
                (helper_image_sha256, privilege_bitmap)
            }
            StreamReply::Failure(failure) => {
                let response = ElevatedResponse::Failure(failure);
                validate_response(
                    &response,
                    &request,
                    &helper.image_sha256,
                    if require_resident_handles_before_uac {
                        request.capabilities.len()
                    } else {
                        0
                    },
                )?;
                finish_helper(&pipe, &mut child)?;
                return Ok(response);
            }
            _ => {
                return Err(ElevatedTransportError::Protocol(
                    "helper did not acknowledge the committed stream batch".to_string(),
                ))
            }
        };

        // The compatibility resident API returns its bounded result vector.
        // The production lazy API commits/consumes each authenticated chunk
        // immediately and returns only the batch summary, keeping peak result
        // memory bounded by MAX_CHUNK_CAPABILITIES.
        let mut resident_items = require_resident_handles_before_uac
            .then(|| Vec::with_capacity(request.capabilities.len()));
        let mut stopped_after = None;
        for (chunk_index, planned) in planned_chunks.iter().enumerate() {
            let MaterializedCapabilityChunk {
                mut capabilities,
                guard,
            } = batch.materialize_chunk(planned.start..planned.end, &nonce)?;
            if capabilities.len() != planned.end - planned.start {
                return Err(ElevatedTransportError::Protocol(format!(
                    "lazy provider materialized {} capabilities for committed range {}..{}",
                    capabilities.len(),
                    planned.start,
                    planned.end
                )));
            }
            assign_dynamic_capability_fields_slice(&mut capabilities, &nonce, planned.start);
            if capability_chunk_digest(planned.start, &capabilities)? != planned.digest {
                return Err(ElevatedTransportError::Protocol(
                    "lazy provider changed a pre-UAC capability authorization".to_string(),
                ));
            }
            let fragment =
                StreamRequestHeader::from_request(&request).request_with(capabilities.clone());
            fragment
                .validate_capability_slice(
                    admitted_at_unix_seconds,
                    &nonce,
                    parent.pid,
                    parent.session_id,
                    None,
                    planned.start as u32,
                )
                .map_err(ElevatedTransportError::Protocol)?;
            let chunk = StreamChunk {
                schema: STREAM_SCHEMA.to_string(),
                batch_blake3: begin.batch_blake3.clone(),
                chunk_index: chunk_index as u32,
                start_index: planned.start as u32,
                capabilities,
            };
            let command = StreamCommand::Chunk(chunk);
            let reply: StreamReply =
                parent_roundtrip(&pipe, &key, operation_uuid, &nonce, sequence, &command)?;
            sequence = sequence.checked_add(2).ok_or_else(|| {
                ElevatedTransportError::Protocol("stream sequence overflow".to_string())
            })?;
            match reply {
                StreamReply::ChunkResults {
                    schema,
                    batch_blake3,
                    chunk_index: returned_chunk,
                    start_index,
                    items: returned,
                } if schema == STREAM_SCHEMA
                    && batch_blake3 == begin.batch_blake3
                    && returned_chunk == chunk_index as u32
                    && start_index == planned.start as u32
                    && returned.len() == planned.end - planned.start =>
                {
                    for (offset, item) in returned.iter().enumerate() {
                        let expected = (planned.start + offset) as u32;
                        let actual = match item {
                            ElevatedItemResult::Ready(value) => {
                                validate_ready_result(value)?;
                                value.capability_index
                            }
                            ElevatedItemResult::Blocked {
                                capability_index, ..
                            } => *capability_index,
                        };
                        if actual != expected {
                            return Err(ElevatedTransportError::Protocol(
                                "helper reordered a streamed capability result".to_string(),
                            ));
                        }
                    }
                    batch.consume_chunk(planned.start, guard, &returned)?;
                    if let Some(items) = resident_items.as_mut() {
                        items.extend(returned);
                    }
                }
                StreamReply::Failure(failure) => {
                    return Err(ElevatedTransportError::Protocol(format!(
                        "helper aborted committed batch {}: {}",
                        failure.code, failure.message
                    )))
                }
                _ => {
                    return Err(ElevatedTransportError::Protocol(
                        "helper returned an invalid capability chunk response".to_string(),
                    ))
                }
            }
            if batch.stop_stream_requested() {
                if require_resident_handles_before_uac {
                    return Err(ElevatedTransportError::Protocol(
                        "resident elevated batches cannot stop a committed stream early"
                            .to_string(),
                    ));
                }
                stopped_after = Some((planned.end as u32, (chunk_index + 1) as u32));
                break;
            }
        }

        if let Some((processed_capabilities, processed_chunks)) = stopped_after {
            let stop = StreamCommand::Stop(StreamStop {
                schema: STREAM_SCHEMA.to_string(),
                batch_blake3: begin.batch_blake3.clone(),
                processed_capabilities,
                processed_chunks,
            });
            let stop_reply: StreamReply =
                parent_roundtrip(&pipe, &key, operation_uuid, &nonce, sequence, &stop)?;
            match stop_reply {
                StreamReply::StopReady {
                    schema,
                    batch_blake3,
                    processed_capabilities: returned_capabilities,
                    processed_chunks: returned_chunks,
                } if schema == STREAM_SCHEMA
                    && batch_blake3 == begin.batch_blake3
                    && returned_capabilities == processed_capabilities
                    && returned_chunks == processed_chunks => {}
                _ => {
                    return Err(ElevatedTransportError::Protocol(
                        "helper did not acknowledge the authenticated stream stop".to_string(),
                    ))
                }
            }
        } else {
            let end = StreamCommand::End(StreamEnd {
                schema: STREAM_SCHEMA.to_string(),
                batch_blake3: begin.batch_blake3.clone(),
                total_capabilities: begin.total_capabilities,
                chunk_count: begin.chunks.len() as u32,
            });
            let end_reply: StreamReply =
                parent_roundtrip(&pipe, &key, operation_uuid, &nonce, sequence, &end)?;
            match end_reply {
                StreamReply::EndReady {
                    schema,
                    batch_blake3,
                    total_capabilities,
                    chunk_count,
                } if schema == STREAM_SCHEMA
                    && batch_blake3 == begin.batch_blake3
                    && total_capabilities == begin.total_capabilities
                    && chunk_count == begin.chunks.len() as u32 => {}
                _ => {
                    return Err(ElevatedTransportError::Protocol(
                        "helper did not close the committed stream batch exactly".to_string(),
                    ))
                }
            }
        }
        pipe.ensure_no_queued_message("helper sent a second batch")?;
        let response = ElevatedResponse::Success(ElevatedSuccess {
            schema: PROTOCOL_SCHEMA.to_string(),
            nonce: nonce.clone(),
            operation_id: request.operation_id,
            helper_image_sha256,
            privilege_bitmap,
            items: resident_items.unwrap_or_default(),
        });
        validate_response(
            &response,
            &request,
            &helper.image_sha256,
            if require_resident_handles_before_uac {
                request.capabilities.len()
            } else {
                0
            },
        )?;
        finish_helper(&pipe, &mut child)?;
        Ok(response)
    }

    fn validate_resident_batch_capacity(count: usize) -> Result<(), ElevatedTransportError> {
        if count == 0 || count > super::MAX_RESIDENT_CAPABILITIES_PER_INVOCATION {
            Err(ElevatedTransportError::Protocol(format!(
                "resident elevated batch has {count} capabilities; supported range is 1..={}",
                super::MAX_RESIDENT_CAPABILITIES_PER_INVOCATION
            )))
        } else {
            Ok(())
        }
    }

    pub(crate) fn launch_disposition_guardian(
        launch: DispositionGuardianLaunch<'_>,
    ) -> Result<DispositionGuardian, ElevatedTransportError> {
        let DispositionGuardianLaunch {
            helper_path,
            operation_uuid,
            guardian_nonce,
            operation_id,
            batch_item_id,
            parent_handle_value,
            expected_stamp,
            receipt_path,
        } = launch;
        require_hex(operation_uuid, 32, "guardian operation UUID")?;
        require_hex(guardian_nonce, 64, "guardian nonce")?;
        if operation_id <= 0
            || batch_item_id <= 0
            || parent_handle_value == 0
            || parent_handle_value == u64::MAX
            || expected_stamp.volume_id.is_empty()
            || expected_stamp.file_id.is_empty()
        {
            return Err(ElevatedTransportError::Protocol(
                "guardian binding has invalid durable identity fields".to_string(),
            ));
        }

        let parent = inspect_process(unsafe { GetCurrentProcessId() }, None)?;
        crate::bound_fs::validate_local_mutation_path(helper_path)
            .map_err(|error| ElevatedTransportError::Identity(error.to_string()))?;
        let release = load_release_manifest(helper_path)?;
        verify_identity(
            &parent,
            ReleaseRole::Parent,
            &IdentityPolicy::Release,
            Some(&release),
        )?;

        // Keep the exact helper image open without write/delete sharing from
        // pre-launch verification until the created process image is rebound.
        let helper_file = open_image_file(helper_path)?;
        let helper_stamp = crate::bound_fs::platform_file_stamp(&helper_file)
            .map_err(ElevatedTransportError::Io)?;
        let helper_hash = hash_sha256(&helper_file)?;
        verify_release_identity(helper_path, &helper_hash, ReleaseRole::Helper, &release)?;

        let receipt = crate::bound_fs::BoundGuardianReceipt::create_new(receipt_path)
            .map_err(ElevatedTransportError::Io)?;
        if receipt.initial_stamp().bytes != 0 || receipt.initial_stamp().same_object(expected_stamp)
        {
            return Err(ElevatedTransportError::Identity(
                "guardian receipt is not a fresh, distinct empty FileId".to_string(),
            ));
        }

        let pipe_name = guardian_pipe_name(guardian_nonce);
        let pipe = PipeServer::create(&pipe_name)?;
        let mut command = Command::new(helper_path);
        command
            .arg(GUARDIAN_SELECTOR)
            .arg("--pipe")
            .arg(&pipe_name)
            .arg("--parent-pid")
            .arg(parent.pid.to_string())
            .arg("--nonce")
            .arg(guardian_nonce)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // A guardian inherited into a kill-on-close parent job would die at
            // the same instant as the desktop and would not be independent.
            // Break away or fail before any disposition is armed; job policy is
            // never silently weakened into a parent-only safety claim.
            .creation_flags(CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB);
        let child = command.spawn().map_err(|error| {
            ElevatedTransportError::Launch(format!(
                "non-elevated disposition guardian launch failed: {error}"
            ))
        })?;
        let pending = PendingDispositionGuardian {
            pipe: Some(pipe),
            child: Some(child),
            receipt: Some(receipt),
        };
        let child_handle = pending.child().as_raw_handle() as HANDLE;
        pending.pipe().connect(Some(child_handle))?;

        let mut client_pid = 0u32;
        if unsafe { GetNamedPipeClientProcessId(pending.pipe().handle.raw(), &mut client_pid) } == 0
            || client_pid == 0
            || client_pid != pending.child().id()
        {
            return Err(ElevatedTransportError::Identity(
                "guardian pipe client is not the exact launched process".to_string(),
            ));
        }
        let guardian = inspect_process(client_pid, Some(child_handle))?;
        if guardian.session_id != parent.session_id || guardian.user_sid != parent.user_sid {
            return Err(ElevatedTransportError::Identity(
                "guardian is not in the parent user/session".to_string(),
            ));
        }
        if !helper_stamp.same_object(
            &crate::bound_fs::platform_file_stamp(&guardian._image_file)
                .map_err(ElevatedTransportError::Io)?,
        ) {
            return Err(ElevatedTransportError::Identity(
                "guardian process image is not the selected helper object".to_string(),
            ));
        }
        verify_identity(
            &guardian,
            ReleaseRole::Helper,
            &IdentityPolicy::Release,
            Some(&release),
        )?;
        if guardian.image_sha256 != helper_hash {
            return Err(ElevatedTransportError::Identity(
                "guardian image changed between verification and launch".to_string(),
            ));
        }

        let hello = pending.pipe().read_exact_message(HELLO_MAGIC.len() + 64)?;
        validate_hello(&hello, guardian_nonce, &release.digest)?;
        let key = random_key()?;
        pending
            .pipe()
            .write_message(&key_message(guardian_nonce, &release.digest, &key)?)?;
        let nonce_digest = blake3::hash(guardian_nonce.as_bytes()).to_hex().to_string();
        let receipt_key = GuardianReceiptKey(random_key()?);
        let receipt_context = GuardianReceiptKeyContext {
            schema: GUARDIAN_RECEIPT_SCHEMA.to_string(),
            operation_id,
            batch_item_id,
            nonce_digest: nonce_digest.clone(),
            guardian_pid: guardian.pid,
            guardian_started_100ns: guardian.started_100ns,
            guardian_image_sha256: guardian.image_sha256.clone(),
            target_stamp: expected_stamp.clone(),
            receipt_volume_id: pending.receipt().initial_stamp().volume_id.clone(),
            receipt_file_id: pending.receipt().initial_stamp().file_id.clone(),
        };
        let protected_key_hex = protect_receipt_key(&receipt_key, &receipt_context)?;
        let bind = GuardianCommand::Bind {
            schema: GUARDIAN_SCHEMA.to_string(),
            operation_id,
            batch_item_id,
            parent_handle_value,
            expected_stamp: expected_stamp.clone(),
            receipt_handle_value: pending.receipt().raw_handle_value(),
            expected_receipt_stamp: pending.receipt().initial_stamp().clone(),
            receipt_key,
        };
        let reply: GuardianReply = parent_roundtrip(
            pending.pipe(),
            &key,
            operation_uuid,
            guardian_nonce,
            1,
            &bind,
        )?;
        reply.require_common(batch_item_id)?;
        match reply {
            GuardianReply::HandleBound {
                guardian_pid,
                guardian_started_100ns,
                guardian_image_sha256,
                ..
            } if guardian_pid == guardian.pid
                && guardian_started_100ns == guardian.started_100ns
                && guardian_image_sha256 == guardian.image_sha256 => {}
            GuardianReply::Refused { message, .. } => {
                return Err(ElevatedTransportError::Protocol(message));
            }
            _ => {
                return Err(ElevatedTransportError::Protocol(
                    "guardian did not bind the exact duplicated handle".to_string(),
                ));
            }
        }
        let identity = DispositionGuardianIdentity {
            pid: guardian.pid,
            process_started_100ns: guardian.started_100ns,
            image_sha256: guardian.image_sha256,
            nonce_digest,
            receipt: GuardianReceiptAuthority {
                path: pending.receipt().path().to_path_buf(),
                initial_stamp: pending.receipt().initial_stamp().clone(),
                protected_key_hex,
            },
        };
        Ok(pending.promote(key, operation_uuid, guardian_nonce, batch_item_id, identity))
    }

    pub(super) fn run_cli<I>(args: I) -> Result<(), ElevatedTransportError>
    where
        I: IntoIterator<Item = OsString>,
    {
        let mut values = args.into_iter().collect::<Vec<_>>();
        if values
            .first()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value == GUARDIAN_SELECTOR)
        {
            values.remove(0);
            let cli = parse_guardian_cli(values)?;
            return guardian_exchange(&cli);
        }
        let cli = parse_cli(values)?;
        helper_exchange(
            &cli.pipe_name,
            cli.parent_pid,
            &cli.nonce,
            IdentityPolicy::Release,
            None,
        )
    }

    struct HelperCli {
        pipe_name: String,
        parent_pid: u32,
        nonce: String,
    }

    fn parse_cli<I>(args: I) -> Result<HelperCli, ElevatedTransportError>
    where
        I: IntoIterator<Item = OsString>,
    {
        let values = args.into_iter().collect::<Vec<_>>();
        if values.len() != 6 {
            return Err(ElevatedTransportError::Protocol(
                "helper command line must contain exactly pipe, parent PID and nonce".to_string(),
            ));
        }
        let mut pipe = None;
        let mut parent_pid = None;
        let mut nonce = None;
        for pair in values.as_chunks::<2>().0 {
            let flag = pair[0].to_str().ok_or_else(|| {
                ElevatedTransportError::Protocol("helper flag is not valid Unicode".to_string())
            })?;
            match flag {
                "--pipe" if pipe.is_none() => pipe = Some(os_string(&pair[1], "pipe")?),
                "--parent-pid" if parent_pid.is_none() => {
                    let text = os_string(&pair[1], "parent PID")?;
                    let parsed = text.parse::<u32>().map_err(|_| {
                        ElevatedTransportError::Protocol("helper parent PID is invalid".to_string())
                    })?;
                    if parsed == 0 {
                        return Err(ElevatedTransportError::Protocol(
                            "helper parent PID is invalid".to_string(),
                        ));
                    }
                    parent_pid = Some(parsed);
                }
                "--nonce" if nonce.is_none() => nonce = Some(os_string(&pair[1], "nonce")?),
                _ => {
                    return Err(ElevatedTransportError::Protocol(
                        "helper command line contains an unknown or duplicate flag".to_string(),
                    ))
                }
            }
        }
        let nonce = nonce.ok_or_else(|| {
            ElevatedTransportError::Protocol("helper nonce is missing".to_string())
        })?;
        require_hex(&nonce, 64, "nonce")?;
        let expected_pipe = pipe_name(&nonce);
        let pipe_name = pipe.ok_or_else(|| {
            ElevatedTransportError::Protocol("helper pipe is missing".to_string())
        })?;
        if pipe_name != expected_pipe {
            return Err(ElevatedTransportError::Protocol(
                "helper pipe name is not derived from its nonce".to_string(),
            ));
        }
        Ok(HelperCli {
            pipe_name,
            parent_pid: parent_pid.ok_or_else(|| {
                ElevatedTransportError::Protocol("helper parent PID is missing".to_string())
            })?,
            nonce,
        })
    }

    struct GuardianCli {
        pipe_name: String,
        parent_pid: u32,
        nonce: String,
    }

    fn parse_guardian_cli<I>(args: I) -> Result<GuardianCli, ElevatedTransportError>
    where
        I: IntoIterator<Item = OsString>,
    {
        let values = args.into_iter().collect::<Vec<_>>();
        if values.len() != 6 {
            return Err(ElevatedTransportError::Protocol(
                "guardian command line must contain exactly pipe, parent PID and nonce".to_string(),
            ));
        }
        let mut pipe = None;
        let mut parent_pid = None;
        let mut nonce = None;
        for pair in values.as_chunks::<2>().0 {
            let flag = pair[0].to_str().ok_or_else(|| {
                ElevatedTransportError::Protocol("guardian flag is not valid Unicode".to_string())
            })?;
            match flag {
                "--pipe" if pipe.is_none() => pipe = Some(os_string(&pair[1], "pipe")?),
                "--parent-pid" if parent_pid.is_none() => {
                    let text = os_string(&pair[1], "parent PID")?;
                    let parsed = text.parse::<u32>().map_err(|_| {
                        ElevatedTransportError::Protocol(
                            "guardian parent PID is invalid".to_string(),
                        )
                    })?;
                    if parsed == 0 {
                        return Err(ElevatedTransportError::Protocol(
                            "guardian parent PID is invalid".to_string(),
                        ));
                    }
                    parent_pid = Some(parsed);
                }
                "--nonce" if nonce.is_none() => nonce = Some(os_string(&pair[1], "nonce")?),
                _ => {
                    return Err(ElevatedTransportError::Protocol(
                        "guardian command line contains an unknown or duplicate flag".to_string(),
                    ));
                }
            }
        }
        let nonce = nonce.ok_or_else(|| {
            ElevatedTransportError::Protocol("guardian nonce is missing".to_string())
        })?;
        require_hex(&nonce, 64, "guardian nonce")?;
        let pipe_name = pipe.ok_or_else(|| {
            ElevatedTransportError::Protocol("guardian pipe is missing".to_string())
        })?;
        if pipe_name != guardian_pipe_name(&nonce) {
            return Err(ElevatedTransportError::Protocol(
                "guardian pipe name is not derived from its nonce".to_string(),
            ));
        }
        Ok(GuardianCli {
            pipe_name,
            parent_pid: parent_pid.ok_or_else(|| {
                ElevatedTransportError::Protocol("guardian parent PID is missing".to_string())
            })?,
            nonce,
        })
    }

    fn guardian_exchange(cli: &GuardianCli) -> Result<(), ElevatedTransportError> {
        guardian_exchange_with_policy(cli, IdentityPolicy::Release)
    }

    fn guardian_exchange_with_policy(
        cli: &GuardianCli,
        policy: IdentityPolicy,
    ) -> Result<(), ElevatedTransportError> {
        let guardian = inspect_process(unsafe { GetCurrentProcessId() }, None)?;
        let release = match &policy {
            IdentityPolicy::Release => Some(load_release_manifest(&guardian.image_path)?),
            #[cfg(test)]
            IdentityPolicy::SyntheticFixture { .. } => None,
        };
        verify_identity(&guardian, ReleaseRole::Helper, &policy, release.as_ref())?;
        let mut parent = inspect_process(cli.parent_pid, None)?;
        verify_identity(&parent, ReleaseRole::Parent, &policy, release.as_ref())?;
        if guardian.session_id != parent.session_id || guardian.user_sid != parent.user_sid {
            return Err(ElevatedTransportError::Identity(
                "guardian pipe server is not in the same user/session".to_string(),
            ));
        }

        let pipe = open_pipe_client(&cli.pipe_name)?;
        let mut server_pid = 0u32;
        if unsafe { GetNamedPipeServerProcessId(pipe.raw(), &mut server_pid) } == 0
            || server_pid != cli.parent_pid
        {
            return Err(ElevatedTransportError::Identity(
                "guardian pipe server PID does not match the verified parent".to_string(),
            ));
        }
        let manifest_digest = release.as_ref().map_or([0u8; 32], |proof| proof.digest);
        sync_write_message(pipe.raw(), &hello_message(&cli.nonce, &manifest_digest)?)?;
        let key_bytes = sync_read_exact_message(pipe.raw(), KEY_MAGIC.len() + 96)?;
        let key = parse_key_message(&key_bytes, &cli.nonce, &manifest_digest)?;
        let (bind, bind_context): (GuardianCommand, FrameContext) =
            helper_receive(pipe.raw(), &key, &cli.nonce, 1, None)?;
        let (
            operation_id,
            batch_item_id,
            parent_handle_value,
            expected_stamp,
            receipt_handle_value,
            expected_receipt_stamp,
            receipt_key,
        ) = match bind {
            GuardianCommand::Bind {
                schema,
                operation_id,
                batch_item_id,
                parent_handle_value,
                expected_stamp,
                receipt_handle_value,
                expected_receipt_stamp,
                receipt_key,
            } if schema == GUARDIAN_SCHEMA
                && operation_id > 0
                && batch_item_id > 0
                && parent_handle_value != 0
                && parent_handle_value != u64::MAX
                && receipt_handle_value != 0
                && receipt_handle_value != u64::MAX
                && receipt_handle_value != parent_handle_value
                && !expected_stamp.volume_id.is_empty()
                && !expected_stamp.file_id.is_empty()
                && expected_receipt_stamp.bytes == 0
                && !expected_receipt_stamp.volume_id.is_empty()
                && !expected_receipt_stamp.file_id.is_empty() =>
            {
                (
                    operation_id,
                    batch_item_id,
                    parent_handle_value,
                    expected_stamp,
                    receipt_handle_value,
                    expected_receipt_stamp,
                    receipt_key,
                )
            }
            _ => {
                return Err(ElevatedTransportError::Protocol(
                    "guardian first frame is not one bounded handle binding".to_string(),
                ));
            }
        };
        let parent_process = parent._process.take().ok_or_else(|| {
            ElevatedTransportError::Identity(
                "guardian did not retain the exact parent process lifetime handle".to_string(),
            )
        })?;
        let mut guarded = GuardianTargetHandle::new(
            duplicate_parent_file(cli.parent_pid, parent_handle_value)?,
            parent_process,
        );
        let actual = crate::bound_fs::platform_file_stamp(guarded.file())?;
        if actual != expected_stamp {
            return Err(ElevatedTransportError::Identity(
                "guardian duplicated handle does not match the journaled FileId/stamp".to_string(),
            ));
        }
        crate::bound_fs::validate_final_disposition_profile(guarded.file(), false, None)
            .map_err(ElevatedTransportError::Io)?;
        let receipt = duplicate_parent_file(cli.parent_pid, receipt_handle_value)?;
        let actual_receipt = crate::bound_fs::platform_file_stamp(&receipt)?;
        if actual_receipt != expected_receipt_stamp
            || actual_receipt.same_object(&actual)
            || actual_receipt.bytes != 0
        {
            return Err(ElevatedTransportError::Identity(
                "guardian duplicated receipt handle does not match its fresh distinct FileId"
                    .to_string(),
            ));
        }
        crate::bound_fs::validate_final_disposition_profile(&receipt, false, None)
            .map_err(ElevatedTransportError::Io)?;
        helper_send(
            pipe.raw(),
            &key,
            &bind_context,
            &GuardianReply::HandleBound {
                schema: GUARDIAN_SCHEMA.to_string(),
                batch_item_id,
                guardian_pid: guardian.pid,
                guardian_started_100ns: guardian.started_100ns,
                guardian_image_sha256: guardian.image_sha256.clone(),
            },
        )?;

        let mut sequence = 3u64;
        let mut phase = GuardianPhase::Bound;
        let mut preferred_mode = None;
        loop {
            let received: Result<(GuardianCommand, FrameContext), ElevatedTransportError> =
                helper_receive(
                    pipe.raw(),
                    &key,
                    &cli.nonce,
                    sequence,
                    Some(&bind_context.operation_uuid),
                );
            let (command, context) = match received {
                Ok(value) => value,
                Err(_) => {
                    guarded.retain_until_cancelled();
                    return Ok(());
                }
            };
            sequence = sequence.checked_add(2).ok_or_else(|| {
                ElevatedTransportError::Protocol("guardian sequence overflow".to_string())
            })?;

            let schema_and_item_match = match &command {
                GuardianCommand::Bind { .. } => false,
                GuardianCommand::ArmAuthorized {
                    schema,
                    batch_item_id: returned,
                }
                | GuardianCommand::ProveArmed {
                    schema,
                    batch_item_id: returned,
                    ..
                }
                | GuardianCommand::Cancel {
                    schema,
                    batch_item_id: returned,
                    ..
                }
                | GuardianCommand::CloseAuthorized {
                    schema,
                    batch_item_id: returned,
                } => schema == GUARDIAN_SCHEMA && *returned == batch_item_id,
            };
            if !schema_and_item_match {
                let _ = helper_send(
                    pipe.raw(),
                    &key,
                    &context,
                    &guardian_refused(batch_item_id, "guardian command binding changed"),
                );
                guarded.retain_until_cancelled();
                return Ok(());
            }

            match command {
                GuardianCommand::ArmAuthorized { .. } if phase == GuardianPhase::Bound => {
                    phase = GuardianPhase::ArmAuthorized;
                    // Set this before the reply write. A complete message may
                    // reach the parent even if the transport subsequently
                    // reports failure, and the parent can arm immediately.
                    guarded.authorize_arm();
                    helper_send(
                        pipe.raw(),
                        &key,
                        &context,
                        &GuardianReply::ArmReady {
                            schema: GUARDIAN_SCHEMA.to_string(),
                            batch_item_id,
                        },
                    )?;
                }
                GuardianCommand::ProveArmed { mode, .. }
                    if phase == GuardianPhase::ArmAuthorized =>
                {
                    preferred_mode = Some(mode);
                    guarded.observe_mode(mode);
                    match crate::bound_fs::validate_final_disposition_profile(
                        guarded.file(),
                        true,
                        Some(mode),
                    ) {
                        Ok(()) => {
                            phase = GuardianPhase::ProfileProved;
                            injected_guardian_before_prove_reply()?;
                            helper_send(
                                pipe.raw(),
                                &key,
                                &context,
                                &GuardianReply::FinalProfileProvedHeld {
                                    schema: GUARDIAN_SCHEMA.to_string(),
                                    batch_item_id,
                                    mode,
                                },
                            )?;
                        }
                        Err(error) => {
                            helper_send(
                                pipe.raw(),
                                &key,
                                &context,
                                &guardian_refused(
                                    batch_item_id,
                                    &format!("guardian post-arm proof failed: {error}"),
                                ),
                            )?;
                        }
                    }
                }
                GuardianCommand::Cancel {
                    preferred_mode: requested,
                    ..
                } => {
                    // The authenticated signed parent has explicitly entered
                    // cancellation and will never arm this handle afterward.
                    guarded.exclude_future_arm();
                    preferred_mode = requested.or(preferred_mode);
                    if let Some(mode) = preferred_mode {
                        guarded.observe_mode(mode);
                    }
                    match guardian_try_cancel(guarded.file(), preferred_mode) {
                        Ok(()) => {
                            guarded.cancellation_proved();
                            helper_send(
                                pipe.raw(),
                                &key,
                                &context,
                                &GuardianReply::CancelledSafe {
                                    schema: GUARDIAN_SCHEMA.to_string(),
                                    batch_item_id,
                                },
                            )?;
                            return Ok(());
                        }
                        Err(error) => {
                            phase = GuardianPhase::CancellationPending;
                            helper_send(
                                pipe.raw(),
                                &key,
                                &context,
                                &GuardianReply::CancellationPendingRetained {
                                    schema: GUARDIAN_SCHEMA.to_string(),
                                    batch_item_id,
                                    message: bounded_message(&error.to_string()),
                                },
                            )?;
                        }
                    }
                }
                GuardianCommand::CloseAuthorized { .. }
                    if phase == GuardianPhase::ProfileProved =>
                {
                    // A second exact-handle query makes a stale parent proof
                    // unable to close the guardian. This is the only branch
                    // that intentionally closes an armed duplicate.
                    let mode = preferred_mode.ok_or_else(|| {
                        ElevatedTransportError::Protocol(
                            "guardian close has no proved disposition mode".to_string(),
                        )
                    })?;
                    if let Err(error) = crate::bound_fs::validate_final_disposition_profile(
                        guarded.file(),
                        true,
                        Some(mode),
                    ) {
                        helper_send(
                            pipe.raw(),
                            &key,
                            &context,
                            &guardian_refused(
                                batch_item_id,
                                &format!("guardian close revalidation failed: {error}"),
                            ),
                        )?;
                        guarded.retain_until_cancelled();
                        return Ok(());
                    }
                    let payload = GuardianCloseReceiptPayload {
                        schema: GUARDIAN_RECEIPT_SCHEMA.to_string(),
                        operation_id,
                        batch_item_id,
                        nonce_digest: blake3::hash(cli.nonce.as_bytes()).to_hex().to_string(),
                        guardian_pid: guardian.pid,
                        guardian_started_100ns: guardian.started_100ns,
                        guardian_image_sha256: guardian.image_sha256.clone(),
                        target_stamp: expected_stamp.clone(),
                        receipt_volume_id: expected_receipt_stamp.volume_id.clone(),
                        receipt_file_id: expected_receipt_stamp.file_id.clone(),
                        disposition_mode: mode,
                    };
                    if let Err(error) =
                        write_guardian_close_receipt(&receipt, &receipt_key, payload)
                    {
                        helper_send(
                            pipe.raw(),
                            &key,
                            &context,
                            &guardian_refused(
                                batch_item_id,
                                &format!("guardian durable receipt failed: {error}"),
                            ),
                        )?;
                        guarded.retain_until_cancelled();
                        return Ok(());
                    }
                    // The receipt is now durably flushed. If this process dies
                    // anywhere after this point, Windows necessarily closes the
                    // duplicated target handle. Recovery also requires this exact
                    // guardian process to be dead before trusting the receipt.
                    guarded.close_authorized_by_durable_receipt();
                    drop(guarded);
                    drop(receipt);
                    helper_send(
                        pipe.raw(),
                        &key,
                        &context,
                        &GuardianReply::HandleClosed {
                            schema: GUARDIAN_SCHEMA.to_string(),
                            batch_item_id,
                        },
                    )?;
                    if unsafe { FlushFileBuffers(pipe.raw()) } == 0 {
                        let error = unsafe { GetLastError() };
                        if !is_closed_pipe_error(error) {
                            return Err(last_io("cannot flush guardian close acknowledgement"));
                        }
                    }
                    return Ok(());
                }
                _ => {
                    helper_send(
                        pipe.raw(),
                        &key,
                        &context,
                        &guardian_refused(
                            batch_item_id,
                            "guardian command is invalid for its fail-closed phase",
                        ),
                    )?;
                }
            }
        }
    }

    fn guardian_refused(batch_item_id: i64, message: &str) -> GuardianReply {
        GuardianReply::Refused {
            schema: GUARDIAN_SCHEMA.to_string(),
            batch_item_id,
            message: bounded_message(message),
        }
    }

    fn duplicate_parent_file(
        parent_pid: u32,
        parent_handle_value: u64,
    ) -> Result<File, ElevatedTransportError> {
        if parent_handle_value == 0 || parent_handle_value == u64::MAX {
            return Err(ElevatedTransportError::Protocol(
                "guardian parent handle value is invalid".to_string(),
            ));
        }
        let parent = OwnedHandle::new(
            unsafe {
                OpenProcess(
                    PROCESS_DUP_HANDLE | PROCESS_QUERY_LIMITED_INFORMATION,
                    0,
                    parent_pid,
                )
            },
            "guardian cannot open the verified parent for handle duplication",
        )?;
        let mut duplicate = std::ptr::null_mut();
        let copied = unsafe {
            DuplicateHandle(
                parent.raw(),
                parent_handle_value as HANDLE,
                GetCurrentProcess(),
                &mut duplicate,
                0,
                0,
                DUPLICATE_SAME_ACCESS,
            )
        };
        if copied == 0 || duplicate.is_null() || duplicate == INVALID_HANDLE_VALUE {
            return Err(last_io("guardian cannot duplicate the exact parent handle"));
        }
        Ok(unsafe { File::from_raw_handle(duplicate as _) })
    }

    fn guardian_try_cancel(
        guarded: &File,
        preferred_mode: Option<crate::bound_fs::WindowsDeleteDispositionMode>,
    ) -> Result<(), std::io::Error> {
        let mut last = None;
        for _ in 0..GUARDIAN_RETRY_LIMIT {
            match crate::bound_fs::guardian_cancel_delete_on_close(guarded, preferred_mode) {
                Ok(()) => return Ok(()),
                Err(error) => last = Some(error),
            }
            std::thread::yield_now();
        }
        Err(last.unwrap_or_else(|| std::io::Error::other("guardian cancellation made no attempt")))
    }

    /// A failed/disconnected ArmReady reply is inherently ambiguous: the
    /// complete authenticated frame may already be waiting in the parent while
    /// its target handle is not delete-pending *yet*. Retain the guardian's
    /// duplicate for the exact remaining parent lifetime, cancelling any arm
    /// observed meanwhile. Only after that process object is signalled can a
    /// final DeletePending=false observation exclude every future arm.
    fn guardian_retain_until_parent_exit_and_cancelled(
        guarded: &File,
        preferred_mode: Option<crate::bound_fs::WindowsDeleteDispositionMode>,
        parent_process: HANDLE,
    ) {
        injected_guardian_prearm_retention_checkpoint();
        loop {
            // This may prove a current arm cancelled, but cannot by itself end
            // the loop while the authenticated parent is still alive.
            let _ = guardian_try_cancel(guarded, preferred_mode);
            match unsafe { WaitForSingleObject(parent_process, 0) } {
                WAIT_OBJECT_0 => {
                    guardian_retain_until_cancelled(guarded, preferred_mode);
                    return;
                }
                WAIT_TIMEOUT => {}
                WAIT_FAILED => {
                    // Fail closed without burning a core on a permanently
                    // invalid process wait. We cannot prove parent exit, so the
                    // guardian must retain the exact handle and keep trying to
                    // cancel, but a slower cadence is sufficient here.
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                }
                _ => {}
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// Deterministic coordination for the pre-arm disconnect regression. This
    /// code is absent from release builds.
    #[cfg(test)]
    fn injected_guardian_prearm_retention_checkpoint() {
        let Some(ready_path) = std::env::var_os("CODEHANGAR_TEST_GUARDIAN_PREARM_READY") else {
            return;
        };
        let Some(release_path) = std::env::var_os("CODEHANGAR_TEST_GUARDIAN_PREARM_RELEASE") else {
            return;
        };
        let _ = std::fs::write(Path::new(&ready_path), b"prearm-disconnect-retained");
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while !Path::new(&release_path).exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(not(test))]
    fn injected_guardian_prearm_retention_checkpoint() {}

    /// Deterministic subprocess coordination for the SAFE-05 regression. This
    /// code is absent from release builds. The guardian announces that it has
    /// consumed and validated `ProveArmed`, then waits while the parent test
    /// process exits before attempting the reply write.
    #[cfg(test)]
    fn injected_guardian_before_prove_reply() -> Result<(), ElevatedTransportError> {
        let Some(ready_path) = std::env::var_os("CODEHANGAR_TEST_GUARDIAN_PROVE_READY") else {
            return Ok(());
        };
        let release_path =
            std::env::var_os("CODEHANGAR_TEST_GUARDIAN_PROVE_RELEASE").ok_or_else(|| {
                ElevatedTransportError::Protocol(
                    "guardian prove-reply fixture has no release marker".to_string(),
                )
            })?;
        std::fs::write(Path::new(&ready_path), b"prove-armed-consumed")
            .map_err(ElevatedTransportError::Io)?;
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while !Path::new(&release_path).exists() {
            if std::time::Instant::now() >= deadline {
                return Err(ElevatedTransportError::Timeout(
                    "waiting for the parent-crash release marker".to_string(),
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }

    #[cfg(not(test))]
    fn injected_guardian_before_prove_reply() -> Result<(), ElevatedTransportError> {
        Ok(())
    }

    fn guardian_retain_until_cancelled(
        guarded: &File,
        preferred_mode: Option<crate::bound_fs::WindowsDeleteDispositionMode>,
    ) {
        // No timeout is safe here. Closing this duplicated handle while it may
        // be delete-pending is precisely the parent-crash data-loss branch the
        // guardian exists to close. Retry with bounded CPU until the kernel
        // proves cancellation; simultaneous guardian termination or machine
        // power loss remains an explicitly documented residual.
        loop {
            if guardian_try_cancel(guarded, preferred_mode).is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    fn helper_exchange(
        pipe_name: &str,
        parent_pid: u32,
        nonce: &str,
        policy: IdentityPolicy,
        synthetic_root: Option<&Path>,
    ) -> Result<(), ElevatedTransportError> {
        let helper = inspect_process(unsafe { GetCurrentProcessId() }, None)?;
        let release = match &policy {
            IdentityPolicy::Release => Some(load_release_manifest(&helper.image_path)?),
            #[cfg(test)]
            IdentityPolicy::SyntheticFixture { .. } => None,
        };
        verify_identity(&helper, ReleaseRole::Helper, &policy, release.as_ref())?;
        let parent = inspect_process(parent_pid, None)?;
        verify_identity(&parent, ReleaseRole::Parent, &policy, release.as_ref())?;
        if helper.session_id != parent.session_id || helper.user_sid != parent.user_sid {
            return Err(ElevatedTransportError::Identity(
                "pipe server is not in the helper user/session".to_string(),
            ));
        }

        let pipe = open_pipe_client(pipe_name)?;
        let mut server_pid = 0u32;
        if unsafe { GetNamedPipeServerProcessId(pipe.raw(), &mut server_pid) } == 0
            || server_pid != parent_pid
        {
            return Err(ElevatedTransportError::Identity(
                "named-pipe server PID does not match the command-line parent".to_string(),
            ));
        }

        let manifest_digest = release.as_ref().map_or([0u8; 32], |proof| proof.digest);
        sync_write_message(pipe.raw(), &hello_message(nonce, &manifest_digest)?)?;
        let key_bytes = sync_read_exact_message(pipe.raw(), KEY_MAGIC.len() + 96)?;
        let key = parse_key_message(&key_bytes, nonce, &manifest_digest)?;
        let (begin, begin_context): (StreamBegin, FrameContext) =
            helper_receive(pipe.raw(), &key, nonce, 1, None)?;
        // Lifetime is an admission condition for this authenticated one-shot
        // session. Reusing that instant for every chunk prevents a valid large
        // archive batch from expiring halfway through execution.
        let admitted_at_unix_seconds = now_unix_seconds()?;
        validate_stream_begin(
            &begin,
            &begin_context,
            &parent,
            nonce,
            admitted_at_unix_seconds,
        )?;
        match (&policy, begin.header.synthetic_test) {
            (IdentityPolicy::Release, false) => {}
            #[cfg(test)]
            (IdentityPolicy::SyntheticFixture { .. }, true) => {}
            _ => {
                return Err(ElevatedTransportError::Identity(
                    "request identity mode does not match the helper invocation".to_string(),
                ))
            }
        }
        let privilege_guard = match enable_object_backup_privileges() {
            Ok(guard) => guard,
            Err(message) => {
                let failure = StreamReply::Failure(ElevatedFailure {
                    schema: PROTOCOL_SCHEMA.to_string(),
                    nonce: begin.header.nonce.clone(),
                    operation_id: begin.header.operation_id,
                    code: "privilege_proof_failed".to_string(),
                    message: bounded_message(&message),
                });
                helper_send(pipe.raw(), &key, &begin_context, &failure)?;
                if unsafe { FlushFileBuffers(pipe.raw()) } == 0 {
                    return Err(last_io("cannot flush helper failure"));
                }
                return Ok(());
            }
        };
        let begin_ready = StreamReply::BeginReady {
            schema: STREAM_SCHEMA.to_string(),
            batch_blake3: begin.batch_blake3.clone(),
            helper_image_sha256: helper.image_sha256.clone(),
            privilege_bitmap: privilege_guard.proof().enabled_bitmap,
        };
        helper_send(pipe.raw(), &key, &begin_context, &begin_ready)?;

        let mut sequence = 3u64;
        let mut processed = 0usize;
        let mut processed_chunks = 0usize;
        loop {
            let (command, context): (StreamCommand, FrameContext) = helper_receive(
                pipe.raw(),
                &key,
                nonce,
                sequence,
                Some(&begin_context.operation_uuid),
            )?;
            sequence = sequence.checked_add(2).ok_or_else(|| {
                ElevatedTransportError::Protocol("stream sequence overflow".to_string())
            })?;
            match command {
                StreamCommand::Chunk(chunk) => {
                    let commitment = begin.chunks.get(processed_chunks).ok_or_else(|| {
                        ElevatedTransportError::Protocol(
                            "parent sent a capability chunk after the committed batch ended"
                                .to_string(),
                        )
                    })?;
                    validate_stream_chunk(&begin, commitment, processed_chunks, &chunk)?;
                    let fragment = begin.header.request_with(chunk.capabilities.clone());
                    fragment
                        .validate_capability_slice(
                            admitted_at_unix_seconds,
                            nonce,
                            parent_pid,
                            parent.session_id,
                            synthetic_root,
                            commitment.start_index,
                        )
                        .map_err(ElevatedTransportError::Protocol)?;
                    let returned =
                        execute_chunk(&fragment, &privilege_guard, commitment.start_index);
                    processed += returned.len();
                    let reply = StreamReply::ChunkResults {
                        schema: STREAM_SCHEMA.to_string(),
                        batch_blake3: begin.batch_blake3.clone(),
                        chunk_index: processed_chunks as u32,
                        start_index: commitment.start_index,
                        items: returned,
                    };
                    processed_chunks += 1;
                    helper_send(pipe.raw(), &key, &context, &reply)?;
                }
                StreamCommand::Stop(stop) => {
                    validate_stream_stop(&begin, &stop, processed, processed_chunks)?;
                    let reply = StreamReply::StopReady {
                        schema: STREAM_SCHEMA.to_string(),
                        batch_blake3: begin.batch_blake3.clone(),
                        processed_capabilities: processed as u32,
                        processed_chunks: processed_chunks as u32,
                    };
                    helper_send(pipe.raw(), &key, &context, &reply)?;
                    break;
                }
                StreamCommand::End(end) => {
                    if end.schema != STREAM_SCHEMA
                        || end.batch_blake3 != begin.batch_blake3
                        || end.total_capabilities != begin.total_capabilities
                        || end.chunk_count != begin.chunks.len() as u32
                        || processed != begin.total_capabilities as usize
                        || processed_chunks != begin.chunks.len()
                    {
                        return Err(ElevatedTransportError::Protocol(
                            "stream end does not close the committed batch".to_string(),
                        ));
                    }
                    let reply = StreamReply::EndReady {
                        schema: STREAM_SCHEMA.to_string(),
                        batch_blake3: begin.batch_blake3.clone(),
                        total_capabilities: begin.total_capabilities,
                        chunk_count: begin.chunks.len() as u32,
                    };
                    helper_send(pipe.raw(), &key, &context, &reply)?;
                    break;
                }
            }
        }

        if unsafe { FlushFileBuffers(pipe.raw()) } == 0 {
            return Err(last_io("cannot flush helper response"));
        }
        ensure_no_queued_message(pipe.raw(), "parent queued a second capability batch")?;
        drop(privilege_guard);
        Ok(())
    }

    fn validate_stream_begin(
        begin: &StreamBegin,
        context: &FrameContext,
        parent: &ProcessIdentity,
        expected_nonce: &str,
        now_unix_seconds: i64,
    ) -> Result<(), ElevatedTransportError> {
        if begin.schema != STREAM_SCHEMA
            || begin.operation_uuid != context.operation_uuid
            || begin.header.protocol_schema != PROTOCOL_SCHEMA
            || begin.header.nonce != expected_nonce
            || context.nonce != expected_nonce
            || begin.header.operation_id <= 0
            || begin.header.plan_fingerprint.len() != 67
            || !begin.header.plan_fingerprint.starts_with("v2:")
            || !begin.header.plan_fingerprint[3..]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || begin.header.issued_at_unix_seconds > now_unix_seconds
            || begin.header.expires_at_unix_seconds < now_unix_seconds
            || begin
                .header
                .expires_at_unix_seconds
                .checked_sub(begin.header.issued_at_unix_seconds)
                .is_none_or(|lifetime| !(0..=MAX_CAPABILITY_LIFETIME_SECONDS).contains(&lifetime))
            || begin.total_capabilities == 0
            || begin.total_capabilities as usize > MAX_CAPABILITIES_PER_INVOCATION
            || begin.chunks.is_empty()
            || begin.chunks.len() > begin.total_capabilities as usize
        {
            return Err(ElevatedTransportError::Protocol(
                "stream begin has invalid schema or bounds".to_string(),
            ));
        }
        require_hex(&begin.header.nonce, 64, "stream nonce")?;
        require_hex(&begin.operation_uuid, 32, "stream operation UUID")?;
        require_hex(
            &begin.header.journal_capability_blake3,
            64,
            "journal capability digest",
        )?;
        require_parent_binding(&begin.header.parent, parent)?;
        let mut next = 0u32;
        for chunk in &begin.chunks {
            if chunk.start_index != next
                || chunk.item_count == 0
                || chunk.item_count as usize > MAX_CHUNK_CAPABILITIES
            {
                return Err(ElevatedTransportError::Protocol(
                    "stream commitments are not a contiguous bounded sequence".to_string(),
                ));
            }
            require_hex(&chunk.capability_blake3, 64, "capability chunk digest")?;
            next = next.checked_add(chunk.item_count).ok_or_else(|| {
                ElevatedTransportError::Protocol("stream item count overflow".to_string())
            })?;
        }
        if next != begin.total_capabilities
            || stream_batch_digest(
                &begin.operation_uuid,
                &begin.header,
                begin.total_capabilities as usize,
                &begin.chunks,
            )? != begin.batch_blake3
        {
            return Err(ElevatedTransportError::Protocol(
                "stream batch commitment is invalid".to_string(),
            ));
        }
        Ok(())
    }

    fn validate_stream_chunk(
        begin: &StreamBegin,
        commitment: &ChunkCommitment,
        chunk_index: usize,
        chunk: &StreamChunk,
    ) -> Result<(), ElevatedTransportError> {
        if chunk.schema != STREAM_SCHEMA
            || chunk.batch_blake3 != begin.batch_blake3
            || chunk.chunk_index != chunk_index as u32
            || chunk.start_index != commitment.start_index
            || chunk.capabilities.len() != commitment.item_count as usize
            || capability_chunk_digest(chunk.start_index as usize, &chunk.capabilities)?
                != commitment.capability_blake3
        {
            return Err(ElevatedTransportError::Protocol(
                "capability chunk does not match its precommitted digest".to_string(),
            ));
        }
        Ok(())
    }

    fn validate_stream_stop(
        begin: &StreamBegin,
        stop: &StreamStop,
        processed_capabilities: usize,
        processed_chunks: usize,
    ) -> Result<(), ElevatedTransportError> {
        let committed_prefix = begin
            .chunks
            .get(processed_chunks.wrapping_sub(1))
            .map(|chunk| chunk.start_index.saturating_add(chunk.item_count))
            .unwrap_or(0);
        if stop.schema != STREAM_SCHEMA
            || stop.batch_blake3 != begin.batch_blake3
            || stop.processed_capabilities as usize != processed_capabilities
            || stop.processed_chunks as usize != processed_chunks
            || processed_chunks > begin.chunks.len()
            || committed_prefix as usize != processed_capabilities
        {
            return Err(ElevatedTransportError::Protocol(
                "stream stop does not match the authenticated processed prefix".to_string(),
            ));
        }
        Ok(())
    }

    fn execute_chunk(
        request: &ElevatedRequest,
        privilege_guard: &PrivilegeGuard,
        start_index: u32,
    ) -> Vec<ElevatedItemResult> {
        let mut items = Vec::with_capacity(request.capabilities.len());
        for (offset, capability) in request.capabilities.iter().enumerate() {
            let index = start_index + offset as u32;
            let result = execute_capability(request, privilege_guard, capability, index);
            items.push(match result {
                Ok(value) => ElevatedItemResult::Ready(Box::new(value)),
                Err((code, message)) => ElevatedItemResult::Blocked {
                    capability_index: index,
                    code,
                    message: bounded_message(&message),
                },
            });
        }
        items
    }

    fn execute_capability(
        request: &ElevatedRequest,
        privilege_guard: &PrivilegeGuard,
        capability: &ElevatedCapability,
        index: u32,
    ) -> Result<ElevatedObjectResult, (String, String)> {
        let proof = match capability {
            ElevatedCapability::ObjectBackupV2 {
                source,
                parent_archive_handle_value,
                archive_path,
                scratch_root,
                scratch_leaf,
            } => finalize_object_archive_v2(
                privilege_guard,
                FinalizeObjectArchiveParams {
                    parent_pid: request.parent.pid,
                    source_parent_handle: source.parent_handle_value,
                    archive_parent_handle: *parent_archive_handle_value,
                    scratch_root_parent_handle: scratch_root.parent_handle_value,
                    expected_archive_path: archive_path,
                    source_path: &source.path,
                    scratch_leaf,
                    nonce: &request.nonce,
                    expected_stamp: &source.stamp,
                    expected_content_hash: &source.content_blake3,
                    expected_scratch_root_stamp: &scratch_root.stamp,
                },
            ),
            ElevatedCapability::RoundtripVerify {
                source,
                parent_archive_handle_value,
                archive_path,
                expected_archive_stamp,
                expected_archive_blake3,
                scratch_root,
                scratch_leaf,
            } => {
                let semantic = source.semantic_blake3.as_deref().ok_or_else(|| {
                    (
                        "semantic_proof_missing".to_string(),
                        "roundtrip capability has no semantic digest".to_string(),
                    )
                })?;
                verify_object_archive_v2(
                    privilege_guard,
                    VerifyObjectArchiveParams {
                        parent_pid: request.parent.pid,
                        source_parent_handle: source.parent_handle_value,
                        archive_parent_handle: *parent_archive_handle_value,
                        scratch_root_parent_handle: scratch_root.parent_handle_value,
                        expected_archive_path: archive_path,
                        source_path: &source.path,
                        scratch_leaf,
                        expected_source_stamp: &source.stamp,
                        expected_content_hash: &source.content_blake3,
                        expected_archive_stamp,
                        expected_archive_hash: expected_archive_blake3,
                        expected_semantic: semantic,
                        expected_scratch_root_stamp: &scratch_root.stamp,
                        allow_internal_directory_time_drift: source
                            .allow_internal_directory_time_drift,
                    },
                )
            }
        }
        .map_err(|error| {
            let code = if matches!(error, crate::ObjectArchiveError::ScratchCleanup(_)) {
                "scratchCleanupPending"
            } else {
                "object_archive_v2_blocked"
            };
            (code.to_string(), error.to_string())
        })?;
        Ok(ElevatedObjectResult {
            capability_index: index,
            proof_schema: proof.schema,
            source_before: proof.source_stamp.clone(),
            source_after: proof.source_stamp,
            archive_stamp: Some(proof.archive_stamp),
            archive_blake3: Some(proof.archive_blake3),
            raw_backup_blake3: Some(proof.raw_backup_blake3),
            semantic_blake3: Some(proof.semantic_blake3),
            roundtrip_blake3: Some(proof.roundtrip_blake3),
            stream_count: Some(proof.stream_count),
            security_stream_present: proof.security_stream_present,
            cleanup_complete: proof.cleanup_complete,
        })
    }

    fn validate_response(
        response: &ElevatedResponse,
        request: &ElevatedRequest,
        helper_hash: &str,
        expected_success_items: usize,
    ) -> Result<(), ElevatedTransportError> {
        match response {
            ElevatedResponse::Success(success) => {
                if success.schema != PROTOCOL_SCHEMA
                    || success.nonce != request.nonce
                    || success.operation_id != request.operation_id
                    || success.helper_image_sha256 != helper_hash
                    || success.privilege_bitmap != REQUIRED_OBJECT_PRIVILEGES
                    || success.items.len() != expected_success_items
                {
                    return Err(ElevatedTransportError::Protocol(
                        "helper success response is not bound to this invocation".to_string(),
                    ));
                }
                for (index, item) in success.items.iter().enumerate() {
                    let item_index = match item {
                        ElevatedItemResult::Ready(value) => {
                            validate_ready_result(value)?;
                            value.capability_index
                        }
                        ElevatedItemResult::Blocked {
                            capability_index, ..
                        } => *capability_index,
                    };
                    if item_index != index as u32 {
                        return Err(ElevatedTransportError::Protocol(
                            "helper response reordered or duplicated a capability result"
                                .to_string(),
                        ));
                    }
                }
            }
            ElevatedResponse::Failure(failure) => {
                if failure.schema != PROTOCOL_SCHEMA
                    || failure.nonce != request.nonce
                    || failure.operation_id != request.operation_id
                    || failure.code.is_empty()
                {
                    return Err(ElevatedTransportError::Protocol(
                        "helper failure response is not bound to this invocation".to_string(),
                    ));
                }
            }
        }
        Ok(())
    }

    fn validate_ready_result(value: &ElevatedObjectResult) -> Result<(), ElevatedTransportError> {
        let archive_stamp = value.archive_stamp.as_ref().ok_or_else(|| {
            ElevatedTransportError::Protocol("ready proof has no archive identity".to_string())
        })?;
        let archive = value.archive_blake3.as_deref().ok_or_else(|| {
            ElevatedTransportError::Protocol("ready proof has no archive digest".to_string())
        })?;
        let raw = value.raw_backup_blake3.as_deref().ok_or_else(|| {
            ElevatedTransportError::Protocol("ready proof has no raw-stream digest".to_string())
        })?;
        let semantic = value.semantic_blake3.as_deref().ok_or_else(|| {
            ElevatedTransportError::Protocol("ready proof has no semantic digest".to_string())
        })?;
        let roundtrip = value.roundtrip_blake3.as_deref().ok_or_else(|| {
            ElevatedTransportError::Protocol("ready proof has no roundtrip digest".to_string())
        })?;
        require_hex(archive, 64, "ready archive digest")?;
        require_hex(raw, 64, "ready raw-stream digest")?;
        require_hex(semantic, 64, "ready semantic digest")?;
        require_hex(roundtrip, 64, "ready roundtrip digest")?;
        if archive_stamp.volume_id.is_empty()
            || archive_stamp.file_id.is_empty()
            || archive_stamp.bytes == 0
            || archive_stamp.modified_unix_seconds.is_none()
            || value.proof_schema != "object_archive/2"
            || semantic != roundtrip
            || value.stream_count.is_none_or(|count| count == 0)
            || !value.security_stream_present
            || !value.cleanup_complete
        {
            return Err(ElevatedTransportError::Protocol(
                "helper labelled an incomplete object proof ready".to_string(),
            ));
        }
        Ok(())
    }

    fn launch_helper(
        helper_path: &Path,
        pipe_name: &str,
        parent_pid: u32,
        nonce: &str,
    ) -> Result<ChildGuard, ElevatedTransportError> {
        let verb = wide("runas");
        let executable = wide_os(helper_path.as_os_str());
        let parameters = wide(&format!(
            "--pipe {pipe_name} --parent-pid {parent_pid} --nonce {nonce}"
        ));
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS,
            lpVerb: verb.as_ptr(),
            lpFile: executable.as_ptr(),
            lpParameters: parameters.as_ptr(),
            nShow: SW_HIDE,
            ..Default::default()
        };
        if unsafe { ShellExecuteExW(&mut info) } == 0 {
            return Err(ElevatedTransportError::Launch(format!(
                "UAC helper launch was refused or failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        ChildGuard::new(info.hProcess)
    }

    fn open_pipe_client(pipe_name: &str) -> Result<OwnedHandle, ElevatedTransportError> {
        let name = wide(pipe_name);
        if unsafe { WaitNamedPipeW(name.as_ptr(), CONNECT_TIMEOUT_MS) } == 0 {
            return Err(last_io("one-shot parent pipe did not become available"));
        }
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        let handle = OwnedHandle::new(handle, "cannot open the one-shot parent pipe")?;
        let mode = PIPE_READMODE_MESSAGE;
        if unsafe {
            SetNamedPipeHandleState(handle.raw(), &mode, std::ptr::null(), std::ptr::null())
        } == 0
        {
            return Err(last_io("cannot select named-pipe message mode"));
        }
        Ok(handle)
    }

    fn inspect_process(
        pid: u32,
        borrowed_handle: Option<HANDLE>,
    ) -> Result<ProcessIdentity, ElevatedTransportError> {
        if pid == 0 {
            return Err(ElevatedTransportError::Identity(
                "process identity has a zero PID".to_string(),
            ));
        }
        let owned = if borrowed_handle.is_none() {
            Some(OwnedHandle::new(
                // Retaining SYNCHRONIZE alongside the identity rights lets the
                // disposition guardian prove that this exact process object has
                // terminated before it closes a pre-ProveArmed target handle.
                unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid) },
                "cannot open peer process for identity proof",
            )?)
        } else {
            None
        };
        let process = borrowed_handle.unwrap_or_else(|| owned.as_ref().unwrap().raw());
        if unsafe { GetProcessId(process) } != pid {
            return Err(ElevatedTransportError::Identity(
                "peer process handle PID mismatch".to_string(),
            ));
        }

        let mut image = vec![0u16; 32_768];
        let mut image_len = image.len() as u32;
        if unsafe { QueryFullProcessImageNameW(process, 0, image.as_mut_ptr(), &mut image_len) }
            == 0
            || image_len == 0
            || image_len as usize >= image.len()
        {
            return Err(last_io("cannot query peer process image"));
        }
        image.truncate(image_len as usize);
        let image_path = PathBuf::from(OsString::from_wide(&image));
        crate::bound_fs::validate_local_mutation_path(&image_path)
            .map_err(|error| ElevatedTransportError::Identity(error.to_string()))?;
        let image_file = open_image_file(&image_path)?;
        let image_sha256 = hash_sha256(&image_file)?;

        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        if unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) }
            == 0
        {
            return Err(last_io("cannot query peer process start time"));
        }
        let started_100ns =
            ((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64;
        let mut session_id = 0u32;
        if unsafe { ProcessIdToSessionId(pid, &mut session_id) } == 0 {
            return Err(last_io("cannot query peer process session"));
        }
        let user_sid = process_user_sid(process)?;
        Ok(ProcessIdentity {
            _process: owned,
            _image_file: image_file,
            pid,
            session_id,
            started_100ns,
            image_path,
            image_sha256,
            user_sid,
        })
    }

    pub(super) fn exact_guardian_liveness(
        pid: u32,
        process_started_100ns: u64,
        image_sha256: &str,
    ) -> Result<ExactGuardianLiveness, ElevatedTransportError> {
        if pid == 0 || process_started_100ns == 0 {
            return Err(ElevatedTransportError::Identity(
                "durable guardian identity has a zero PID/start time".to_string(),
            ));
        }
        require_hex(image_sha256, 64, "durable guardian image SHA-256")?;

        let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid) };
        if raw.is_null() {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error().map(|code| code as u32) == Some(ERROR_INVALID_PARAMETER) {
                // Windows reports ERROR_INVALID_PARAMETER when the PID no longer
                // names a process. Since PID/start/image were all durably bound,
                // this proves that exact guardian has terminated.
                return Ok(ExactGuardianLiveness::Terminated);
            }
            return Err(ElevatedTransportError::Io(std::io::Error::new(
                error.kind(),
                format!("cannot prove exact guardian process liveness: {error}"),
            )));
        }
        let process = OwnedHandle::new(raw, "cannot retain exact guardian process handle")?;
        match unsafe { WaitForSingleObject(process.raw(), 0) } {
            WAIT_OBJECT_0 => return Ok(ExactGuardianLiveness::Terminated),
            WAIT_TIMEOUT => {}
            WAIT_FAILED => {
                return Err(last_io("cannot query exact guardian process liveness"));
            }
            status => {
                return Err(ElevatedTransportError::Identity(format!(
                    "exact guardian process wait returned unexpected status {status}"
                )));
            }
        }

        let identity = inspect_process(pid, Some(process.raw()))?;
        if identity.started_100ns != process_started_100ns {
            // A live process with the same PID but a different creation time is
            // PID reuse, which proves the durably identified guardian exited.
            return Ok(ExactGuardianLiveness::Terminated);
        }
        if identity.image_sha256 != image_sha256 {
            return Err(ElevatedTransportError::Identity(
                "live guardian PID/start identity has a different image hash".to_string(),
            ));
        }
        Ok(ExactGuardianLiveness::Alive)
    }

    fn verify_identity(
        identity: &ProcessIdentity,
        role: ReleaseRole,
        policy: &IdentityPolicy,
        release: Option<&ReleaseManifestProof>,
    ) -> Result<(), ElevatedTransportError> {
        match policy {
            IdentityPolicy::Release => verify_release_identity(
                &identity.image_path,
                &identity.image_sha256,
                role,
                release.ok_or_else(|| {
                    ElevatedTransportError::Identity(
                        "release manifest proof is missing".to_string(),
                    )
                })?,
            ),
            #[cfg(test)]
            IdentityPolicy::SyntheticFixture { expected_image } => {
                let expected = open_image_file(expected_image)?;
                let expected_stamp = crate::bound_fs::platform_file_stamp(&expected)
                    .map_err(ElevatedTransportError::Io)?;
                let actual_stamp = crate::bound_fs::platform_file_stamp(&identity._image_file)
                    .map_err(ElevatedTransportError::Io)?;
                if !expected_stamp.same_object(&actual_stamp)
                    || hash_sha256(&expected)? != identity.image_sha256
                {
                    return Err(ElevatedTransportError::Identity(
                        "synthetic peer is not the explicitly selected test image".to_string(),
                    ));
                }
                Ok(())
            }
        }
    }

    fn load_release_manifest(
        anchor_image: &Path,
    ) -> Result<ReleaseManifestProof, ElevatedTransportError> {
        let directory = anchor_image.parent().ok_or_else(|| {
            ElevatedTransportError::Identity(
                "release image has no installation directory".to_string(),
            )
        })?;
        load_release_manifest_from_directory(directory)
    }

    fn load_release_manifest_from_directory(
        directory: &Path,
    ) -> Result<ReleaseManifestProof, ElevatedTransportError> {
        crate::bound_fs::validate_local_mutation_path(directory)
            .map_err(|error| ElevatedTransportError::Identity(error.to_string()))?;
        let manifest_path = directory.join(RELEASE_MANIFEST_FILE_NAME);
        crate::bound_fs::validate_local_mutation_path(&manifest_path)
            .map_err(|error| ElevatedTransportError::Identity(error.to_string()))?;
        let file = OpenOptions::new()
            .read(true)
            .access_mode(GENERIC_READ)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_OPEN_NO_RECALL)
            .open(&manifest_path)
            .map_err(|error| {
                ElevatedTransportError::Identity(format!(
                    "{}: cannot open {}: {error}",
                    RELEASE_IDENTITY_REQUIREMENT, RELEASE_MANIFEST_FILE_NAME
                ))
            })?;
        let length = file.metadata()?.len();
        if length == 0 || length > 64 * 1024 {
            return Err(ElevatedTransportError::Identity(
                "release manifest has invalid bounds".to_string(),
            ));
        }
        let mut reader = file.try_clone()?;
        reader.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::with_capacity(length as usize);
        reader.read_to_end(&mut bytes)?;
        if bytes.len() != length as usize {
            return Err(ElevatedTransportError::Identity(
                "release manifest changed while it was read".to_string(),
            ));
        }
        let manifest: ReleaseManifest = serde_json::from_slice(&bytes).map_err(|error| {
            ElevatedTransportError::Identity(format!("release manifest JSON is invalid: {error}"))
        })?;
        validate_manifest_fields(&manifest)?;
        let public_blob_hex = RELEASE_ROOT_RSA_PUBLIC_BLOB_HEX.ok_or_else(|| {
            ElevatedTransportError::Identity(format!(
                "{RELEASE_IDENTITY_REQUIREMENT}: build has no release-root public key"
            ))
        })?;
        let public_blob = decode_hex_vec(public_blob_hex, "release-root RSA public blob")?;
        verify_manifest_signature(&manifest, &public_blob)?;
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        Ok(ReleaseManifestProof {
            manifest,
            digest,
            directory: directory.to_path_buf(),
            _file: file,
        })
    }

    pub(super) fn verify_release_installation(
        directory: &Path,
    ) -> Result<ReleaseInstallationVerification, ElevatedTransportError> {
        let release = load_release_manifest_from_directory(directory)?;
        let parent_path = release.directory.join(&release.manifest.parent.file_name);
        let helper_path = release.directory.join(&release.manifest.helper.file_name);
        let parent_file = open_image_file(&parent_path)?;
        let helper_file = open_image_file(&helper_path)?;
        let parent_stamp = crate::bound_fs::platform_file_stamp(&parent_file)
            .map_err(ElevatedTransportError::Io)?;
        let helper_stamp = crate::bound_fs::platform_file_stamp(&helper_file)
            .map_err(ElevatedTransportError::Io)?;
        if parent_stamp.same_object(&helper_stamp) {
            return Err(ElevatedTransportError::Identity(
                "release manifest parent and helper resolve to the same filesystem object"
                    .to_string(),
            ));
        }

        // Both files remain open without FILE_SHARE_WRITE/FILE_SHARE_DELETE until
        // their exact post-sign bytes and offline Authenticode chains are proved.
        let parent_sha256 = hash_sha256(&parent_file)?;
        let helper_sha256 = hash_sha256(&helper_file)?;
        verify_release_identity(&parent_path, &parent_sha256, ReleaseRole::Parent, &release)?;
        verify_release_identity(&helper_path, &helper_sha256, ReleaseRole::Helper, &release)?;

        Ok(ReleaseInstallationVerification {
            release_id: release.manifest.release_id.clone(),
            manifest_sha256: hex(&release.digest),
            parent_sha256,
            helper_sha256,
        })
    }

    fn validate_manifest_fields(manifest: &ReleaseManifest) -> Result<(), ElevatedTransportError> {
        if manifest.schema != RELEASE_MANIFEST_SCHEMA {
            return Err(ElevatedTransportError::Identity(
                "release manifest schema is unsupported".to_string(),
            ));
        }
        require_hex(&manifest.release_id, 64, "release id")?;
        for (label, image) in [("parent", &manifest.parent), ("helper", &manifest.helper)] {
            if image.file_name.is_empty()
                || image.file_name.len() > 128
                || !image
                    .file_name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
                || image.file_name == "."
                || image.file_name == ".."
            {
                return Err(ElevatedTransportError::Identity(format!(
                    "release manifest {label} file name is invalid"
                )));
            }
            require_hex(&image.sha256, 64, "release image SHA-256")?;
        }
        if manifest
            .parent
            .file_name
            .eq_ignore_ascii_case(&manifest.helper.file_name)
        {
            return Err(ElevatedTransportError::Identity(
                "release manifest aliases parent and helper images".to_string(),
            ));
        }
        let signature = decode_hex_vec(
            &manifest.signature_rsa_pss_sha256,
            "release manifest signature",
        )?;
        if signature.len() < 384 || signature.len() > 1024 {
            return Err(ElevatedTransportError::Identity(
                "release manifest signature has invalid bounds".to_string(),
            ));
        }
        Ok(())
    }

    fn manifest_signing_payload(manifest: &ReleaseManifest) -> Vec<u8> {
        // The signed payload is deliberately independent of JSON whitespace and
        // key order. Every variable field is ASCII-constrained above.
        format!(
            "schema={}\nrelease_id={}\nparent_file={}\nparent_sha256={}\nhelper_file={}\nhelper_sha256={}\n",
            manifest.schema,
            manifest.release_id,
            manifest.parent.file_name,
            manifest.parent.sha256,
            manifest.helper.file_name,
            manifest.helper.sha256,
        )
        .into_bytes()
    }

    fn verify_manifest_signature(
        manifest: &ReleaseManifest,
        public_blob: &[u8],
    ) -> Result<(), ElevatedTransportError> {
        let modulus_bytes = validate_rsa_public_blob(public_blob)?;
        let signature = decode_hex_vec(
            &manifest.signature_rsa_pss_sha256,
            "release manifest signature",
        )?;
        if signature.len() != modulus_bytes {
            return Err(ElevatedTransportError::Identity(
                "release manifest signature size does not match its trust root".to_string(),
            ));
        }
        let digest: [u8; 32] = Sha256::digest(manifest_signing_payload(manifest)).into();
        let mut algorithm: BCRYPT_ALG_HANDLE = std::ptr::null_mut();
        let opened = unsafe {
            BCryptOpenAlgorithmProvider(&mut algorithm, BCRYPT_RSA_ALGORITHM, std::ptr::null(), 0)
        };
        if opened != 0 || algorithm.is_null() {
            return Err(ElevatedTransportError::Identity(format!(
                "cannot open RSA release-root provider: 0x{:08x}",
                opened as u32
            )));
        }
        struct Algorithm(BCRYPT_ALG_HANDLE);
        impl Drop for Algorithm {
            fn drop(&mut self) {
                unsafe {
                    BCryptCloseAlgorithmProvider(self.0, 0);
                }
            }
        }
        let algorithm = Algorithm(algorithm);
        let mut key: BCRYPT_KEY_HANDLE = std::ptr::null_mut();
        let imported = unsafe {
            BCryptImportKeyPair(
                algorithm.0,
                std::ptr::null_mut(),
                BCRYPT_RSAPUBLIC_BLOB,
                &mut key,
                public_blob.as_ptr(),
                public_blob.len() as u32,
                0,
            )
        };
        if imported != 0 || key.is_null() {
            return Err(ElevatedTransportError::Identity(format!(
                "cannot import release-root public key: 0x{:08x}",
                imported as u32
            )));
        }
        struct Key(BCRYPT_KEY_HANDLE);
        impl Drop for Key {
            fn drop(&mut self) {
                unsafe {
                    BCryptDestroyKey(self.0);
                }
            }
        }
        let key = Key(key);
        let padding = BCRYPT_PSS_PADDING_INFO {
            pszAlgId: BCRYPT_SHA256_ALGORITHM,
            cbSalt: 32,
        };
        let verified = unsafe {
            BCryptVerifySignature(
                key.0,
                (&padding as *const BCRYPT_PSS_PADDING_INFO).cast(),
                digest.as_ptr(),
                digest.len() as u32,
                signature.as_ptr(),
                signature.len() as u32,
                BCRYPT_PAD_PSS,
            )
        };
        if verified != 0 {
            return Err(ElevatedTransportError::Identity(format!(
                "release manifest RSA-PSS signature is invalid: 0x{:08x}",
                verified as u32
            )));
        }
        Ok(())
    }

    fn validate_rsa_public_blob(blob: &[u8]) -> Result<usize, ElevatedTransportError> {
        // BCRYPT_RSAKEY_BLOB is six little-endian u32 values followed by the
        // big-endian public exponent and modulus.
        if blob.len() < 24 {
            return Err(ElevatedTransportError::Identity(
                "release-root RSA blob is truncated".to_string(),
            ));
        }
        let field = |offset: usize| {
            u32::from_le_bytes(blob[offset..offset + 4].try_into().expect("bounded field"))
        };
        let magic = field(0);
        let bits = field(4) as usize;
        let exponent_bytes = field(8) as usize;
        let modulus_bytes = field(12) as usize;
        let prime1 = field(16);
        let prime2 = field(20);
        let total = 24usize
            .checked_add(exponent_bytes)
            .and_then(|value| value.checked_add(modulus_bytes));
        if magic != BCRYPT_RSAPUBLIC_MAGIC
            || !(3072..=8192).contains(&bits)
            || !bits.is_multiple_of(8)
            || modulus_bytes != bits / 8
            || !(3..=8).contains(&exponent_bytes)
            || prime1 != 0
            || prime2 != 0
            || total != Some(blob.len())
        {
            return Err(ElevatedTransportError::Identity(
                "release-root RSA public blob has invalid structure".to_string(),
            ));
        }
        let exponent = &blob[24..24 + exponent_bytes];
        if exponent[0] == 0 || exponent[exponent.len() - 1] & 1 == 0 {
            return Err(ElevatedTransportError::Identity(
                "release-root RSA exponent is invalid".to_string(),
            ));
        }
        let modulus = &blob[24 + exponent_bytes..];
        if modulus[0] & 0x80 == 0 || modulus[modulus.len() - 1] & 1 == 0 {
            return Err(ElevatedTransportError::Identity(
                "release-root RSA modulus is not a full-width odd integer".to_string(),
            ));
        }
        Ok(modulus_bytes)
    }

    fn verify_release_identity(
        path: &Path,
        actual_sha256: &str,
        role: ReleaseRole,
        release: &ReleaseManifestProof,
    ) -> Result<(), ElevatedTransportError> {
        let entry = release.image(role);
        let file_name = path.file_name().and_then(OsStr::to_str).ok_or_else(|| {
            ElevatedTransportError::Identity(format!(
                "{} release image has no valid file name",
                role.label()
            ))
        })?;
        let directory = path.parent().ok_or_else(|| {
            ElevatedTransportError::Identity(format!(
                "{} release image has no installation directory",
                role.label()
            ))
        })?;
        if !file_name.eq_ignore_ascii_case(&entry.file_name)
            || normalize_windows_path(directory) != normalize_windows_path(&release.directory)
        {
            return Err(ElevatedTransportError::Identity(format!(
                "{} image is not the manifest-selected installation object",
                role.label()
            )));
        }
        if !actual_sha256.eq_ignore_ascii_case(&entry.sha256) {
            return Err(ElevatedTransportError::Identity(format!(
                "{} image does not match the signed release manifest",
                role.label()
            )));
        }
        verify_authenticode_offline(path).map_err(|message| {
            ElevatedTransportError::Identity(format!(
                "{} Authenticode proof failed: {message}",
                role.label()
            ))
        })
    }

    fn normalize_windows_path(path: &Path) -> String {
        let raw = path.as_os_str().to_string_lossy().into_owned();
        raw.strip_prefix(r"\\?\")
            .unwrap_or(&raw)
            .trim_end_matches(['\\', '/'])
            .replace('/', "\\")
            .to_ascii_lowercase()
    }

    fn verify_authenticode_offline(path: &Path) -> Result<(), String> {
        let path_wide = wide_os(path.as_os_str());
        let mut file_info = WINTRUST_FILE_INFO {
            cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
            pcwszFilePath: path_wide.as_ptr(),
            ..Default::default()
        };
        let mut data = WINTRUST_DATA {
            cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
            dwUIChoice: WTD_UI_NONE,
            fdwRevocationChecks: WTD_REVOKE_NONE,
            dwUnionChoice: WTD_CHOICE_FILE,
            Anonymous: WINTRUST_DATA_0 {
                pFile: &mut file_info,
            },
            dwStateAction: WTD_STATEACTION_VERIFY,
            // No URL retrieval is permitted by Code Hangar's local-only invariant.
            dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL,
            ..Default::default()
        };
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        let status = unsafe {
            WinVerifyTrust(
                std::ptr::null_mut(),
                &mut action,
                (&mut data as *mut WINTRUST_DATA).cast(),
            )
        };
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        unsafe {
            WinVerifyTrust(
                std::ptr::null_mut(),
                &mut action,
                (&mut data as *mut WINTRUST_DATA).cast(),
            );
        }
        if status == 0 {
            Ok(())
        } else {
            Err(format!("WinVerifyTrust returned 0x{:08x}", status as u32))
        }
    }

    fn require_parent_binding(
        binding: &ParentBinding,
        identity: &ProcessIdentity,
    ) -> Result<(), ElevatedTransportError> {
        if binding.pid != identity.pid
            || binding.session_id != identity.session_id
            || binding.process_started_100ns != identity.started_100ns
            || !binding
                .image_sha256
                .eq_ignore_ascii_case(&identity.image_sha256)
        {
            return Err(ElevatedTransportError::Identity(
                "request parent binding does not match the live pipe server".to_string(),
            ));
        }
        Ok(())
    }

    fn open_image_file(path: &Path) -> Result<File, ElevatedTransportError> {
        crate::bound_fs::validate_local_mutation_path(path)
            .map_err(|error| ElevatedTransportError::Identity(error.to_string()))?;
        OpenOptions::new()
            .read(true)
            .access_mode(GENERIC_READ)
            // Deliberately deny write/delete sharing while the identity is live.
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_OPEN_NO_RECALL)
            .open(path)
            .map_err(ElevatedTransportError::Io)
    }

    fn hash_sha256(file: &File) -> Result<String, ElevatedTransportError> {
        let mut reader = file.try_clone()?;
        reader.seek(SeekFrom::Start(0))?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        Ok(hex(&hasher.finalize()))
    }

    fn current_user_sid() -> Result<String, ElevatedTransportError> {
        let process =
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, GetCurrentProcessId()) };
        let process = OwnedHandle::new(process, "cannot open current process")?;
        process_user_sid(process.raw())
    }

    fn process_user_sid(process: HANDLE) -> Result<String, ElevatedTransportError> {
        let mut token = std::ptr::null_mut();
        if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
            return Err(last_io("cannot open peer token for SID proof"));
        }
        let token = OwnedHandle::new(token, "cannot retain peer token")?;
        let mut needed = 0u32;
        unsafe {
            GetTokenInformation(token.raw(), TokenUser, std::ptr::null_mut(), 0, &mut needed);
        }
        if needed < std::mem::size_of::<TOKEN_USER>() as u32 || needed > 64 * 1024 {
            return Err(ElevatedTransportError::Identity(
                "peer TokenUser result has invalid bounds".to_string(),
            ));
        }
        let words = (needed as usize).div_ceil(std::mem::size_of::<usize>());
        let mut storage = vec![0usize; words];
        if unsafe {
            GetTokenInformation(
                token.raw(),
                TokenUser,
                storage.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        } == 0
        {
            return Err(last_io("cannot query peer TokenUser"));
        }
        let token_user = unsafe { &*(storage.as_ptr().cast::<TOKEN_USER>()) };
        if token_user.User.Sid.is_null() {
            return Err(ElevatedTransportError::Identity(
                "peer user SID is missing".to_string(),
            ));
        }
        let mut sid_text = std::ptr::null_mut();
        if unsafe { ConvertSidToStringSidW(token_user.User.Sid, &mut sid_text) } == 0
            || sid_text.is_null()
        {
            return Err(last_io("cannot serialize peer user SID"));
        }
        let allocation = LocalAllocation(sid_text.cast());
        let mut len = 0usize;
        while len < 184 && unsafe { *sid_text.add(len) } != 0 {
            len += 1;
        }
        if len == 0 || len == 184 {
            return Err(ElevatedTransportError::Identity(
                "peer user SID string has invalid bounds".to_string(),
            ));
        }
        let result = OsString::from_wide(unsafe { std::slice::from_raw_parts(sid_text, len) })
            .into_string()
            .map_err(|_| {
                ElevatedTransportError::Identity("peer user SID is not Unicode".to_string())
            });
        drop(allocation);
        result
    }

    fn random_key() -> Result<[u8; 32], ElevatedTransportError> {
        let mut key = [0u8; 32];
        let status = unsafe {
            BCryptGenRandom(
                std::ptr::null_mut(),
                key.as_mut_ptr(),
                key.len() as u32,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        };
        if status != 0 {
            return Err(ElevatedTransportError::Protocol(format!(
                "Windows CSPRNG failed with status 0x{:08x}",
                status as u32
            )));
        }
        Ok(key)
    }

    fn guardian_receipt_entropy(
        context: &GuardianReceiptKeyContext,
    ) -> Result<[u8; 32], ElevatedTransportError> {
        let canonical = serde_json::to_vec(context).map_err(|error| {
            ElevatedTransportError::Protocol(format!(
                "cannot canonicalize guardian receipt key context: {error}"
            ))
        })?;
        Ok(*blake3::hash(&canonical).as_bytes())
    }

    fn protect_receipt_key(
        key: &GuardianReceiptKey,
        context: &GuardianReceiptKeyContext,
    ) -> Result<String, ElevatedTransportError> {
        let entropy = guardian_receipt_entropy(context)?;
        let input = CRYPT_INTEGER_BLOB {
            cbData: key.0.len() as u32,
            pbData: key.0.as_ptr() as *mut u8,
        };
        let entropy_blob = CRYPT_INTEGER_BLOB {
            cbData: entropy.len() as u32,
            pbData: entropy.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB::default();
        if unsafe {
            CryptProtectData(
                &input,
                std::ptr::null(),
                &entropy_blob,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } == 0
        {
            return Err(last_io("cannot DPAPI-protect guardian receipt key"));
        }
        if output.pbData.is_null() || !(32..=4096).contains(&(output.cbData as usize)) {
            if !output.pbData.is_null() {
                unsafe { LocalFree(output.pbData.cast()) };
            }
            return Err(ElevatedTransportError::Protocol(
                "DPAPI guardian receipt key ciphertext has invalid bounds".to_string(),
            ));
        }
        let protected =
            unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
        unsafe { LocalFree(output.pbData.cast()) };
        Ok(hex(&protected))
    }

    fn unprotect_receipt_key(
        protected_key_hex: &str,
        context: &GuardianReceiptKeyContext,
    ) -> Result<GuardianReceiptKey, ElevatedTransportError> {
        if protected_key_hex.len() > 8192 {
            return Err(ElevatedTransportError::Protocol(
                "protected guardian receipt key exceeds its bound".to_string(),
            ));
        }
        let protected = decode_hex_vec(protected_key_hex, "protected guardian receipt key")?;
        if !(32..=4096).contains(&protected.len()) {
            return Err(ElevatedTransportError::Protocol(
                "protected guardian receipt key has invalid bounds".to_string(),
            ));
        }
        let entropy = guardian_receipt_entropy(context)?;
        let input = CRYPT_INTEGER_BLOB {
            cbData: protected.len() as u32,
            pbData: protected.as_ptr() as *mut u8,
        };
        let entropy_blob = CRYPT_INTEGER_BLOB {
            cbData: entropy.len() as u32,
            pbData: entropy.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB::default();
        if unsafe {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                &entropy_blob,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } == 0
        {
            return Err(last_io("cannot DPAPI-unprotect guardian receipt key"));
        }
        if output.pbData.is_null() || output.cbData != 32 {
            if !output.pbData.is_null() {
                unsafe {
                    std::ptr::write_bytes(output.pbData, 0, output.cbData as usize);
                    LocalFree(output.pbData.cast());
                }
            }
            return Err(ElevatedTransportError::Protocol(
                "unprotected guardian receipt key has invalid length".to_string(),
            ));
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(unsafe { std::slice::from_raw_parts(output.pbData, 32) });
        unsafe {
            std::ptr::write_bytes(output.pbData, 0, 32);
            LocalFree(output.pbData.cast());
        }
        Ok(GuardianReceiptKey(key))
    }

    fn write_guardian_close_receipt(
        receipt_file: &File,
        key: &GuardianReceiptKey,
        payload: GuardianCloseReceiptPayload,
    ) -> Result<(), ElevatedTransportError> {
        use std::os::windows::io::AsRawHandle as _;

        let payload_bytes = serde_json::to_vec(&payload).map_err(|error| {
            ElevatedTransportError::Protocol(format!(
                "cannot canonicalize guardian close receipt: {error}"
            ))
        })?;
        let receipt = GuardianCloseReceipt {
            mac_blake3: blake3::keyed_hash(&key.0, &payload_bytes)
                .to_hex()
                .to_string(),
            payload,
        };
        let bytes = serde_json::to_vec(&receipt).map_err(|error| {
            ElevatedTransportError::Protocol(format!(
                "cannot encode guardian close receipt: {error}"
            ))
        })?;
        if bytes.is_empty() || bytes.len() > GUARDIAN_RECEIPT_MAX_BYTES {
            return Err(ElevatedTransportError::Protocol(
                "guardian close receipt exceeds its bounded format".to_string(),
            ));
        }
        let initial = crate::bound_fs::platform_file_stamp(receipt_file)?;
        if initial.bytes != 0 {
            return Err(ElevatedTransportError::Identity(
                "guardian close receipt was not empty before its one-shot write".to_string(),
            ));
        }
        let mut writer = receipt_file.try_clone()?;
        writer.seek(SeekFrom::Start(0))?;
        writer.write_all(&bytes)?;
        writer.flush()?;
        if unsafe { FlushFileBuffers(receipt_file.as_raw_handle() as HANDLE) } == 0 {
            return Err(last_io("cannot durably flush guardian close receipt"));
        }
        let committed = crate::bound_fs::platform_file_stamp(receipt_file)?;
        if committed.bytes != bytes.len() as u64 {
            return Err(ElevatedTransportError::Identity(
                "guardian close receipt length changed across durable flush".to_string(),
            ));
        }
        Ok(())
    }

    pub(super) fn verify_guardian_close_receipt(
        authority: &GuardianReceiptAuthority,
        expected: &GuardianCloseReceiptExpectation,
    ) -> Result<(), ElevatedTransportError> {
        let context = GuardianReceiptKeyContext {
            schema: GUARDIAN_RECEIPT_SCHEMA.to_string(),
            operation_id: expected.operation_id,
            batch_item_id: expected.batch_item_id,
            nonce_digest: expected.nonce_digest.clone(),
            guardian_pid: expected.guardian_pid,
            guardian_started_100ns: expected.guardian_started_100ns,
            guardian_image_sha256: expected.guardian_image_sha256.clone(),
            target_stamp: expected.target_stamp.clone(),
            receipt_volume_id: authority.initial_stamp.volume_id.clone(),
            receipt_file_id: authority.initial_stamp.file_id.clone(),
        };
        let key = unprotect_receipt_key(&authority.protected_key_hex, &context)?;
        let bytes = crate::bound_fs::read_exact_guardian_receipt(
            &authority.path,
            &authority.initial_stamp,
            GUARDIAN_RECEIPT_MAX_BYTES,
        )?;
        let receipt: GuardianCloseReceipt = serde_json::from_slice(&bytes).map_err(|error| {
            ElevatedTransportError::Protocol(format!(
                "guardian close receipt is not its canonical schema: {error}"
            ))
        })?;
        let expected_payload = GuardianCloseReceiptPayload {
            schema: GUARDIAN_RECEIPT_SCHEMA.to_string(),
            operation_id: expected.operation_id,
            batch_item_id: expected.batch_item_id,
            nonce_digest: expected.nonce_digest.clone(),
            guardian_pid: expected.guardian_pid,
            guardian_started_100ns: expected.guardian_started_100ns,
            guardian_image_sha256: expected.guardian_image_sha256.clone(),
            target_stamp: expected.target_stamp.clone(),
            receipt_volume_id: authority.initial_stamp.volume_id.clone(),
            receipt_file_id: authority.initial_stamp.file_id.clone(),
            disposition_mode: expected.disposition_mode,
        };
        if receipt.payload != expected_payload {
            return Err(ElevatedTransportError::Identity(
                "guardian close receipt was replayed for another object/mode/nonce".to_string(),
            ));
        }
        let canonical = serde_json::to_vec(&receipt.payload).map_err(|error| {
            ElevatedTransportError::Protocol(format!(
                "cannot re-canonicalize guardian close receipt: {error}"
            ))
        })?;
        let expected_mac = blake3::keyed_hash(&key.0, &canonical);
        let actual_mac = decode_hex_32(&receipt.mac_blake3, "guardian receipt MAC")?;
        if !constant_time_equal(expected_mac.as_bytes(), &actual_mac) {
            return Err(ElevatedTransportError::Identity(
                "guardian close receipt MAC authentication failed".to_string(),
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn create_guardian_receipt_fixture(
        path: &Path,
        expected: &GuardianCloseReceiptExpectation,
        durably_write: bool,
    ) -> Result<GuardianReceiptAuthority, ElevatedTransportError> {
        let mut receipt = crate::bound_fs::BoundGuardianReceipt::create_new(path)?;
        let key = GuardianReceiptKey(random_key()?);
        let authority = GuardianReceiptAuthority {
            path: receipt.path().to_path_buf(),
            initial_stamp: receipt.initial_stamp().clone(),
            protected_key_hex: String::new(),
        };
        let context = GuardianReceiptKeyContext {
            schema: GUARDIAN_RECEIPT_SCHEMA.to_string(),
            operation_id: expected.operation_id,
            batch_item_id: expected.batch_item_id,
            nonce_digest: expected.nonce_digest.clone(),
            guardian_pid: expected.guardian_pid,
            guardian_started_100ns: expected.guardian_started_100ns,
            guardian_image_sha256: expected.guardian_image_sha256.clone(),
            target_stamp: expected.target_stamp.clone(),
            receipt_volume_id: authority.initial_stamp.volume_id.clone(),
            receipt_file_id: authority.initial_stamp.file_id.clone(),
        };
        let mut authority = authority;
        authority.protected_key_hex = protect_receipt_key(&key, &context)?;
        if durably_write {
            write_guardian_close_receipt(
                receipt.file_for_test(),
                &key,
                GuardianCloseReceiptPayload {
                    schema: GUARDIAN_RECEIPT_SCHEMA.to_string(),
                    operation_id: expected.operation_id,
                    batch_item_id: expected.batch_item_id,
                    nonce_digest: expected.nonce_digest.clone(),
                    guardian_pid: expected.guardian_pid,
                    guardian_started_100ns: expected.guardian_started_100ns,
                    guardian_image_sha256: expected.guardian_image_sha256.clone(),
                    target_stamp: expected.target_stamp.clone(),
                    receipt_volume_id: authority.initial_stamp.volume_id.clone(),
                    receipt_file_id: authority.initial_stamp.file_id.clone(),
                    disposition_mode: expected.disposition_mode,
                },
            )?;
        }
        receipt.preserve_for_recovery();
        drop(receipt);
        Ok(authority)
    }

    #[cfg(test)]
    fn random_hex_256() -> Result<String, ElevatedTransportError> {
        Ok(hex(&random_key()?))
    }

    fn pipe_name(nonce: &str) -> String {
        format!("{PIPE_PREFIX}{nonce}")
    }

    fn guardian_pipe_name(nonce: &str) -> String {
        format!("{GUARDIAN_PIPE_PREFIX}{nonce}")
    }

    fn hello_message(
        nonce: &str,
        manifest_digest: &[u8; 32],
    ) -> Result<Vec<u8>, ElevatedTransportError> {
        let nonce = decode_hex_32(nonce, "nonce")?;
        let mut out = Vec::with_capacity(72);
        out.extend_from_slice(HELLO_MAGIC);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(manifest_digest);
        Ok(out)
    }

    fn validate_hello(
        bytes: &[u8],
        nonce: &str,
        manifest_digest: &[u8; 32],
    ) -> Result<(), ElevatedTransportError> {
        let expected = hello_message(nonce, manifest_digest)?;
        if !constant_time_equal(bytes, &expected) {
            return Err(ElevatedTransportError::Protocol(
                "helper hello is not bound to the pipe nonce".to_string(),
            ));
        }
        Ok(())
    }

    fn key_message(
        nonce: &str,
        manifest_digest: &[u8; 32],
        key: &[u8; 32],
    ) -> Result<Vec<u8>, ElevatedTransportError> {
        let nonce = decode_hex_32(nonce, "nonce")?;
        let mut out = Vec::with_capacity(104);
        out.extend_from_slice(KEY_MAGIC);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(manifest_digest);
        out.extend_from_slice(key);
        Ok(out)
    }

    fn parse_key_message(
        bytes: &[u8],
        nonce: &str,
        manifest_digest: &[u8; 32],
    ) -> Result<[u8; 32], ElevatedTransportError> {
        if bytes.len() != 104
            || !constant_time_equal(&bytes[..8], KEY_MAGIC)
            || !constant_time_equal(&bytes[8..40], &decode_hex_32(nonce, "nonce")?)
            || !constant_time_equal(&bytes[40..72], manifest_digest)
        {
            return Err(ElevatedTransportError::Protocol(
                "session-key message is not bound to the pipe nonce".to_string(),
            ));
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes[72..]);
        Ok(key)
    }

    fn parent_context_from_frame(
        frame: &[u8],
        expected_nonce: &str,
        expected_sequence: u64,
    ) -> Result<FrameContext, ElevatedTransportError> {
        let header = FRAME_PREFIX_BYTES;
        if frame.len() < FRAME_PREFIX_BYTES + FRAME_HEADER_BYTES + FRAME_MAC_BYTES
            || frame[header] != FRAME_WIRE_VERSION
            || frame[header + 1] != FrameRole::ParentRequest as u8
        {
            return Err(ElevatedTransportError::Protocol(
                "request frame has no valid parent-request header".to_string(),
            ));
        }
        let sequence = u64::from_le_bytes(frame[header + 4..header + 12].try_into().unwrap());
        if sequence != expected_sequence || sequence == 0 || sequence.is_multiple_of(2) {
            return Err(ElevatedTransportError::Protocol(
                "request frame sequence is not the next parent sequence".to_string(),
            ));
        }
        let nonce = hex(&frame[header + 28..header + 60]);
        if nonce != expected_nonce {
            return Err(ElevatedTransportError::Protocol(
                "request frame nonce differs from the pipe nonce".to_string(),
            ));
        }
        Ok(FrameContext {
            role: FrameRole::ParentRequest,
            sequence,
            operation_uuid: hex(&frame[header + 12..header + 28]),
            nonce,
            request_blake3: hex(&frame[header + 60..header + 92]),
        })
    }

    fn overlapped_write(handle: HANDLE, bytes: &[u8]) -> Result<(), ElevatedTransportError> {
        if bytes.is_empty() || bytes.len() > PIPE_BUFFER_BYTES as usize {
            return Err(ElevatedTransportError::Protocol(
                "named-pipe message has invalid bounds".to_string(),
            ));
        }
        let event = OwnedHandle::new(
            unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) },
            "cannot create pipe write event",
        )?;
        let mut overlapped = OVERLAPPED {
            hEvent: event.raw(),
            ..Default::default()
        };
        let mut written = 0u32;
        let ok = unsafe {
            WriteFile(
                handle,
                bytes.as_ptr(),
                bytes.len() as u32,
                &mut written,
                &mut overlapped,
            )
        };
        if ok == 0 && unsafe { GetLastError() } != ERROR_IO_PENDING {
            return Err(last_io("cannot write one-shot pipe message"));
        }
        if ok == 0 {
            wait_overlapped(
                handle,
                &mut overlapped,
                event.raw(),
                &mut written,
                "pipe write",
            )?;
        }
        if written as usize != bytes.len() {
            return Err(ElevatedTransportError::Protocol(
                "named-pipe message write was partial".to_string(),
            ));
        }
        Ok(())
    }

    fn overlapped_read_framed(
        handle: HANDLE,
        limit: usize,
    ) -> Result<Vec<u8>, ElevatedTransportError> {
        let (prefix, prefix_more) = overlapped_read_piece(handle, FRAME_PREFIX_BYTES)?;
        if prefix.len() != FRAME_PREFIX_BYTES || !prefix_more {
            return Err(ElevatedTransportError::Protocol(
                "one-shot frame prefix is not a single bounded pipe message".to_string(),
            ));
        }
        let declared =
            u32::from_le_bytes(prefix[..FRAME_PREFIX_BYTES].try_into().unwrap()) as usize;
        if declared < FRAME_MIN_DECLARED_BYTES
            || declared.checked_add(FRAME_PREFIX_BYTES).is_none()
            || declared + FRAME_PREFIX_BYTES > limit
        {
            return Err(ElevatedTransportError::Protocol(
                "one-shot frame length exceeds its fixed bound".to_string(),
            ));
        }
        let (body, body_more) = overlapped_read_piece(handle, declared)?;
        if body.len() != declared || body_more {
            return Err(ElevatedTransportError::Protocol(
                "one-shot frame did not end at its authenticated length".to_string(),
            ));
        }
        let mut frame = prefix;
        frame.extend_from_slice(&body);
        Ok(frame)
    }

    fn overlapped_read_piece(
        handle: HANDLE,
        bytes: usize,
    ) -> Result<(Vec<u8>, bool), ElevatedTransportError> {
        let event = OwnedHandle::new(
            unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) },
            "cannot create pipe read event",
        )?;
        let mut overlapped = OVERLAPPED {
            hEvent: event.raw(),
            ..Default::default()
        };
        let mut buffer = vec![0u8; bytes];
        let mut read = 0u32;
        let ok = unsafe {
            ReadFile(
                handle,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                &mut read,
                &mut overlapped,
            )
        };
        let mut more = false;
        if ok == 0 {
            let error = unsafe { GetLastError() };
            if error == ERROR_IO_PENDING {
                let wait = unsafe { WaitForSingleObject(event.raw(), IO_TIMEOUT_MS) };
                if wait == WAIT_TIMEOUT {
                    cancel_and_drain(handle, &mut overlapped);
                    return Err(ElevatedTransportError::Timeout("pipe read".to_string()));
                }
                if wait != WAIT_OBJECT_0 {
                    cancel_and_drain(handle, &mut overlapped);
                    return Err(last_io("waiting for pipe read failed"));
                }
                if unsafe { GetOverlappedResult(handle, &overlapped, &mut read, 0) } == 0 {
                    let completion_error = unsafe { GetLastError() };
                    if completion_error == ERROR_MORE_DATA {
                        more = true;
                    } else {
                        return Err(last_io("overlapped pipe read failed"));
                    }
                }
            } else if error == ERROR_MORE_DATA {
                more = true;
            } else {
                return Err(last_io("cannot read one-shot pipe message"));
            }
        }
        if read == 0 || read as usize > buffer.len() {
            return Err(ElevatedTransportError::Protocol(
                "named-pipe read made no valid progress".to_string(),
            ));
        }
        buffer.truncate(read as usize);
        Ok((buffer, more))
    }

    fn wait_overlapped(
        handle: HANDLE,
        overlapped: &mut OVERLAPPED,
        event: HANDLE,
        transferred: &mut u32,
        label: &str,
    ) -> Result<(), ElevatedTransportError> {
        let wait = unsafe { WaitForSingleObject(event, IO_TIMEOUT_MS) };
        if wait == WAIT_TIMEOUT {
            cancel_and_drain(handle, overlapped);
            return Err(ElevatedTransportError::Timeout(label.to_string()));
        }
        if wait != WAIT_OBJECT_0 {
            return Err(last_io("waiting for pipe io failed"));
        }
        if unsafe { GetOverlappedResult(handle, overlapped, transferred, 0) } == 0 {
            return Err(last_io("overlapped pipe io failed"));
        }
        Ok(())
    }

    fn cancel_and_drain(handle: HANDLE, overlapped: &mut OVERLAPPED) {
        let mut ignored = 0u32;
        unsafe {
            CancelIoEx(handle, overlapped);
            // An OVERLAPPED must remain live until cancellation completes.
            // Waiting here prevents the kernel from retaining a pointer to a
            // stack value after this function returns.
            GetOverlappedResult(handle, overlapped, &mut ignored, 1);
        }
    }

    fn sync_write_message(handle: HANDLE, bytes: &[u8]) -> Result<(), ElevatedTransportError> {
        if bytes.is_empty() || bytes.len() > PIPE_BUFFER_BYTES as usize {
            return Err(ElevatedTransportError::Protocol(
                "helper pipe message has invalid bounds".to_string(),
            ));
        }
        let mut written = 0u32;
        if unsafe {
            WriteFile(
                handle,
                bytes.as_ptr(),
                bytes.len() as u32,
                &mut written,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(last_io("cannot write helper pipe message"));
        }
        if written as usize != bytes.len() {
            return Err(ElevatedTransportError::Protocol(
                "helper pipe message write was partial".to_string(),
            ));
        }
        Ok(())
    }

    fn sync_read_message(handle: HANDLE, limit: usize) -> Result<Vec<u8>, ElevatedTransportError> {
        let (prefix, prefix_more) = sync_read_piece(handle, FRAME_PREFIX_BYTES)?;
        if prefix.len() != FRAME_PREFIX_BYTES || !prefix_more {
            return Err(ElevatedTransportError::Protocol(
                "helper frame prefix is not one bounded pipe message".to_string(),
            ));
        }
        let declared =
            u32::from_le_bytes(prefix[..FRAME_PREFIX_BYTES].try_into().unwrap()) as usize;
        if declared < FRAME_MIN_DECLARED_BYTES
            || declared.checked_add(FRAME_PREFIX_BYTES).is_none()
            || declared + FRAME_PREFIX_BYTES > limit
        {
            return Err(ElevatedTransportError::Protocol(
                "helper frame exceeds its fixed bound".to_string(),
            ));
        }
        let (body, body_more) = sync_read_piece(handle, declared)?;
        if body.len() != declared || body_more {
            return Err(ElevatedTransportError::Protocol(
                "helper frame did not end at its authenticated length".to_string(),
            ));
        }
        let mut frame = prefix;
        frame.extend_from_slice(&body);
        Ok(frame)
    }

    fn sync_read_exact_message(
        handle: HANDLE,
        bytes: usize,
    ) -> Result<Vec<u8>, ElevatedTransportError> {
        let (message, more) = sync_read_piece(handle, bytes)?;
        if message.len() != bytes || more {
            return Err(ElevatedTransportError::Protocol(
                "helper handshake message has an invalid message boundary".to_string(),
            ));
        }
        Ok(message)
    }

    fn sync_read_piece(
        handle: HANDLE,
        bytes: usize,
    ) -> Result<(Vec<u8>, bool), ElevatedTransportError> {
        let mut buffer = vec![0u8; bytes];
        let mut read = 0u32;
        let ok = unsafe {
            ReadFile(
                handle,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                &mut read,
                std::ptr::null_mut(),
            )
        };
        let error = if ok == 0 {
            unsafe { GetLastError() }
        } else {
            0
        };
        if ok == 0 && error != ERROR_MORE_DATA {
            return Err(last_io("cannot read helper pipe message"));
        }
        if read == 0 || read as usize > buffer.len() {
            return Err(ElevatedTransportError::Protocol(
                "helper pipe read made no valid progress".to_string(),
            ));
        }
        buffer.truncate(read as usize);
        Ok((buffer, error == ERROR_MORE_DATA))
    }

    fn ensure_no_queued_message(handle: HANDLE, label: &str) -> Result<(), ElevatedTransportError> {
        let mut total = 0u32;
        let mut left = 0u32;
        if unsafe {
            PeekNamedPipe(
                handle,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut total,
                &mut left,
            )
        } == 0
        {
            let error = unsafe { GetLastError() };
            if is_closed_pipe_error(error) {
                return Ok(());
            }
            return Err(last_io("cannot inspect one-shot pipe queue"));
        }
        if total != 0 || left != 0 {
            return Err(ElevatedTransportError::Protocol(label.to_string()));
        }
        Ok(())
    }

    fn is_closed_pipe_error(error: u32) -> bool {
        matches!(
            error,
            ERROR_BROKEN_PIPE | ERROR_NO_DATA | ERROR_PIPE_NOT_CONNECTED
        )
    }

    fn now_unix_seconds() -> Result<i64, ElevatedTransportError> {
        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ElevatedTransportError::Protocol("system clock predates Unix".into()))?;
        i64::try_from(duration.as_secs()).map_err(|_| {
            ElevatedTransportError::Protocol("system clock exceeds protocol range".into())
        })
    }

    fn require_hex(value: &str, len: usize, label: &str) -> Result<(), ElevatedTransportError> {
        if value.len() == len && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            Ok(())
        } else {
            Err(ElevatedTransportError::Protocol(format!(
                "{label} has invalid encoding"
            )))
        }
    }

    fn decode_hex_32(value: &str, label: &str) -> Result<[u8; 32], ElevatedTransportError> {
        require_hex(value, 64, label)?;
        let mut out = [0u8; 32];
        for (index, pair) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
            let text = std::str::from_utf8(pair)
                .map_err(|_| ElevatedTransportError::Protocol(format!("{label} is not ASCII")))?;
            out[index] = u8::from_str_radix(text, 16).map_err(|_| {
                ElevatedTransportError::Protocol(format!("{label} has invalid encoding"))
            })?;
        }
        Ok(out)
    }

    fn decode_hex_vec(value: &str, label: &str) -> Result<Vec<u8>, ElevatedTransportError> {
        if value.is_empty() || !value.len().is_multiple_of(2) || value.len() > 16 * 1024 {
            return Err(ElevatedTransportError::Protocol(format!(
                "{label} has invalid bounds"
            )));
        }
        let mut out = Vec::with_capacity(value.len() / 2);
        for pair in value.as_bytes().as_chunks::<2>().0 {
            let text = std::str::from_utf8(pair)
                .map_err(|_| ElevatedTransportError::Protocol(format!("{label} is not ASCII")))?;
            out.push(u8::from_str_radix(text, 16).map_err(|_| {
                ElevatedTransportError::Protocol(format!("{label} has invalid encoding"))
            })?);
        }
        Ok(out)
    }

    fn hex(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            use std::fmt::Write as _;
            write!(&mut out, "{byte:02x}").expect("String write cannot fail");
        }
        out
    }

    fn constant_time_equal(left: &[u8], right: &[u8]) -> bool {
        if left.len() != right.len() {
            return false;
        }
        left.iter()
            .zip(right)
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
    }

    fn bounded_message(message: &str) -> String {
        const MAX: usize = 256;
        if message.len() <= MAX {
            return message.to_string();
        }
        let mut boundary = MAX;
        while !message.is_char_boundary(boundary) {
            boundary -= 1;
        }
        format!("{}...", &message[..boundary])
    }

    fn wide(value: &str) -> Vec<u16> {
        OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn wide_os(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(std::iter::once(0)).collect()
    }

    fn os_string(value: &OsStr, label: &str) -> Result<String, ElevatedTransportError> {
        value.to_str().map(str::to_string).ok_or_else(|| {
            ElevatedTransportError::Protocol(format!("helper {label} is not valid Unicode"))
        })
    }

    fn last_io(label: &str) -> ElevatedTransportError {
        ElevatedTransportError::Io(std::io::Error::other(format!(
            "{label}: {}",
            std::io::Error::last_os_error()
        )))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::{ExpectedObject, ExpectedScratchRoot, FileStamp};
        use windows_sys::Win32::Security::Cryptography::{
            BCryptExportKey, BCryptFinalizeKeyPair, BCryptGenerateKeyPair, BCryptSignHash,
        };

        fn sign_test_manifest(
            manifest: &mut ReleaseManifest,
        ) -> Result<Vec<u8>, ElevatedTransportError> {
            let mut algorithm: BCRYPT_ALG_HANDLE = std::ptr::null_mut();
            let opened = unsafe {
                BCryptOpenAlgorithmProvider(
                    &mut algorithm,
                    BCRYPT_RSA_ALGORITHM,
                    std::ptr::null(),
                    0,
                )
            };
            if opened != 0 || algorithm.is_null() {
                return Err(ElevatedTransportError::Identity(format!(
                    "test RSA provider failed: 0x{:08x}",
                    opened as u32
                )));
            }
            struct TestAlgorithm(BCRYPT_ALG_HANDLE);
            impl Drop for TestAlgorithm {
                fn drop(&mut self) {
                    unsafe { BCryptCloseAlgorithmProvider(self.0, 0) };
                }
            }
            let algorithm = TestAlgorithm(algorithm);

            let mut key: BCRYPT_KEY_HANDLE = std::ptr::null_mut();
            let generated = unsafe { BCryptGenerateKeyPair(algorithm.0, &mut key, 3072, 0) };
            if generated != 0 || key.is_null() {
                return Err(ElevatedTransportError::Identity(format!(
                    "test RSA key generation failed: 0x{:08x}",
                    generated as u32
                )));
            }
            struct TestKey(BCRYPT_KEY_HANDLE);
            impl Drop for TestKey {
                fn drop(&mut self) {
                    unsafe { BCryptDestroyKey(self.0) };
                }
            }
            let key = TestKey(key);
            let finalized = unsafe { BCryptFinalizeKeyPair(key.0, 0) };
            if finalized != 0 {
                return Err(ElevatedTransportError::Identity(format!(
                    "test RSA key finalization failed: 0x{:08x}",
                    finalized as u32
                )));
            }

            let mut public_len = 0u32;
            let measured = unsafe {
                BCryptExportKey(
                    key.0,
                    std::ptr::null_mut(),
                    BCRYPT_RSAPUBLIC_BLOB,
                    std::ptr::null_mut(),
                    0,
                    &mut public_len,
                    0,
                )
            };
            if measured != 0 || public_len == 0 {
                return Err(ElevatedTransportError::Identity(format!(
                    "test RSA public-key measurement failed: 0x{:08x}",
                    measured as u32
                )));
            }
            let mut public_blob = vec![0u8; public_len as usize];
            let exported = unsafe {
                BCryptExportKey(
                    key.0,
                    std::ptr::null_mut(),
                    BCRYPT_RSAPUBLIC_BLOB,
                    public_blob.as_mut_ptr(),
                    public_blob.len() as u32,
                    &mut public_len,
                    0,
                )
            };
            if exported != 0 || public_len as usize != public_blob.len() {
                return Err(ElevatedTransportError::Identity(format!(
                    "test RSA public-key export failed: 0x{:08x}",
                    exported as u32
                )));
            }

            let digest: [u8; 32] = Sha256::digest(manifest_signing_payload(manifest)).into();
            let padding = BCRYPT_PSS_PADDING_INFO {
                pszAlgId: BCRYPT_SHA256_ALGORITHM,
                cbSalt: 32,
            };
            let mut signature_len = 0u32;
            let measured = unsafe {
                BCryptSignHash(
                    key.0,
                    (&padding as *const BCRYPT_PSS_PADDING_INFO).cast(),
                    digest.as_ptr(),
                    digest.len() as u32,
                    std::ptr::null_mut(),
                    0,
                    &mut signature_len,
                    BCRYPT_PAD_PSS,
                )
            };
            if measured != 0 || signature_len != 384 {
                return Err(ElevatedTransportError::Identity(format!(
                    "test RSA signature measurement failed: 0x{:08x}",
                    measured as u32
                )));
            }
            let mut signature = vec![0u8; signature_len as usize];
            let signed = unsafe {
                BCryptSignHash(
                    key.0,
                    (&padding as *const BCRYPT_PSS_PADDING_INFO).cast(),
                    digest.as_ptr(),
                    digest.len() as u32,
                    signature.as_mut_ptr(),
                    signature.len() as u32,
                    &mut signature_len,
                    BCRYPT_PAD_PSS,
                )
            };
            if signed != 0 || signature_len as usize != signature.len() {
                return Err(ElevatedTransportError::Identity(format!(
                    "test RSA signing failed: 0x{:08x}",
                    signed as u32
                )));
            }
            manifest.signature_rsa_pss_sha256 = hex(&signature);
            Ok(public_blob)
        }

        fn synthetic_request(root: &Path, parent: ParentBinding) -> ElevatedRequest {
            let now = now_unix_seconds().unwrap();
            ElevatedRequest {
                schema: PROTOCOL_SCHEMA.to_string(),
                nonce: "ab".repeat(32),
                issued_at_unix_seconds: now,
                expires_at_unix_seconds: now + 60,
                parent,
                plan_fingerprint: format!("v2:{}", "cd".repeat(32)),
                operation_id: 9,
                journal_capability_blake3: "ef".repeat(32),
                synthetic_test: true,
                capabilities: vec![ElevatedCapability::ObjectBackupV2 {
                    source: ExpectedObject {
                        path: root.join("source.bin"),
                        parent_handle_value: 1,
                        stamp: FileStamp {
                            volume_id: "fixture-volume".to_string(),
                            file_id: "fixture-file".to_string(),
                            bytes: 0,
                            modified_unix_seconds: None,
                        },
                        content_blake3: blake3::hash(&[]).to_hex().to_string(),
                        semantic_blake3: None,
                        allow_internal_directory_time_drift: false,
                    },
                    parent_archive_handle_value: 2,
                    archive_path: archive_path_for_capability(
                        &root.join("backup/scratch"),
                        &"ab".repeat(32),
                        0,
                    ),
                    scratch_root: ExpectedScratchRoot {
                        path: root.join("backup/scratch"),
                        parent_handle_value: 3,
                        stamp: FileStamp {
                            volume_id: "fixture-volume".to_string(),
                            file_id: "fixture-scratch".to_string(),
                            bytes: 0,
                            modified_unix_seconds: None,
                        },
                    },
                    scratch_leaf: scratch_leaf_for_capability(&"ab".repeat(32), 0),
                }],
            }
        }

        fn synthetic_receipt_controller(path: &Path) -> DispositionGuardian {
            let receipt = crate::bound_fs::BoundGuardianReceipt::create_new(path).unwrap();
            let nonce = random_hex_256().unwrap();
            let pipe = PipeServer::create(&guardian_pipe_name(&nonce)).unwrap();
            let mut command = Command::new("cmd.exe");
            command
                .args(["/D", "/C", "exit 0"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW);
            let child = command.spawn().unwrap();
            let authority = GuardianReceiptAuthority {
                path: receipt.path().to_path_buf(),
                initial_stamp: receipt.initial_stamp().clone(),
                protected_key_hex: "fixture-only".to_string(),
            };
            DispositionGuardian {
                pipe: Some(pipe),
                _child: child,
                receipt: Some(receipt),
                receipt_journaled: false,
                key: [0x5a; 32],
                operation_uuid: "61".repeat(16),
                nonce: nonce.clone(),
                sequence: 3,
                batch_item_id: 7,
                identity: DispositionGuardianIdentity {
                    pid: u32::MAX,
                    process_started_100ns: 1,
                    image_sha256: "72".repeat(32),
                    nonce_digest: blake3::hash(nonce.as_bytes()).to_hex().to_string(),
                    receipt: authority,
                },
            }
        }

        #[test]
        fn command_line_accepts_only_pipe_parent_and_nonce() {
            let nonce = "ab".repeat(32);
            let pipe = pipe_name(&nonce);
            let parsed = parse_cli([
                OsString::from("--pipe"),
                OsString::from(pipe),
                OsString::from("--parent-pid"),
                OsString::from("42"),
                OsString::from("--nonce"),
                OsString::from(nonce),
            ])
            .unwrap();
            assert_eq!(parsed.parent_pid, 42);

            assert!(parse_cli([
                OsString::from("--pipe"),
                OsString::from("x"),
                OsString::from("--parent-pid"),
                OsString::from("42"),
                OsString::from("--key"),
                OsString::from("secret"),
            ])
            .is_err());
        }

        #[test]
        fn guardian_command_line_is_nonce_derived_and_has_no_path_or_handle() {
            let nonce = "5c".repeat(32);
            let parsed = parse_guardian_cli([
                OsString::from("--parent-pid"),
                OsString::from("42"),
                OsString::from("--nonce"),
                OsString::from(&nonce),
                OsString::from("--pipe"),
                OsString::from(guardian_pipe_name(&nonce)),
            ])
            .unwrap();
            assert_eq!(parsed.parent_pid, 42);
            assert_eq!(parsed.nonce, nonce);

            assert!(parse_guardian_cli([
                OsString::from("--pipe"),
                OsString::from(r"\\.\pipe\attacker-selected"),
                OsString::from("--parent-pid"),
                OsString::from("42"),
                OsString::from("--nonce"),
                OsString::from("5c".repeat(32)),
            ])
            .is_err());
            assert!(parse_guardian_cli([
                OsString::from("--pipe"),
                OsString::from(guardian_pipe_name(&"5c".repeat(32))),
                OsString::from("--parent-pid"),
                OsString::from("42"),
                OsString::from("--handle"),
                OsString::from("1234"),
            ])
            .is_err());
        }

        #[test]
        fn unjournaled_guardian_drop_waits_before_exact_receipt_cleanup() {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("unjournaled-guardian-receipt.bin");
            let guardian = synthetic_receipt_controller(&path);
            assert!(path.exists());

            drop(guardian);

            assert!(!path.exists());
        }

        #[test]
        fn failed_prejournal_guardian_launch_waits_before_exact_receipt_cleanup() {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("failed-launch-guardian-receipt.bin");
            let receipt = crate::bound_fs::BoundGuardianReceipt::create_new(&path).unwrap();
            let nonce = random_hex_256().unwrap();
            let pipe = PipeServer::create(&guardian_pipe_name(&nonce)).unwrap();
            let mut command = Command::new("cmd.exe");
            command
                .args(["/D", "/C", "exit 0"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW);
            let child = command.spawn().unwrap();
            let pending = PendingDispositionGuardian {
                pipe: Some(pipe),
                child: Some(child),
                receipt: Some(receipt),
            };
            assert!(path.exists());

            // Models any identity/handshake `?` after process creation but
            // before a complete guardian controller is returned.
            drop(pending);

            assert!(!path.exists());
        }

        #[test]
        fn terminal_guardian_cleanup_deletes_the_exact_journaled_receipt() {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("terminal-guardian-receipt.bin");
            let mut guardian = synthetic_receipt_controller(&path);
            guardian.mark_receipt_journaled();
            assert!(path.exists());

            guardian.cleanup_receipt_after_terminal_state().unwrap();

            assert!(!path.exists());
        }

        #[test]
        fn durable_guardian_receipt_rejects_object_mode_nonce_and_operation_replay() {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("guardian-receipt.bin");
            let expected = GuardianCloseReceiptExpectation {
                operation_id: 41,
                batch_item_id: 73,
                nonce_digest: "15".repeat(32),
                guardian_pid: u32::MAX,
                guardian_started_100ns: 99,
                guardian_image_sha256: "26".repeat(32),
                target_stamp: FileStamp {
                    volume_id: "target-volume".to_string(),
                    file_id: "target-file".to_string(),
                    bytes: 17,
                    modified_unix_seconds: Some(23),
                },
                disposition_mode: crate::bound_fs::WindowsDeleteDispositionMode::ExtendedOnClose,
            };
            let authority = create_guardian_receipt_fixture(&path, &expected, true).unwrap();
            verify_guardian_close_receipt(&authority, &expected).unwrap();

            let mut replay = expected.clone();
            replay.operation_id += 1;
            assert!(verify_guardian_close_receipt(&authority, &replay).is_err());
            replay = expected.clone();
            replay.batch_item_id += 1;
            assert!(verify_guardian_close_receipt(&authority, &replay).is_err());
            replay = expected.clone();
            replay.nonce_digest = "37".repeat(32);
            assert!(verify_guardian_close_receipt(&authority, &replay).is_err());
            replay = expected.clone();
            replay.target_stamp.file_id = "another-object".to_string();
            assert!(verify_guardian_close_receipt(&authority, &replay).is_err());
            replay = expected.clone();
            replay.disposition_mode = crate::bound_fs::WindowsDeleteDispositionMode::Legacy;
            assert!(verify_guardian_close_receipt(&authority, &replay).is_err());

            crate::bound_fs::cleanup_exact_guardian_receipt(
                &authority.path,
                &authority.initial_stamp,
            )
            .unwrap();
            assert!(!path.exists());
        }

        #[test]
        fn synthetic_guardian_cancels_after_parent_handle_and_pipe_die() {
            let root = tempfile::tempdir().unwrap();
            let source = root.path().join("held.bin");
            let raced_link = root.path().join("raced-link.bin");
            std::fs::write(&source, b"guardian must preserve these bytes").unwrap();
            let (stamp, hash) = crate::bound_fs::inspect_local_mutation_file(&source).unwrap();
            let proof =
                crate::bound_fs::BoundObjectProof::open_for_archive_delete(&source, &stamp, &hash)
                    .unwrap()
                    .detach_exclusive_for_final_disposition()
                    .unwrap();
            proof.validate_final_disposition_prearm().unwrap();
            let receipt_path = root.path().join("guardian-receipt.bin");
            let receipt = crate::bound_fs::BoundGuardianReceipt::create_new(&receipt_path).unwrap();

            let parent = current_parent_binding().unwrap();
            let nonce = random_hex_256().unwrap();
            let operation_uuid = "72".repeat(16);
            let pipe_name = guardian_pipe_name(&nonce);
            let pipe = PipeServer::create(&pipe_name).unwrap();
            let cli = GuardianCli {
                pipe_name: pipe_name.clone(),
                parent_pid: parent.pid,
                nonce: nonce.clone(),
            };
            let expected_image = std::env::current_exe().unwrap();
            let guardian_thread = std::thread::spawn(move || {
                guardian_exchange_with_policy(
                    &cli,
                    IdentityPolicy::SyntheticFixture { expected_image },
                )
            });
            pipe.connect(None).unwrap();
            let hello = pipe.read_exact_message(HELLO_MAGIC.len() + 64).unwrap();
            validate_hello(&hello, &nonce, &[0u8; 32]).unwrap();
            let key = random_key().unwrap();
            pipe.write_message(&key_message(&nonce, &[0u8; 32], &key).unwrap())
                .unwrap();

            let bind = GuardianCommand::Bind {
                schema: GUARDIAN_SCHEMA.to_string(),
                operation_id: 11,
                batch_item_id: 7,
                parent_handle_value: proof.raw_handle_value(),
                expected_stamp: stamp,
                receipt_handle_value: receipt.raw_handle_value(),
                expected_receipt_stamp: receipt.initial_stamp().clone(),
                receipt_key: GuardianReceiptKey(random_key().unwrap()),
            };
            let reply: GuardianReply =
                parent_roundtrip(&pipe, &key, &operation_uuid, &nonce, 1, &bind).unwrap();
            assert!(matches!(reply, GuardianReply::HandleBound { .. }));
            let arm = GuardianCommand::ArmAuthorized {
                schema: GUARDIAN_SCHEMA.to_string(),
                batch_item_id: 7,
            };
            let reply: GuardianReply =
                parent_roundtrip(&pipe, &key, &operation_uuid, &nonce, 3, &arm).unwrap();
            assert!(matches!(reply, GuardianReply::ArmReady { .. }));

            // Win the exact namespace race after both peers bind but before the
            // parent arms. Both same-handle proofs must refuse the new link.
            std::fs::hard_link(&source, &raced_link).unwrap();
            let mode = proof.arm_final_disposition().unwrap();
            assert!(proof.validate_armed_final_disposition(mode).is_err());
            let prove = GuardianCommand::ProveArmed {
                schema: GUARDIAN_SCHEMA.to_string(),
                batch_item_id: 7,
                mode,
            };
            let reply: GuardianReply =
                parent_roundtrip(&pipe, &key, &operation_uuid, &nonce, 5, &prove).unwrap();
            assert!(matches!(reply, GuardianReply::Refused { .. }));

            crate::bound_fs::set_cancel_delete_on_close_fault(true);
            assert!(proof.cancel_final_disposition(mode).is_err());
            crate::bound_fs::set_cancel_delete_on_close_fault(false);

            // Model abrupt desktop termination: its handle and authenticated
            // control channel disappear without a Cancel frame. The independent
            // guardian thread must cancel through its duplicate before closing.
            drop(proof);
            drop(pipe);
            guardian_thread.join().unwrap().unwrap();

            assert_eq!(
                std::fs::read(&source).unwrap(),
                b"guardian must preserve these bytes"
            );
            assert_eq!(
                std::fs::read(&raced_link).unwrap(),
                b"guardian must preserve these bytes"
            );
        }

        #[test]
        fn parent_proved_preprove_cancel_notifies_guardian_and_allows_same_session_retry() {
            let root = tempfile::tempdir().unwrap();
            let source = root.path().join("held.bin");
            std::fs::write(&source, b"same-session guardian cancellation").unwrap();
            let (stamp, hash) = crate::bound_fs::inspect_local_mutation_file(&source).unwrap();
            let proof =
                crate::bound_fs::BoundObjectProof::open_for_archive_delete(&source, &stamp, &hash)
                    .unwrap()
                    .detach_exclusive_for_final_disposition()
                    .unwrap();
            proof.validate_final_disposition_prearm().unwrap();
            let receipt_path = root.path().join("guardian-receipt.bin");
            let receipt = crate::bound_fs::BoundGuardianReceipt::create_new(&receipt_path).unwrap();

            let parent = current_parent_binding().unwrap();
            let nonce = random_hex_256().unwrap();
            let operation_uuid = "83".repeat(16);
            let pipe_name = guardian_pipe_name(&nonce);
            let pipe = PipeServer::create(&pipe_name).unwrap();
            let cli = GuardianCli {
                pipe_name: pipe_name.clone(),
                parent_pid: parent.pid,
                nonce: nonce.clone(),
            };
            let expected_image = std::env::current_exe().unwrap();
            let guardian_thread = std::thread::spawn(move || {
                guardian_exchange_with_policy(
                    &cli,
                    IdentityPolicy::SyntheticFixture { expected_image },
                )
            });
            pipe.connect(None).unwrap();
            let hello = pipe.read_exact_message(HELLO_MAGIC.len() + 64).unwrap();
            validate_hello(&hello, &nonce, &[0u8; 32]).unwrap();
            let key = random_key().unwrap();
            pipe.write_message(&key_message(&nonce, &[0u8; 32], &key).unwrap())
                .unwrap();

            let bind = GuardianCommand::Bind {
                schema: GUARDIAN_SCHEMA.to_string(),
                operation_id: 13,
                batch_item_id: 9,
                parent_handle_value: proof.raw_handle_value(),
                expected_stamp: stamp,
                receipt_handle_value: receipt.raw_handle_value(),
                expected_receipt_stamp: receipt.initial_stamp().clone(),
                receipt_key: GuardianReceiptKey(random_key().unwrap()),
            };
            let reply: GuardianReply =
                parent_roundtrip(&pipe, &key, &operation_uuid, &nonce, 1, &bind).unwrap();
            assert!(matches!(reply, GuardianReply::HandleBound { .. }));
            let arm = GuardianCommand::ArmAuthorized {
                schema: GUARDIAN_SCHEMA.to_string(),
                batch_item_id: 9,
            };
            let reply: GuardianReply =
                parent_roundtrip(&pipe, &key, &operation_uuid, &nonce, 3, &arm).unwrap();
            assert!(matches!(reply, GuardianReply::ArmReady { .. }));

            let mode = proof.arm_final_disposition().unwrap();
            proof.validate_armed_final_disposition(mode).unwrap();
            proof.cancel_final_disposition(mode).unwrap();
            let cancel = GuardianCommand::Cancel {
                schema: GUARDIAN_SCHEMA.to_string(),
                batch_item_id: 9,
                preferred_mode: Some(mode),
            };
            let reply: GuardianReply =
                parent_roundtrip(&pipe, &key, &operation_uuid, &nonce, 5, &cancel).unwrap();
            assert!(matches!(reply, GuardianReply::CancelledSafe { .. }));

            drop(pipe);
            guardian_thread.join().unwrap().unwrap();
            drop(proof);
            drop(receipt);

            let (retry_stamp, retry_hash) =
                crate::bound_fs::inspect_local_mutation_file(&source).unwrap();
            let retry = crate::bound_fs::BoundObjectProof::open_for_archive_delete(
                &source,
                &retry_stamp,
                &retry_hash,
            )
            .unwrap()
            .detach_exclusive_for_final_disposition()
            .unwrap();
            retry.validate_final_disposition_prearm().unwrap();
            drop(retry);
            assert_eq!(
                std::fs::read(&source).unwrap(),
                b"same-session guardian cancellation"
            );
        }

        const SAFE05_CHILD_PROBE: &str =
            "elevated_transport::windows_impl::tests::guardian_crash_after_prove_child_probe";
        const SAFE05_ROLE_ENV: &str = "CODEHANGAR_TEST_SAFE05_ROLE";
        const SAFE05_SCENARIO_ENV: &str = "CODEHANGAR_TEST_SAFE05_SCENARIO";
        const SAFE05_ROOT_ENV: &str = "CODEHANGAR_TEST_SAFE05_ROOT";
        const SAFE05_PIPE_ENV: &str = "CODEHANGAR_TEST_SAFE05_PIPE";
        const SAFE05_NONCE_ENV: &str = "CODEHANGAR_TEST_SAFE05_NONCE";
        const SAFE05_PARENT_PID_ENV: &str = "CODEHANGAR_TEST_SAFE05_PARENT_PID";
        const SAFE05_DONE_ENV: &str = "CODEHANGAR_TEST_SAFE05_DONE";

        fn wait_for_fixture_path(path: &Path, label: &str) {
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            while !path.exists() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "timed out waiting for {label}: {}",
                    path.display()
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        fn run_safe05_guardian_probe() {
            let pipe_name = std::env::var(SAFE05_PIPE_ENV).unwrap();
            let nonce = std::env::var(SAFE05_NONCE_ENV).unwrap();
            let parent_pid = std::env::var(SAFE05_PARENT_PID_ENV)
                .unwrap()
                .parse::<u32>()
                .unwrap();
            let done = PathBuf::from(std::env::var_os(SAFE05_DONE_ENV).unwrap());
            let cli = GuardianCli {
                pipe_name,
                parent_pid,
                nonce,
            };
            let expected_image = std::env::current_exe().unwrap();
            let result = guardian_exchange_with_policy(
                &cli,
                IdentityPolicy::SyntheticFixture { expected_image },
            );
            std::fs::write(&done, format!("{result:?}")).unwrap();
        }

        fn run_safe05_crashing_parent_probe() -> ! {
            let root = PathBuf::from(std::env::var_os(SAFE05_ROOT_ENV).unwrap());
            let scenario = std::env::var(SAFE05_SCENARIO_ENV)
                .unwrap_or_else(|_| "prove-reply-failure".to_string());
            let source = root.join("held.bin");
            let receipt_path = root.join("guardian-receipt.bin");
            let ready_path = root.join("prove-consumed.ready");
            let release_path = root.join("parent-exited.release");
            let prearm_ready_path = root.join("prearm-retaining.ready");
            let prearm_release_path = root.join("prearm-armed.release");
            let done_path = root.join("guardian-cancelled.done");

            let (stamp, hash) = crate::bound_fs::inspect_local_mutation_file(&source).unwrap();
            let proof =
                crate::bound_fs::BoundObjectProof::open_for_archive_delete(&source, &stamp, &hash)
                    .unwrap()
                    .detach_exclusive_for_final_disposition()
                    .unwrap();
            proof.validate_final_disposition_prearm().unwrap();
            let receipt = crate::bound_fs::BoundGuardianReceipt::create_new(&receipt_path).unwrap();

            let parent = current_parent_binding().unwrap();
            let nonce = random_hex_256().unwrap();
            let operation_uuid = "91".repeat(16);
            let pipe_name = guardian_pipe_name(&nonce);
            let pipe = PipeServer::create(&pipe_name).unwrap();
            let binary = std::env::current_exe().unwrap();
            let mut command = Command::new(binary);
            command
                .args([
                    "--exact",
                    SAFE05_CHILD_PROBE,
                    "--ignored",
                    "--test-threads=1",
                ])
                .env(SAFE05_ROLE_ENV, "guardian")
                .env(SAFE05_PIPE_ENV, &pipe_name)
                .env(SAFE05_NONCE_ENV, &nonce)
                .env(SAFE05_PARENT_PID_ENV, parent.pid.to_string())
                .env(SAFE05_DONE_ENV, &done_path)
                .env("CODEHANGAR_TEST_GUARDIAN_PROVE_READY", &ready_path)
                .env("CODEHANGAR_TEST_GUARDIAN_PROVE_RELEASE", &release_path)
                .env("CODEHANGAR_TEST_GUARDIAN_PREARM_READY", &prearm_ready_path)
                .env(
                    "CODEHANGAR_TEST_GUARDIAN_PREARM_RELEASE",
                    &prearm_release_path,
                )
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                // This is a separate process, but remains in the test runner's
                // enclosing job so CI sandboxes that forbid breakaway can run
                // the crash interval. Production launch separately requires
                // CREATE_BREAKAWAY_FROM_JOB before any arm is possible.
                .creation_flags(CREATE_NO_WINDOW);
            let guardian = command.spawn().unwrap();
            pipe.connect(Some(guardian.as_raw_handle() as HANDLE))
                .unwrap();

            let hello = pipe.read_exact_message(HELLO_MAGIC.len() + 64).unwrap();
            validate_hello(&hello, &nonce, &[0u8; 32]).unwrap();
            let key = random_key().unwrap();
            pipe.write_message(&key_message(&nonce, &[0u8; 32], &key).unwrap())
                .unwrap();
            let bind = GuardianCommand::Bind {
                schema: GUARDIAN_SCHEMA.to_string(),
                operation_id: 11,
                batch_item_id: 7,
                parent_handle_value: proof.raw_handle_value(),
                expected_stamp: stamp,
                receipt_handle_value: receipt.raw_handle_value(),
                expected_receipt_stamp: receipt.initial_stamp().clone(),
                receipt_key: GuardianReceiptKey(random_key().unwrap()),
            };
            let reply: GuardianReply =
                parent_roundtrip(&pipe, &key, &operation_uuid, &nonce, 1, &bind).unwrap();
            assert!(matches!(reply, GuardianReply::HandleBound { .. }));
            let arm = GuardianCommand::ArmAuthorized {
                schema: GUARDIAN_SCHEMA.to_string(),
                batch_item_id: 7,
            };
            let reply: GuardianReply =
                parent_roundtrip(&pipe, &key, &operation_uuid, &nonce, 3, &arm).unwrap();
            assert!(matches!(reply, GuardianReply::ArmReady { .. }));

            if scenario == "disconnect-before-arm" {
                // ArmReady was fully delivered, but the transport disappears
                // before ProveArmed. The guardian must not mistake the current
                // DeletePending=false state for terminal cancellation while
                // this exact parent is still alive and able to perform its
                // already-authorized arm.
                drop(pipe);
                wait_for_fixture_path(&prearm_ready_path, "guardian pre-arm retention checkpoint");
                let mode = proof.arm_final_disposition().unwrap();
                proof.validate_armed_final_disposition(mode).unwrap();
                std::fs::write(&prearm_release_path, b"parent armed after disconnect").unwrap();
                let _keep_resources_live = (proof, receipt, guardian);
                std::process::exit(78)
            }

            let mode = proof.arm_final_disposition().unwrap();
            proof.validate_armed_final_disposition(mode).unwrap();
            let prove = GuardianCommand::ProveArmed {
                schema: GUARDIAN_SCHEMA.to_string(),
                batch_item_id: 7,
                mode,
            };
            let payload = serde_json::to_vec(&prove).unwrap();
            let context = FrameContext {
                role: FrameRole::ParentRequest,
                sequence: 5,
                operation_uuid,
                nonce,
                request_blake3: blake3::hash(&payload).to_hex().to_string(),
            };
            let frame = encode_authenticated(&prove, &key, &context).unwrap();
            pipe.write_message(&frame).unwrap();

            // The separate guardian has consumed and validated ProveArmed but
            // is deliberately paused before its reply. Exit without running
            // destructors: the parent handle and pipe vanish abruptly while
            // the guardian is the sole armed holder.
            wait_for_fixture_path(&ready_path, "guardian ProveArmed checkpoint");
            let _keep_resources_live = (proof, receipt, pipe, guardian);
            std::process::exit(77)
        }

        #[test]
        #[ignore]
        fn guardian_crash_after_prove_child_probe() {
            match std::env::var(SAFE05_ROLE_ENV).as_deref() {
                Ok("guardian") => run_safe05_guardian_probe(),
                Ok("parent") => run_safe05_crashing_parent_probe(),
                _ => {}
            }
        }

        #[test]
        fn guardian_reply_failure_after_prove_armed_parent_crash_preserves_bytes() {
            let root = tempfile::tempdir().unwrap();
            let source = root.path().join("held.bin");
            let receipt = root.path().join("guardian-receipt.bin");
            let release = root.path().join("parent-exited.release");
            let done = root.path().join("guardian-cancelled.done");
            std::fs::write(&source, b"SAFE-05 bytes must survive").unwrap();

            let binary = std::env::current_exe().unwrap();
            let status = Command::new(binary)
                .args([
                    "--exact",
                    SAFE05_CHILD_PROBE,
                    "--ignored",
                    "--test-threads=1",
                ])
                .env(SAFE05_ROLE_ENV, "parent")
                .env(SAFE05_SCENARIO_ENV, "prove-reply-failure")
                .env(SAFE05_ROOT_ENV, root.path())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();
            assert_eq!(
                status.code(),
                Some(77),
                "parent fixture did not crash at the checkpoint",
            );

            // Let the already-independent guardian attempt the now-broken
            // ProveArmed reply. The armed-handle Drop guard must cancel and
            // prove DeletePending=false before the guardian process can exit.
            std::fs::write(&release, b"parent exit observed").unwrap();
            wait_for_fixture_path(&done, "guardian cancellation completion");
            let guardian_result = std::fs::read_to_string(&done).unwrap();
            assert!(
                guardian_result.starts_with("Err("),
                "the guardian fixture did not exercise the broken reply path: {guardian_result}"
            );

            assert_eq!(
                std::fs::read(&source).unwrap(),
                b"SAFE-05 bytes must survive"
            );
            assert!(
                receipt.exists(),
                "the crash-side receipt identity must remain recoverable"
            );
            assert!(
                std::fs::read(&receipt).unwrap().is_empty(),
                "no durable close receipt may exist before CloseAuthorized"
            );
        }

        #[test]
        fn guardian_disconnect_after_arm_ready_retains_through_late_arm_and_parent_crash() {
            let root = tempfile::tempdir().unwrap();
            let source = root.path().join("held.bin");
            let receipt = root.path().join("guardian-receipt.bin");
            let done = root.path().join("guardian-cancelled.done");
            std::fs::write(&source, b"SAFE-05 late-arm bytes must survive").unwrap();

            let binary = std::env::current_exe().unwrap();
            let status = Command::new(binary)
                .args([
                    "--exact",
                    SAFE05_CHILD_PROBE,
                    "--ignored",
                    "--test-threads=1",
                ])
                .env(SAFE05_ROLE_ENV, "parent")
                .env(SAFE05_SCENARIO_ENV, "disconnect-before-arm")
                .env(SAFE05_ROOT_ENV, root.path())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();
            assert_eq!(
                status.code(),
                Some(78),
                "parent fixture did not arm after the pre-arm disconnect",
            );

            wait_for_fixture_path(&done, "guardian late-arm cancellation completion");
            let guardian_result = std::fs::read_to_string(&done).unwrap();
            assert!(
                guardian_result.starts_with("Ok("),
                "the guardian did not close the disconnected parent path safely: {guardian_result}"
            );
            assert_eq!(
                std::fs::read(&source).unwrap(),
                b"SAFE-05 late-arm bytes must survive"
            );
            assert!(
                receipt.exists() && std::fs::read(&receipt).unwrap().is_empty(),
                "pre-CloseAuthorized recovery authority must remain an empty exact sidecar"
            );
        }

        #[test]
        fn transport_uses_the_journal_persisted_nonce_verbatim() {
            let root = tempfile::tempdir().unwrap();
            let parent = ParentBinding {
                pid: 42,
                session_id: 7,
                process_started_100ns: 99,
                image_sha256: "12".repeat(32),
            };
            let mut request = synthetic_request(root.path(), parent);
            request.nonce = "5a".repeat(32);
            assert_eq!(validated_persisted_nonce(&request).unwrap(), request.nonce);

            request.nonce = "not-persistable".to_string();
            assert!(validated_persisted_nonce(&request).is_err());
        }

        #[test]
        fn resident_handle_budget_is_exact_and_refused_before_launch() {
            assert!(validate_resident_batch_capacity(0).is_err());
            assert!(validate_resident_batch_capacity(
                super::super::MAX_RESIDENT_CAPABILITIES_PER_INVOCATION
            )
            .is_ok());
            assert!(validate_resident_batch_capacity(
                super::super::MAX_RESIDENT_CAPABILITIES_PER_INVOCATION + 1
            )
            .is_err());
        }

        #[test]
        fn stream_commitment_ignores_only_process_local_handles() {
            let root = tempfile::tempdir().unwrap();
            let mut request = synthetic_request(root.path(), current_parent_binding().unwrap());
            assign_dynamic_capability_fields(&mut request);
            let original = capability_chunk_digest(0, &request.capabilities).unwrap();

            let mut other_handles = request.capabilities.clone();
            if let ElevatedCapability::ObjectBackupV2 {
                source,
                parent_archive_handle_value,
                scratch_root,
                ..
            } = &mut other_handles[0]
            {
                source.parent_handle_value = 0x1111;
                *parent_archive_handle_value = 0x2222;
                scratch_root.parent_handle_value = 0x3333;
            }
            assert_eq!(
                capability_chunk_digest(0, &other_handles).unwrap(),
                original
            );

            if let ElevatedCapability::ObjectBackupV2 { source, .. } = &mut other_handles[0] {
                source.stamp.file_id = "different-object".to_string();
            }
            assert_ne!(
                capability_chunk_digest(0, &other_handles).unwrap(),
                original
            );

            let mut redirected_archive = request.capabilities.clone();
            if let ElevatedCapability::ObjectBackupV2 { archive_path, .. } =
                &mut redirected_archive[0]
            {
                archive_path.set_file_name("redirected.partial");
            }
            assert_ne!(
                capability_chunk_digest(0, &redirected_archive).unwrap(),
                original
            );
        }

        #[test]
        fn post_sign_manifest_is_non_circular_and_detects_byte_changes() {
            let root = tempfile::tempdir().unwrap();
            let parent_path = root.path().join("CodeHangar.exe");
            let helper_path = root.path().join("code-hangar-elevated.exe");
            std::fs::write(&parent_path, b"fixed-final-signed-parent-bytes").unwrap();
            std::fs::write(&helper_path, b"fixed-final-signed-helper-bytes").unwrap();
            let parent_sha256 = hash_sha256(&open_image_file(&parent_path).unwrap()).unwrap();
            let helper_sha256 = hash_sha256(&open_image_file(&helper_path).unwrap()).unwrap();
            let mut manifest = ReleaseManifest {
                schema: RELEASE_MANIFEST_SCHEMA.to_string(),
                release_id: "12".repeat(32),
                parent: ReleaseImageEntry {
                    file_name: "CodeHangar.exe".to_string(),
                    sha256: parent_sha256,
                },
                helper: ReleaseImageEntry {
                    file_name: "code-hangar-elevated.exe".to_string(),
                    sha256: helper_sha256,
                },
                signature_rsa_pss_sha256: String::new(),
            };
            let public_blob = sign_test_manifest(&mut manifest).unwrap();
            validate_manifest_fields(&manifest).unwrap();
            verify_manifest_signature(&manifest, &public_blob).unwrap();

            // Packaging signs the manifest only after the executable bytes are
            // final. Altering either image hash afterwards invalidates the pin
            // without changing or rebuilding either executable.
            manifest.helper.sha256 = "34".repeat(32);
            assert!(verify_manifest_signature(&manifest, &public_blob).is_err());
        }

        #[test]
        fn stale_stream_header_is_rejected_before_privilege_gate() {
            let root = tempfile::tempdir().unwrap();
            let parent_binding = current_parent_binding().unwrap();
            let parent = inspect_process(unsafe { GetCurrentProcessId() }, None).unwrap();
            let mut request = synthetic_request(root.path(), parent_binding);
            assign_dynamic_capability_fields(&mut request);
            let operation_uuid = "12".repeat(16);
            let (mut begin, _) = plan_stream(&request, &operation_uuid).unwrap();
            let now = now_unix_seconds().unwrap();
            begin.header.issued_at_unix_seconds = now - 120;
            begin.header.expires_at_unix_seconds = now - 1;
            begin.batch_blake3 = stream_batch_digest(
                &operation_uuid,
                &begin.header,
                begin.total_capabilities as usize,
                &begin.chunks,
            )
            .unwrap();
            let context = FrameContext {
                role: FrameRole::ParentRequest,
                sequence: 1,
                operation_uuid,
                nonce: begin.header.nonce.clone(),
                request_blake3: "34".repeat(32),
            };
            assert!(
                validate_stream_begin(&begin, &context, &parent, &request.nonce, now,).is_err()
            );
        }

        #[test]
        fn malformed_stream_header_is_rejected_before_privilege_gate() {
            let root = tempfile::tempdir().unwrap();
            let parent_binding = current_parent_binding().unwrap();
            let parent = inspect_process(unsafe { GetCurrentProcessId() }, None).unwrap();
            let mut request = synthetic_request(root.path(), parent_binding);
            assign_dynamic_capability_fields(&mut request);
            let operation_uuid = "12".repeat(16);
            let (begin, _) = plan_stream(&request, &operation_uuid).unwrap();
            let now = now_unix_seconds().unwrap();
            let reject = |mut candidate: StreamBegin| {
                candidate.batch_blake3 = stream_batch_digest(
                    &candidate.operation_uuid,
                    &candidate.header,
                    candidate.total_capabilities as usize,
                    &candidate.chunks,
                )
                .unwrap();
                let context = FrameContext {
                    role: FrameRole::ParentRequest,
                    sequence: 1,
                    operation_uuid: candidate.operation_uuid.clone(),
                    nonce: request.nonce.clone(),
                    request_blake3: "34".repeat(32),
                };
                assert!(
                    validate_stream_begin(&candidate, &context, &parent, &request.nonce, now,)
                        .is_err()
                );
            };

            let mut no_operation = begin.clone();
            no_operation.header.operation_id = 0;
            reject(no_operation);
            let mut wrong_plan = begin.clone();
            wrong_plan.header.plan_fingerprint = format!("v1:{}", "cd".repeat(32));
            reject(wrong_plan);
            let mut bad_journal = begin.clone();
            bad_journal.header.journal_capability_blake3 = "not-a-digest".to_string();
            reject(bad_journal);
            let mut wrong_nonce = begin;
            wrong_nonce.header.nonce = "56".repeat(32);
            reject(wrong_nonce);
        }

        #[test]
        fn remacced_chunk_cannot_change_begin_operation_uuid() {
            let root = tempfile::tempdir().unwrap();
            let mut request = synthetic_request(root.path(), current_parent_binding().unwrap());
            assign_dynamic_capability_fields(&mut request);
            let begin_uuid = "12".repeat(16);
            let (begin, planned) = plan_stream(&request, &begin_uuid).unwrap();
            let chunk = StreamCommand::Chunk(StreamChunk {
                schema: STREAM_SCHEMA.to_string(),
                batch_blake3: begin.batch_blake3,
                chunk_index: 0,
                start_index: 0,
                capabilities: request.capabilities[planned[0].start..planned[0].end].to_vec(),
            });
            let key = [7u8; 32];
            let payload = serde_json::to_vec(&chunk).unwrap();
            let altered = FrameContext {
                role: FrameRole::ParentRequest,
                sequence: 3,
                operation_uuid: "34".repeat(16),
                nonce: request.nonce.clone(),
                request_blake3: blake3::hash(&payload).to_hex().to_string(),
            };
            let frame = encode_authenticated(&chunk, &key, &altered).unwrap();
            let parsed = parent_context_from_frame(&frame, &request.nonce, 3).unwrap();
            let _: StreamCommand = decode_authenticated(&frame, &key, &parsed).unwrap();
            assert!(require_stream_operation_uuid(&parsed, Some(&begin_uuid)).is_err());
        }

        #[test]
        fn stream_stop_must_close_an_exact_committed_prefix() {
            let root = tempfile::tempdir().unwrap();
            let mut request = synthetic_request(root.path(), current_parent_binding().unwrap());
            assign_dynamic_capability_fields(&mut request);
            let (begin, planned) = plan_stream(&request, &"12".repeat(16)).unwrap();
            let before_first = StreamStop {
                schema: STREAM_SCHEMA.to_string(),
                batch_blake3: begin.batch_blake3.clone(),
                processed_capabilities: 0,
                processed_chunks: 0,
            };
            validate_stream_stop(&begin, &before_first, 0, 0).unwrap();

            let after_first = StreamStop {
                schema: STREAM_SCHEMA.to_string(),
                batch_blake3: begin.batch_blake3.clone(),
                processed_capabilities: planned[0].end as u32,
                processed_chunks: 1,
            };
            validate_stream_stop(&begin, &after_first, planned[0].end, 1).unwrap();

            let mut forged = after_first;
            forged.processed_capabilities = forged.processed_capabilities.saturating_sub(1);
            assert!(validate_stream_stop(&begin, &forged, planned[0].end, 1).is_err());
        }

        #[test]
        fn synthetic_named_pipe_is_one_shot_and_never_claims_unproven_success() {
            let root = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(root.path().join("backup/scratch")).unwrap();
            let parent = current_parent_binding().unwrap();
            let mut request = synthetic_request(root.path(), parent.clone());
            let nonce = random_hex_256().unwrap();
            request.nonce.clone_from(&nonce);
            assign_dynamic_capability_fields(&mut request);
            let operation_uuid = "12".repeat(16);
            let (begin, planned) = plan_stream(&request, &operation_uuid).unwrap();
            let pipe_name = pipe_name(&nonce);
            let pipe = PipeServer::create(&pipe_name).unwrap();
            let image = std::env::current_exe().unwrap();
            let helper_image = image.clone();
            let helper_pipe = pipe_name.clone();
            let helper_nonce = nonce.clone();
            let synthetic_root = root.path().to_path_buf();
            let thread = std::thread::spawn(move || {
                helper_exchange(
                    &helper_pipe,
                    parent.pid,
                    &helper_nonce,
                    IdentityPolicy::SyntheticFixture {
                        expected_image: helper_image,
                    },
                    Some(&synthetic_root),
                )
            });
            pipe.connect(None).unwrap();
            let current = inspect_process(unsafe { GetCurrentProcessId() }, None).unwrap();
            let mut client_pid = 0;
            assert_ne!(
                unsafe { GetNamedPipeClientProcessId(pipe.handle.raw(), &mut client_pid) },
                0
            );
            assert_eq!(client_pid, current.pid);
            let manifest_digest = [0u8; 32];
            let hello = pipe.read_exact_message(72).unwrap();
            validate_hello(&hello, &nonce, &manifest_digest).unwrap();
            let key = random_key().unwrap();
            pipe.write_message(&key_message(&nonce, &manifest_digest, &key).unwrap())
                .unwrap();
            let reply: StreamReply =
                parent_roundtrip(&pipe, &key, &operation_uuid, &nonce, 1, &begin).unwrap();
            if let StreamReply::BeginReady { .. } = reply {
                let chunk = StreamCommand::Chunk(StreamChunk {
                    schema: STREAM_SCHEMA.to_string(),
                    batch_blake3: begin.batch_blake3.clone(),
                    chunk_index: 0,
                    start_index: 0,
                    capabilities: request.capabilities[planned[0].start..planned[0].end].to_vec(),
                });
                let reply: StreamReply =
                    parent_roundtrip(&pipe, &key, &operation_uuid, &nonce, 3, &chunk).unwrap();
                assert!(matches!(
                    reply,
                    StreamReply::ChunkResults { ref items, .. }
                        if matches!(items[0], ElevatedItemResult::Blocked { .. })
                ));
                let end = StreamCommand::End(StreamEnd {
                    schema: STREAM_SCHEMA.to_string(),
                    batch_blake3: begin.batch_blake3.clone(),
                    total_capabilities: 1,
                    chunk_count: 1,
                });
                let reply: StreamReply =
                    parent_roundtrip(&pipe, &key, &operation_uuid, &nonce, 5, &end).unwrap();
                assert!(matches!(reply, StreamReply::EndReady { .. }));
            } else {
                assert!(matches!(
                    reply,
                    StreamReply::Failure(ref failure)
                        if failure.code == "privilege_proof_failed"
                ));
            }
            unsafe { DisconnectNamedPipe(pipe.handle.raw()) };
            assert!(thread.join().unwrap().is_ok());
        }

        #[test]
        fn synthetic_named_pipe_accepts_authenticated_early_stop() {
            let root = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(root.path().join("backup/scratch")).unwrap();
            let parent = current_parent_binding().unwrap();
            let mut request = synthetic_request(root.path(), parent.clone());
            let nonce = random_hex_256().unwrap();
            request.nonce.clone_from(&nonce);
            assign_dynamic_capability_fields(&mut request);
            let operation_uuid = "12".repeat(16);
            let (begin, _) = plan_stream(&request, &operation_uuid).unwrap();
            let pipe_name = pipe_name(&nonce);
            let pipe = PipeServer::create(&pipe_name).unwrap();
            let helper_image = std::env::current_exe().unwrap();
            let helper_pipe = pipe_name.clone();
            let helper_nonce = nonce.clone();
            let synthetic_root = root.path().to_path_buf();
            let thread = std::thread::spawn(move || {
                helper_exchange(
                    &helper_pipe,
                    parent.pid,
                    &helper_nonce,
                    IdentityPolicy::SyntheticFixture {
                        expected_image: helper_image,
                    },
                    Some(&synthetic_root),
                )
            });
            pipe.connect(None).unwrap();
            let manifest_digest = [0u8; 32];
            let hello = pipe.read_exact_message(72).unwrap();
            validate_hello(&hello, &nonce, &manifest_digest).unwrap();
            let key = random_key().unwrap();
            pipe.write_message(&key_message(&nonce, &manifest_digest, &key).unwrap())
                .unwrap();
            let reply: StreamReply =
                parent_roundtrip(&pipe, &key, &operation_uuid, &nonce, 1, &begin).unwrap();
            if let StreamReply::BeginReady { .. } = reply {
                let stop = StreamCommand::Stop(StreamStop {
                    schema: STREAM_SCHEMA.to_string(),
                    batch_blake3: begin.batch_blake3.clone(),
                    processed_capabilities: 0,
                    processed_chunks: 0,
                });
                let reply: StreamReply =
                    parent_roundtrip(&pipe, &key, &operation_uuid, &nonce, 3, &stop).unwrap();
                assert!(matches!(
                    reply,
                    StreamReply::StopReady {
                        processed_capabilities: 0,
                        processed_chunks: 0,
                        ..
                    }
                ));
            } else {
                assert!(matches!(
                    reply,
                    StreamReply::Failure(ref failure)
                        if failure.code == "privilege_proof_failed"
                ));
            }
            unsafe { DisconnectNamedPipe(pipe.handle.raw()) };
            assert!(thread.join().unwrap().is_ok());
        }
    }
}
