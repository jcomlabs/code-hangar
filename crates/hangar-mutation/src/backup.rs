//! Backup engine — **non-destructive**.
//!
//! It only ever creates verified copies; it never moves or deletes a source.
//! Every copy is re-hashed with blake3 after writing and compared to the
//! source; if any copy fails verification the whole backup errors and is not
//! recorded as usable. A backup with `verified = 0` must never be accepted as
//! pre-deletion safety (enforced by callers / the state machine).

use std::collections::HashMap;
#[cfg(test)]
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::bound_fs::{self, BoundFile, FileStamp};

const MANIFEST_NAME: &str = "codehangar-backup-manifest.json";

/// Above this size, refuse a same-volume backup unless explicitly overridden: a
/// same-volume copy does not protect against volume loss and consumes space on
/// the very volume the user is usually trying to free.
const LARGE_BACKUP_BYTES: u64 = 256 * 1024 * 1024;
const BACKUP_FIXED_SPACE_MARGIN: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupLevel {
    Minimal,
    Standard,
    Full,
}

impl BackupLevel {
    fn as_str(self) -> &'static str {
        match self {
            BackupLevel::Minimal => "minimal",
            BackupLevel::Standard => "standard",
            BackupLevel::Full => "full",
        }
    }
}

#[derive(Debug, Error)]
pub enum BackupError {
    #[error("backup io error: {0}")]
    Io(#[from] io::Error),
    #[error("backup journal error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("backup manifest error: {0}")]
    Manifest(#[from] serde_json::Error),
    #[error("insufficient destination space: need {needed} bytes, {available} available")]
    InsufficientSpace { needed: u64, available: u64 },
    #[error("refusing a large backup on the same disk; choose another disk or override")]
    SameVolumeRefused,
    #[error("checksum mismatch after copying {path}")]
    ChecksumMismatch { path: String },
    #[error("backup {0} was not found")]
    BackupNotFound(i64),
    #[error("backup {0} is not a verified backup")]
    BackupNotVerified(i64),
    #[error("backup manifest is missing or unreadable: {0}")]
    ManifestUnreadable(String),
    #[error("unsafe backup path component in {0}")]
    UnsafeRelative(String),
    #[error("refusing to overwrite an existing backup target: {0}")]
    DestinationExists(String),
    #[error("backup destination refused: {0}")]
    DestinationRefused(String),
    #[error("refusing to back up through a reparse point: {0}")]
    SourceIsReparse(String),
    #[error("refusing to back up cloud-backed source: {0}")]
    SourceIsCloudFile(String),
}

/// A file to include in the backup: absolute `source` plus the path it should
/// occupy under the backup root (preserving the original layout).
#[derive(Debug, Clone)]
pub struct BackupItem {
    pub source: PathBuf,
    pub relative: String,
    /// Identity captured by the reviewed inventory. The backup executor must
    /// bind this exact object before reading any bytes.
    pub expected_source_stamp: FileStamp,
    /// blake3 reviewed through the same safe bound-source primitive. Backup is
    /// non-destructive, but it must still refuse content drift before creating
    /// any payload because this manifest later gates destructive movement.
    pub expected_source_hash: String,
}

pub struct BackupRequest<'a> {
    pub level: BackupLevel,
    pub source_root: &'a Path,
    pub destination_root: &'a Path,
    pub items: Vec<BackupItem>,
    /// The Operation Plan JSON that triggered this backup (recorded verbatim).
    pub plan_json: String,
    /// Allow a same-volume destination even for a large backup.
    pub allow_same_volume: bool,
}

#[derive(Debug, Clone)]
pub struct BackupResult {
    pub backup_id: i64,
    pub manifest_path: PathBuf,
    pub total_bytes: u64,
    pub verified: bool,
}

#[derive(Debug, Serialize)]
struct ManifestEntry {
    original_path: String,
    backup_path: String,
    bytes: u64,
    blake3: String,
    source_stamp: FileStamp,
    backup_stamp: FileStamp,
}

#[derive(Debug, Serialize)]
struct BackupManifest {
    schema: &'static str,
    level: BackupLevel,
    created_at: String,
    source_root: String,
    total_bytes: u64,
    verified: bool,
    files: Vec<ManifestEntry>,
    plan_json: String,
}

/// Create a verified backup. Copies every item, verifies each copy with blake3,
/// writes `codehangar-backup-manifest.json`, and records a `backup` journal row.
/// Returns an error (writing nothing usable) if space is insufficient, a large
/// same-volume backup is refused, or any copy fails verification.
pub fn create_backup(
    conn: &Connection,
    request: BackupRequest<'_>,
) -> Result<BackupResult, BackupError> {
    bound_fs::validate_local_mutation_path(request.source_root)?;
    bound_fs::validate_local_mutation_path(request.destination_root)?;
    bound_fs::validate_local_mutation_path(&request.destination_root.join(MANIFEST_NAME))?;
    let total_bytes: u64 = request
        .items
        .iter()
        .map(|item| item.expected_source_stamp.bytes)
        .sum();

    // Refuse a destination that is itself protected/sensitive. Existing payload
    // or manifest files are handled atomically by CREATE_NEW; a path-based
    // exists/free-space probe here would reintroduce a check/use race and could
    // traverse an unbound junction.
    let dest_text = request.destination_root.to_string_lossy().to_string();
    if hangar_protect::is_strong_protected_path(&dest_text)
        || hangar_protect::is_sensitive_path(&dest_text)
        || hangar_protect::protected_level_for_path(&dest_text).is_some()
    {
        return Err(BackupError::DestinationRefused(format!(
            "{dest_text} is a protected or sensitive location"
        )));
    }
    // Refuse before creating the destination. Creating a backup folder inside
    // the reviewed tree is itself an unwanted mutation of the source.
    if is_inside(request.destination_root, request.source_root) {
        return Err(BackupError::DestinationRefused(
            "the backup destination is inside the source folder".to_string(),
        ));
    }
    let containment_guard = bound_fs::bind_destination_outside_directory(
        request.destination_root,
        request.source_root,
    )?;

    // Validate and bind every reviewed source before any destination-side
    // creation. Hash as well as identity/size/mtime is checked now, and then
    // checked again on the exact handle used for each copy.
    for item in &request.items {
        bound_fs::validate_local_mutation_path(&item.source)?;
        let mut source = BoundFile::open_read(&item.source)?;
        source.verify_stamp(&item.expected_source_stamp)?;
        source.verify_hash(&item.expected_source_hash)?;
    }

    if !request.allow_same_volume
        && total_bytes >= LARGE_BACKUP_BYTES
        && containment_guard.same_volume()
    {
        return Err(BackupError::SameVolumeRefused);
    }

    // The nearest existing destination ancestor remains handle-bound while the
    // volume query runs, so a junction/mount swap cannot redirect this check.
    if total_bytes > 0 {
        let needed = total_bytes
            .saturating_add(total_bytes / 20)
            .saturating_add(BACKUP_FIXED_SPACE_MARGIN);
        let available = containment_guard.available_space_bytes().ok_or_else(|| {
            BackupError::DestinationRefused(
                "cannot prove free space on the bound backup destination volume".to_string(),
            )
        })?;
        if available < needed {
            return Err(BackupError::InsufficientSpace { needed, available });
        }
    }

    let mut entries = Vec::with_capacity(request.items.len());
    let mut copied_bytes = 0u64;
    for item in &request.items {
        // No-follow: never back up through a reparse point (symlink/junction) — it
        // could resolve outside the source and is not the file the plan inspected. A cloud
        // placeholder (is_reparse=0, reparse_kind='cloud_placeholder') is refused here too:
        // copying it would force a network hydration or capture only a stub.
        // Engine-level path safety (defense-in-depth, not just the API caller): only
        // plain components, so the destination can never escape destination_root.
        let dest = safe_dest(request.destination_root, &item.relative)?;
        // One source handle binds no-follow classification, file identity and
        // content. The destination uses CREATE_NEW and is copied, flushed and
        // verified through its still-open handle, so a raced occupant is never
        // overwritten and a replaced source is never silently accepted.
        let mut source = BoundFile::open_read(&item.source)?;
        source.verify_stamp(&item.expected_source_stamp)?;
        let source_hash = source.verify_hash(&item.expected_source_hash)?;
        let source_stamp = source.stamp().clone();
        let created = bound_fs::copy_to_new(&mut source, &dest, &source_hash)?;
        let dest_hash = created.hash().to_string();
        let backup_stamp = created.stamp().clone();
        let bytes = backup_stamp.bytes;
        copied_bytes += bytes;
        entries.push(ManifestEntry {
            original_path: item.source.to_string_lossy().to_string(),
            backup_path: dest.to_string_lossy().to_string(),
            bytes,
            blake3: dest_hash,
            source_stamp,
            backup_stamp,
        });
    }

    // Reaching here means every copy verified.
    let verified = true;
    let created_at = chrono::Utc::now().to_rfc3339();
    let manifest = BackupManifest {
        schema: "backup_manifest/1",
        level: request.level,
        created_at: created_at.clone(),
        source_root: request.source_root.to_string_lossy().to_string(),
        total_bytes: copied_bytes,
        verified,
        files: entries,
        plan_json: request.plan_json,
    };
    let manifest_path = request.destination_root.join(MANIFEST_NAME);
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    bound_fs::write_new(&manifest_path, &manifest_bytes)?;

    conn.execute(
        "INSERT INTO backup(level, destination, manifest_path, total_bytes, verified, created_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            request.level.as_str(),
            request.destination_root.to_string_lossy(),
            manifest_path.to_string_lossy(),
            copied_bytes as i64,
            verified as i64,
            created_at
        ],
    )?;
    let backup_id = conn.last_insert_rowid();

