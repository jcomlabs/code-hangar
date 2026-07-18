//! ADVERSARIAL EDGE-CASE BATTERY — scratch integration tests, UNCOMMITTED, for lead review.
//!
//! Targets the backup/quarantine/restore/purge pipeline through the crate's public API,
//! mirroring the fixture patterns of the unit tests in `restore.rs` / `quarantine.rs` /
//! `purge.rs`.
//!
//! Run:
//!   cargo test -p hangar-mutation --all-features --test adversarial_battery -- --nocapture
//!
//! Filesystem writes stay strictly under:
//!   - `tempfile::tempdir()` (auto-cleaned),
//!   - the repo-local `.local/adversarial-qa` scratch area (guard-cleaned per test), and
//!   - for the cross-volume test only, a self-cleaning `D:\codehangar-adv-qa-<pid>` dir
//!     (skipped when D:\ is absent/unwritable or temp already lives on D:).
//!
//! No real user data is ever touched.
#![cfg(feature = "mutation")]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rusqlite::Connection;

use hangar_mutation::{
    create_backup, ensure_journal_schema, load_verified_backup, permanent_delete_entry, quarantine,
    restore_entry, BackupItem, BackupLevel, BackupRequest, ConfirmAction, ConfirmTokenStore,
    ItemOutcome, PurgeError, QuarantineItem, QuarantineRequest, RestoreOutcome,
};

// ---------------------------------------------------------------------------
// Shared fixtures / helpers (same shape as the crate's own unit tests)
// ---------------------------------------------------------------------------

fn journaled_conn() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    ensure_journal_schema(&conn).unwrap();
    conn
}

fn item(source: &Path, relative: &str) -> QuarantineItem {
    QuarantineItem {
        source: source.to_path_buf(),
        relative: relative.to_string(),
        backup_hash: None,
    }
}

fn basic_request(quarantine_root: &Path, items: Vec<QuarantineItem>) -> QuarantineRequest<'_> {
    QuarantineRequest {
        quarantine_root,
        items,
        plan_json: "{}".to_string(),
        target_node_id: None,
        target_fingerprint: None,
        backup_id: 0,
        cleanup_root: None,
        include_protected: false,
        reparse_links: Vec::new(),
    }
}

/// Remove a directory tree on drop (best effort), so scratch areas are cleaned even
/// when an assert unwinds.
struct DirGuard(PathBuf);
impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = clear_readonly_recursive(&self.0);
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn clear_readonly_recursive(path: &Path) -> std::io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() {
        for entry in fs::read_dir(path)? {
            let _ = clear_readonly_recursive(&entry?.path());
        }
    } else if meta.permissions().readonly() {
        let mut perms = meta.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        let _ = fs::set_permissions(path, perms);
    }
    Ok(())
}

