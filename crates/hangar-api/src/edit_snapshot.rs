#[cfg(feature = "agent_automation")]
use hangar_core::AiEditSessionSummary;
use hangar_core::{
    EditSnapshotRestoreResult, EditSnapshotSummary, SessionChangeCoverage, SessionChangeEdit,
    SessionChangeSet, SessionDiffHunk, SessionDiffLine, SessionFileChange, SessionFileReality,
};
use hangar_db::{Db, DbError, DbResult};
use rusqlite::{params, Connection};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const MAX_EDIT_BYTES: usize = 2 * 1024 * 1024;
const SNAPSHOTS_PER_FILE: usize = 20;
const GLOBAL_SNAPSHOT_CAP: usize = 1000;
const LEDGER_DIFF_LINE_CAP: usize = 4_000;
const LEDGER_LINE_CHAR_CAP: usize = 16_384;

#[derive(Debug)]
pub(crate) struct WriteOutcome {
    pub previous: String,
    pub snapshot_id: i64,
    pub after_hash: String,
    pub ledger_warning: Option<String>,
}

struct SnapshotRow {
    id: i64,
    node_id: i64,
    backup_id: i64,
    destination: PathBuf,
    session_id: Option<String>,
}

struct SnapshotWrite<'a> {
    node_id: i64,
    project_id: i64,
    target: &'a Path,
    content: &'a str,
    origin: &'a str,
    session_id: Option<&'a str>,
    expected_before_hash: Option<&'a str>,
}

pub(crate) fn write_file_with_snapshot(
    state: &super::AppState,
    node_id: i64,
    content: &str,
    origin: &str,
    session_id: Option<&str>,
    expected_before_hash: Option<&str>,
) -> Result<WriteOutcome, String> {
    validate_edit_content(content)?;
    let origin = validate_origin(origin)?;
    let (path, project_paths) = super::resolve_ai_explain_inventory_target(state, node_id)?;
    super::validate_ai_explain_disk_target(&path, &project_paths)?;
    let db = state.db()?;
    let project_id = db
        .node_project_id(node_id)
        .map_err(super::to_message)?
        .ok_or_else(|| "Not saved: the file is no longer attached to a project.".to_string())?;
    let project = db
        .project_get(project_id)
        .map_err(super::to_message)?
        .ok_or_else(|| "Not saved: the owning project is no longer registered.".to_string())?;
    let snapshot_root = snapshot_root(state)?;
    let mut outcome = db
        .with_recovery_writer(|conn| {
            hangar_mutation::ensure_journal_schema(conn)
                .map_err(|error| DbError::FileRead(error.to_string()))?;
            snapshot_and_write_with_conn(
                conn,
                &snapshot_root,
                SnapshotWrite {
                    node_id,
                    project_id,
                    target: Path::new(&path),
                    content,
                    origin,
                    session_id,
                    expected_before_hash,
                },
            )
            .map_err(DbError::FileRead)
        })
        .map_err(super::to_message)?;
    outcome.ledger_warning = retain_in_app_change(
        &db,
        project_id,
        node_id,
        Path::new(&project.path),
        Path::new(&path),
        &outcome.previous,
        content,
        origin,
        session_id,
        outcome.snapshot_id,
        &outcome.after_hash,
    )
    .err();
    Ok(outcome)
}

#[allow(clippy::too_many_arguments)]
fn retain_in_app_change(
    db: &Db,
    project_id: i64,
    node_id: i64,
    project_root: &Path,
    target: &Path,
    before: &str,
    after: &str,
    origin: &str,
    session_id: Option<&str>,
    snapshot_id: i64,
    after_hash: &str,
) -> Result<(), String> {
    let Some(change_set) = build_in_app_change_set(project_root, target, before, after, origin)
    else {
        return Ok(());
    };
    let before_hash = blake3::hash(before.as_bytes()).to_hex().to_string();
    db.store_change_evidence(
        project_id,
        Some(node_id),
        &format!("codehangar:{node_id}:{snapshot_id}"),
        Some(chrono::Utc::now().timestamp_millis()),
        Some(origin),
        session_id,
        Some(&before_hash),
        Some(after_hash),
        &change_set,
    )
    .map(|_| ())
    .map_err(|error| {
        format!(
            "The file was saved with a verified previous version, but its Recap ledger entry could not be retained ({error})."
        )
    })
}

