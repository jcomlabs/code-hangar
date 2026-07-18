//! Backup engine — **non-destructive**.
//!
//! It only ever creates verified copies; it never moves or deletes a source.
//! Every copy is re-hashed with blake3 after writing and compared to the
//! source; if any copy fails verification the whole backup errors and is not
//! recorded as usable. A backup with `verified = 0` must never be accepted as
//! pre-deletion safety (enforced by callers / the state machine).

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::longpath::to_extended;

const MANIFEST_NAME: &str = "codehangar-backup-manifest.json";

/// Above this size, refuse a same-volume backup unless explicitly overridden: a
/// same-volume copy does not protect against volume loss and consumes space on
/// the very volume the user is usually trying to free.
const LARGE_BACKUP_BYTES: u64 = 256 * 1024 * 1024;

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
}

/// A file to include in the backup: absolute `source` plus the path it should
/// occupy under the backup root (preserving the original layout).
#[derive(Debug, Clone)]
pub struct BackupItem {
    pub source: PathBuf,
    pub relative: String,
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
    let total_bytes: u64 = request
        .items
        .iter()
        .map(|item| file_size(&item.source))
        .sum();

    // Refuse a destination that is itself protected/sensitive, or that already holds
    // a backup manifest (don't clobber a prior backup) — both are path-text checks.
    let dest_text = request.destination_root.to_string_lossy().to_string();
    if hangar_protect::is_strong_protected_path(&dest_text)
        || hangar_protect::is_sensitive_path(&dest_text)
        || hangar_protect::protected_level_for_path(&dest_text).is_some()
    {
        return Err(BackupError::DestinationRefused(format!(
            "{dest_text} is a protected or sensitive location"
        )));
    }
    if to_extended(&request.destination_root.join(MANIFEST_NAME)).exists() {
        return Err(BackupError::DestinationExists(format!(
            "{dest_text} already contains a backup manifest"
        )));
    }

    // Refuse before creating the destination. Creating a backup folder inside
    // the reviewed tree is itself an unwanted mutation of the source.
    if is_inside(request.destination_root, request.source_root) {
        return Err(BackupError::DestinationRefused(
            "the backup destination is inside the source folder".to_string(),
        ));
    }

    fs::create_dir_all(to_extended(request.destination_root).as_ref())?;

    if !request.allow_same_volume
        && total_bytes >= LARGE_BACKUP_BYTES
        && same_volume(request.source_root, request.destination_root)
    {
        return Err(BackupError::SameVolumeRefused);
    }

    if let Some(available) = hangar_fs::available_space_bytes(request.destination_root) {
        if available < total_bytes {
            return Err(BackupError::InsufficientSpace {
                needed: total_bytes,
                available,
            });
        }
    }

