//! Safe, per-host registration of the `code-hangar` connected-AI-app (MCP) server
//! into each AI app's configuration file.
//!
//! Hosts and formats (verified June 2026):
//! - **Claude** — `~/.claude.json`, JSON, top-level `mcpServers` (global scope).
//! - **Cursor** — `~/.cursor/mcp.json`, JSON, top-level `mcpServers`.
//! - **Codex** — `~/.codex/config.toml`, TOML, `[mcp_servers.code-hangar]`.
//!
//! Every mutating operation follows the same safe pipeline:
//!   1. Parse the existing config. An UNPARSEABLE config aborts that host — it is
//!      never overwritten.
//!   2. Before registration, back up the original file to
//!      `<config><.codehangar.bak>` and verify the copy by re-reading it.
//!   3. Round-trip merge ONLY our `code-hangar` entry, preserving every other key
//!      and (JSON) key order / (TOML) formatting and comments.
//!   4. Write to a sibling temp file, `fsync`, atomically rename over the original,
//!      then re-read and verify the entry is present (register) or gone (unregister).
//!   5. A hash-only state sidecar binds the registered bytes to the original backup.
//!      If the host did not edit its config, unregister restores the original bytes
//!      exactly. If it did, only our entry is removed from the current document.
//!
//! The token and database path live in the host config's `env` in plaintext, so the
//! token is a same-Windows-user secret (documented). Each host gets its own token;
//! revoking removes both the DB credential and this config entry.

use std::ffi::OsString;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use toml_edit::{Array, DocumentMut, Item, Table};

/// The single config key we own across every host. Also a valid TOML bare key.
const SERVER_KEY: &str = "code-hangar";
/// The connected-app server executable's filename. Defined here (a guardrail-exempt
/// crate) so callers can reference it without embedding the literal name elsewhere.
pub const SERVER_EXE_NAME: &str = "code-hangar-mcp.exe";
const BACKUP_SUFFIX: &str = ".codehangar.bak";
const STATE_SUFFIX: &str = ".codehangar.state";
const PENDING_SUFFIX: &str = ".codehangar.pending";
const STATE_SCHEMA_VERSION: u8 = 2;

/// Cross-process, per-Windows-profile/host lease. The open file handle is
/// exclusive (share mode 0), so a second desktop/server process cannot enter a
/// config transaction until this owner drops the guard. The file body is only
/// diagnostic owner metadata; the kernel handle, not its contents, is the lock.
pub struct HostOperationLock {
    #[allow(dead_code)]
    file: fs::File,
    path: PathBuf,
    owner_id: String,
}

impl HostOperationLock {
    pub fn owner_id(&self) -> &str {
        &self.owner_id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(windows)]
struct BoundAncestor {
    path: PathBuf,
    file: fs::File,
    volume_serial: u32,
    file_index: u64,
}

#[cfg(not(windows))]
struct BoundAncestor {
    path: PathBuf,
    file: fs::File,
}

/// Binds every existing directory ancestor by handle for the full
/// prepare/apply/DB-commit/finalize lifetime. On Windows each handle denies
/// FILE_SHARE_DELETE, preventing a junction/directory swap underneath the
/// operation. The identities are revalidated before every phase.
struct AncestorGuard {
    target: PathBuf,
    bound: Mutex<Option<Vec<BoundAncestor>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistrationState {
    schema_version: u8,
    host: String,
    config_path_hash: String,
    agent_identity_id: String,
    owned_entry_hash: String,
    original_exists: bool,
    original_hash: Option<String>,
    registered_hash: String,
    auth_tag: String,
}

/// Secret-backed binding between the encrypted Code Hangar inventory and one
/// external host config. The 32-byte key must come from SQLCipher-protected
/// state, never from the host config or its plaintext MCP token.
#[derive(Clone)]
pub struct RegistrationBinding {
    agent_identity_id: String,
    auth_key: [u8; 32],
}

impl RegistrationBinding {
    pub fn from_hex(agent_identity_id: &str, auth_key_hex: &str) -> Result<Self, String> {
        let identity = agent_identity_id.trim().to_ascii_lowercase();
        if identity.len() != 32 || !identity.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("The connected-app immutable identity is invalid.".to_string());
        }
        let decoded = decode_hex_32(auth_key_hex)
            .ok_or_else(|| "The connected-app state authentication key is invalid.".to_string())?;
        Ok(Self {
            agent_identity_id: identity,
            auth_key: decoded,
        })
    }

    pub fn agent_identity_id(&self) -> &str {
        &self.agent_identity_id
    }
}

/// An AI app whose config Code Hangar can register itself into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Host {
    Claude,
    Cursor,
    Codex,
}

#[derive(Clone, Copy)]
enum Format {
    Json,
    Toml,
}

impl Host {
    /// The big-three hosts, in presentation order.
    pub const ALL: [Host; 3] = [Host::Claude, Host::Cursor, Host::Codex];

    pub fn id(self) -> &'static str {
        match self {
            Host::Claude => "claude",
            Host::Cursor => "cursor",
            Host::Codex => "codex",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Host::Claude => "Claude",
            Host::Cursor => "Cursor",
            Host::Codex => "Codex",
        }
    }

    pub fn from_id(id: &str) -> Option<Host> {
        Host::ALL.into_iter().find(|host| host.id() == id)
    }

    fn format(self) -> Format {
        match self {
            Host::Codex => Format::Toml,
            _ => Format::Json,
        }
    }

    /// Config path segments relative to the user's home directory.
    fn relative_segments(self) -> &'static [&'static str] {
        match self {
            Host::Claude => &[".claude.json"],
            Host::Cursor => &[".cursor", "mcp.json"],
            Host::Codex => &[".codex", "config.toml"],
        }
    }
}

/// How to launch the server: the absolute exe path, any args, the ordered env
/// (token + db path), and the cold-start timeout Codex needs for the SQLCipher open.
#[derive(Debug, Clone)]
pub struct ServerSpec {
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub startup_timeout_sec: u64,
}

/// What we can see about one host's config without modifying it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStatus {
    pub host: String,
    pub label: String,
    pub config_path: String,
    /// The config file is present on disk.
    pub config_exists: bool,
    /// The config is absent (we can create it) or parses cleanly. `false` means it
    /// exists but is malformed, so we refuse to touch it.
    pub readable: bool,
    /// Our `code-hangar` entry is present.
    pub registered: bool,
    /// Scopes that are effective for the credential currently present in this
    /// host's config. Populated by `hangar-api`; appconfig itself has no DB access.
    #[serde(default)]
    pub effective_scopes: Vec<String>,
    /// Project ids that are effective for the credential currently present in
    /// this host's config. An empty list is never interpreted as every project.
    #[serde(default)]
    pub effective_project_ids: Vec<i64>,
    /// True only when the configured token hash resolves to one enabled DB agent.
    #[serde(default)]
    pub credential_active: bool,
    /// A cross-store change exists but could not be reconciled safely. No config
    /// bytes are overwritten while this is true.
    #[serde(default)]
    pub recovery_required: bool,
    /// Durable DB identity for this host, populated by hangar-api even when the
    /// external config is missing, malformed or no longer contains our entry.
    #[serde(default)]
    pub durable_agent_id: Option<i64>,
    #[serde(default)]
    pub durable_identity_id: Option<String>,
    #[serde(default)]
    pub durable_credential_enabled: bool,
    /// True when a DB credential remains but cannot be proven effective from the
    /// current external config. It is always revocable/forgettable DB-only.
    #[serde(default)]
    pub credential_orphaned: bool,
    #[serde(default)]
    pub orphan_reason: Option<String>,
}

/// Hash-only description of one file. It is safe to persist in the encrypted
/// cross-store journal: it contains neither config bytes nor an MCP token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFingerprint {
    pub exists: bool,
    pub hash: Option<String>,
}

/// Hash-only contract for the config and sidecar transition performed by one
/// prepared registration/removal. `hangar-api` persists this beside the pending
/// DB credential change, never the plaintext config or token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeFingerprints {
    pub config_before: FileFingerprint,
    pub config_after: FileFingerprint,
    pub backup_before: FileFingerprint,
    pub backup_after: FileFingerprint,
    pub backup_changed: bool,
    pub state_before: FileFingerprint,
    pub state_after: FileFingerprint,
    pub state_changed: bool,
}

#[derive(Clone)]
struct FileSnapshot {
    fingerprint: FileFingerprint,
    bytes: Option<Vec<u8>>,
}

struct ChangeSnapshots {
    config: FileSnapshot,
    backup: FileSnapshot,
    state: FileSnapshot,
}

#[derive(Clone)]
enum SidecarAction {
    Preserve,
    Replace(Vec<u8>),
    Remove,
}

/// Opaque in-memory plan. It intentionally does not implement `Debug`: the
/// before/after config snapshots can contain the old/new plaintext MCP token.
pub struct PreparedChange {
    host: Host,
    home: PathBuf,
    config: FileSnapshot,
    backup: FileSnapshot,
    state: FileSnapshot,
    config_after: Option<Vec<u8>>,
    backup_action: SidecarAction,
    state_action: SidecarAction,
    fingerprints: ChangeFingerprints,
    ancestry: AncestorGuard,
}

impl PreparedChange {
    pub fn fingerprints(&self) -> &ChangeFingerprints {
        &self.fingerprints
    }