    Ok(BackupResult {
        backup_id,
        manifest_path,
        total_bytes: copied_bytes,
        verified,
    })
}

#[cfg(test)]
fn validate_backup_identity_fields(
    relative: &str,
    is_reparse: bool,
    reparse_kind: Option<&str>,
) -> Result<(), BackupError> {
    if crate::is_cloud_reparse_kind(reparse_kind) {
        Err(BackupError::SourceIsCloudFile(relative.to_string()))
    } else if is_reparse {
        Err(BackupError::SourceIsReparse(relative.to_string()))
    } else {
        Ok(())
    }
}

/// Read-only view of a backup manifest entry (for coverage verification).
#[derive(Debug, Deserialize)]
struct ManifestReadEntry {
    original_path: String,
    backup_path: String,
    blake3: String,
    #[serde(default)]
    source_stamp: Option<FileStamp>,
    #[serde(default)]
    backup_stamp: Option<FileStamp>,
}

#[derive(Debug, Deserialize)]
struct ManifestRead {
    verified: bool,
    files: Vec<ManifestReadEntry>,
}

/// The recorded location and hash of one backed-up file.
#[derive(Debug, Clone)]
pub struct BackupCopy {
    /// Absolute path of the backup payload on disk.
    pub backup_path: String,
    /// blake3 the backup recorded for the copy.
    pub blake3: String,
    source_stamp: Option<FileStamp>,
    pub(crate) backup_stamp: Option<FileStamp>,
}