/// Per-test scratch dir inside the repo-local `.local/adversarial-qa` area — the only
/// non-tempdir write location this battery is allowed to use (besides the sanctioned
/// D:\ cross-volume probe).
fn adversarial_scratch(name: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join(".local")
        .join("adversarial-qa");
    let dir = root.join(format!("{name}-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn entries_for_op(conn: &Connection, operation_id: i64) -> Vec<(i64, String, String, String)> {
    let mut stmt = conn
        .prepare(
            "SELECT id, original_path, quarantine_path, status FROM quarantine_entry
             WHERE operation_id = ?1 ORDER BY id",
        )
        .unwrap();
    let rows = stmt
        .query_map([operation_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .unwrap();
    rows.map(Result::unwrap).collect()
}

fn entry_status(conn: &Connection, entry_id: i64) -> String {
    conn.query_row(
        "SELECT status FROM quarantine_entry WHERE id = ?1",
        [entry_id],
        |row| row.get(0),
    )
    .unwrap()
}

fn op_status(conn: &Connection, operation_id: i64) -> String {
    conn.query_row(
        "SELECT status FROM operation WHERE id = ?1",
        [operation_id],
        |row| row.get(0),
    )
    .unwrap()
}

fn item_status_counts(conn: &Connection, operation_id: i64, action: &str) -> (i64, i64) {
    let done: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM operation_item WHERE operation_id = ?1 AND action = ?2 AND status = 'done'",
            rusqlite::params![operation_id, action],
            |row| row.get(0),
        )
        .unwrap();
    let failed: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM operation_item WHERE operation_id = ?1 AND action = ?2 AND status = 'failed'",
            rusqlite::params![operation_id, action],
            |row| row.get(0),
        )
        .unwrap();
    (done, failed)
}

/// Backup-then-quarantine fixture (the Gate-3 shape used by purge unit tests): returns
/// (entry_id, held_path, backup_payload_path) per relative, in input order.
fn backed_quarantine(
    conn: &Connection,
    root: &Path,
    rels: &[&str],
) -> Vec<(i64, PathBuf, PathBuf)> {
    let project = root.join("project");
    for rel in rels {
        let src = project.join(rel);
        fs::create_dir_all(src.parent().unwrap()).unwrap();
        fs::write(&src, format!("payload-{rel}")).unwrap();
    }
    let backup = create_backup(
        conn,
        BackupRequest {
            level: BackupLevel::Standard,
            source_root: &project,
            destination_root: &root.join("backup"),
            items: rels
                .iter()
                .map(|rel| BackupItem {
                    source: project.join(rel),
                    relative: (*rel).to_string(),
                })
                .collect(),
            plan_json: "{}".to_string(),
            allow_same_volume: true,
        },
    )
    .unwrap();
    let verified = load_verified_backup(conn, backup.backup_id).unwrap();
    let items = rels
        .iter()
        .map(|rel| {
            let src = project.join(rel);
            QuarantineItem {
                source: src.clone(),
                relative: (*rel).to_string(),
                backup_hash: verified
                    .hash_for(&src.to_string_lossy())
                    .map(str::to_string),
            }
        })
        .collect();
    let holding = root.join("holding");
    let mut request = basic_request(&holding, items);
    request.backup_id = backup.backup_id;
    let result = quarantine(conn, request).unwrap();
    assert_eq!(result.failed, 0, "fixture quarantine must not fail");

    rels.iter()
        .map(|rel| {
            let original = project.join(rel).to_string_lossy().to_string();
            let (id, held): (i64, String) = conn
                .query_row(
                    "SELECT id, quarantine_path FROM quarantine_entry WHERE original_path = ?1",
                    [original],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            (id, PathBuf::from(held), root.join("backup").join(rel))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 1. Long paths (> 260 chars, non-verbatim) — quarantine + cleanup + restore
// ---------------------------------------------------------------------------

#[test]
fn t01_long_path_round_trip_survives() {
    let scratch = adversarial_scratch("t01-longpath");
    let _guard = DirGuard(scratch.clone());
    // Normalize to a clean absolute path WITHOUT the \\?\ prefix so this test exercises
    // the realistic user-visible long-path form (relies on LongPathsEnabled + the
    // longPathAware manifest rustc links into test binaries).
    let canon = fs::canonicalize(&scratch).unwrap();
    let base = PathBuf::from(
        canon
            .to_string_lossy()
            .strip_prefix(r"\\?\")
            .map(str::to_string)
            .unwrap_or_else(|| canon.to_string_lossy().to_string()),
    );

    let project = base.join("project");
    let seg = "abcdefghijklmnopqrstuvwxyz";
    let mut deep = project.clone();
    let mut relative = String::new();
    for _ in 0..10 {
        deep.push(seg);
        relative.push_str(seg);
        relative.push('/');
    }
    relative.push_str("payload.bin");
    let file = deep.join("payload.bin");
    assert!(
        file.as_os_str().len() > 260,
        "fixture must exceed MAX_PATH, got {} chars",
        file.as_os_str().len()
    );

    // Long-path support is environment-dependent (needs LongPathsEnabled + the
    // longPathAware manifest). Where it is off, a >260-char create fails cleanly —
    // skip rather than fail, so this permanent battery is portable to CI/other machines.
    if fs::create_dir_all(&deep).is_err() {
        println!(
            "t01 SKIPPED: cannot create a >260-char tree without \\\\?\\ (LongPathsEnabled off here)"
        );
        return;
    }
    fs::write(&file, b"long path payload").unwrap();

    let holding = base.join("holding");
    let conn = journaled_conn();
    let mut request = basic_request(&holding, vec![item(&file, &relative)]);
    request.cleanup_root = Some(project.clone());
    let result = quarantine(&conn, request).unwrap();

    assert_eq!(
        result.failed, 0,
        "long-path quarantine failed: {:?}",
        result.entries
    );
    assert_eq!(result.moved, 1);
    assert!(!file.exists(), "source must be moved out");
    assert!(
        !project.exists(),
        "cleanup_root should remove the emptied deep tree"
    );

    let entries = entries_for_op(&conn, result.operation_id);
    assert_eq!(entries.len(), 1);
    let held = PathBuf::from(&entries[0].2);
    assert!(
        held.as_os_str().len() > 260,
        "held path should itself be a long path ({} chars)",
        held.as_os_str().len()
    );
    assert_eq!(fs::read(&held).unwrap(), b"long path payload");

    let outcome = restore_entry(&conn, entries[0].0).unwrap();
    assert!(
        matches!(outcome, RestoreOutcome::Restored { .. }),
        "long-path restore outcome: {outcome:?}"
    );
    assert_eq!(fs::read(&file).unwrap(), b"long path payload");
    assert_eq!(entry_status(&conn, entries[0].0), "restored");
    println!(
        "t01 OK: {}-char source and {}-char held path round-tripped",
        file.as_os_str().len(),
        held.as_os_str().len()
    );
}

// ---------------------------------------------------------------------------
// 2. Unicode / emoji / Windows-hostile names
// ---------------------------------------------------------------------------

#[test]
fn t02a_unicode_emoji_and_combining_names_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    fs::create_dir_all(&project).unwrap();
    // NFC vs NFD spellings of "café.txt" are DISTINCT files on NTFS (no normalization).
    let names: [(&str, &[u8]); 4] = [
        ("\u{1F680}\u{1F525}\u{1F4BE}.bin", b"emoji payload"), // 🚀🔥💾.bin
        ("caf\u{e9}.txt", b"precomposed"),                     // café (NFC)
        ("cafe\u{301}.txt", b"decomposed!"),                   // café (NFD)
        (
            "\u{30D5}\u{30A1}\u{30A4}\u{30EB} \u{540D}\u{524D}.txt",
            b"cjk + space",
        ),
    ];
    for (name, bytes) in &names {
        fs::write(project.join(name), bytes).unwrap();
    }
    // Sanity: NFC and NFD really are two files.
    assert_eq!(fs::read_dir(&project).unwrap().count(), 4);

    let holding = dir.path().join("holding");
    let conn = journaled_conn();
    let items = names
        .iter()
        .map(|(name, _)| item(&project.join(name), name))
        .collect();
    let result = quarantine(&conn, basic_request(&holding, items)).unwrap();
    assert_eq!(
        result.moved, 4,
        "all unicode names should move: {:?}",
        result.entries
    );
    assert_eq!(result.failed, 0);

    for entry in entries_for_op(&conn, result.operation_id) {
        let outcome = restore_entry(&conn, entry.0).unwrap();
        assert!(matches!(outcome, RestoreOutcome::Restored { .. }));
    }
    for (name, bytes) in &names {
        assert_eq!(
            fs::read(project.join(name)).unwrap(),
            *bytes,
            "content mismatch after round-trip for {name:?}"
        );
    }
    println!("t02a OK: emoji, NFC/NFD pair, and CJK-with-space names all round-tripped");
}

#[cfg(windows)]
#[test]
fn t02b_trailing_dot_and_space_names_round_trip_or_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    // Names with trailing dot/space can only be CREATED through the \\?\ verbatim form;
    // fs::canonicalize returns exactly that form.
    let vroot = fs::canonicalize(dir.path()).unwrap();
    let project = vroot.join("project");
    fs::create_dir_all(&project).unwrap();

    let hostile = [
        ("trail.", b"dotted".as_slice()),
        ("spaced ", b"spaced".as_slice()),
    ];
    for (name, bytes) in &hostile {
        fs::write(project.join(name), bytes).unwrap();
    }

    let holding = vroot.join("holding");
    let conn = journaled_conn();
    let items = hostile
        .iter()
        .map(|(name, _)| item(&project.join(name), name))
        .collect();
    let result = quarantine(&conn, basic_request(&holding, items)).unwrap();

    // Safety invariant first: whatever happened, no bytes may be lost.
    for (idx, (name, bytes)) in hostile.iter().enumerate() {
        let source = project.join(name);
        match result.entries[idx].outcome {
            ItemOutcome::Moved | ItemOutcome::Copied => {
                assert!(
                    !source.exists(),
                    "{name:?} reported moved but still at source"
                );
            }
            _ => {
                assert_eq!(
                    fs::read(&source).unwrap(),
                    *bytes,
                    "{name:?} failed to move AND source bytes were damaged"
                );
            }
        }
    }
    assert_eq!(
        result.moved, 2,
        "expected both Windows-hostile names to quarantine; got {:?}",
        result.entries
    );

    // The HELD file lands under a NON-verbatim path, so Win32 strips the trailing
    // dot/space in the holding area; the journal remembers the hostile spelling.
    for entry in entries_for_op(&conn, result.operation_id) {
        let outcome = restore_entry(&conn, entry.0).unwrap();
        assert!(
            matches!(outcome, RestoreOutcome::Restored { .. }),
            "restore of hostile-named entry: {outcome:?}"
        );
    }
    for (name, bytes) in &hostile {
        assert_eq!(
            fs::read(project.join(name)).unwrap(),
            *bytes,
            "hostile name {name:?} not faithfully restored (checked via verbatim path)"
        );
    }
    println!("t02b OK: trailing-dot and trailing-space names round-tripped via \\\\?\\ originals");
}

#[test]
fn t02c_case_only_collision_in_one_op_fails_closed() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a/x.txt");
    let b = dir.path().join("b/X.TXT");
    fs::create_dir_all(a.parent().unwrap()).unwrap();
    fs::create_dir_all(b.parent().unwrap()).unwrap();
    fs::write(&a, b"AAAA").unwrap();
    fs::write(&b, b"BBBB").unwrap();

    let holding = dir.path().join("holding");
    let conn = journaled_conn();
    let result = quarantine(
        &conn,
        basic_request(&holding, vec![item(&a, "x.txt"), item(&b, "X.TXT")]),
    )
    .unwrap();

    let entries = entries_for_op(&conn, result.operation_id);
    let first_held = PathBuf::from(&entries[0].2);
    assert_eq!(
        fs::read(&first_held).unwrap(),
        b"AAAA",
        "first held copy must never be clobbered by a case-variant sibling"
    );
    if result.failed == 1 {
        // Case-insensitive NTFS (the default): the second move must fail closed.
        assert_eq!(result.moved, 1);
        assert_eq!(result.entries[1].outcome, ItemOutcome::Failed);
        let detail = result.entries[1].detail.clone().unwrap_or_default();
        assert!(
            detail.contains("occupied"),
            "expected DestinationOccupied semantics, got: {detail}"
        );
        assert_eq!(
            fs::read(&b).unwrap(),
            b"BBBB",
            "failed item's source must be intact"
        );
        assert_eq!(op_status(&conn, result.operation_id), "failed");
        println!("t02c OK: case-insensitive collision refused (DestinationOccupied), no overwrite");
    } else {
        // Case-sensitive filesystem (e.g. Dev Drive): both may move; they must be distinct.
        assert_eq!(result.moved, 2);
        assert_eq!(entries.len(), 2);
        assert_ne!(entries[0].2, entries[1].2);
        assert_eq!(fs::read(PathBuf::from(&entries[1].2)).unwrap(), b"BBBB");
        println!("t02c OK (case-sensitive volume): both case-variants held distinctly");
    }
}

// ---------------------------------------------------------------------------
// 3. Read-only attribute preservation
// ---------------------------------------------------------------------------

#[test]
fn t03_readonly_file_round_trips_with_attribute() {
    let dir = tempfile::tempdir().unwrap();
    let _guard = DirGuard(dir.path().to_path_buf()); // clears ReadOnly so tempdir can delete
    let project = dir.path().join("project");
    fs::create_dir_all(&project).unwrap();
    let file = project.join("frozen.txt");
    fs::write(&file, b"read only payload").unwrap();
    let mut perms = fs::metadata(&file).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&file, perms).unwrap();

    let holding = dir.path().join("holding");
    let conn = journaled_conn();
    let result = quarantine(
        &conn,
        basic_request(&holding, vec![item(&file, "frozen.txt")]),
    )
    .unwrap();
    assert_eq!(
        result.moved, 1,
        "read-only file should quarantine: {:?}",
        result.entries
    );
    assert_eq!(result.failed, 0);

    let entries = entries_for_op(&conn, result.operation_id);
    let held = PathBuf::from(&entries[0].2);
    assert!(
        fs::metadata(&held).unwrap().permissions().readonly(),
        "ReadOnly attribute lost on the held copy"
    );

    let outcome = restore_entry(&conn, entries[0].0).unwrap();
    assert!(matches!(outcome, RestoreOutcome::Restored { .. }));
    assert_eq!(fs::read(&file).unwrap(), b"read only payload");
    assert!(
        fs::metadata(&file).unwrap().permissions().readonly(),
        "ReadOnly attribute lost after restore"
    );
    println!("t03 OK: ReadOnly attribute survived quarantine and restore (same-volume rename)");
}

// ---------------------------------------------------------------------------
// 4. Locked file in a multi-item op — per-item isolation
// ---------------------------------------------------------------------------

#[cfg(windows)]
#[test]
fn t04_locked_file_fails_cleanly_others_proceed() {
    use std::os::windows::fs::OpenOptionsExt;

    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    fs::create_dir_all(&project).unwrap();
    let f1 = project.join("one.bin");
    let f2 = project.join("two.bin");
    let f3 = project.join("three.bin");
    fs::write(&f1, b"one").unwrap();
    fs::write(&f2, b"two locked").unwrap();
    fs::write(&f3, b"three").unwrap();

    // Exclusive handle (share_mode 0): rename must fail with a sharing violation.
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .open(&f2)
        .unwrap();

    let holding = dir.path().join("holding");
    let conn = journaled_conn();
    let result = quarantine(
        &conn,
        basic_request(
            &holding,
            vec![
                item(&f1, "one.bin"),
                item(&f2, "two.bin"),
                item(&f3, "three.bin"),
            ],
        ),
    )
    .unwrap();

    assert_eq!(
        result.moved, 2,
        "unlocked items must proceed: {:?}",
        result.entries
    );
    assert_eq!(result.failed, 1, "locked item must fail, not abort the op");
    assert_eq!(result.entries[1].outcome, ItemOutcome::Failed);
    assert!(
        !f1.exists() && !f3.exists(),
        "unlocked items should be held"
    );
    // While the exclusive handle is open we can only assert presence (share_mode 0
    // blocks our own verification read as well); content is checked after drop(lock).
    assert!(
        f2.exists(),
        "locked source must still be at its original path"
    );

    // Journal consistency: op failed, 2 done + 1 failed move items, exactly 2 entries
    // (no phantom quarantine_entry for the failed item).
    assert_eq!(op_status(&conn, result.operation_id), "failed");
    let (done, failed) = item_status_counts(&conn, result.operation_id, "move");
    assert_eq!((done, failed), (2, 1));
    let entries = entries_for_op(&conn, result.operation_id);
    assert_eq!(
        entries.len(),
        2,
        "failed item must not get a quarantine_entry"
    );
    assert!(entries.iter().all(|e| e.3 == "quarantined"));

    // Release the lock; the locked source must be byte-identical, and the two held
    // entries restore cleanly.
    drop(lock);
    assert_eq!(
        fs::read(&f2).unwrap(),
        b"two locked",
        "locked source must be untouched"
    );
    for entry in &entries {
        let outcome = restore_entry(&conn, entry.0).unwrap();
        assert!(matches!(outcome, RestoreOutcome::Restored { .. }));
    }
    assert_eq!(fs::read(&f1).unwrap(), b"one");
    assert_eq!(fs::read(&f3).unwrap(), b"three");
    println!("t04 OK: locked item isolated (1 failed, 2 moved+restored), journal consistent");
}

// ---------------------------------------------------------------------------
// 5. Big fan-out: 500 files quarantine → full restore
// ---------------------------------------------------------------------------

#[test]
fn t05_fanout_500_files_round_trip_with_consistent_journal() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    let mut items = Vec::with_capacity(500);
    for i in 0..500 {
        let rel = format!("bucket{:02}/file{:03}.bin", i % 20, i);
        let src = project.join(&rel);
        fs::create_dir_all(src.parent().unwrap()).unwrap();
        fs::write(&src, format!("payload-{i}")).unwrap();
        items.push(item(&src, &rel));
    }

    let holding = dir.path().join("holding");
    let conn = journaled_conn();
    let started = Instant::now();
    let mut request = basic_request(&holding, items);
    request.cleanup_root = Some(project.clone());
    let result = quarantine(&conn, request).unwrap();
    let quarantine_elapsed = started.elapsed();

    assert_eq!(result.moved, 500);
    assert_eq!(result.failed, 0);
    assert!(
        !project.exists(),
        "500-file cleanup should empty the project tree"
    );
    let entries = entries_for_op(&conn, result.operation_id);
    assert_eq!(entries.len(), 500);
    let (done, failed) = item_status_counts(&conn, result.operation_id, "move");
    assert_eq!((done, failed), (500, 0));

    let started = Instant::now();
    for entry in &entries {
        let outcome = restore_entry(&conn, entry.0).unwrap();
        assert!(matches!(outcome, RestoreOutcome::Restored { .. }));
    }
    let restore_elapsed = started.elapsed();

    for i in 0..500 {
        let rel = format!("bucket{:02}/file{:03}.bin", i % 20, i);
        assert_eq!(
            fs::read(project.join(&rel)).unwrap(),
            format!("payload-{i}").as_bytes(),
            "content mismatch for {rel}"
        );
    }
    let restored: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM quarantine_entry WHERE operation_id = ?1 AND status = 'restored'",
            [result.operation_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(restored, 500);
    println!(
        "t05 OK: 500 files quarantined in {quarantine_elapsed:?}, restored in {restore_elapsed:?}"
    );
    assert!(
        quarantine_elapsed.as_secs() < 60 && restore_elapsed.as_secs() < 120,
        "pathological fan-out time: quarantine {quarantine_elapsed:?}, restore {restore_elapsed:?}"
    );
}

// ---------------------------------------------------------------------------
// 6. Deep tree: 35 nested levels
// ---------------------------------------------------------------------------

#[test]
fn t06_deep_tree_35_levels_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    let mut deep = project.clone();
    let mut relative = String::new();
    for level in 0..35 {
        let seg = format!("d{level:02}");
        deep.push(&seg);
        relative.push_str(&seg);
        relative.push('/');
    }
    relative.push_str("deep.txt");
    let file = deep.join("deep.txt");
    fs::create_dir_all(&deep).unwrap();
    fs::write(&file, b"deep payload").unwrap();

    let holding = dir.path().join("holding");
    let conn = journaled_conn();
    let mut request = basic_request(&holding, vec![item(&file, &relative)]);
    request.cleanup_root = Some(project.clone());
    let result = quarantine(&conn, request).unwrap();

    assert_eq!(
        result.moved, 1,
        "deep-tree quarantine: {:?}",
        result.entries
    );
    assert_eq!(result.failed, 0);
    assert!(
        result.removed_dirs >= 35,
        "expected the 35 emptied levels removed, got {}",
        result.removed_dirs
    );
    assert!(!project.exists(), "emptied deep project should be gone");

    let entries = entries_for_op(&conn, result.operation_id);
    let outcome = restore_entry(&conn, entries[0].0).unwrap();
    assert!(matches!(outcome, RestoreOutcome::Restored { .. }));
    assert_eq!(fs::read(&file).unwrap(), b"deep payload");
    println!(
        "t06 OK: 35-level tree emptied ({} dirs) and restored",
        result.removed_dirs
    );
}

// ---------------------------------------------------------------------------
// 7. Same-name different-dir collision inside ONE op (relative flattening)
// ---------------------------------------------------------------------------

#[test]
fn t07_same_relative_in_one_op_never_overwrites_held_copy() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a/x.txt");
    let b = dir.path().join("b/x.txt");
    fs::create_dir_all(a.parent().unwrap()).unwrap();
    fs::create_dir_all(b.parent().unwrap()).unwrap();
    fs::write(&a, b"AAAA").unwrap();
    fs::write(&b, b"BBBB").unwrap();

    let holding = dir.path().join("holding");
    let conn = journaled_conn();
    // Both items carry the SAME flattened relative — an adversarial plan.
    let result = quarantine(
        &conn,
        basic_request(&holding, vec![item(&a, "x.txt"), item(&b, "x.txt")]),
    )
    .unwrap();

    // The first item wins the slot; the second must be refused (DestinationOccupied),
    // never a silent clobber of the first entry's only held copy.
    assert_eq!(result.moved, 1);
    assert_eq!(result.failed, 1);
    assert_eq!(result.entries[0].outcome, ItemOutcome::Moved);
    assert_eq!(result.entries[1].outcome, ItemOutcome::Failed);
    let detail = result.entries[1].detail.clone().unwrap_or_default();
    assert!(
        detail.contains("occupied"),
        "expected DestinationOccupied semantics, got: {detail}"
    );

    let entries = entries_for_op(&conn, result.operation_id);
    assert_eq!(entries.len(), 1);
    assert_eq!(fs::read(PathBuf::from(&entries[0].2)).unwrap(), b"AAAA");
    assert_eq!(
        fs::read(&b).unwrap(),
        b"BBBB",
        "refused item's source must be intact"
    );
    assert_eq!(op_status(&conn, result.operation_id), "failed");

    // And the held copy restores to ITS original home.
    let outcome = restore_entry(&conn, entries[0].0).unwrap();
    assert!(matches!(outcome, RestoreOutcome::Restored { .. }));
    assert_eq!(fs::read(&a).unwrap(), b"AAAA");
    println!("t07 OK: flattened-relative collision refused; both byte-sets safe");
}

// ---------------------------------------------------------------------------
// 8. Cross-volume (C: temp → D: holding): copy + verify + delete, then restore
// ---------------------------------------------------------------------------

#[cfg(windows)]
#[test]
fn t08_cross_volume_quarantine_and_restore() {
    let dir = tempfile::tempdir().unwrap();
    let temp_drive = dir
        .path()
        .to_string_lossy()
        .chars()
        .next()
        .map(|c| c.to_ascii_uppercase());
    if temp_drive == Some('D') {
        println!("t08 SKIPPED: temp dir already lives on D:, no second volume to cross");
        return;
    }
    let d_scratch = PathBuf::from(format!("D:\\codehangar-adv-qa-{}", std::process::id()));
    if fs::create_dir_all(&d_scratch).is_err() {
        println!("t08 SKIPPED: D:\\ is not writable");
        return;
    }
    let _guard = DirGuard(d_scratch.clone());

    let project = dir.path().join("project");
    fs::create_dir_all(&project).unwrap();
    let file = project.join("cross.bin");
    let payload: Vec<u8> = (0..64 * 1024u32).map(|i| (i % 251) as u8).collect();
    fs::write(&file, &payload).unwrap();

    let holding = d_scratch.join("holding");
    let conn = journaled_conn();
    let result = quarantine(
        &conn,
        basic_request(&holding, vec![item(&file, "cross.bin")]),
    )
    .unwrap();

    assert_eq!(
        result.moved, 1,
        "cross-volume quarantine: {:?}",
        result.entries
    );
    assert_eq!(
        result.entries[0].outcome,
        ItemOutcome::Copied,
        "C:→D: must take the copy+verify+delete path"
    );
    assert_eq!(
        result.space_recovered,
        payload.len() as u64,
        "cross-volume move should report the source bytes as recovered"
    );
    assert!(
        !file.exists(),
        "source must be deleted after the journaled verified copy"
    );
    let entries = entries_for_op(&conn, result.operation_id);
    let held = PathBuf::from(&entries[0].2);
    assert!(held.starts_with(&d_scratch));
    assert_eq!(
        fs::read(&held).unwrap(),
        payload,
        "held copy must be byte-identical"
    );

    // Restore crosses back D: → C: (copy + verify + delete the held copy).
    let outcome = restore_entry(&conn, entries[0].0).unwrap();
    assert!(matches!(outcome, RestoreOutcome::Restored { .. }));
    assert_eq!(fs::read(&file).unwrap(), payload);
    assert!(
        !held.exists(),
        "held copy should be removed after verified restore"
    );
    assert_eq!(entry_status(&conn, entries[0].0), "restored");
    println!("t08 OK: 64 KiB C:→D: quarantine (Copied) and D:→C: restore both verified");
}

// ---------------------------------------------------------------------------
// 9. Purge gates: token discipline + Gate-3 backup requirements
// ---------------------------------------------------------------------------

#[test]
fn t09_purge_token_and_gate3_refusals() {
    let dir = tempfile::tempdir().unwrap();
    let conn = journaled_conn();
    let tokens = ConfirmTokenStore::default();
    let backed = backed_quarantine(&conn, dir.path(), &["cache/a.bin", "cache/b.bin"]);
    let (entry_a, held_a, _payload_a) = &backed[0];
    let (entry_b, held_b, payload_b) = &backed[1];

    // (a) No token / bogus token → refused, nothing touched.
    for bad in ["", "bogus-token"] {
        assert!(matches!(
            permanent_delete_entry(&conn, &tokens, bad, *entry_a),
            Err(PurgeError::ConfirmRequired)
        ));
    }
    assert!(held_a.exists());
    assert_eq!(entry_status(&conn, *entry_a), "quarantined");

    // (b) A token for the WRONG action must not authorize a purge.
    let wrong_action = tokens.issue(ConfirmAction::EnterMutationMode);
    assert!(matches!(
        permanent_delete_entry(&conn, &tokens, &wrong_action, *entry_a),
        Err(PurgeError::ConfirmRequired)
    ));
    assert!(held_a.exists());

    // (c) Valid token purges entry A…
    let token = tokens.issue(ConfirmAction::PermanentDelete);
    let outcome = permanent_delete_entry(&conn, &tokens, &token, *entry_a).unwrap();
    assert!(outcome.freed_bytes > 0);
    assert!(!held_a.exists());
    assert_eq!(entry_status(&conn, *entry_a), "permanently_deleted");

    // (d) …and REUSING that token on entry B is refused (single-use).
    assert!(matches!(
        permanent_delete_entry(&conn, &tokens, &token, *entry_b),
        Err(PurgeError::ConfirmRequired)
    ));
    assert!(held_b.exists());
    assert_eq!(entry_status(&conn, *entry_b), "quarantined");

    // (e) Gate 3: backup payload for B vanishes → purge refused even with a fresh token.
    fs::remove_file(payload_b).unwrap();
    let fresh = tokens.issue(ConfirmAction::PermanentDelete);
    assert!(matches!(
        permanent_delete_entry(&conn, &tokens, &fresh, *entry_b),
        Err(PurgeError::BackupUnusable(_, _))
    ));
    assert!(held_b.exists(), "held copy must survive a Gate-3 refusal");
    assert_eq!(entry_status(&conn, *entry_b), "quarantined");
    // Observed behavior note: the refusal CONSUMED the fresh token (fail-closed).
    assert!(
        !tokens.consume(&fresh, ConfirmAction::PermanentDelete),
        "token is burned by a refused purge — retry needs a new confirmation"
    );

    // (f) Entry with NO linked backup at all → BackupRequired.
    let orphan_src = dir.path().join("orphan/project/junk.bin");
    fs::create_dir_all(orphan_src.parent().unwrap()).unwrap();
    fs::write(&orphan_src, b"junk with no backup").unwrap();
    let holding = dir.path().join("orphan/holding");
    let result = quarantine(
        &conn,
        basic_request(&holding, vec![item(&orphan_src, "junk.bin")]),
    )
    .unwrap();
    let orphan_entry = entries_for_op(&conn, result.operation_id)[0].clone();
    let token = tokens.issue(ConfirmAction::PermanentDelete);
    assert!(matches!(
        permanent_delete_entry(&conn, &tokens, &token, orphan_entry.0),
        Err(PurgeError::BackupRequired(_))
    ));
    assert!(PathBuf::from(&orphan_entry.2).exists());
    assert_eq!(entry_status(&conn, orphan_entry.0), "quarantined");
    println!("t09 OK: token gates (missing/wrong-action/reused) + Gate-3 (no backup / dead payload) all refused");
}

// ---------------------------------------------------------------------------
// 10. Restore conflict: occupied destination → Conflict, nothing overwritten
// ---------------------------------------------------------------------------

#[test]
fn t10_restore_conflict_overwrites_nothing_and_stays_restorable() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("project");
    fs::create_dir_all(&project).unwrap();
    let file = project.join("report.txt");
    fs::write(&file, b"held bytes").unwrap();

    let holding = dir.path().join("holding");
    let conn = journaled_conn();
    let result = quarantine(
        &conn,
        basic_request(&holding, vec![item(&file, "report.txt")]),
    )
    .unwrap();
    assert_eq!(result.moved, 1);
    let entries = entries_for_op(&conn, result.operation_id);
    let (entry_id, held) = (entries[0].0, PathBuf::from(&entries[0].2));

    // A third party races a NEW file into the original slot.
    fs::write(&file, b"newer third-party bytes").unwrap();

    let outcome = restore_entry(&conn, entry_id).unwrap();
    assert!(
        matches!(outcome, RestoreOutcome::Conflict { .. }),
        "occupied destination must surface a Conflict, got {outcome:?}"
    );
    assert_eq!(
        fs::read(&file).unwrap(),
        b"newer third-party bytes",
        "conflicting file must be untouched"
    );
    assert_eq!(
        fs::read(&held).unwrap(),
        b"held bytes",
        "held copy must be untouched"
    );
    assert_eq!(entry_status(&conn, entry_id), "quarantined");

    // Once the blocker clears, the same entry restores fine.
    fs::remove_file(&file).unwrap();
    let outcome = restore_entry(&conn, entry_id).unwrap();
    assert!(matches!(outcome, RestoreOutcome::Restored { .. }));
    assert_eq!(fs::read(&file).unwrap(), b"held bytes");
    println!("t10 OK: Conflict surfaced, nothing overwritten, entry stayed restorable");
}