    /// True only when a failed `apply` left neither this operation's after
    /// image nor any sidecar staging behind. The caller may then delete its
    /// still-prepared DB journal even if another process changed the config in
    /// the meantime; that third-party image is never overwritten.
    pub fn can_abort_after_failed_apply(&self) -> Result<bool, String> {
        self.ancestry.verify_if_bound()?;
        let path = host_config_path(self.host, &self.home);
        if current_fingerprint(&path)? == self.fingerprints.config_after {
            return Ok(false);
        }
        for (managed, before) in [
            (
                sidecar_path(&path, BACKUP_SUFFIX),
                &self.fingerprints.backup_before,
            ),
            (
                sidecar_path(&path, STATE_SUFFIX),
                &self.fingerprints.state_before,
            ),
        ] {
            if current_fingerprint(&managed)? != *before
                || current_fingerprint(&pending_path(&managed))?.exists
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Apply the filesystem half after the DB journal row is durable. Every
    /// input is compare-and-swapped against the prepared fingerprints. On a
    /// normal error we restore the exact in-memory snapshots when still safe.
    pub fn apply(&self) -> Result<(), String> {
        if let Err(error) = self.apply_inner() {
            let rollback = self.rollback();
            return match rollback {
                Ok(()) => Err(error),
                Err(_) => Err(format!(
                    "{error} The connected-app change also needs recovery; no credential was committed."
                )),
            };
        }
        Ok(())
    }

    /// Restore the exact prior config and sidecars, but only while every path is
    /// still either the expected before or expected after image. Host edits are
    /// never overwritten to make rollback look successful.
    pub fn rollback(&self) -> Result<(), String> {
        self.ancestry.bind_or_verify()?;
        restore_config_snapshot(
            &host_config_path(self.host, &self.home),
            &self.config,
            &self.fingerprints.config_after,
        )?;
        rollback_sidecar(
            &sidecar_path(&host_config_path(self.host, &self.home), BACKUP_SUFFIX),
            &self.backup,
            &self.fingerprints.backup_after,
            self.fingerprints.backup_changed,
        )?;
        rollback_sidecar(
            &sidecar_path(&host_config_path(self.host, &self.home), STATE_SUFFIX),
            &self.state,
            &self.fingerprints.state_after,
            self.fingerprints.state_changed,
        )?;
        Ok(())
    }

    /// Remove the rollback sidecars after the DB commit. Idempotent: a crash
    /// after DB commit and before this call is completed is reconciled on the
    /// next status/register/remove operation.
    pub fn finalize(&self) -> Result<(), String> {
        self.ancestry.bind_or_verify()?;
        finalize_sidecar(
            &sidecar_path(&host_config_path(self.host, &self.home), BACKUP_SUFFIX),
            &self.backup,
            &self.fingerprints.backup_after,
            self.fingerprints.backup_changed,
        )?;
        finalize_sidecar(
            &sidecar_path(&host_config_path(self.host, &self.home), STATE_SUFFIX),
            &self.state,
            &self.fingerprints.state_after,
            self.fingerprints.state_changed,
        )?;
        Ok(())
    }

    fn apply_inner(&self) -> Result<(), String> {
        self.ancestry.bind_or_verify()?;
        let path = host_config_path(self.host, &self.home);
        assert_snapshot(&path, &self.config)?;
        apply_sidecar(
            &sidecar_path(&path, BACKUP_SUFFIX),
            &self.backup,
            &self.backup_action,
        )?;
        apply_sidecar(
            &sidecar_path(&path, STATE_SUFFIX),
            &self.state,
            &self.state_action,
        )?;
        assert_snapshot(&path, &self.config)?;
        write_optional_bytes(&path, self.config_after.as_deref())?;
        let after = read_snapshot(&path)?;
        if after.fingerprint != self.fingerprints.config_after {
            return Err("the connected-app config could not be verified after writing".to_string());
        }
        let inspection = inspect(self.host, &self.home);
        let should_be_registered =
            self.config_after.is_some() && configured_token_hash(self.host, &self.home)?.is_some();
        if !inspection.status.readable || inspection.status.registered != should_be_registered {
            return Err("the connected-app config could not be verified after writing".to_string());
        }
        Ok(())
    }
}

/// The absolute config path for a host under the given home directory.
pub fn host_config_path(host: Host, home: &Path) -> PathBuf {
    let mut path = home.to_path_buf();
    for segment in host.relative_segments() {
        path.push(segment);
    }
    path
}

/// Acquire the durable kernel lease for one profile/host transaction. Callers
/// must keep this guard alive across appconfig preparation, filesystem apply,
/// the SQLite CAS commit/compensation, and sidecar finalization.
#[cfg(windows)]
pub fn acquire_host_operation_lock(
    host: Host,
    home: &Path,
    owner_hint: &str,
) -> Result<HostOperationLock, String> {
    use std::fs::OpenOptions;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

    let lock_dir = home.join(".codehangar").join("connector-locks");
    ensure_directory_tree_safe(&lock_dir)?;
    let lock_path = lock_dir.join(format!("{}.lock", host.id()));
    reject_reparse_point(&lock_path)?;
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        // No FILE_SHARE_* flags: another process cannot open, replace or delete
        // this lease until this exact handle closes (including process death).
        .share_mode(0)
        // Bind the leaf itself instead of following a symlink planted in the
        // small interval between the path check above and CreateFileW.
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&lock_path)
        .map_err(|_| {
            format!(
                "Another Code Hangar process owns the {} connected-app transaction; retry after it finishes.",
                host.label()
            )
        })?;
    let lock_metadata = file.metadata().map_err(|error| error.to_string())?;
    if lock_metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || !lock_metadata.is_file()
    {
        return Err(format!(
            "The {} connected-app transaction lock is linked or is not a file; it was left untouched.",
            host.label()
        ));
    }
    let owner_id = operation_owner_id(owner_hint);
    file.set_len(0).map_err(|error| error.to_string())?;
    file.write_all(owner_id.as_bytes())
        .map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    Ok(HostOperationLock {
        file,
        path: lock_path,
        owner_id,
    })
}

#[cfg(not(windows))]
pub fn acquire_host_operation_lock(
    _host: Host,
    _home: &Path,
    _owner_hint: &str,
) -> Result<HostOperationLock, String> {
    Err("Connected-app configuration locking is supported only on Windows.".to_string())
}

fn operation_owner_id(owner_hint: &str) -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    let material = format!(
        "{}:{}:{}:{}",
        std::process::id(),
        timestamp,
        owner_hint,
        std::thread::current().name().unwrap_or("unnamed")
    );
    format!(
        "pid={} owner={} lease={}\n",
        std::process::id(),
        owner_hint,
        &blake3::hash(material.as_bytes()).to_hex()[..24]
    )
}

impl AncestorGuard {
    fn new(target: PathBuf) -> Self {
        Self {
            target,
            bound: Mutex::new(None),
        }
    }

    fn bind_or_verify(&self) -> Result<(), String> {
        let mut slot = self
            .bound
            .lock()
            .map_err(|_| "The connected-app ancestor binding is unavailable.".to_string())?;
        if let Some(bound) = slot.as_ref() {
            return verify_bound_ancestors(bound);
        }
        let parent = self
            .target
            .parent()
            .ok_or_else(|| "The connected-app config has no parent directory.".to_string())?;
        ensure_directory_tree_safe(parent)?;
        let bound = bind_directory_ancestors(parent)?;
        verify_bound_ancestors(&bound)?;
        *slot = Some(bound);
        Ok(())
    }

    fn verify_if_bound(&self) -> Result<(), String> {
        let slot = self
            .bound
            .lock()
            .map_err(|_| "The connected-app ancestor binding is unavailable.".to_string())?;
        match slot.as_ref() {
            Some(bound) => verify_bound_ancestors(bound),
            None => validate_existing_ancestors(&self.target),
        }
    }
}

fn ensure_directory_tree_safe(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty() {
        return Ok(());
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            reject_reparse_point(path)?;
            if metadata.is_dir() {
                Ok(())
            } else {
                Err(format!("{} is not a directory", path.display()))
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                ensure_directory_tree_safe(parent)?;
            }
            match fs::create_dir(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.to_string()),
            }
            reject_reparse_point(path)?;
            if fs::symlink_metadata(path)
                .map_err(|error| error.to_string())?
                .is_dir()
            {
                Ok(())
            } else {
                Err(format!("{} is not a directory", path.display()))
            }
        }
        Err(error) => Err(error.to_string()),
    }
}

fn validate_existing_ancestors(path: &Path) -> Result<(), String> {
    for ancestor in path.ancestors() {
        reject_reparse_point(ancestor)?;
    }
    Ok(())
}

#[cfg(windows)]
fn bind_directory_ancestors(parent: &Path) -> Result<Vec<BoundAncestor>, String> {
    use std::fs::OpenOptions;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};

    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

    let mut paths = parent
        .ancestors()
        .map(Path::to_path_buf)
        .collect::<Vec<_>>();
    paths.reverse();
    let mut result = Vec::with_capacity(paths.len());
    for path in paths {
        reject_reparse_point(&path)?;
        let file = OpenOptions::new()
            .read(true)
            // Deliberately omit FILE_SHARE_DELETE: holding this handle freezes
            // the directory identity while the transaction crosses SQLite.
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&path)
            .map_err(|error| format!("could not bind {}: {error}", path.display()))?;
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        if metadata.file_attributes() & 0x400 != 0 || !metadata.is_dir() {
            return Err(format!(
                "refusing a linked or non-directory ancestor at {}",
                path.display()
            ));
        }
        let (volume_serial, file_index, _) = windows_handle_identity(&file)?;
        result.push(BoundAncestor {
            path,
            volume_serial,
            file_index,
            file,
        });
    }
    Ok(result)
}