/// A backup confirmed verified, with the per-source-file copies it recorded.
/// Used to enforce the Gate-3 invariant: no move / permanent delete without a
/// verified backup that covers the item.
#[derive(Debug, Clone)]
pub struct VerifiedBackup {
    pub backup_id: i64,
    pub manifest_path: String,
    /// Normalised original source path -> recorded backup copy (path + blake3).
    pub copies: HashMap<String, BackupCopy>,
}

/// Opaque collection of verified, still-open backup payload handles. Keeping
/// this guard alive prevents a payload from being replaced between the move
/// gate and the last corresponding source rename.
#[derive(Debug)]
pub struct VerifiedBackupPayloadGuard {
    _payloads: Vec<BoundFile>,
}

impl VerifiedBackup {
    /// True if this backup contains a verified copy of `source_path`.
    pub fn covers(&self, source_path: &str) -> bool {
        self.copies.contains_key(&normalize_source_key(source_path))
    }

    /// The recorded blake3 of the backed-up copy of `source_path`, if covered.
    pub fn hash_for(&self, source_path: &str) -> Option<&str> {
        self.copies
            .get(&normalize_source_key(source_path))
            .map(|copy| copy.blake3.as_str())
    }

    /// The verified backup payload metadata for one original source path.
    pub fn copy_for(&self, source_path: &str) -> Option<&BackupCopy> {
        self.copies.get(&normalize_source_key(source_path))
    }