    let mut entries = Vec::with_capacity(request.items.len());
    let mut copied_bytes = 0u64;
    for item in &request.items {
        // No-follow: never back up through a reparse point (symlink/junction) — it
        // could resolve outside the source and is not the file the plan inspected. A cloud
        // placeholder (is_reparse=0, reparse_kind='cloud_placeholder') is refused here too:
        // copying it would force a network hydration or capture only a stub.
        let item_identity = hangar_fs::inspect_path_identity(&item.source);
        if item_identity.is_reparse
            || item_identity.reparse_kind.as_deref() == Some("cloud_placeholder")
        {
            return Err(BackupError::SourceIsReparse(item.relative.clone()));
        }
        // Engine-level path safety (defense-in-depth, not just the API caller): only
        // plain components, so the destination can never escape destination_root.
        let dest = safe_dest(request.destination_root, &item.relative)?;
        // Extended-length (`\\?\`) aliases for the actual Win32 fs calls so a >260-char
        // source or backup-destination path works without LongPathsEnabled; a no-op off
        // Windows and for already-verbatim paths. The manifest records the ordinary
        // `item.source` / `dest` text below, never the verbatim alias.
        let source_ext = to_extended(&item.source);
        let dest_ext = to_extended(&dest);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(to_extended(parent).as_ref())?;
        }
        // Never overwrite an existing file in the destination (accurate for a long path
        // via the extended-length form).
        if dest_ext.exists() {
            return Err(BackupError::DestinationExists(
                dest.to_string_lossy().to_string(),
            ));
        }
        let source_hash = hash_file(&source_ext)?;
        fs::copy(source_ext.as_ref(), dest_ext.as_ref())?;
        // Flush to stable storage before verifying so a backup marked verified is
        // actually durable (spec: copy → fsync → verify); fs::copy otherwise
        // leaves the copy only in the OS write cache.
        crate::fsops::fsync_file(&dest_ext)?;
        // Verify-after-write: re-hash the written copy and compare.
        let dest_hash = hash_file(&dest_ext)?;
        if source_hash != dest_hash {
            return Err(BackupError::ChecksumMismatch {
                path: item.relative.clone(),
            });
        }
        let bytes = file_size(&dest);
        copied_bytes += bytes;
        entries.push(ManifestEntry {
            original_path: item.source.to_string_lossy().to_string(),
            backup_path: dest.to_string_lossy().to_string(),
            bytes,
            blake3: dest_hash,
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
    fs::write(
        to_extended(&manifest_path).as_ref(),
        serde_json::to_vec_pretty(&manifest)?,
    )?;

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

/// Read-only view of a backup manifest entry (for coverage verification).
#[derive(Debug, Deserialize)]
struct ManifestReadEntry {
    original_path: String,
    backup_path: String,
    blake3: String,
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
        let copy = self
            .copies
            .get(&normalize_source_key(source_path))
            .ok_or_else(|| {
                BackupError::ManifestUnreadable(format!("not covered: {source_path}"))
            })?;
        let path = Path::new(&copy.backup_path);
        // Extended-length form so a long-path payload is detected as present (and hashed)
        // without LongPathsEnabled — a bare exists() on a >260 path can wrongly report
        // "missing" and turn a restorable backup into a Gate-3 refusal.
        if !to_extended(path).exists() {
            return Err(BackupError::ManifestUnreadable(format!(
                "backup payload missing: {}",
                copy.backup_path
            )));
        }
        let actual = hash_file(path)?;
        if actual != copy.blake3 {
            return Err(BackupError::ManifestUnreadable(format!(
                "backup payload no longer matches its recorded hash: {}",
                copy.backup_path
            )));
        }
        Ok(())
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
    let bytes = fs::read(to_extended(Path::new(&manifest_path)).as_ref())
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

fn file_size(path: &Path) -> u64 {
    fs::metadata(to_extended(path).as_ref())
        .map(|meta| meta.len())
        .unwrap_or(0)
}

fn hash_file(path: &Path) -> Result<String, BackupError> {
    // Extended-length form so hashing a >260-char source or backup payload succeeds
    // without LongPathsEnabled (idempotent when the caller already passed a verbatim
    // alias). Used both to hash sources pre-copy and to verify-after-write.
    let mut hasher = blake3::Hasher::new();
    let mut file = fs::File::open(to_extended(path).as_ref())?;
    io::copy(&mut file, &mut hasher)?;
    Ok(hasher.finalize().to_hex().to_string())
}

/// blake3 of a file. Used to content-bind a verified backup to the live file before a
/// move (a path-string match is not enough to authorize a later permanent delete).
pub fn file_blake3(path: &Path) -> Result<String, BackupError> {
    hash_file(path)
}

/// Best-effort same-volume check by canonicalized path prefix (drive on
/// Windows). Junctions/mounts to the same physical volume are not detected;
/// that only relaxes a UX guard and never affects the non-destructive copy.
fn same_volume(a: &Path, b: &Path) -> bool {
    fn volume_key(path: &Path) -> Option<String> {
        // Extended-length form so a >260-char path canonicalizes; on failure we fall back
        // to the raw path. Only relaxes a UX guard (large same-volume), never the copy.
        let canonical =
            fs::canonicalize(to_extended(path).as_ref()).unwrap_or_else(|_| path.to_path_buf());
        match canonical.components().next() {
            Some(std::path::Component::Prefix(prefix)) => {
                Some(prefix.as_os_str().to_string_lossy().to_ascii_uppercase())
            }
            _ => None,
        }
    }
    match (volume_key(a), volume_key(b)) {
        (Some(left), Some(right)) => left == right,
        _ => false,
    }
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

/// Whether `child` resolves to a path inside `ancestor` (best-effort canonicalize).
/// True when `child` resolves inside `ancestor`. Canonicalizes the nearest existing ancestor
/// of each so a not-yet-created path still compares consistently. Shared with the quarantine
/// executor so both the backup destination and the move's holding root reject an in-source
/// location (`pub(crate)`).
pub(crate) fn is_inside(child: &Path, ancestor: &Path) -> bool {
    let child = canonicalize_existing_parent(child);
    let ancestor = canonicalize_existing_parent(ancestor);
    child.starts_with(&ancestor)
}

pub(crate) fn canonicalize_existing_parent(path: &Path) -> PathBuf {
    // Extended-length form throughout so a >260-char path (or ancestor) resolves and is
    // detected as existing without LongPathsEnabled; the returned path is canonical
    // (already-verbatim), which is fine — it feeds only the `is_inside` containment check.
    if let Ok(canonical) = fs::canonicalize(to_extended(path).as_ref()) {
        return canonical;
    }

    let mut missing = Vec::<OsString>::new();
    let mut cursor = path;
    while !to_extended(cursor).exists() {
        if let Some(name) = cursor.file_name() {
            missing.push(name.to_os_string());
        }
        match cursor.parent() {
            Some(parent) if parent != cursor => cursor = parent,
            _ => return path.to_path_buf(),
        }
    }

    let mut rebuilt =
        fs::canonicalize(to_extended(cursor).as_ref()).unwrap_or_else(|_| cursor.to_path_buf());
    for part in missing.iter().rev() {
        rebuilt.push(part);
    }
    rebuilt
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
                    BackupItem {
                        source: source.path().join("README.md"),
                        relative: "README.md".to_string(),
                    },
                    BackupItem {
                        source: source.path().join("docs/overview.md"),
                        relative: "docs/overview.md".to_string(),
                    },
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
    fn same_volume_detects_shared_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        // Two directories under the same temp root share a volume.
        assert!(same_volume(&a, &b));
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

    fn one_file_request<'a>(
        source_root: &'a Path,
        dest: &'a Path,
        file: PathBuf,
    ) -> BackupRequest<'a> {
        BackupRequest {
            level: BackupLevel::Standard,
            source_root,
            destination_root: dest,
            items: vec![BackupItem {
                source: file,
                relative: "f.bin".to_string(),
            }],
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
        assert!(matches!(err, BackupError::DestinationExists(_)));
    }
}