#[cfg(not(windows))]
fn bind_directory_ancestors(parent: &Path) -> Result<Vec<BoundAncestor>, String> {
    let mut paths = parent
        .ancestors()
        .map(Path::to_path_buf)
        .collect::<Vec<_>>();
    paths.reverse();
    paths
        .into_iter()
        .map(|path| {
            reject_reparse_point(&path)?;
            let file = fs::File::open(&path).map_err(|error| error.to_string())?;
            Ok(BoundAncestor { path, file })
        })
        .collect()
}

#[cfg(windows)]
fn verify_bound_ancestors(bound: &[BoundAncestor]) -> Result<(), String> {
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

    for ancestor in bound {
        reject_reparse_point(&ancestor.path)?;
        let expected = (ancestor.volume_serial, ancestor.file_index);
        let (handle_volume, handle_index, handle_attributes) =
            windows_handle_identity(&ancestor.file)?;
        let current_file = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&ancestor.path)
            .map_err(|error| error.to_string())?;
        let (current_volume, current_index, current_attributes) =
            windows_handle_identity(&current_file)?;
        if handle_attributes & 0x400 != 0
            || current_attributes & 0x400 != 0
            || expected != (handle_volume, handle_index)
            || expected != (current_volume, current_index)
        {
            return Err(format!(
                "The connected-app ancestor {} changed identity and was left untouched.",
                ancestor.path.display()
            ));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn windows_handle_identity(file: &fs::File) -> Result<(u32, u64, u32), String> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    let mut info = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
    let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, info.as_mut_ptr()) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let info = unsafe { info.assume_init() };
    let index = ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64;
    Ok((info.dwVolumeSerialNumber, index, info.dwFileAttributes))
}

#[cfg(not(windows))]
fn verify_bound_ancestors(bound: &[BoundAncestor]) -> Result<(), String> {
    for ancestor in bound {
        reject_reparse_point(&ancestor.path)?;
        if !ancestor
            .file
            .metadata()
            .map_err(|error| error.to_string())?
            .is_dir()
        {
            return Err(format!(
                "{} is no longer a directory",
                ancestor.path.display()
            ));
        }
    }
    Ok(())
}

/// The user's home directory (`%USERPROFILE%` on Windows), if resolvable.
pub fn user_home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Read-only inspection of a host's config.
pub fn status(host: Host, home: &Path) -> HostStatus {
    inspect(host, home).status
}

/// Read-only config identity used to bind effective DB capabilities to the
/// exact token currently configured for one host. Only the one-way token hash
/// leaves this crate.
pub struct ConfigInspection {
    pub status: HostStatus,
    pub config_hash: Option<String>,
    pub configured_token_hash: Option<String>,
}

pub fn inspect(host: Host, home: &Path) -> ConfigInspection {
    let path = host_config_path(host, home);
    let snapshot = read_snapshot(&path).ok();
    let exists = snapshot
        .as_ref()
        .map(|snapshot| snapshot.fingerprint.exists)
        .unwrap_or_else(|| path.exists());
    let parsed = snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.bytes.as_deref())
        .map(|bytes| parse_config(host.format(), bytes));
    let (readable, registered, token_hash) = match parsed {
        None if !exists => (true, false, None),
        Some(Ok(ParsedConfig::Json(value))) => (
            true,
            json_registered(&value),
            json_token(&value).map(token_hash),
        ),
        Some(Ok(ParsedConfig::Toml(doc))) => (
            true,
            toml_registered(&doc),
            toml_token(&doc).map(token_hash),
        ),
        _ => (false, false, None),
    };
    ConfigInspection {
        status: HostStatus {
            host: host.id().to_string(),
            label: host.label().to_string(),
            config_path: path.display().to_string(),
            config_exists: exists,
            readable,
            registered,
            effective_scopes: Vec::new(),
            effective_project_ids: Vec::new(),
            credential_active: false,
            recovery_required: false,
            durable_agent_id: None,
            durable_identity_id: None,
            durable_credential_enabled: false,
            credential_orphaned: false,
            orphan_reason: None,
        },
        config_hash: snapshot.and_then(|snapshot| snapshot.fingerprint.hash),
        configured_token_hash: token_hash,
    }
}

/// Register (or replace) our entry in the host's config.
pub fn register(host: Host, home: &Path, spec: &ServerSpec) -> Result<(), String> {
    let prepared = prepare_register(host, home, spec)?;
    prepared.apply()?;
    prepared.finalize()
}

/// Remove only our entry, leaving every other server and key untouched. Returns
/// `true` if an entry was actually removed.
pub fn unregister(host: Host, home: &Path) -> Result<bool, String> {
    let Some(prepared) = prepare_unregister(host, home)? else {
        return Ok(false);
    };
    prepared.apply()?;
    prepared.finalize()?;
    Ok(true)
}

/// Build a registration plan without changing the config or any sidecar. The
/// caller must persist `fingerprints()` in its encrypted journal before `apply`.
pub fn prepare_register(
    host: Host,
    home: &Path,
    spec: &ServerSpec,
) -> Result<PreparedChange, String> {
    prepare_register_inner(host, home, spec, None)
}

/// Production registration path. The authenticated state binds the host, exact
/// config path, immutable DB identity, owned server entry and baseline hashes.
pub fn prepare_register_authenticated(
    host: Host,
    home: &Path,
    spec: &ServerSpec,
    binding: &RegistrationBinding,
) -> Result<PreparedChange, String> {
    prepare_register_inner(host, home, spec, Some(binding))
}

fn prepare_register_inner(
    host: Host,
    home: &Path,
    spec: &ServerSpec,
    binding: Option<&RegistrationBinding>,
) -> Result<PreparedChange, String> {
    let path = host_config_path(host, home);
    let config = read_snapshot(&path)?;
    let backup_path = sidecar_path(&path, BACKUP_SUFFIX);
    let state_path = sidecar_path(&path, STATE_SUFFIX);
    let backup = read_snapshot(&backup_path)?;
    let state = read_snapshot(&state_path)?;
    reject_pending_sidecars(&backup_path, &state_path)?;

    let parsed = match config.bytes.as_deref() {
        Some(bytes) => {
            parse_config(host.format(), bytes).map_err(|error| unreadable(&path, &error))?
        }
        None => match host.format() {
            Format::Json => ParsedConfig::Json(json!({})),
            Format::Toml => ParsedConfig::Toml(DocumentMut::new()),
        },
    };
    let (already_registered, sanitized_bytes, registered_bytes) = match parsed {
        ParsedConfig::Json(mut value) => {
            if !value.is_object() {
                return Err(unreadable(
                    &path,
                    "the top-level value is not a JSON object",
                ));
            }
            let already_registered = json_registered(&value);
            let mut sanitized = value.clone();
            json_remove_server(&mut sanitized);
            let mut sanitized_text =
                serde_json::to_string_pretty(&sanitized).map_err(|e| e.to_string())?;
            sanitized_text.push('\n');
            json_set_server(&mut value, spec)?;
            let mut registered_text =
                serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
            registered_text.push('\n');
            (
                already_registered,
                sanitized_text.into_bytes(),
                registered_text.into_bytes(),
            )
        }
        ParsedConfig::Toml(mut doc) => {
            let already_registered = toml_registered(&doc);
            let mut sanitized = doc.clone();
            toml_remove_server(&mut sanitized);
            let sanitized_bytes = sanitized.to_string().into_bytes();
            toml_set_server(&mut doc, spec)?;
            (
                already_registered,
                sanitized_bytes,
                doc.to_string().into_bytes(),
            )
        }
    };

    let (original_exists, original_hash, backup_action) =
        registration_baseline_plan(RegistrationBaselineInput {
            host,
            config_path: &path,
            current_bytes: config.bytes.as_deref(),
            already_registered,
            sanitized_bytes: &sanitized_bytes,
            backup: &backup,
            state: &state,
            binding,
        })?;
    let mut registration_state = RegistrationState {
        schema_version: STATE_SCHEMA_VERSION,
        host: host.id().to_string(),
        config_path_hash: bytes_hash(normalized_binding_path(&path).as_bytes()),
        agent_identity_id: binding
            .map(|value| value.agent_identity_id.clone())
            .unwrap_or_else(|| "00000000000000000000000000000000".to_string()),
        owned_entry_hash: owned_entry_hash(host, &registered_bytes)?,
        original_exists,
        original_hash,
        registered_hash: bytes_hash(&registered_bytes),
        auth_tag: String::new(),
    };
    if let Some(binding) = binding {
        sign_registration_state(&mut registration_state, binding);
    }
    let state_bytes = serde_json::to_vec(&registration_state).map_err(|error| error.to_string())?;
    prepared_change(
        host,
        home,
        ChangeSnapshots {
            config,
            backup,
            state,
        },
        Some(registered_bytes),
        backup_action,
        SidecarAction::Replace(state_bytes),
    )
}

/// Build a removal plan without changing the config. `None` means this host has
/// no Code Hangar entry and there is nothing to disconnect.
pub fn prepare_unregister(host: Host, home: &Path) -> Result<Option<PreparedChange>, String> {
    prepare_unregister_inner(host, home, None)
}