    /// Prove the backup can actually restore `source_path`: the recorded payload
    /// file must still exist on disk AND still hash to the recorded blake3. This is
    /// the guarantee a path-string (or even a manifest-hash) match cannot give — a
    /// manifest can outlive its payload (volume gone, file truncated, antivirus
    /// quarantine). Call this before an irreversible delete of the last live copy.
    pub fn verify_payload(&self, source_path: &str) -> Result<(), BackupError> {
        let _payload = self.bind_payload(source_path)?;
        Ok(())
    }

    pub fn bind_payloads(
        &self,
        source_paths: &[String],
    ) -> Result<VerifiedBackupPayloadGuard, BackupError> {
        let mut payloads = Vec::with_capacity(source_paths.len());
        for source_path in source_paths {
            payloads.push(self.bind_payload(source_path)?);
        }
        Ok(VerifiedBackupPayloadGuard {
            _payloads: payloads,
        })
    }

    /// Prove and hold the exact backup payload while an irreversible action is
    /// in flight. Legacy manifests without handle identity cannot authorize it.
    pub(crate) fn bind_payload(&self, source_path: &str) -> Result<BoundFile, BackupError> {
        let copy = self
            .copies
            .get(&normalize_source_key(source_path))
            .ok_or_else(|| {
                BackupError::ManifestUnreadable(format!("not covered: {source_path}"))
            })?;
        let expected_stamp = copy.backup_stamp.as_ref().ok_or_else(|| {
            BackupError::ManifestUnreadable(format!(
                "backup payload has no handle identity proof: {}",
                copy.backup_path
            ))
        })?;
        let mut payload = BoundFile::open_read(Path::new(&copy.backup_path))?;
        payload.verify_stamp(expected_stamp)?;
        payload.verify_hash(&copy.blake3)?;
        Ok(payload)
    }
}

impl BackupCopy {
    pub fn source_stamp(&self) -> Option<&FileStamp> {
        self.source_stamp.as_ref()
    }
}

/// Normalise a source path for manifest lookups (separator/case-insensitive on
/// Windows so the same file matches however the path was spelled).
fn normalize_source_key(path: &str) -> String {
    path.replace('\\', "/").to_ascii_lowercase()
}

/// Load a backup row, require `verified = 1`, re-read its manifest from disk,
/// require the manifest itself reports `verified: true`, and return the per-file
/// hashes. Fails (so no move/delete proceeds) if the backup is missing,
/// unverified, or its manifest is gone/corrupt.
pub fn load_verified_backup(
    conn: &Connection,
    backup_id: i64,
) -> Result<VerifiedBackup, BackupError> {
    let row = conn
        .query_row(
            "SELECT manifest_path, verified FROM backup WHERE id = ?1",
            [backup_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? == 1)),
        )
        .optional()?;
    let (manifest_path, verified) = row.ok_or(BackupError::BackupNotFound(backup_id))?;
    if !verified {
        return Err(BackupError::BackupNotVerified(backup_id));
    }
    let manifest_path_ref = Path::new(&manifest_path);
    bound_fs::validate_local_mutation_path(manifest_path_ref)
        .map_err(|err| BackupError::ManifestUnreadable(format!("{manifest_path}: {err}")))?;
    let mut manifest_file = BoundFile::open_read(manifest_path_ref)
        .map_err(|err| BackupError::ManifestUnreadable(format!("{manifest_path}: {err}")))?;
    let bytes = manifest_file
        .read_all()
        .map_err(|err| BackupError::ManifestUnreadable(format!("{manifest_path}: {err}")))?;
    let manifest: ManifestRead = serde_json::from_slice(&bytes)
        .map_err(|err| BackupError::ManifestUnreadable(format!("{manifest_path}: {err}")))?;
    if !manifest.verified {
        return Err(BackupError::BackupNotVerified(backup_id));
    }
    let copies = manifest
        .files
        .into_iter()
        .map(|entry| {
            (
                normalize_source_key(&entry.original_path),
                BackupCopy {
                    backup_path: entry.backup_path,
                    blake3: entry.blake3,
                    source_stamp: entry.source_stamp,
                    backup_stamp: entry.backup_stamp,
                },
            )
        })
        .collect();
    Ok(VerifiedBackup {
        backup_id,
        manifest_path,
        copies,
    })
}