pub(crate) fn list_snapshots(
    state: &super::AppState,
    node_id: i64,
    limit: usize,
) -> Result<Vec<EditSnapshotSummary>, String> {
    state
        .db()?
        .with_recovery_writer(|conn| {
            hangar_mutation::ensure_journal_schema(conn)
                .map_err(|error| DbError::FileRead(error.to_string()))?;
            load_snapshot_summaries(conn, node_id, limit).map_err(DbError::from)
        })
        .map_err(super::to_message)
}

pub(crate) fn compare_snapshot(
    state: &super::AppState,
    snapshot_id: i64,
) -> Result<hangar_core::EditSnapshotComparison, String> {
    let db = state.db()?;
    let (node_id, project_id, recorded_path, backup_id, before_hash) = db
        .with_recovery_writer(|conn| {
            hangar_mutation::ensure_journal_schema(conn)
                .map_err(|error| DbError::FileRead(error.to_string()))?;
            conn.query_row(
                "SELECT node_id, project_id, path, backup_id, blake3_before
                 FROM edit_snapshot WHERE id = ?1",
                [snapshot_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .map_err(|_| {
                DbError::FileRead("That previous file version is no longer available.".to_string())
            })
        })
        .map_err(super::to_message)?;
    let (current_path, project_paths) = super::resolve_ai_explain_inventory_target(state, node_id)?;
    super::validate_ai_explain_disk_target(&current_path, &project_paths)?;
    if !same_canonical_file(Path::new(&current_path), Path::new(&recorded_path)) {
        return Err(
            "Compare refused: the inventory now resolves this item to a different file."
                .to_string(),
        );
    }
    let project = db
        .project_get(project_id)
        .map_err(super::to_message)?
        .ok_or_else(|| {
            "Compare refused: the owning project is no longer registered.".to_string()
        })?;
    if !Path::new(&current_path)
        .canonicalize()
        .ok()
        .zip(Path::new(&project.path).canonicalize().ok())
        .is_some_and(|(target, root)| target.starts_with(root))
    {
        return Err("Compare refused: the file is outside its registered project.".to_string());
    }
    let previous_bytes = db
        .with_recovery_writer(|conn| {
            hangar_mutation::ensure_journal_schema(conn)
                .map_err(|error| DbError::FileRead(error.to_string()))?;
            let verified = hangar_mutation::load_verified_backup(conn, backup_id)
                .map_err(|error| DbError::FileRead(error.to_string()))?;
            verified
                .verify_payload(&recorded_path)
                .map_err(|error| DbError::FileRead(error.to_string()))?;
            let copy = verified.copy_for(&recorded_path).ok_or_else(|| {
                DbError::FileRead("The verified backup does not cover this file.".to_string())
            })?;
            let bytes = fs::read(&copy.backup_path).map_err(|error| {
                DbError::FileRead(format!("Previous version could not be read: {error}"))
            })?;
            let actual_hash = blake3::hash(&bytes).to_hex().to_string();
            if actual_hash != before_hash || actual_hash != copy.blake3 {
                return Err(DbError::FileRead(
                    "Previous version verification failed; no comparison was produced.".to_string(),
                ));
            }
            Ok(bytes)
        })
        .map_err(super::to_message)?;
    let current_bytes = fs::read(&current_path).map_err(|error| {
        format!("Compare unavailable: the current file could not be read ({error}).")
    })?;
    if previous_bytes.len() > MAX_EDIT_BYTES || current_bytes.len() > MAX_EDIT_BYTES {
        return Err(
            "Compare unavailable: one file version exceeds the local diff limit.".to_string(),
        );
    }
    let previous = String::from_utf8(previous_bytes)
        .map_err(|_| "Compare unavailable: the previous version is not UTF-8 text.".to_string())?;
    let current = String::from_utf8(current_bytes)
        .map_err(|_| "Compare unavailable: the current file is not UTF-8 text.".to_string())?;
    let already_current = current == previous;
    let diff = super::edit_review::build_diff(&current, &previous);
    Ok(hangar_core::EditSnapshotComparison {
        snapshot_id,
        node_id,
        added_lines: diff.added_lines,
        removed_lines: diff.removed_lines,
        hunks: diff.hunks,
        diff_truncated: diff.truncated,
        already_current,
    })
}

pub(crate) fn restore_snapshot(
    state: &super::AppState,
    snapshot_id: i64,
) -> Result<EditSnapshotRestoreResult, String> {
    let snapshot_root = snapshot_root(state)?;
    let db = state.db()?;
    let record = db
        .with_recovery_writer(|conn| {
            hangar_mutation::ensure_journal_schema(conn)
                .map_err(|error| DbError::FileRead(error.to_string()))?;
            conn.query_row(
                "SELECT node_id, project_id, path, backup_id, blake3_before
                 FROM edit_snapshot WHERE id = ?1",
                [snapshot_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .map_err(|_| {
                DbError::FileRead("That previous file version is no longer available.".to_string())
            })
        })
        .map_err(super::to_message)?;
    let (node_id, project_id, recorded_path, backup_id, before_hash) = record;
    // Do not recursively open the inventory while holding the journal writer. The in-memory
    // edition uses one mutex-backed connection and would otherwise deadlock here.
    let (current_path, project_paths) = super::resolve_ai_explain_inventory_target(state, node_id)?;
    super::validate_ai_explain_disk_target(&current_path, &project_paths)?;
    if !same_canonical_file(Path::new(&current_path), Path::new(&recorded_path)) {
        return Err(
            "Restore refused: the inventory now resolves this item to a different file."
                .to_string(),
        );
    }
    let project = db
        .project_get(project_id)
        .map_err(super::to_message)?
        .ok_or_else(|| {
            "Restore refused: the owning project is no longer registered.".to_string()
        })?;
    let (mut result, previous, restored_content, after_hash, safety_snapshot_id) = db
        .with_recovery_writer(|conn| {
            hangar_mutation::ensure_journal_schema(conn)
                .map_err(|error| DbError::FileRead(error.to_string()))?;
            let verified = hangar_mutation::load_verified_backup(conn, backup_id)
                .map_err(|error| DbError::FileRead(error.to_string()))?;
            verified
                .verify_payload(&recorded_path)
                .map_err(|error| DbError::FileRead(error.to_string()))?;
            let copy = verified.copy_for(&recorded_path).ok_or_else(|| {
                DbError::FileRead("The verified backup does not cover this file.".to_string())
            })?;
            let bytes = fs::read(&copy.backup_path)
                .map_err(|error| DbError::FileRead(format!("Restore backup could not be read: {error}")))?;
            let actual_hash = blake3::hash(&bytes).to_hex().to_string();
            if actual_hash != before_hash || actual_hash != copy.blake3 {
                return Err(DbError::FileRead(
                    "Restore refused: the previous version no longer matches its verified hash."
                        .to_string(),
                ));
            }
            let content = String::from_utf8(bytes).map_err(|_| {
                DbError::FileRead("Restore refused: the previous version is not UTF-8 text.".to_string())
            })?;
            let outcome = snapshot_and_write_with_conn(
                conn,
                &snapshot_root,
                SnapshotWrite {
                    node_id,
                    project_id,
                    target: Path::new(&current_path),
                    content: &content,
                    origin: "restore",
                    session_id: None,
                    expected_before_hash: None,
                },
            )
            .map_err(DbError::FileRead)?;
            conn.execute(
                "UPDATE edit_snapshot SET restored_at = ?2 WHERE id = ?1",
                params![snapshot_id, chrono::Utc::now().to_rfc3339()],
            )?;
            Ok((
                EditSnapshotRestoreResult {
                    restored_snapshot_id: snapshot_id,
                    safety_snapshot_id: outcome.snapshot_id,
                    node_id,
                    message: "Previous version restored. The version it replaced was saved too, so this restore can be undone."
                        .to_string(),
                },
                outcome.previous,
                content,
                outcome.after_hash,
                outcome.snapshot_id,
            ))
        })
        .map_err(super::to_message)?;
    if let Err(warning) = retain_in_app_change(
        &db,
        project_id,
        node_id,
        Path::new(&project.path),
        Path::new(&current_path),
        &previous,
        &restored_content,
        "restore",
        None,
        safety_snapshot_id,
        &after_hash,
    ) {
        result.message.push(' ');
        result.message.push_str(&warning);
    }
    Ok(result)
}

#[cfg(feature = "agent_automation")]
pub(crate) fn list_ai_sessions(
    state: &super::AppState,
    node_id: i64,
    limit: usize,
) -> Result<Vec<AiEditSessionSummary>, String> {
    state
        .db()?
        .with_recovery_writer(|conn| {
            hangar_mutation::ensure_journal_schema(conn)
                .map_err(|error| DbError::FileRead(error.to_string()))?;
            let mut stmt = conn.prepare(
                "SELECT session_id, node_id, project_id, path, MIN(id), COUNT(*),
                        MIN(created_at), MAX(created_at)
                 FROM edit_snapshot
                 WHERE node_id = ?1
                   AND session_id IS NOT NULL
                   AND origin IN ('ai_suggestion', 'ai_session')
                   AND status = 'saved'
                 GROUP BY session_id, node_id, project_id, path
                 ORDER BY MIN(id) DESC
                 LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![node_id, limit.clamp(1, 50) as i64], |row| {
                Ok(AiEditSessionSummary {
                    session_id: row.get(0)?,
                    node_id: row.get(1)?,
                    project_id: row.get(2)?,
                    path: row.get(3)?,
                    first_snapshot_id: row.get(4)?,
                    edit_count: row.get::<_, i64>(5)?.max(0) as u64,
                    started_at: row.get(6)?,
                    last_edit_at: row.get(7)?,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .map_err(super::to_message)
}

#[cfg(feature = "agent_automation")]
pub(crate) fn restore_ai_session(
    state: &super::AppState,
    node_id: i64,
    session_id: &str,
) -> Result<EditSnapshotRestoreResult, String> {
    if node_id <= 0 {
        return Err("That file is not valid.".to_string());
    }
    if session_id.trim().is_empty() || session_id.len() > 128 {
        return Err("That AI edit session is not valid.".to_string());
    }
    let snapshot_id = state
        .db()?
        .with_recovery_writer(|conn| {
            hangar_mutation::ensure_journal_schema(conn)
                .map_err(|error| DbError::FileRead(error.to_string()))?;
            conn.query_row(
                "SELECT MIN(id) FROM edit_snapshot
                 WHERE session_id = ?1
                   AND node_id = ?2
                   AND origin IN ('ai_suggestion', 'ai_session')
                   AND status = 'saved'",
                params![session_id, node_id],
                |row| row.get::<_, Option<i64>>(0),
            )?
            .ok_or_else(|| {
                DbError::FileRead(
                    "The original version for that AI edit session is no longer available."
                        .to_string(),
                )
            })
        })
        .map_err(super::to_message)?;
    restore_snapshot(state, snapshot_id)
}

fn snapshot_and_write_with_conn(
    conn: &Connection,
    snapshot_root: &Path,
    request: SnapshotWrite<'_>,
) -> Result<WriteOutcome, String> {
    let SnapshotWrite {
        node_id,
        project_id,
        target,
        content,
        origin,
        session_id,
        expected_before_hash,
    } = request;
    validate_edit_content(content)?;
    let origin = validate_origin(origin)?;
    let existing = fs::read(target)
        .map_err(|error| format!("Not saved: the file could not be read ({error})."))?;
    let previous = String::from_utf8(existing).map_err(|_| {
        "Not saved: this file is not UTF-8 text and cannot be edited here.".to_string()
    })?;
    let before_hash = blake3::hash(previous.as_bytes()).to_hex().to_string();
    if expected_before_hash.is_some_and(|expected| expected != before_hash) {
        return Err(
            "Not saved: the file changed on disk. Reload it before applying this edit.".to_string(),
        );
    }
    let after_hash = blake3::hash(content.as_bytes()).to_hex().to_string();
    if before_hash == after_hash {
        return Err("Not saved: the new content is identical to the file on disk.".to_string());
    }
    super::edit_review::enforce_write_validation(target, &previous, content, origin)?;
    let file_name = target
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| "Not saved: the file name cannot be backed up safely.".to_string())?;
    let source_root = target
        .parent()
        .ok_or_else(|| "Not saved: the file has no parent directory.".to_string())?;
    fs::create_dir_all(snapshot_root)
        .map_err(|error| format!("Not saved: could not prepare edit snapshots ({error})."))?;
    let stamp = chrono::Utc::now().timestamp_micros();
    let destination = snapshot_root.join(format!(
        "{node_id}-{stamp}-{}",
        before_hash.chars().take(12).collect::<String>()
    ));
    let backup = hangar_mutation::create_backup(
        conn,
        hangar_mutation::BackupRequest {
            level: hangar_mutation::BackupLevel::Minimal,
            source_root,
            destination_root: &destination,
            items: vec![hangar_mutation::BackupItem {
                source: target.to_path_buf(),
                relative: file_name.to_string(),
            }],
            plan_json: serde_json::json!({
                "kind": "edit_snapshot",
                "nodeId": node_id,
                "projectId": project_id,
                "origin": origin,
            })
            .to_string(),
            allow_same_volume: true,
        },
    )
    .map_err(|error| format!("Not saved: verified snapshot failed ({error})."))?;
    if !backup.verified {
        return Err("Not saved: the pre-edit snapshot was not verified.".to_string());
    }
    let verified = hangar_mutation::load_verified_backup(conn, backup.backup_id)
        .map_err(|error| format!("Not saved: snapshot verification failed ({error})."))?;
    verified
        .verify_payload(&target.to_string_lossy())
        .map_err(|error| format!("Not saved: snapshot payload verification failed ({error})."))?;

    let created_at = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO edit_snapshot(
            node_id, project_id, path, backup_id, bytes, blake3_before, blake3_after,
            origin, session_id, status, created_at
         ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, ?8, 'prepared', ?9)",
        params![
            node_id,
            project_id,
            target.to_string_lossy(),
            backup.backup_id,
            previous.len() as i64,
            before_hash,
            origin,
            session_id,
            created_at,
        ],
    )
    .map_err(|error| format!("Not saved: snapshot journal failed ({error})."))?;
    let snapshot_id = conn.last_insert_rowid();

    atomic_replace(target, content.as_bytes(), node_id)?;
    let written_hash = hangar_mutation::file_blake3(target)
        .map_err(|error| format!("Saved file could not be verified ({error})."))?;
    if written_hash != after_hash {
        return Err(
            "Saved file did not match the requested content. The verified previous version remains available in Recover."
                .to_string(),
        );
    }
    conn.execute(
        "UPDATE edit_snapshot SET blake3_after = ?2, status = 'saved' WHERE id = ?1",
        params![snapshot_id, after_hash],
    )
    .map_err(|error| {
        format!(
            "File saved, but the snapshot journal could not be finalized ({error}). The prepared snapshot remains recoverable."
        )
    })?;
    let _ = prune_snapshots(conn, snapshot_root);
    Ok(WriteOutcome {
        previous,
        snapshot_id,
        after_hash: written_hash,
        ledger_warning: None,
    })
}

fn build_in_app_change_set(
    project_root: &Path,
    target: &Path,
    before: &str,
    after: &str,
    origin: &str,
) -> Option<SessionChangeSet> {
    let relative = target.strip_prefix(project_root).ok()?;
    let relative_path = hangar_core::normalize_path(&relative.to_string_lossy());
    if relative_path.is_empty()
        || hangar_protect::is_sensitive_path(&relative_path)
        || hangar_protect::protected_level_for_path(&relative_path).is_some()
        || hangar_protect::is_heavy_or_protected_container_path(&relative_path)
    {
        return None;
    }

    let (before, before_redactions) = super::redact_secrets(before);
    let (after, after_redactions) = super::redact_secrets(after);
    let before_lines = before.lines().collect::<Vec<_>>();
    let after_lines = after.lines().collect::<Vec<_>>();
    let mut prefix = 0usize;
    while prefix < before_lines.len()
        && prefix < after_lines.len()
        && before_lines[prefix] == after_lines[prefix]
    {
        prefix += 1;
    }
    let mut suffix = 0usize;
    while suffix < before_lines.len().saturating_sub(prefix)
        && suffix < after_lines.len().saturating_sub(prefix)
        && before_lines[before_lines.len() - 1 - suffix]
            == after_lines[after_lines.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let removed = &before_lines[prefix..before_lines.len().saturating_sub(suffix)];
    let added = &after_lines[prefix..after_lines.len().saturating_sub(suffix)];
    let retained_removed = removed.len().min(LEDGER_DIFF_LINE_CAP / 2);
    let retained_added = added
        .len()
        .min(LEDGER_DIFF_LINE_CAP.saturating_sub(retained_removed));
    let mut lines = Vec::with_capacity(retained_removed + retained_added + 1);
    for (offset, line) in removed.iter().take(retained_removed).enumerate() {
        lines.push(SessionDiffLine {
            kind: "removed".to_string(),
            content: bounded_ledger_line(line),
            old_line: Some((prefix + offset + 1) as u64),
            new_line: None,
        });
    }
    for (offset, line) in added.iter().take(retained_added).enumerate() {
        lines.push(SessionDiffLine {
            kind: "added".to_string(),
            content: bounded_ledger_line(line),
            old_line: None,
            new_line: Some((prefix + offset + 1) as u64),
        });
    }
    let omitted_lines = removed
        .len()
        .saturating_sub(retained_removed)
        .saturating_add(added.len().saturating_sub(retained_added));
    if omitted_lines > 0 {
        lines.push(SessionDiffLine {
            kind: "note".to_string(),
            content: format!(
                "{omitted_lines} changed lines were omitted by the bounded local ledger."
            ),
            old_line: None,
            new_line: None,
        });
    }
    if lines.is_empty() {
        lines.push(SessionDiffLine {
            kind: "note".to_string(),
            content: "The file bytes changed outside line content, such as a final newline."
                .to_string(),
            old_line: None,
            new_line: None,
        });
    }

    let observed_ms = chrono::Utc::now().timestamp_millis();
    let reality = SessionFileReality {
        status: "applied".to_string(),
        label: "Applied by Code Hangar".to_string(),
        note: "This change was observed at the verified in-app write boundary.".to_string(),
        observed_ms: Some(observed_ms),
    };
    let summary = match origin {
        "value" => "Changed one recognized value",
        "ai_suggestion" | "ai_session" => "Applied one confirmed AI suggestion",
        "restore" => "Restored a verified previous version",
        _ => "Saved one in-app file edit",
    };
    let edit = SessionChangeEdit {
        source: "Code Hangar verified write".to_string(),
        summary: summary.to_string(),
        provenance: Some("Observed in-app edit ledger".to_string()),
        confidence: Some("observed".to_string()),
        reality: Some(reality.clone()),
        request: None,
        hunks: vec![SessionDiffHunk {
            header: format!(
                "@@ -{},{} +{},{} @@",
                prefix + 1,
                removed.len(),
                prefix + 1,
                added.len()
            ),
            old_start: Some((prefix + 1) as u64),
            new_start: Some((prefix + 1) as u64),
            lines,
        }],
        added_lines: added.len() as u64,
        removed_lines: removed.len() as u64,
    };
    Some(SessionChangeSet {
        path: format!("codehangar:{relative_path}"),
        source_kind: "Code Hangar edit history".to_string(),
        coverage: SessionChangeCoverage {
            level: "full".to_string(),
            label: "Observed in-app edit".to_string(),
            note: "Hashes and a bounded secret-redacted diff were recorded when Code Hangar made the verified write. Full prior bytes remain in the verified snapshot outside the project tree."
                .to_string(),
        },
        files: vec![SessionFileChange {
            path: relative_path,
            edits: vec![edit],
            added_lines: added.len() as u64,
            removed_lines: removed.len() as u64,
            reality: Some(reality),
        }],
        edit_count: 1,
        added_lines: added.len() as u64,
        removed_lines: removed.len() as u64,
        redacted_count: before_redactions.saturating_add(after_redactions),
        parsed_records: 1,
        omitted_records: omitted_lines as u64,
    })
}

fn bounded_ledger_line(line: &str) -> String {
    if line.chars().count() <= LEDGER_LINE_CHAR_CAP {
        return line.to_string();
    }
    let mut retained = line.chars().take(LEDGER_LINE_CHAR_CAP).collect::<String>();
    retained.push_str(" [line truncated by local ledger]");
    retained
}

fn load_snapshot_summaries(
    conn: &Connection,
    node_id: i64,
    limit: usize,
) -> Result<Vec<EditSnapshotSummary>, rusqlite::Error> {
    let mut stmt = conn.prepare(
        "SELECT id, node_id, project_id, path, origin, session_id, created_at, status,
                bytes, blake3_before, blake3_after, restored_at
         FROM edit_snapshot
         WHERE node_id = ?1
         ORDER BY id DESC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![node_id, limit.clamp(1, 100) as i64], |row| {
        Ok(EditSnapshotSummary {
            id: row.get(0)?,
            node_id: row.get(1)?,
            project_id: row.get(2)?,
            path: row.get(3)?,
            origin: row.get(4)?,
            session_id: row.get(5)?,
            created_at: row.get(6)?,
            status: row.get(7)?,
            bytes: row.get::<_, i64>(8)?.max(0) as u64,
            blake3_before: row.get(9)?,
            blake3_after: row.get(10)?,
            restored_at: row.get(11)?,
        })
    })?;
    rows.collect()
}

fn prune_snapshots(conn: &Connection, snapshot_root: &Path) -> DbResult<()> {
    let mut stmt = conn.prepare(
        "SELECT es.id, es.node_id, es.backup_id, b.destination, es.session_id
         FROM edit_snapshot es
         JOIN backup b ON b.id = es.backup_id
         ORDER BY es.id DESC",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(SnapshotRow {
                id: row.get(0)?,
                node_id: row.get(1)?,
                backup_id: row.get(2)?,
                destination: PathBuf::from(row.get::<_, String>(3)?),
                session_id: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);
    let mut earliest = HashMap::<i64, i64>::new();
    for row in &rows {
        earliest
            .entry(row.node_id)
            .and_modify(|id| *id = (*id).min(row.id))
            .or_insert(row.id);
    }
    let mut per_node = HashMap::<i64, usize>::new();
    let mut keep = HashSet::<i64>::new();
    for row in &rows {
        let count = per_node.entry(row.node_id).or_default();
        if *count < SNAPSHOTS_PER_FILE {
            keep.insert(row.id);
            *count += 1;
        }
    }
    keep.extend(earliest.values().copied());
    let mut earliest_session = HashMap::<(String, i64), i64>::new();
    for row in &rows {
        if let Some(session_id) = row.session_id.as_ref() {
            earliest_session
                .entry((session_id.clone(), row.node_id))
                .and_modify(|id| *id = (*id).min(row.id))
                .or_insert(row.id);
        }
    }
    keep.extend(earliest_session.values().copied());
    let mut remove: HashSet<i64> = rows
        .iter()
        .filter(|row| !keep.contains(&row.id))
        .map(|row| row.id)
        .collect();
    let mut remaining = rows.len().saturating_sub(remove.len());
    if remaining > GLOBAL_SNAPSHOT_CAP {
        for row in rows.iter().rev() {
            if remaining <= GLOBAL_SNAPSHOT_CAP {
                break;
            }
            if earliest.get(&row.node_id) == Some(&row.id) || remove.contains(&row.id) {
                continue;
            }
            remove.insert(row.id);
            remaining -= 1;
        }
    }
    for row in rows.iter().filter(|row| remove.contains(&row.id)) {
        if !snapshot_destination_is_owned(snapshot_root, &row.destination) {
            continue;
        }
        if row.destination.exists() && fs::remove_dir_all(&row.destination).is_err() {
            continue;
        }
        conn.execute("DELETE FROM edit_snapshot WHERE id = ?1", [row.id])?;
        conn.execute("DELETE FROM backup WHERE id = ?1", [row.backup_id])?;
    }
    Ok(())
}

fn snapshot_destination_is_owned(root: &Path, destination: &Path) -> bool {
    destination.parent() == Some(root) && destination.file_name().is_some()
}

fn atomic_replace(target: &Path, content: &[u8], node_id: i64) -> Result<(), String> {
    let dir = target
        .parent()
        .ok_or_else(|| "Not saved: the file has no parent directory.".to_string())?;
    let temp = dir.join(format!(
        ".code-hangar-edit-{node_id}-{}.tmp",
        chrono::Utc::now().timestamp_micros()
    ));
    let permissions = fs::metadata(target)
        .map_err(|error| format!("Not saved: could not preserve file permissions ({error})."))?
        .permissions();
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)
        .map_err(|error| format!("Not saved: could not stage the change ({error})."))?;
    if let Err(error) = file.write_all(content).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(&temp);
        return Err(format!(
            "Not saved: could not flush the staged change ({error})."
        ));
    }
    drop(file);
    if let Err(error) = fs::set_permissions(&temp, permissions) {
        let _ = fs::remove_file(&temp);
        return Err(format!(
            "Not saved: could not preserve file permissions ({error})."
        ));
    }
    if let Err(error) = fs::rename(&temp, target) {
        let _ = fs::remove_file(&temp);
        return Err(format!("Not saved: could not replace the file ({error})."));
    }
    Ok(())
}

pub(crate) fn validate_edit_content(content: &str) -> Result<(), String> {
    if content.len() > MAX_EDIT_BYTES {
        return Err(
            "Not saved: the edited content is larger than the editable size limit.".to_string(),
        );
    }
    if content.contains('\0') {
        return Err("Not saved: the content is not text.".to_string());
    }
    Ok(())
}

fn validate_origin(origin: &str) -> Result<&str, String> {
    match origin {
        "manual" | "value" | "ai_suggestion" | "ai_session" | "restore" => Ok(origin),
        _ => Err("Not saved: unknown edit origin.".to_string()),
    }
}

fn snapshot_root(state: &super::AppState) -> Result<PathBuf, String> {
    if state.db_path().as_os_str().is_empty() {
        return Err("Durable edit history requires the file-backed desktop inventory.".to_string());
    }
    state
        .db_path()
        .parent()
        .map(|parent| parent.join("edit-snapshots"))
        .ok_or_else(|| "Durable edit history could not locate the app data folder.".to_string())
}

fn same_canonical_file(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_app_ledger_diff_is_redacted_bounded_and_excludes_generated_trees() {
        let root = tempfile::tempdir().unwrap();
        let safe = root.path().join("src").join("settings.ts");
        fs::create_dir_all(safe.parent().unwrap()).unwrap();
        let change = build_in_app_change_set(
            root.path(),
            &safe,
            "const token = \"sk-ABCDEF1234567890ABCDEFGH\";\nconst count = 2;\n",
            "const token = \"sk-ABCDEF1234567890ABCDEFGH\";\nconst count = 3;\n",
            "value",
        )
        .unwrap();
        let encoded = serde_json::to_string(&change).unwrap();
        assert!(!encoded.contains("sk-ABCDEF1234567890ABCDEFGH"));
        assert!(change.redacted_count > 0);
        assert_eq!(change.files[0].path, "src/settings.ts");
        assert_eq!(
            change.files[0].edits[0].summary,
            "Changed one recognized value"
        );

        let cache = root
            .path()
            .join("node_modules")
            .join("package")
            .join("index.js");
        assert!(build_in_app_change_set(root.path(), &cache, "a", "b", "manual").is_none());
        let sensitive = root.path().join(".env");
        assert!(build_in_app_change_set(root.path(), &sensitive, "A=1", "A=2", "manual").is_none());
    }

    #[test]
    fn verified_snapshot_exists_before_atomic_write_and_can_restore_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("project");
        let snapshots = temp.path().join("snapshots");
        fs::create_dir_all(&source).unwrap();
        let file = source.join("settings.json");
        fs::write(&file, "{\"enabled\":false}\n").unwrap();
        let conn = Connection::open_in_memory().unwrap();
        hangar_mutation::ensure_journal_schema(&conn).unwrap();

        let result = snapshot_and_write_with_conn(
            &conn,
            &snapshots,
            SnapshotWrite {
                node_id: 7,
                project_id: 3,
                target: &file,
                content: "{\"enabled\":true}\n",
                origin: "manual",
                session_id: None,
                expected_before_hash: None,
            },
        )
        .unwrap();
        assert_eq!(result.previous, "{\"enabled\":false}\n");
        assert_eq!(fs::read_to_string(&file).unwrap(), "{\"enabled\":true}\n");

        let (backup_id, before_hash, status): (i64, String, String) = conn
            .query_row(
                "SELECT backup_id, blake3_before, status FROM edit_snapshot WHERE id = ?1",
                [result.snapshot_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(status, "saved");
        let backup = hangar_mutation::load_verified_backup(&conn, backup_id).unwrap();
        backup.verify_payload(&file.to_string_lossy()).unwrap();
        let copy = backup.copy_for(&file.to_string_lossy()).unwrap();
        assert_eq!(
            fs::read_to_string(&copy.backup_path).unwrap(),
            result.previous
        );
        assert_eq!(
            before_hash,
            blake3::hash(result.previous.as_bytes())
                .to_hex()
                .to_string()
        );
    }

    #[test]
    fn refuses_identical_content_without_creating_a_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("same.toml");
        fs::write(&file, "enabled = true\n").unwrap();
        let conn = Connection::open_in_memory().unwrap();
        hangar_mutation::ensure_journal_schema(&conn).unwrap();

        let error = snapshot_and_write_with_conn(
            &conn,
            &temp.path().join("snapshots"),
            SnapshotWrite {
                node_id: 1,
                project_id: 1,
                target: &file,
                content: "enabled = true\n",
                origin: "manual",
                session_id: None,
                expected_before_hash: None,
            },
        )
        .unwrap_err();
        assert!(error.contains("identical"));
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM edit_snapshot", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn refuses_a_stale_expected_hash_before_snapshot_or_write() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("stale.json");
        fs::write(&file, "{\"count\":2}\n").unwrap();
        let conn = Connection::open_in_memory().unwrap();
        hangar_mutation::ensure_journal_schema(&conn).unwrap();

        let error = snapshot_and_write_with_conn(
            &conn,
            &temp.path().join("snapshots"),
            SnapshotWrite {
                node_id: 1,
                project_id: 1,
                target: &file,
                content: "{\"count\":3}\n",
                origin: "manual",
                session_id: None,
                expected_before_hash: Some("stale-hash"),
            },
        )
        .unwrap_err();
        assert!(error.contains("changed on disk"));
        assert_eq!(fs::read_to_string(&file).unwrap(), "{\"count\":2}\n");
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM edit_snapshot", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
}