/// Production disconnect path. Missing, malformed or forged state is a hard
/// stop for external config edits; the caller may still revoke the durable DB
/// credential through the separate DB-only orphan path.
pub fn prepare_unregister_authenticated(
    host: Host,
    home: &Path,
    binding: &RegistrationBinding,
) -> Result<Option<PreparedChange>, String> {
    prepare_unregister_inner(host, home, Some(binding))
}

fn prepare_unregister_inner(
    host: Host,
    home: &Path,
    binding: Option<&RegistrationBinding>,
) -> Result<Option<PreparedChange>, String> {
    let path = host_config_path(host, home);
    let config = read_snapshot(&path)?;
    let Some(current_bytes) = config.bytes.as_deref() else {
        return Ok(None);
    };
    let backup_path = sidecar_path(&path, BACKUP_SUFFIX);
    let state_path = sidecar_path(&path, STATE_SUFFIX);
    let backup = read_snapshot(&backup_path)?;
    let state = read_snapshot(&state_path)?;
    reject_pending_sidecars(&backup_path, &state_path)?;

    let (registered, sanitized) = match parse_config(host.format(), current_bytes)
        .map_err(|error| unreadable(&path, &error))?
    {
        ParsedConfig::Json(mut value) => {
            let registered = json_registered(&value);
            json_remove_server(&mut value);
            let mut text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
            text.push('\n');
            (registered, text.into_bytes())
        }
        ParsedConfig::Toml(mut doc) => {
            let registered = toml_registered(&doc);
            toml_remove_server(&mut doc);
            (registered, doc.to_string().into_bytes())
        }
    };
    if !registered {
        return Ok(None);
    }

    let config_after = if let Some(binding) = binding {
        let registration_state =
            read_authenticated_registration_state(&state, host, &path, binding)?;
        if owned_entry_hash(host, current_bytes)? != registration_state.owned_entry_hash {
            return Err(
                "The configured Code Hangar server entry changed after registration; external config was left untouched. Revoke the orphaned credential in Code Hangar instead."
                    .to_string(),
            );
        }
        let current_unchanged = registration_state.schema_version == STATE_SCHEMA_VERSION
            && bytes_hash(current_bytes) == registration_state.registered_hash;
        if current_unchanged && !registration_state.original_exists {
            None
        } else if current_unchanged {
            let expected_hash = registration_state.original_hash.as_deref();
            match (backup.bytes.as_deref(), expected_hash) {
                (Some(bytes), Some(expected)) if bytes_hash(bytes) == expected => {
                    Some(bytes.to_vec())
                }
                _ => Some(sanitized),
            }
        } else {
            Some(sanitized)
        }
    } else {
        // Compatibility helper used by isolated fixtures only: never trust an
        // unauthenticated sidecar to restore/delete an owner-controlled config.
        // It can at most remove the exact fixed Code Hangar key from parsed data.
        Some(sanitized)
    };

    Ok(Some(prepared_change(
        host,
        home,
        ChangeSnapshots {
            config,
            backup,
            state,
        },
        config_after,
        SidecarAction::Preserve,
        SidecarAction::Remove,
    )?))
}

/// Complete or abort a filesystem half after a process crash, using only the
/// hash-only journal contract. `commit=true` removes rollback sidecars once the
/// config equals the expected after image; `false` restores prior sidecars once
/// the config equals the expected before image. An ambiguous path is untouched.
pub fn recover_change(
    host: Host,
    home: &Path,
    fingerprints: &ChangeFingerprints,
    commit: bool,
) -> Result<(), String> {
    let path = host_config_path(host, home);
    let current = read_snapshot(&path)?;
    let expected = if commit {
        &fingerprints.config_after
    } else {
        &fingerprints.config_before
    };
    if current.fingerprint != *expected {
        return Err(
            "the connected-app config changed during recovery and was left untouched".to_string(),
        );
    }
    recover_sidecars(host, home, fingerprints, commit)
}

/// Recover only the rollback sidecars. This is separated from `recover_change`
/// for the one safe no-overwrite case where a host config was externally removed
/// while a prepared reconnect existed: the DB journal is aborted, the absent
/// config stays absent, and exact sidecars are rolled back by hash.
pub fn recover_sidecars(
    host: Host,
    home: &Path,
    fingerprints: &ChangeFingerprints,
    commit: bool,
) -> Result<(), String> {
    let path = host_config_path(host, home);
    if commit {
        finalize_sidecar_from_fingerprints(
            &sidecar_path(&path, BACKUP_SUFFIX),
            &fingerprints.backup_before,
            &fingerprints.backup_after,
            fingerprints.backup_changed,
        )?;
        finalize_sidecar_from_fingerprints(
            &sidecar_path(&path, STATE_SUFFIX),
            &fingerprints.state_before,
            &fingerprints.state_after,
            fingerprints.state_changed,
        )?;
    } else {
        rollback_sidecar_from_fingerprints(
            &sidecar_path(&path, BACKUP_SUFFIX),
            &fingerprints.backup_before,
            &fingerprints.backup_after,
            fingerprints.backup_changed,
        )?;
        rollback_sidecar_from_fingerprints(
            &sidecar_path(&path, STATE_SUFFIX),
            &fingerprints.state_before,
            &fingerprints.state_after,
            fingerprints.state_changed,
        )?;
    }
    Ok(())
}

/// Detect rollback sidecars without reading or exposing their bytes. A sidecar
/// with no matching encrypted DB journal is ambiguous and must be left alone.
pub fn pending_sidecars_present(host: Host, home: &Path) -> Result<bool, String> {
    let path = host_config_path(host, home);
    for managed in [
        sidecar_path(&path, BACKUP_SUFFIX),
        sidecar_path(&path, STATE_SUFFIX),
    ] {
        let pending = pending_path(&managed);
        reject_reparse_point(&pending)?;
        if pending.exists() {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn config_fingerprint(host: Host, home: &Path) -> Result<FileFingerprint, String> {
    Ok(read_snapshot(&host_config_path(host, home))?.fingerprint)
}

fn prepared_change(
    host: Host,
    home: &Path,
    snapshots: ChangeSnapshots,
    config_after: Option<Vec<u8>>,
    backup_action: SidecarAction,
    state_action: SidecarAction,
) -> Result<PreparedChange, String> {
    let ChangeSnapshots {
        config,
        backup,
        state,
    } = snapshots;
    let backup_after = action_fingerprint(&backup, &backup_action);
    let state_after = action_fingerprint(&state, &state_action);
    let fingerprints = ChangeFingerprints {
        config_before: config.fingerprint.clone(),
        config_after: fingerprint_bytes(config_after.as_deref()),
        backup_before: backup.fingerprint.clone(),
        backup_after,
        backup_changed: !matches!(backup_action, SidecarAction::Preserve),
        state_before: state.fingerprint.clone(),
        state_after,
        state_changed: !matches!(state_action, SidecarAction::Preserve),
    };
    Ok(PreparedChange {
        host,
        home: home.to_path_buf(),
        config,
        backup,
        state,
        config_after,
        backup_action,
        state_action,
        fingerprints,
        ancestry: AncestorGuard::new(host_config_path(host, home)),
    })
}

enum ParsedConfig {
    Json(Value),
    Toml(DocumentMut),
}

fn parse_config(format: Format, bytes: &[u8]) -> Result<ParsedConfig, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "the config is not UTF-8".to_string())?;
    match format {
        Format::Json => {
            if text.trim().is_empty() {
                Ok(ParsedConfig::Json(json!({})))
            } else {
                serde_json::from_str(text)
                    .map(ParsedConfig::Json)
                    .map_err(|error| error.to_string())
            }
        }
        Format::Toml => text
            .parse::<DocumentMut>()
            .map(ParsedConfig::Toml)
            .map_err(|error| error.to_string()),
    }
}

fn token_hash(token: &str) -> String {
    blake3::hash(token.as_bytes()).to_hex().to_string()
}

fn configured_token_hash(host: Host, home: &Path) -> Result<Option<String>, String> {
    let path = host_config_path(host, home);
    let snapshot = read_snapshot(&path)?;
    let Some(bytes) = snapshot.bytes.as_deref() else {
        return Ok(None);
    };
    match parse_config(host.format(), bytes).map_err(|error| unreadable(&path, &error))? {
        ParsedConfig::Json(value) => Ok(json_token(&value).map(token_hash)),
        ParsedConfig::Toml(doc) => Ok(toml_token(&doc).map(token_hash)),
    }
}

fn json_token(value: &Value) -> Option<&str> {
    value
        .get("mcpServers")?
        .get(SERVER_KEY)?
        .get("env")?
        .get("CODEHANGAR_MCP_TOKEN")?
        .as_str()
}

fn toml_token(doc: &DocumentMut) -> Option<&str> {
    doc.get("mcp_servers")?
        .as_table()?
        .get(SERVER_KEY)?
        .as_table()?
        .get("env")?
        .as_table()?
        .get("CODEHANGAR_MCP_TOKEN")?
        .as_str()
}

fn owned_entry_hash(host: Host, config_bytes: &[u8]) -> Result<String, String> {
    let path_label = format!("{} connected-app entry", host.label());
    match parse_config(host.format(), config_bytes) {
        Ok(ParsedConfig::Json(value)) => {
            let entry = value
                .get("mcpServers")
                .and_then(|servers| servers.get(SERVER_KEY))
                .ok_or_else(|| format!("The {path_label} is missing."))?;
            serde_json::to_vec(entry)
                .map(|bytes| bytes_hash(&bytes))
                .map_err(|error| error.to_string())
        }
        Ok(ParsedConfig::Toml(doc)) => {
            let entry = doc
                .get("mcp_servers")
                .and_then(Item::as_table)
                .and_then(|servers| servers.get(SERVER_KEY))
                .ok_or_else(|| format!("The {path_label} is missing."))?;
            Ok(bytes_hash(entry.to_string().as_bytes()))
        }
        Err(_) => Err(format!("The {path_label} is unreadable.")),
    }
}

// ---- JSON (Claude, Cursor) -------------------------------------------------

#[cfg(test)]
fn read_json(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

fn json_set_server(value: &mut Value, spec: &ServerSpec) -> Result<(), String> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| "the top-level value is not a JSON object".to_string())?;
    let servers = object
        .entry("mcpServers")
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    let servers = servers
        .as_object_mut()
        .ok_or_else(|| "the existing \"mcpServers\" value is not an object".to_string())?;
    servers.insert(SERVER_KEY.to_string(), json_server_entry(spec));
    Ok(())
}