/// Resolve `relative` under `root`, rejecting any component that is not a plain
/// path segment (`..`, absolute roots, Windows drive prefixes). Because only
/// `Normal`/`CurDir` components are allowed, the result can never escape `root`.
fn safe_dest(root: &Path, relative: &str) -> Result<PathBuf, BackupError> {
    let normalized = relative.replace('\\', "/");
    let rel = Path::new(normalized.trim_start_matches('/'));
    for component in rel.components() {
        match component {
            std::path::Component::Normal(_) | std::path::Component::CurDir => {}
            _ => return Err(BackupError::UnsafeRelative(relative.to_string())),
        }
    }
    Ok(root.join(rel))
}

/// Pure lexical component-boundary containment. Callers validate both absolute
/// local paths before this function, and bound_fs rejects any reparse ancestor
/// before creation, so containment never needs path-following canonicalization.
pub(crate) fn is_inside(child: &Path, ancestor: &Path) -> bool {
    fn normalized(path: &Path) -> String {
        let text = path.to_string_lossy();
        let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
        let text = text.replace('/', "\\");
        #[cfg(windows)]
        let text = text.to_ascii_lowercase();
        text.trim_end_matches('\\').to_string()
    }
    let child = normalized(child);
    let ancestor = normalized(ancestor);
    child == ancestor
        || child
            .strip_prefix(&ancestor)
            .is_some_and(|suffix| suffix.starts_with('\\'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn journaled_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::ensure_journal_schema(&conn).unwrap();
        conn
    }

    fn reviewed_item(source: PathBuf, relative: &str) -> BackupItem {
        let (expected_source_stamp, expected_source_hash) =
            crate::bound_fs::inspect_local_mutation_file(&source).unwrap();
        BackupItem {
            source,
            relative: relative.to_string(),
            expected_source_stamp,
            expected_source_hash,
        }
    }

    #[test]
    fn backup_copies_verifies_and_records() {
        let source = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        fs::create_dir_all(source.path().join("docs")).unwrap();
        fs::write(source.path().join("README.md"), b"hello readme").unwrap();
        fs::write(source.path().join("docs/overview.md"), b"overview body").unwrap();

        let conn = journaled_conn();
        let result = create_backup(
            &conn,
            BackupRequest {
                level: BackupLevel::Standard,
                source_root: source.path(),
                destination_root: &dest.path().join("backup"),
                items: vec![
                    reviewed_item(source.path().join("README.md"), "README.md"),
                    reviewed_item(source.path().join("docs/overview.md"), "docs/overview.md"),
                ],
                plan_json: "{\"schema\":\"operation_plan/1\"}".to_string(),
                allow_same_volume: true,
            },
        )
        .unwrap();

        assert!(result.verified);
        assert!(result.total_bytes > 0);

        // Copies exist and are byte-identical.
        let copied = fs::read(dest.path().join("backup/README.md")).unwrap();
        assert_eq!(copied, b"hello readme");

        // Manifest exists, records both files, and its recorded hash matches a
        // re-hash of the written copy (proves verify-after-write).
        let manifest_text = fs::read_to_string(&result.manifest_path).unwrap();
        assert!(manifest_text.contains("backup_manifest/1"));
        assert!(manifest_text.contains("docs/overview.md"));
        assert!(manifest_text.contains("\"verified\": true"));
        let verified_backup = load_verified_backup(&conn, result.backup_id).unwrap();
        let source_path = source.path().join("README.md");
        let manifest_stamp = verified_backup
            .copy_for(&source_path.to_string_lossy())
            .and_then(BackupCopy::source_stamp)
            .expect("a new verified manifest must preserve the reviewed source stamp");
        assert_eq!(
            manifest_stamp,
            &crate::inspect_local_mutation_file(&source_path).unwrap().0
        );

        // Journal row recorded as verified.
        let (verified, total): (i64, i64) = conn
            .query_row(
                "SELECT verified, total_bytes FROM backup WHERE id = ?1",
                [result.backup_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(verified, 1);
        assert_eq!(total as u64, result.total_bytes);
    }

    #[test]
    fn empty_backup_is_verified_and_recorded() {
        let source = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let conn = journaled_conn();
        let result = create_backup(
            &conn,
            BackupRequest {
                level: BackupLevel::Minimal,
                source_root: source.path(),
                destination_root: &dest.path().join("empty-backup"),
                items: Vec::new(),
                plan_json: "{}".to_string(),
                allow_same_volume: true,
            },
        )
        .unwrap();
        assert!(result.verified);
        assert_eq!(result.total_bytes, 0);
        assert!(result.manifest_path.exists());
    }

    #[test]
    fn same_volume_uses_bound_volume_identity() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let guard = bound_fs::bind_destination_outside_directory(destination.path(), source.path())
            .unwrap();
        assert!(guard.same_volume());
        assert!(guard.available_space_bytes().is_some());
    }

    #[test]
    fn safe_dest_rejects_parent_traversal() {
        let root = Path::new("backup-root");
        assert!(safe_dest(root, "a/b/c.txt").is_ok());
        assert!(matches!(
            safe_dest(root, "../escape").unwrap_err(),
            BackupError::UnsafeRelative(_)
        ));
        assert!(matches!(
            safe_dest(root, "a/../../escape").unwrap_err(),
            BackupError::UnsafeRelative(_)
        ));
    }

    #[test]
    fn backup_identity_gate_refuses_both_cloud_states() {
        for (is_reparse, kind) in [(true, "cloud_local"), (false, "cloud_placeholder")] {
            assert!(matches!(
                validate_backup_identity_fields("cloud.bin", is_reparse, Some(kind)),
                Err(BackupError::SourceIsCloudFile(_))
            ));
        }
    }

    fn one_file_request<'a>(
        source_root: &'a Path,
        dest: &'a Path,
        file: PathBuf,
    ) -> BackupRequest<'a> {
        BackupRequest {
            level: BackupLevel::Standard,
            source_root,
            destination_root: dest,
            items: vec![reviewed_item(file, "f.bin")],
            plan_json: "{}".to_string(),
            allow_same_volume: true,
        }
    }

    #[test]
    fn backup_refuses_destination_inside_source() {
        let source = tempfile::tempdir().unwrap();
        let file = source.path().join("f.bin");
        fs::write(&file, b"x").unwrap();
        let inside = source.path().join("inside-backup");
        let conn = journaled_conn();
        let err = create_backup(&conn, one_file_request(source.path(), &inside, file)).unwrap_err();
        assert!(matches!(err, BackupError::DestinationRefused(_)));
        assert!(
            !inside.exists(),
            "refusing an unsafe destination must not create it inside the source"
        );
    }

    #[test]
    fn backup_refuses_to_clobber_an_existing_manifest() {
        let source = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let file = source.path().join("f.bin");
        fs::write(&file, b"x").unwrap();
        let conn = journaled_conn();
        create_backup(
            &conn,
            one_file_request(source.path(), dest.path(), file.clone()),
        )
        .unwrap();
        // A second backup into the same destination is refused (would clobber).
        let err =
            create_backup(&conn, one_file_request(source.path(), dest.path(), file)).unwrap_err();
        assert!(match err {
            BackupError::DestinationExists(_) => true,
            BackupError::Io(error) => error.kind() == std::io::ErrorKind::AlreadyExists,
            _ => false,
        });
    }

    #[test]
    fn backup_rejects_same_bytes_with_a_different_reviewed_identity() {
        let source = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let file = source.path().join("f.bin");
        fs::write(&file, b"same bytes").unwrap();
        let reviewed = reviewed_item(file.clone(), "f.bin");
        fs::remove_file(&file).unwrap();
        fs::write(&file, b"same bytes").unwrap();

        let conn = journaled_conn();
        let request = BackupRequest {
            level: BackupLevel::Standard,
            source_root: source.path(),
            destination_root: &dest.path().join("backup"),
            items: vec![reviewed],
            plan_json: "{}".to_string(),
            allow_same_volume: true,
        };
        let error = create_backup(&conn, request).unwrap_err();
        assert!(error.to_string().contains("identity changed"));
        assert!(!dest.path().join("backup").exists());
    }
}