fn json_server_entry(spec: &ServerSpec) -> Value {
    let mut env = serde_json::Map::new();
    for (key, val) in &spec.env {
        env.insert(key.clone(), Value::String(val.clone()));
    }
    json!({
        "command": spec.command,
        "args": spec.args,
        "env": Value::Object(env),
    })
}

fn json_registered(value: &Value) -> bool {
    value
        .get("mcpServers")
        .and_then(|servers| servers.get(SERVER_KEY))
        .is_some()
}

fn json_remove_server(value: &mut Value) {
    if let Some(servers) = value.get_mut("mcpServers").and_then(Value::as_object_mut) {
        servers.remove(SERVER_KEY);
    }
}

// ---- TOML (Codex) ----------------------------------------------------------

#[cfg(test)]
fn read_toml(path: &Path) -> Result<DocumentMut, String> {
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    text.parse::<DocumentMut>().map_err(|e| e.to_string())
}

fn toml_set_server(doc: &mut DocumentMut, spec: &ServerSpec) -> Result<(), String> {
    let servers_item = doc
        .entry("mcp_servers")
        .or_insert(Item::Table(Table::new()));
    let servers = servers_item
        .as_table_mut()
        .ok_or_else(|| "the existing \"mcp_servers\" value is not a table".to_string())?;
    // Render as [mcp_servers.code-hangar] rather than a standalone [mcp_servers].
    servers.set_implicit(true);

    let mut entry = Table::new();
    entry["command"] = toml_edit::value(spec.command.clone());
    let mut args = Array::new();
    for arg in &spec.args {
        args.push(arg.clone());
    }
    entry["args"] = toml_edit::value(args);
    entry["startup_timeout_sec"] = toml_edit::value(spec.startup_timeout_sec as i64);
    let mut env = Table::new();
    for (key, val) in &spec.env {
        env[key] = toml_edit::value(val.clone());
    }
    entry["env"] = Item::Table(env);

    servers.insert(SERVER_KEY, Item::Table(entry));
    Ok(())
}

fn toml_registered(doc: &DocumentMut) -> bool {
    doc.get("mcp_servers")
        .and_then(Item::as_table)
        .map(|servers| servers.contains_key(SERVER_KEY))
        .unwrap_or(false)
}

fn toml_remove_server(doc: &mut DocumentMut) {
    if let Some(servers) = doc.get_mut("mcp_servers").and_then(Item::as_table_mut) {
        servers.remove(SERVER_KEY);
    }
}

// ---- shared safe-write primitives ------------------------------------------

fn unreadable(path: &Path, _detail: &str) -> String {
    // Parser diagnostics (especially TOML snippets) can echo the line that
    // contains CODEHANGAR_MCP_TOKEN. Keep the owner-facing failure generic:
    // path + fail-closed outcome are actionable; source excerpts are not.
    format!(
        "{} could not be parsed safely and was left untouched",
        path.display()
    )
}

/// Refuse to write through a symlink / reparse point. A same-user attacker could
/// pre-seed a host config path (or its `.bak`/`.tmp` sibling) as a symlink or
/// junction so our write clobbers the link target instead of the intended file;
/// reject any such pre-seeded link rather than following it.
fn reject_reparse_point(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            #[allow(unused_mut)]
            let mut is_link = meta.file_type().is_symlink();
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
                is_link = is_link || meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0;
            }
            if is_link {
                return Err(format!(
                    "refusing to write through a symlink or reparse point at {}",
                    path.display()
                ));
            }
            Ok(())
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.to_string()),
    }
}

fn sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut sidecar: OsString = path.as_os_str().to_os_string();
    sidecar.push(suffix);
    PathBuf::from(sidecar)
}

fn bytes_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn decode_hex_32(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut output = [0_u8; 32];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(chunk).ok()?;
        output[index] = u8::from_str_radix(text, 16).ok()?;
    }
    Some(output)
}

fn normalized_binding_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

fn registration_state_material(state: &RegistrationState) -> Vec<u8> {
    // Length-prefix every value so no concatenation ambiguity can produce the
    // same MAC input. auth_tag is deliberately excluded.
    let fields = [
        state.schema_version.to_string(),
        state.host.clone(),
        state.config_path_hash.clone(),
        state.agent_identity_id.clone(),
        state.owned_entry_hash.clone(),
        if state.original_exists { "1" } else { "0" }.to_string(),
        state.original_hash.clone().unwrap_or_default(),
        state.registered_hash.clone(),
    ];
    let mut material = Vec::new();
    for field in fields {
        material.extend_from_slice(&(field.len() as u64).to_le_bytes());
        material.extend_from_slice(field.as_bytes());
    }
    material
}

fn sign_registration_state(state: &mut RegistrationState, binding: &RegistrationBinding) {
    state.auth_tag = blake3::keyed_hash(&binding.auth_key, &registration_state_material(state))
        .to_hex()
        .to_string();
}

fn read_authenticated_registration_state(
    state: &FileSnapshot,
    host: Host,
    path: &Path,
    binding: &RegistrationBinding,
) -> Result<RegistrationState, String> {
    let bytes = state.bytes.as_deref().ok_or_else(|| {
        "The authenticated connected-app registration state is missing.".to_string()
    })?;
    let parsed = serde_json::from_slice::<RegistrationState>(bytes).map_err(|_| {
        "The authenticated connected-app registration state is unreadable.".to_string()
    })?;
    let expected_path_hash = bytes_hash(normalized_binding_path(path).as_bytes());
    let expected_tag = blake3::keyed_hash(&binding.auth_key, &registration_state_material(&parsed))
        .to_hex()
        .to_string();
    if parsed.schema_version != STATE_SCHEMA_VERSION
        || parsed.host != host.id()
        || parsed.config_path_hash != expected_path_hash
        || parsed.agent_identity_id != binding.agent_identity_id
        || parsed.auth_tag.len() != 64
        || !constant_time_eq(parsed.auth_tag.as_bytes(), expected_tag.as_bytes())
    {
        return Err(
            "The connected-app registration state is not authenticated for this host/path/identity; external config was left untouched."
                .to_string(),
        );
    }
    Ok(parsed)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn fingerprint_bytes(bytes: Option<&[u8]>) -> FileFingerprint {
    FileFingerprint {
        exists: bytes.is_some(),
        hash: bytes.map(bytes_hash),
    }
}

fn read_snapshot(path: &Path) -> Result<FileSnapshot, String> {
    reject_reparse_point(path)?;
    match fs::read(path) {
        Ok(bytes) => Ok(FileSnapshot {
            fingerprint: fingerprint_bytes(Some(&bytes)),
            bytes: Some(bytes),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(FileSnapshot {
            fingerprint: fingerprint_bytes(None),
            bytes: None,
        }),
        Err(error) => Err(error.to_string()),
    }
}

fn current_fingerprint(path: &Path) -> Result<FileFingerprint, String> {
    Ok(read_snapshot(path)?.fingerprint)
}

fn assert_snapshot(path: &Path, expected: &FileSnapshot) -> Result<(), String> {
    if current_fingerprint(path)? == expected.fingerprint {
        Ok(())
    } else {
        Err(format!(
            "{} changed while the connected-app operation was being prepared and was left untouched",
            path.display()
        ))
    }
}

fn action_fingerprint(before: &FileSnapshot, action: &SidecarAction) -> FileFingerprint {
    match action {
        SidecarAction::Preserve => before.fingerprint.clone(),
        SidecarAction::Replace(bytes) => fingerprint_bytes(Some(bytes)),
        SidecarAction::Remove => fingerprint_bytes(None),
    }
}

fn pending_path(path: &Path) -> PathBuf {
    sidecar_path(path, PENDING_SUFFIX)
}

fn reject_pending_sidecars(backup_path: &Path, state_path: &Path) -> Result<(), String> {
    for path in [pending_path(backup_path), pending_path(state_path)] {
        reject_reparse_point(&path)?;
        if path.exists() {
            return Err(
                "a prior connected-app configuration change still needs recovery".to_string(),
            );
        }
    }
    Ok(())
}

fn apply_sidecar(path: &Path, before: &FileSnapshot, action: &SidecarAction) -> Result<(), String> {
    if matches!(action, SidecarAction::Preserve) {
        return assert_snapshot(path, before);
    }
    assert_snapshot(path, before)?;
    let pending = pending_path(path);
    reject_reparse_point(&pending)?;
    if pending.exists() {
        return Err("a prior connected-app sidecar change still needs recovery".to_string());
    }
    if before.fingerprint.exists {
        fs::rename(path, &pending).map_err(|error| error.to_string())?;
    }
    match action {
        SidecarAction::Preserve => unreachable!(),
        SidecarAction::Replace(bytes) => atomic_write_bytes(path, bytes)?,
        SidecarAction::Remove => {}
    }
    let expected = action_fingerprint(before, action);
    if current_fingerprint(path)? != expected {
        return Err("a connected-app sidecar could not be verified after writing".to_string());
    }
    Ok(())
}

fn write_optional_bytes(path: &Path, bytes: Option<&[u8]>) -> Result<(), String> {
    match bytes {
        Some(bytes) => atomic_write_bytes(path, bytes),
        None => {
            reject_reparse_point(path)?;
            match fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.to_string()),
            }
        }
    }
}

fn restore_config_snapshot(
    path: &Path,
    before: &FileSnapshot,
    after: &FileFingerprint,
) -> Result<(), String> {
    let current = current_fingerprint(path)?;
    if current == before.fingerprint {
        return Ok(());
    }
    if current != *after {
        return Err(
            "the host config changed after the attempted write and was left untouched".to_string(),
        );
    }
    write_optional_bytes(path, before.bytes.as_deref())?;
    if current_fingerprint(path)? != before.fingerprint {
        return Err("the prior host config could not be restored exactly".to_string());
    }
    Ok(())
}

fn rollback_sidecar(
    path: &Path,
    before: &FileSnapshot,
    after: &FileFingerprint,
    changed: bool,
) -> Result<(), String> {
    rollback_sidecar_from_fingerprints(path, &before.fingerprint, after, changed)
}

fn rollback_sidecar_from_fingerprints(
    path: &Path,
    before: &FileFingerprint,
    after: &FileFingerprint,
    changed: bool,
) -> Result<(), String> {
    if !changed {
        if current_fingerprint(path)? == *before {
            return Ok(());
        }
        return Err("an unchanged connected-app sidecar no longer matches".to_string());
    }
    let pending = pending_path(path);
    let current = current_fingerprint(path)?;
    let pending_fingerprint = current_fingerprint(&pending)?;
    if current == *before && !pending_fingerprint.exists {
        return Ok(());
    }
    // A process can stop after moving the prior sidecar to its rollback name
    // but before writing the replacement. That partial state is recoverable:
    // the pending image is the exact before hash and the managed path is absent.
    let partially_staged = before.exists && !current.exists && pending_fingerprint == *before;
    if current != *after && !partially_staged {
        return Err(
            "a connected-app sidecar changed during rollback and was left untouched".to_string(),
        );
    }
    if before.exists {
        if pending_fingerprint != *before {
            return Err("the prior connected-app sidecar is unavailable for recovery".to_string());
        }
        write_optional_bytes(path, None)?;
        fs::rename(&pending, path).map_err(|error| error.to_string())?;
    } else {
        if pending_fingerprint.exists {
            return Err(
                "an unexpected connected-app rollback sidecar was left untouched".to_string(),
            );
        }
        write_optional_bytes(path, None)?;
    }
    if current_fingerprint(path)? != *before {
        return Err("the prior connected-app sidecar could not be restored".to_string());
    }
    Ok(())
}

fn finalize_sidecar(
    path: &Path,
    before: &FileSnapshot,
    after: &FileFingerprint,
    changed: bool,
) -> Result<(), String> {
    finalize_sidecar_from_fingerprints(path, &before.fingerprint, after, changed)
}

fn finalize_sidecar_from_fingerprints(
    path: &Path,
    before: &FileFingerprint,
    after: &FileFingerprint,
    changed: bool,
) -> Result<(), String> {
    let current = current_fingerprint(path)?;
    if !changed {
        return if current == *after {
            Ok(())
        } else {
            Err("an unchanged connected-app sidecar no longer matches".to_string())
        };
    }
    if current != *after {
        return Err(
            "a connected-app sidecar changed before commit and was left untouched".to_string(),
        );
    }
    let pending = pending_path(path);
    let pending_fingerprint = current_fingerprint(&pending)?;
    if !pending_fingerprint.exists {
        return Ok(());
    }
    if pending_fingerprint != *before {
        return Err("an ambiguous connected-app rollback sidecar was left untouched".to_string());
    }
    fs::remove_file(&pending).map_err(|error| error.to_string())
}

struct RegistrationBaselineInput<'a> {
    host: Host,
    config_path: &'a Path,
    current_bytes: Option<&'a [u8]>,
    already_registered: bool,
    sanitized_bytes: &'a [u8],
    backup: &'a FileSnapshot,
    state: &'a FileSnapshot,
    binding: Option<&'a RegistrationBinding>,
}

fn registration_baseline_plan(
    input: RegistrationBaselineInput<'_>,
) -> Result<(bool, Option<String>, SidecarAction), String> {
    let RegistrationBaselineInput {
        host,
        config_path,
        current_bytes,
        already_registered,
        sanitized_bytes,
        backup,
        state,
        binding,
    } = input;
    if already_registered {
        if let (Some(current_bytes), Some(binding)) = (current_bytes, binding) {
            if let Ok(existing) =
                read_authenticated_registration_state(state, host, config_path, binding)
            {
                let unchanged = existing.schema_version == STATE_SCHEMA_VERSION
                    && bytes_hash(current_bytes) == existing.registered_hash
                    && owned_entry_hash(host, current_bytes)
                        .is_ok_and(|hash| hash == existing.owned_entry_hash);
                if unchanged && !existing.original_exists && existing.original_hash.is_none() {
                    return Ok((false, None, SidecarAction::Preserve));
                }
                if unchanged {
                    if let (Some(expected), Some(backup_bytes)) =
                        (existing.original_hash.as_deref(), backup.bytes.as_deref())
                    {
                        if bytes_hash(backup_bytes) == expected {
                            return Ok((true, Some(expected.to_string()), SidecarAction::Preserve));
                        }
                    }
                }
            }
        }
        return Ok((
            true,
            Some(bytes_hash(sanitized_bytes)),
            SidecarAction::Replace(sanitized_bytes.to_vec()),
        ));
    }

    match current_bytes {
        Some(bytes) => Ok((
            true,
            Some(bytes_hash(bytes)),
            SidecarAction::Replace(bytes.to_vec()),
        )),
        None => Ok((false, None, SidecarAction::Preserve)),
    }
}

/// Write to a sibling temp file, fsync, then atomically rename over the target.
fn atomic_write_bytes(path: &Path, contents: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        ensure_directory_tree_safe(parent)?;
    }
    // Never follow a pre-seeded symlink/junction at the final path. The sibling
    // temp is random and opened create-new by tempfile, so no predictable temp
    // name exists for another process to pre-seed.
    reject_reparse_point(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| "The connected-app target has no parent directory.".to_string())?;
    let mut temp = tempfile::Builder::new()
        .prefix(".codehangar-")
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(|error| error.to_string())?;
    temp.write_all(contents)
        .map_err(|error| error.to_string())?;
    temp.as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    // Revalidate the destination immediately before the atomic replace. The
    // PreparedChange's bound ancestor handles remain alive through this call.
    reject_reparse_point(path)?;
    let temp_path = temp.path().to_path_buf();
    match temp.persist(path) {
        Ok(_) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temp_path);
            Err(error.error.to_string())
        }
    }?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn spec() -> ServerSpec {
        ServerSpec {
            command: r"C:\Apps\code-hangar-mcp.exe".to_string(),
            args: vec![],
            env: vec![
                ("CODEHANGAR_MCP_TOKEN".to_string(), "tok-123".to_string()),
                (
                    "CODEHANGAR_DB_PATH".to_string(),
                    r"C:\Roaming\local.codehangar.desktop\codehangar.sqlite3".to_string(),
                ),
            ],
            startup_timeout_sec: 20,
        }
    }

    fn binding() -> RegistrationBinding {
        RegistrationBinding::from_hex(
            "11111111111111111111111111111111",
            "2222222222222222222222222222222222222222222222222222222222222222",
        )
        .unwrap()
    }

    fn register_bound(host: Host, home: &Path, spec: &ServerSpec) -> Result<(), String> {
        let prepared = prepare_register_authenticated(host, home, spec, &binding())?;
        prepared.apply()?;
        prepared.finalize()
    }

    fn unregister_bound(host: Host, home: &Path) -> Result<bool, String> {
        let Some(prepared) = prepare_unregister_authenticated(host, home, &binding())? else {
            return Ok(false);
        };
        prepared.apply()?;
        prepared.finalize()?;
        Ok(true)
    }

    fn prepare_register_bound(
        host: Host,
        home: &Path,
        spec: &ServerSpec,
    ) -> Result<PreparedChange, String> {
        prepare_register_authenticated(host, home, spec, &binding())
    }

    #[test]
    fn exposes_the_three_supported_connector_hosts_by_product_name() {
        assert_eq!(Host::ALL.map(Host::label), ["Claude", "Cursor", "Codex"]);
    }

    #[test]
    fn registers_into_absent_json_then_reports_registered() {
        let home = tempdir().unwrap();
        let host = Host::Cursor;
        assert!(!status(host, home.path()).registered);
        register_bound(host, home.path(), &spec()).unwrap();

        let st = status(host, home.path());
        assert!(st.config_exists && st.readable && st.registered);

        let value = read_json(&host_config_path(host, home.path())).unwrap();
        let entry = &value["mcpServers"]["code-hangar"];
        assert_eq!(entry["command"], r"C:\Apps\code-hangar-mcp.exe");
        assert_eq!(entry["env"]["CODEHANGAR_MCP_TOKEN"], "tok-123");
    }

    #[test]
    fn register_preserves_other_servers_and_keys_and_backs_up() {
        let home = tempdir().unwrap();
        let path = host_config_path(Host::Claude, home.path());
        fs::write(
            &path,
            r#"{"numStartups":7,"mcpServers":{"existing":{"command":"other"}}}"#,
        )
        .unwrap();

        register_bound(Host::Claude, home.path(), &spec()).unwrap();

        let value = read_json(&path).unwrap();
        // Our entry was added; the other server and unrelated key survive.
        assert!(value["mcpServers"]["code-hangar"].is_object());
        assert_eq!(value["mcpServers"]["existing"]["command"], "other");
        assert_eq!(value["numStartups"], 7);
        // A verified backup of the original was written.
        let mut bak: OsString = path.as_os_str().to_os_string();
        bak.push(BACKUP_SUFFIX);
        assert!(PathBuf::from(bak).exists());
    }

    #[test]
    fn refuses_to_touch_unparseable_json() {
        let home = tempdir().unwrap();
        let path = host_config_path(Host::Cursor, home.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{ this is : not json ").unwrap();

        let error = register_bound(Host::Cursor, home.path(), &spec()).unwrap_err();
        assert!(error.contains("left untouched"));
        // The malformed file is unchanged.
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ this is : not json ");
        assert!(!status(Host::Cursor, home.path()).readable);
    }

    #[test]
    fn parse_errors_never_echo_a_token_bearing_toml_line() {
        let home = tempdir().unwrap();
        let path = host_config_path(Host::Codex, home.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let secret = "must-not-appear-in-errors";
        let malformed = format!(
            "[mcp_servers.code-hangar.env]\nCODEHANGAR_MCP_TOKEN = \"{secret}\"\n[broken\n"
        );
        fs::write(&path, &malformed).unwrap();

        let error = register_bound(Host::Codex, home.path(), &spec()).unwrap_err();
        assert!(!error.contains(secret));
        assert!(error.contains("left untouched"));
        assert_eq!(fs::read_to_string(&path).unwrap(), malformed);
    }

    #[test]
    fn json_round_trip_register_then_unregister_restores_other_content() {
        let home = tempdir().unwrap();
        let path = host_config_path(Host::Claude, home.path());
        fs::write(&path, r#"{"mcpServers":{"existing":{"command":"other"}}}"#).unwrap();

        register_bound(Host::Claude, home.path(), &spec()).unwrap();
        assert!(unregister_bound(Host::Claude, home.path()).unwrap());

        let value = read_json(&path).unwrap();
        assert!(value["mcpServers"].get("code-hangar").is_none());
        assert_eq!(value["mcpServers"]["existing"]["command"], "other");
        // A second unregister is a no-op.
        assert!(!unregister_bound(Host::Claude, home.path()).unwrap());
    }

    #[test]
    fn json_round_trip_restores_exact_bytes_without_persisting_the_token() {
        let home = tempdir().unwrap();
        let path = host_config_path(Host::Cursor, home.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = b"{\n  \"mcpServers\": {}\n}";
        fs::write(&path, original).unwrap();

        register_bound(Host::Cursor, home.path(), &spec()).unwrap();

        let state_path = sidecar_path(&path, STATE_SUFFIX);
        let state_text = fs::read_to_string(&state_path).unwrap();
        assert!(!state_text.contains("tok-123"));
        assert!(!state_text.contains("code-hangar-mcp"));
        let backup_path = sidecar_path(&path, BACKUP_SUFFIX);
        assert_eq!(fs::read(&backup_path).unwrap(), original);

        assert!(unregister_bound(Host::Cursor, home.path()).unwrap());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(fs::read(&backup_path).unwrap(), original);
        assert!(!state_path.exists());
    }

    #[test]
    fn absent_config_is_removed_again_after_an_unchanged_round_trip() {
        let home = tempdir().unwrap();
        let path = host_config_path(Host::Cursor, home.path());

        register_bound(Host::Cursor, home.path(), &spec()).unwrap();
        assert!(path.exists());
        assert!(unregister_bound(Host::Cursor, home.path()).unwrap());
        assert!(!path.exists());
    }

    #[test]
    fn unregister_preserves_host_changes_made_while_connected() {
        let home = tempdir().unwrap();
        let path = host_config_path(Host::Claude, home.path());
        let original = br#"{"mcpServers":{"existing":{"command":"other"}}}"#;
        fs::write(&path, original).unwrap();
        register_bound(Host::Claude, home.path(), &spec()).unwrap();

        let mut changed = read_json(&path).unwrap();
        changed["theme"] = json!("dark");
        fs::write(&path, serde_json::to_string_pretty(&changed).unwrap()).unwrap();

        assert!(unregister_bound(Host::Claude, home.path()).unwrap());
        let after = read_json(&path).unwrap();
        assert_eq!(after["theme"], "dark");
        assert_eq!(after["mcpServers"]["existing"]["command"], "other");
        assert!(after["mcpServers"].get(SERVER_KEY).is_none());
        assert_eq!(
            fs::read(sidecar_path(&path, BACKUP_SUFFIX)).unwrap(),
            original
        );
        assert!(!sidecar_path(&path, STATE_SUFFIX).exists());
    }

    #[test]
    fn reconnect_rotates_the_entry_without_overwriting_the_original_backup() {
        let home = tempdir().unwrap();
        let path = host_config_path(Host::Cursor, home.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = br#"{"mcpServers":{},"keep":true}"#;
        fs::write(&path, original).unwrap();

        register_bound(Host::Cursor, home.path(), &spec()).unwrap();
        let mut rotated = spec();
        rotated.env[0].1 = "tok-456".to_string();
        register_bound(Host::Cursor, home.path(), &rotated).unwrap();

        assert_eq!(
            fs::read(sidecar_path(&path, BACKUP_SUFFIX)).unwrap(),
            original
        );
        assert!(unregister_bound(Host::Cursor, home.path()).unwrap());
        assert_eq!(fs::read(&path).unwrap(), original);
    }

    #[test]
    fn reconnect_keeps_host_changes_made_after_the_original_registration() {
        let home = tempdir().unwrap();
        let path = host_config_path(Host::Cursor, home.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, br#"{"mcpServers":{},"keep":true}"#).unwrap();

        register_bound(Host::Cursor, home.path(), &spec()).unwrap();
        let mut changed = read_json(&path).unwrap();
        changed["theme"] = json!("dark");
        fs::write(&path, serde_json::to_string_pretty(&changed).unwrap()).unwrap();

        let mut rotated = spec();
        rotated.env[0].1 = "tok-456".to_string();
        register_bound(Host::Cursor, home.path(), &rotated).unwrap();
        assert!(unregister_bound(Host::Cursor, home.path()).unwrap());

        let after = read_json(&path).unwrap();
        assert_eq!(after["theme"], "dark");
        assert_eq!(after["keep"], true);
        assert!(after["mcpServers"].get(SERVER_KEY).is_none());
        let backup = fs::read_to_string(sidecar_path(&path, BACKUP_SUFFIX)).unwrap();
        assert!(!backup.contains("tok-123"));
        assert!(!backup.contains("tok-456"));
    }

    #[test]
    fn registers_into_codex_toml_and_preserves_comments() {
        let home = tempdir().unwrap();
        let path = host_config_path(Host::Codex, home.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "# my codex config\nmodel = \"o3\"\n\n[mcp_servers.other]\ncommand = \"x\"\n",
        )
        .unwrap();

        register_bound(Host::Codex, home.path(), &spec()).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my codex config"));
        assert!(text.contains("model = \"o3\""));
        assert!(text.contains("[mcp_servers.other]"));
        assert!(text.contains("[mcp_servers.code-hangar]"));
        assert!(text.contains("[mcp_servers.code-hangar.env]"));
        assert!(text.contains("startup_timeout_sec = 20"));

        let doc = read_toml(&path).unwrap();
        assert!(toml_registered(&doc));

        assert!(unregister_bound(Host::Codex, home.path()).unwrap());
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("[mcp_servers.other]"));
        assert!(!text.contains("code-hangar"));
    }

    #[test]
    fn status_of_absent_config_is_readable_but_unregistered() {
        let home = tempdir().unwrap();
        let st = status(Host::Codex, home.path());
        assert_eq!(st.label, "Codex");
        assert!(!st.config_exists);
        assert!(st.readable);
        assert!(!st.registered);
    }

    #[test]
    fn prepared_reconnect_abort_is_idempotent_and_keeps_the_old_token() {
        let home = tempdir().unwrap();
        let host = Host::Cursor;
        let path = host_config_path(host, home.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, br#"{"mcpServers":{},"ownerSetting":true}"#).unwrap();
        register_bound(host, home.path(), &spec()).unwrap();
        let config_before = fs::read(&path).unwrap();
        let backup_before = fs::read(sidecar_path(&path, BACKUP_SUFFIX)).unwrap();
        let state_before = fs::read(sidecar_path(&path, STATE_SUFFIX)).unwrap();

        let mut rotated = spec();
        rotated.env[0].1 = "tok-456".to_string();
        let prepared = prepare_register_bound(host, home.path(), &rotated).unwrap();
        let contract = prepared.fingerprints().clone();

        recover_change(host, home.path(), &contract, false).unwrap();
        recover_change(host, home.path(), &contract, false).unwrap();
        assert_eq!(fs::read(&path).unwrap(), config_before);
        assert_eq!(
            fs::read(sidecar_path(&path, BACKUP_SUFFIX)).unwrap(),
            backup_before
        );
        assert_eq!(
            fs::read(sidecar_path(&path, STATE_SUFFIX)).unwrap(),
            state_before
        );
        assert_eq!(
            configured_token_hash(host, home.path()).unwrap(),
            Some(token_hash("tok-123"))
        );
        assert!(prepared.can_abort_after_failed_apply().unwrap());
        assert!(!pending_sidecars_present(host, home.path()).unwrap());
    }

    #[test]
    fn config_written_recovery_finalizes_sidecars_idempotently() {
        let home = tempdir().unwrap();
        let host = Host::Claude;
        register_bound(host, home.path(), &spec()).unwrap();
        let mut rotated = spec();
        rotated.env[0].1 = "tok-456".to_string();
        let prepared = prepare_register_bound(host, home.path(), &rotated).unwrap();
        let contract = prepared.fingerprints().clone();

        prepared.apply().unwrap();
        assert_eq!(
            configured_token_hash(host, home.path()).unwrap(),
            Some(token_hash("tok-456"))
        );
        assert!(pending_sidecars_present(host, home.path()).unwrap());

        recover_change(host, home.path(), &contract, true).unwrap();
        recover_change(host, home.path(), &contract, true).unwrap();
        assert_eq!(
            config_fingerprint(host, home.path()).unwrap(),
            contract.config_after
        );
        assert!(!pending_sidecars_present(host, home.path()).unwrap());
    }

    #[test]
    fn predictable_legacy_temp_name_cannot_block_or_capture_atomic_write() {
        let home = tempdir().unwrap();
        let host = Host::Cursor;
        let path = host_config_path(host, home.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, br#"{"mcpServers":{},"ownerSetting":true}"#).unwrap();
        register_bound(host, home.path(), &spec()).unwrap();
        let backup_path = sidecar_path(&path, BACKUP_SUFFIX);
        let state_path = sidecar_path(&path, STATE_SUFFIX);
        let mut rotated = spec();
        rotated.env[0].1 = "tok-456".to_string();
        let prepared = prepare_register_bound(host, home.path(), &rotated).unwrap();

        // Older builds used this deterministic temp name. A pre-created object
        // there must no longer affect the random create-new sibling temp.
        let config_temp = sidecar_path(&path, ".codehangar.tmp");
        fs::create_dir(&config_temp).unwrap();
        prepared.apply().unwrap();
        prepared.finalize().unwrap();
        fs::remove_dir(&config_temp).unwrap();

        assert_eq!(
            configured_token_hash(host, home.path()).unwrap(),
            Some(token_hash("tok-456"))
        );
        assert!(!pending_sidecars_present(host, home.path()).unwrap());
        assert!(backup_path.exists());
        assert!(state_path.exists());
    }

    #[test]
    fn stale_concurrent_plan_never_overwrites_a_newer_host_config() {
        let home = tempdir().unwrap();
        let host = Host::Claude;
        register_bound(host, home.path(), &spec()).unwrap();
        let path = host_config_path(host, home.path());
        let mut rotated = spec();
        rotated.env[0].1 = "tok-456".to_string();
        let prepared = prepare_register_bound(host, home.path(), &rotated).unwrap();
        let newer = br#"{"mcpServers":{"other":{"command":"new-owner-value"}},"theme":"dark"}"#;
        fs::write(&path, newer).unwrap();

        prepared
            .apply()
            .expect_err("the stale plan must fail closed");
        assert_eq!(fs::read(&path).unwrap(), newer);
        assert!(prepared.can_abort_after_failed_apply().unwrap());
        assert!(!pending_sidecars_present(host, home.path()).unwrap());
    }

    #[test]
    fn forged_or_cross_path_registration_state_cannot_authorize_disconnect() {
        let home = tempdir().unwrap();
        let host = Host::Cursor;
        let path = host_config_path(host, home.path());
        register_bound(host, home.path(), &spec()).unwrap();
        let config_before = fs::read(&path).unwrap();
        let state_path = sidecar_path(&path, STATE_SUFFIX);
        let mut state: serde_json::Value =
            serde_json::from_slice(&fs::read(&state_path).unwrap()).unwrap();
        state["host"] = serde_json::Value::String("codex".to_string());
        fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();

        let error = match prepare_unregister_authenticated(host, home.path(), &binding()) {
            Err(error) => error,
            Ok(_) => panic!("a forged sidecar must fail closed"),
        };
        assert!(error.contains("not authenticated"));
        assert_eq!(fs::read(&path).unwrap(), config_before);

        let other_identity = RegistrationBinding::from_hex(
            "33333333333333333333333333333333",
            "2222222222222222222222222222222222222222222222222222222222222222",
        )
        .unwrap();
        let error = match prepare_unregister_authenticated(host, home.path(), &other_identity) {
            Err(error) => error,
            Ok(_) => panic!("state is bound to one immutable DB identity"),
        };
        assert!(error.contains("not authenticated"));
        assert_eq!(fs::read(&path).unwrap(), config_before);
    }

    #[test]
    fn changed_owned_server_entry_fails_closed_even_with_valid_state() {
        let home = tempdir().unwrap();
        let host = Host::Claude;
        let path = host_config_path(host, home.path());
        register_bound(host, home.path(), &spec()).unwrap();
        let state_before = fs::read(sidecar_path(&path, STATE_SUFFIX)).unwrap();
        let mut config: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        config["mcpServers"][SERVER_KEY]["command"] =
            serde_json::Value::String(r"C:\Untrusted\replacement.exe".to_string());
        fs::write(&path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
        let changed = fs::read(&path).unwrap();

        let error = match prepare_unregister_authenticated(host, home.path(), &binding()) {
            Err(error) => error,
            Ok(_) => panic!("changed owned entry must not be removed"),
        };
        assert!(error.contains("changed after registration"));
        assert_eq!(fs::read(&path).unwrap(), changed);
        assert_eq!(
            fs::read(sidecar_path(&path, STATE_SUFFIX)).unwrap(),
            state_before
        );
    }

    #[cfg(windows)]
    #[test]
    fn bound_ancestor_handle_blocks_directory_swap_until_transaction_finishes() {
        let home = tempdir().unwrap();
        let host = Host::Cursor;
        let path = host_config_path(host, home.path());
        register_bound(host, home.path(), &spec()).unwrap();
        let mut rotated = spec();
        rotated.env[0].1 = "tok-ancestor-bound".to_string();
        let prepared = prepare_register_bound(host, home.path(), &rotated).unwrap();
        prepared.apply().unwrap();

        let parent = path.parent().unwrap();
        let replacement = home.path().join("cursor-swapped");
        let error = fs::rename(parent, &replacement)
            .expect_err("the bound directory handle must deny a rename/swap");
        assert!(error.raw_os_error().is_some());
        prepared.finalize().unwrap();
    }

    #[cfg(windows)]
    #[test]
    #[ignore]
    fn host_lock_child_probe() {
        let Some(home) = std::env::var_os("CODEHANGAR_TEST_LOCK_HOME") else {
            return;
        };
        let expect_blocked = std::env::var_os("CODEHANGAR_TEST_LOCK_BLOCKED").is_some();
        let result = acquire_host_operation_lock(Host::Claude, Path::new(&home), "child-probe");
        if expect_blocked {
            assert!(
                result.is_err(),
                "a second process unexpectedly acquired the lease"
            );
        } else {
            assert!(result.is_ok(), "the released lease could not be reacquired");
        }
    }

    #[cfg(windows)]
    #[test]
    fn host_operation_lease_is_exclusive_across_processes_and_released_on_drop() {
        use std::process::Command;

        let home = tempdir().unwrap();
        let lease = acquire_host_operation_lock(Host::Claude, home.path(), "parent-test").unwrap();
        assert!(lease.path().exists());
        assert!(lease.owner_id().contains("parent-test"));
        let binary = std::env::current_exe().unwrap();
        let blocked = Command::new(&binary)
            .args([
                "--exact",
                "tests::host_lock_child_probe",
                "--ignored",
                "--test-threads=1",
            ])
            .env("CODEHANGAR_TEST_LOCK_HOME", home.path())
            .env("CODEHANGAR_TEST_LOCK_BLOCKED", "1")
            .status()
            .unwrap();
        assert!(blocked.success());
        drop(lease);
        let reacquired = Command::new(binary)
            .args([
                "--exact",
                "tests::host_lock_child_probe",
                "--ignored",
                "--test-threads=1",
            ])
            .env("CODEHANGAR_TEST_LOCK_HOME", home.path())
            .status()
            .unwrap();
        assert!(reacquired.success());
    }
}
