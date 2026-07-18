//! Operation Plan + Risk Report — **preview only**.
//!
//! This crate builds a read-only *preview* of a future Operation Plan and its
//! Risk Report. An Operation Plan describes the **backup, quarantine/move, or
//! delete** actions a later (mutation-enabled) phase could perform on files and
//! whole projects to free space or reorganize them — it is a "what would happen"
//! dry run, deliberately built before any operation feature exists.
//!
//! Nothing here executes anything: no file is backed up, moved, or deleted, and
//! no plan is persisted. The only write this crate performs is
//! [`export_risk_report`], which serializes a report to a path the user chose.
use chrono::Utc;
use hangar_accounting::{
    recoverable_for_target, recoverable_for_target_cancellable, AccountingCandidate,
    AccountingError,
};
use hangar_core::{
    ConfidenceSummary, DanglingAfter, ExportResult, GitWarning, OperationPlan, OperationPlanItem,
    OperationPlanTarget, ProtectedHit, RecoverableBytes, RiskReport, RiskTier, RiskTierCount,
    SensitiveFileRef, SharedAsset,
};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use thiserror::Error;

const COMPACT_ITEM_THRESHOLD: usize = 250;
const MAX_VISIBLE_ITEMS: usize = 500;

#[derive(Debug, Error)]
pub enum PlanError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("cancelled")]
    Cancelled,
    #[error("report export error: {0}")]
    Io(#[from] std::io::Error),
    #[error("report serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type PlanResult<T> = Result<T, PlanError>;

/// Build a **preview-only** Operation Plan for `target_node_id` (a project or
/// file/folder). The plan estimates the future backup / quarantine-move / delete
/// actions that could free space or reorganize the target, classified by risk
/// tier, with recoverable bytes, shared assets, dangling references, and
/// sensitive/protected hits. It executes nothing and persists nothing.
pub fn build_operation_plan(
    conn: &Connection,
    target_node_id: i64,
    action_label: &str,
) -> PlanResult<OperationPlan> {
    build_operation_plan_checked(conn, target_node_id, action_label, None)
}

pub fn build_operation_plan_with_cancel(
    conn: &Connection,
    target_node_id: i64,
    action_label: &str,
    cancel: &AtomicBool,
) -> PlanResult<OperationPlan> {
    build_operation_plan_checked(conn, target_node_id, action_label, Some(cancel))
}

fn build_operation_plan_checked(
    conn: &Connection,
    target_node_id: i64,
    action_label: &str,
    cancel: Option<&AtomicBool>,
) -> PlanResult<OperationPlan> {
    check_cancelled(cancel)?;
    let accounting = match cancel {
        Some(cancel) => recoverable_for_target_cancellable(conn, target_node_id, cancel)
            .map_err(map_accounting_error)?,
        None => recoverable_for_target(conn, target_node_id)?,
    };
    check_cancelled(cancel)?;
    let target = load_target(conn, target_node_id, accounting.target_project_id)?;
    let candidates = accounting.candidates.clone();
    let partial_footprint = accounting.summary.partial_footprint;
    check_cancelled(cancel)?;
    let items = build_items(
        &target,
        action_label,
        &candidates,
        &accounting.recoverable_node_ids,
        &accounting.summary.recoverable_bytes,
        partial_footprint,
        cancel,
    )?;
    let sensitive_files = sensitive_files(&candidates, cancel)?;
    let protected_hits = protected_hits(&candidates, cancel)?;
    let (dangling_after, dangling_truncated) = dangling_after(
        conn,
        accounting.target_project_id,
        &accounting.target_node_ids,
        cancel,
    )?;
    check_cancelled(cancel)?;
    let git_warnings = git_warnings(conn, accounting.target_project_id)?;
    let confidence_summary =
        confidence_summary(&items, &accounting.shared_assets, &dangling_after, cancel)?;
    let fingerprint = target_fingerprint(
        target_node_id,
        &candidates,
        &accounting.summary.recoverable_bytes,
        partial_footprint,
        cancel,
    )?;

    Ok(OperationPlan {
        plan_id: format!("preview-{target_node_id}-{}", Utc::now().timestamp_millis()),
        schema: "operation_plan/1".to_string(),
        created_at: Utc::now().to_rfc3339(),
        target,
        action_label: action_label.to_string(),
        items,
        recoverable_bytes: accounting.summary.recoverable_bytes,
        shared_assets: accounting.shared_assets,
        dangling_after,
        sensitive_files,
        protected_hits,
        git_warnings,
        confidence_summary,
        recommended_action:
            "Use this review as evidence, not an action queue. Focus on shared references, protected paths and incomplete scan areas before deciding what matters."
                .to_string(),
        read_only_preview: true,
        plan_stale: false,
        partial_footprint,
        dangling_truncated,
        external_services_unaffected: true,
        target_fingerprint: fingerprint,
    })
}

/// Project an [`OperationPlan`] into a read-only Risk Report (tier roll-ups,
/// recoverable bytes with caveats, shared/dangling/sensitive/protected/git
/// sections). Preview only — reads local evidence and runs no disk action.
pub fn build_risk_report(plan: &OperationPlan) -> RiskReport {
    let caveats = report_caveats(plan);
    RiskReport {
        schema: "risk_report/1".to_string(),
        generated_at: Utc::now().to_rfc3339(),
        target: plan.target.clone(),
        action_label: plan.action_label.clone(),
        read_only_preview: true,
        external_services_unaffected: true,
        recoverable_bytes: plan.recoverable_bytes.clone(),
        risk_counts: risk_counts(&plan.items),
        shared_assets: plan.shared_assets.clone(),
        dangling_after: plan.dangling_after.clone(),
        dangling_truncated: plan.dangling_truncated,
        sensitive_files: plan.sensitive_files.clone(),
        protected_hits: plan.protected_hits.clone(),
        git_warnings: plan.git_warnings.clone(),
        confidence_summary: plan.confidence_summary.clone(),
        recommended_action: plan.recommended_action.clone(),
        caveats,
    }
}

pub fn build_risk_report_for_target(
    conn: &Connection,
    target_node_id: i64,
    action_label: &str,
) -> PlanResult<RiskReport> {
    let plan = build_operation_plan(conn, target_node_id, action_label)?;
    Ok(build_risk_report(&plan))
}

/// Serialize a Risk Report to `path` as JSON. This is the only write the crate
/// performs; it touches no scanned/inventoried files and runs no operation.
pub fn export_risk_report(report: &RiskReport, path: impl AsRef<Path>) -> PlanResult<ExportResult> {
    let bytes = serde_json::to_vec_pretty(report)?;
    std::fs::write(path.as_ref(), &bytes)?;
    Ok(ExportResult {
        path: path.as_ref().to_string_lossy().to_string(),
        bytes_written: bytes.len() as u64,
    })
}

fn map_accounting_error(error: AccountingError) -> PlanError {
    match error {
        AccountingError::Sqlite(error) => PlanError::Sqlite(error),
        AccountingError::Cancelled => PlanError::Cancelled,
    }
}

fn check_cancelled(cancel: Option<&AtomicBool>) -> PlanResult<()> {
    if cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
        Err(PlanError::Cancelled)
    } else {
        Ok(())
    }
}

fn load_target(
    conn: &Connection,
    target_node_id: i64,
    fallback_project_id: i64,
) -> rusqlite::Result<OperationPlanTarget> {
    let target = conn
        .query_row(
            "SELECT n.id, COALESCE(ni.project_id, CASE WHEN n.kind = 'project' THEN n.id END),
                    COALESCE(ni.item_kind, n.kind),
                    COALESCE(ni.path, n.path, n.name, ''),
                    COALESCE(ni.display_name, n.name, n.path, '')
             FROM node n
             LEFT JOIN nav_item ni ON ni.node_id = n.id
             WHERE n.id = ?1
             ORDER BY ni.id
             LIMIT 1",
            params![target_node_id],
            |row| {
                Ok(OperationPlanTarget {
                    node_id: row.get(0)?,
                    project_id: row.get::<_, Option<i64>>(1)?.unwrap_or(fallback_project_id),
                    kind: row.get(2)?,
                    path: row.get(3)?,
                    display_name: row.get(4)?,
                })
            },
        )
        .optional()?;
    target.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

fn build_items(
    target: &OperationPlanTarget,
    action_label: &str,
    candidates: &[AccountingCandidate],
    recoverable_node_ids: &std::collections::HashSet<i64>,
    recoverable_bytes: &RecoverableBytes,
    partial: bool,
    cancel: Option<&AtomicBool>,
) -> PlanResult<Vec<OperationPlanItem>> {
    check_cancelled(cancel)?;
    if target.kind == "project"
        || target.kind == "directory"
        || candidates.len() > COMPACT_ITEM_THRESHOLD
    {
        let mut size_apparent = 0_u64;
        for (index, candidate) in candidates.iter().enumerate() {
            if index % 4096 == 0 {
                check_cancelled(cancel)?;
            }
            size_apparent += candidate.size_apparent;
        }
        return Ok(vec![OperationPlanItem {
            node_id: Some(target.node_id),
            path: target.path.clone(),
            display_name: target.display_name.clone(),
            item_kind: "recursive_dir".to_string(),
            action_label: action_label.to_string(),
            risk: classify_path(&target.path, &target.kind, false, None, false, true),
            confidence: if partial { "Low" } else { "Medium" }.to_string(),
            size_apparent,
            physical_bytes: Some(recoverable_bytes.total),
            hardlink_group: None,
            frees_space: recoverable_bytes.total > 0,
            recursive_dir: true,
            child_count: candidates.len() as u64,
            partial,
        }]);
    }

    let mut items = Vec::new();
    for (index, candidate) in candidates.iter().take(MAX_VISIBLE_ITEMS).enumerate() {
        if index % 1024 == 0 {
            check_cancelled(cancel)?;
        }
        let frees_space = recoverable_node_ids.contains(&candidate.node_id);
        items.push(OperationPlanItem {
            node_id: Some(candidate.node_id),
            path: candidate.path.clone(),
            display_name: candidate.display_name.clone(),
            item_kind: candidate.item_kind.clone(),
            action_label: action_label.to_string(),
            risk: classify_path(
                &candidate.path,
                &candidate.item_kind,
                candidate.is_sensitive,
                candidate.protected_level.as_deref(),
                !frees_space,
                false,
            ),
            confidence: confidence_for_candidate(candidate, frees_space),
            size_apparent: candidate.size_apparent,
            physical_bytes: frees_space.then_some(candidate.physical_bytes),
            hardlink_group: candidate
                .identity_key
                .starts_with("inode:")
                .then(|| candidate.identity_key.clone()),
            frees_space,
            recursive_dir: false,
            child_count: 0,
            partial: candidate.partial,
        });
    }
    Ok(items)
}

fn classify_path(
    path: &str,
    item_kind: &str,
    is_sensitive: bool,
    protected_level: Option<&str>,
    shared_or_external: bool,
    recursive_dir: bool,
) -> RiskTier {
    let lower = path.replace('\\', "/").to_lowercase();
    if is_sensitive
        || protected_level.is_some()
        || lower.contains("/.ssh/")
        || lower.ends_with("/.ssh")
        || lower.contains("/windows/")
        || lower.contains("/appdata/")
    {
        return RiskTier::Black;
    }
    if shared_or_external
        || lower.contains("/models/")
        || lower.contains("/huggingface/")
        || lower.contains("/ollama/")
        || lower.ends_with(".gguf")
        || lower.ends_with(".safetensors")
        || lower.ends_with(".ckpt")
        || lower.ends_with(".onnx")
        || lower.ends_with(".pt")
        || lower.ends_with(".pth")
    {
        return RiskTier::Red;
    }
    if lower.contains("/prompts/")
        || lower.contains("/workflows/")
        || lower.contains("/datasets/")
        || lower.ends_with(".db")
        || lower.ends_with(".sqlite")
        || lower.ends_with(".sqlite3")
        || lower.ends_with(".jsonl")
        || lower.ends_with(".parquet")
        || lower.ends_with(".notes")
    {
        return RiskTier::Orange;
    }
    if lower.contains("/node_modules/")
        || lower.contains("/.venv/")
        || lower.contains("/venv/")
        || lower.contains("/target/")
        || lower.contains("/__pycache__/")
    {
        return RiskTier::Yellow;
    }
    if lower.contains("/.cache/")
        || lower.contains("/dist/")
        || lower.contains("/build/")
        || lower.contains("/tmp/")
        || lower.contains("/temp/")
        || lower.contains("/logs/")
        || lower.ends_with(".log")
    {
        return RiskTier::Green;
    }
    if recursive_dir || item_kind == "directory" {
        RiskTier::Yellow
    } else {
        RiskTier::Orange
    }
}

fn confidence_for_candidate(candidate: &AccountingCandidate, frees_space: bool) -> String {
    if candidate.partial {
        "Low".to_string()
    } else if !frees_space {
        "Medium".to_string()
    } else {
        "High".to_string()
    }
}

fn sensitive_files(
    candidates: &[AccountingCandidate],
    cancel: Option<&AtomicBool>,
) -> PlanResult<Vec<SensitiveFileRef>> {
    let mut files = Vec::new();
    for (index, candidate) in candidates.iter().enumerate() {
        if index % 4096 == 0 {
            check_cancelled(cancel)?;
        }
        if candidate.is_sensitive {
            files.push(SensitiveFileRef {
                node_id: Some(candidate.node_id),
                path: candidate.path.clone(),
                signature: "Sensitive filename or protected pattern".to_string(),
            });
        }
    }
    Ok(files)
}

fn protected_hits(
    candidates: &[AccountingCandidate],
    cancel: Option<&AtomicBool>,
) -> PlanResult<Vec<ProtectedHit>> {
    let mut hits = Vec::new();
    for (index, candidate) in candidates.iter().enumerate() {
        if index % 4096 == 0 {
            check_cancelled(cancel)?;
        }
        if let Some(level) = candidate.protected_level.as_ref() {
            hits.push(ProtectedHit {
                node_id: Some(candidate.node_id),
                path: candidate.path.clone(),
                level: level.clone(),
            });
        }
    }
    Ok(hits)
}

/// Returns the dependents that would be left dangling, plus a `truncated` flag set when either
/// query hit its per-query row cap (so more dependents than listed may exist).
fn dangling_after(
    conn: &Connection,
    project_id: i64,
    target_node_ids: &std::collections::HashSet<i64>,
    cancel: Option<&AtomicBool>,
) -> PlanResult<(Vec<DanglingAfter>, bool)> {
    check_cancelled(cancel)?;
    if target_node_ids.is_empty() {
        return Ok((Vec::new(), false));
    }
    // The name of the project being removed — used to label local broken references.
    let target_project_name: Option<String> = conn
        .query_row(
            "SELECT name FROM node WHERE id = ?1",
            params![project_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten();
    let mut stmt = conn.prepare(
        "SELECT ri.node_id, COALESCE(ni.path, ''), ri.target, ri.confidence, ri.kind
         FROM relationship_issue ri
         LEFT JOIN nav_item ni ON ni.node_id = ri.node_id
         WHERE ri.project_id = ?1
         ORDER BY ri.confidence, ri.target
         LIMIT 200",
    )?;
    let rows = stmt.query_map(params![project_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    let mut issues = Vec::new();
    let mut raw_local = 0usize;
    for (index, row) in rows.enumerate() {
        if index % 512 == 0 {
            check_cancelled(cancel)?;
        }
        raw_local += 1;
        let (referrer_node_id, path, missing_path, confidence, kind) = row?;
        // Local broken references live inside the project being removed (same project, lower risk).
        if target_node_ids.contains(&referrer_node_id) {
            issues.push(DanglingAfter {
                referrer_node_id,
                path,
                missing_path,
                confidence,
                project_id: Some(project_id),
                project_name: target_project_name.clone(),
                dependency_kind: kind,
                cross_project: false,
            });
        }
    }
    let mut truncated = raw_local >= 200;

    // Forward dangling impact: removing a MODEL that local workflows currently
    // resolve to would leave each of those workflows with a missing model
    // reference. The graph's resolved `workflow_references_model` edges (dst = the
    // model being removed) make this precise — and catch workflows in OTHER
    // projects that share the same model, which the project-scoped issue table
    // above never sees. Read-only evidence; it does not authorize anything.
    check_cancelled(cancel)?;
    // A whole-project target can have thousands of node ids; binding them twice (e.dst IN … AND
    // e.src NOT IN …) blows past SQLite's max-variable limit ("too many SQL variables") and broke
    // Build reviews for large projects. Stage the id set in a TEMP table and IN-subquery it
    // instead, which has no variable-count limit.
    let target_ids = target_node_ids.iter().copied().collect::<Vec<_>>();
    conn.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS plan_dangling_target(id INTEGER PRIMARY KEY);
         DELETE FROM plan_dangling_target;",
    )?;
    {
        let mut insert =
            conn.prepare("INSERT OR IGNORE INTO plan_dangling_target(id) VALUES (?1)")?;
        for id in &target_ids {
            insert.execute([id])?;
        }
    }
    // A workflow node can be registered in SEVERAL projects (nav_item is UNIQUE(project_id, path),
    // NOT unique on node_id), so a naive JOIN multiplies each edge into one row per project the
    // referrer belongs to. Aggregate per (src, dst) edge so the row count is stable AND so
    // cross_project is true when ANY registration is outside the removed project — otherwise the
    // removed project's own row could win the dedup and HIDE a cross-project break (under-warning
    // on a delete-safety surface). The named project prefers a cross-project one when present.
    let mut edge_stmt = conn.prepare(
        "SELECT g.src, g.path, g.missing_path, g.confidence, g.cross_project, g.project_id, pn.name
         FROM (
             SELECT e.src AS src,
                    COALESCE(MIN(wf.path), '') AS path,
                    COALESCE(MIN(md.path), MIN(e.evidence), '') AS missing_path,
                    MIN(e.confidence) AS confidence,
                    MAX(CASE WHEN wf.project_id IS NOT NULL AND wf.project_id <> ?1 THEN 1 ELSE 0 END) AS cross_project,
                    COALESCE(
                        MAX(CASE WHEN wf.project_id IS NOT NULL AND wf.project_id <> ?1 THEN wf.project_id END),
                        MAX(wf.project_id)
                    ) AS project_id
             FROM edge e
             LEFT JOIN nav_item wf ON wf.node_id = e.src
             LEFT JOIN nav_item md ON md.node_id = e.dst
             WHERE e.kind = 'workflow_references_model'
               AND e.dst IN (SELECT id FROM plan_dangling_target)
               AND e.src NOT IN (SELECT id FROM plan_dangling_target)
             GROUP BY e.src, e.dst
         ) g
         LEFT JOIN node pn ON pn.id = g.project_id
         ORDER BY g.src
         LIMIT 200",
    )?;
    let edge_rows = edge_stmt.query_map(params![project_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, Option<i64>>(5)?,
            row.get::<_, Option<String>>(6)?,
        ))
    })?;
    let mut seen = issues
        .iter()
        .map(|issue| (issue.referrer_node_id, issue.missing_path.clone()))
        .collect::<std::collections::HashSet<_>>();
    let mut raw_edges = 0usize;
    for (index, row) in edge_rows.enumerate() {
        if index % 512 == 0 {
            check_cancelled(cancel)?;
        }
        raw_edges += 1;
        let (
            referrer_node_id,
            path,
            missing_path,
            confidence,
            cross_flag,
            ref_project_id,
            ref_project_name,
        ) = row?;
        let issue = DanglingAfter {
            referrer_node_id,
            path,
            missing_path,
            confidence,
            project_id: ref_project_id,
            project_name: ref_project_name,
            dependency_kind: "workflow".to_string(),
            cross_project: cross_flag != 0,
        };
        if seen.insert((issue.referrer_node_id, issue.missing_path.clone())) {
            issues.push(issue);
        }
    }
    truncated = truncated || raw_edges >= 200;
    Ok((issues, truncated))
}

fn git_warnings(conn: &Connection, project_id: i64) -> rusqlite::Result<Vec<GitWarning>> {
    let has_git = conn
        .query_row(
            "SELECT 1 FROM git_repo WHERE project_id = ?1 LIMIT 1",
            params![project_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if has_git {
        Ok(vec![GitWarning {
            project_id,
            message: "Local Git metadata was detected. Review local state outside Code Hangar before future disk action.".to_string(),
            confidence: "Medium".to_string(),
        }])
    } else {
        Ok(Vec::new())
    }
}

fn confidence_summary(
    items: &[OperationPlanItem],
    shared_assets: &[SharedAsset],
    dangling_after: &[DanglingAfter],
    cancel: Option<&AtomicBool>,
) -> PlanResult<ConfidenceSummary> {
    let mut summary = ConfidenceSummary::default();
    for (index, confidence) in items
        .iter()
        .map(|item| item.confidence.as_str())
        .chain(shared_assets.iter().map(|item| item.confidence.as_str()))
        .chain(dangling_after.iter().map(|item| item.confidence.as_str()))
        .enumerate()
    {
        if index % 1024 == 0 {
            check_cancelled(cancel)?;
        }
        match confidence {
            "High" => summary.high += 1,
            "Medium" => summary.medium += 1,
            "Low" => summary.low += 1,
            _ => summary.unknown += 1,
        }
    }
    Ok(summary)
}

fn target_fingerprint(
    target_node_id: i64,
    candidates: &[AccountingCandidate],
    recoverable_bytes: &RecoverableBytes,
    partial: bool,
    cancel: Option<&AtomicBool>,
) -> PlanResult<String> {
    let mut apparent = 0_u64;
    for (index, candidate) in candidates.iter().enumerate() {
        if index % 4096 == 0 {
            check_cancelled(cancel)?;
        }
        apparent += candidate.size_apparent;
    }
    Ok(format!(
        "{target_node_id}:{}:{apparent}:{}:{partial}",
        candidates.len(),
        recoverable_bytes.total
    ))
}

fn risk_counts(items: &[OperationPlanItem]) -> Vec<RiskTierCount> {
    let mut map = HashMap::<String, (RiskTier, u64, u64, bool)>::new();
    for item in items {
        let key = format!("{:?}", item.risk);
        let entry = map.entry(key).or_insert((item.risk.clone(), 0, 0, false));
        entry.1 += 1;
        if let Some(bytes) = item.physical_bytes {
            entry.2 += bytes;
            entry.3 = true;
        }
    }
    let order = [
        RiskTier::Green,
        RiskTier::Yellow,
        RiskTier::Orange,
        RiskTier::Red,
        RiskTier::Black,
    ];
    order
        .into_iter()
        .filter_map(|tier| {
            map.remove(&format!("{:?}", tier))
                .map(|(_, count, bytes, has_bytes)| RiskTierCount {
                    tier,
                    count,
                    physical_bytes: has_bytes.then_some(bytes),
                })
        })
        .collect()
}

fn report_caveats(plan: &OperationPlan) -> Vec<String> {
    let mut caveats = vec![
        "Preview only: no filesystem action was run.".to_string(),
        "External services are unaffected.".to_string(),
    ];
    if plan.partial_footprint {
        caveats.push(
            "Some footprint values are lower-bound estimates because inventory is partial."
                .to_string(),
        );
    }
    if plan.plan_stale {
        caveats.push("The target changed after the preview was built.".to_string());
    }
    if !plan.shared_assets.is_empty() {
        caveats
            .push("Shared assets require manual review before any future disk action.".to_string());
    }
    caveats
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE node (
              id INTEGER PRIMARY KEY,
              kind TEXT NOT NULL,
              path TEXT,
              name TEXT,
              protected_level TEXT,
              volume_id TEXT,
              inode_key TEXT,
              link_count INTEGER,
              is_reparse INTEGER NOT NULL DEFAULT 0,
              reparse_kind TEXT,
              size_apparent INTEGER,
              size_allocated INTEGER,
              present INTEGER NOT NULL DEFAULT 1
            );
            CREATE TABLE nav_item (
              id INTEGER PRIMARY KEY,
              project_id INTEGER NOT NULL,
              node_id INTEGER,
              parent_nav_id INTEGER,
              path TEXT NOT NULL,
              display_name TEXT NOT NULL,
              item_kind TEXT NOT NULL,
              is_sensitive INTEGER NOT NULL DEFAULT 0,
              protected_level TEXT,
              fully_scanned INTEGER NOT NULL DEFAULT 1,
              scan_error TEXT,
              aggregate_bytes_partial INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE edge (
              id INTEGER PRIMARY KEY,
              src INTEGER NOT NULL,
              dst INTEGER NOT NULL,
              kind TEXT NOT NULL,
              confidence TEXT NOT NULL,
              evidence TEXT
            );
            CREATE TABLE relationship_issue (
              id INTEGER PRIMARY KEY,
              node_id INTEGER NOT NULL,
              project_id INTEGER,
              kind TEXT NOT NULL,
              confidence TEXT NOT NULL,
              target TEXT NOT NULL,
              evidence TEXT
            );
            CREATE TABLE git_repo (
              project_id INTEGER PRIMARY KEY,
              current_branch TEXT,
              head_ref TEXT,
              origin_url TEXT,
              metadata_error TEXT,
              indexed_at TEXT NOT NULL
            );
            ",
        )
        .unwrap();
        conn
    }

    #[test]
    fn plan_is_read_only_preview() {
        let conn = test_conn();
        conn.execute(
            "INSERT INTO node(id, kind, path, name, present) VALUES(1, 'project', 'fixture://one', 'one', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO node(id, kind, path, name, volume_id, inode_key, link_count, size_apparent, size_allocated, present)
             VALUES(10, 'file', 'README.md', 'README.md', 'vol', 'readme', 1, 100, 100, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO nav_item(id, project_id, node_id, path, display_name, item_kind)
             VALUES(100, 1, 10, 'README.md', 'README.md', 'file')",
            [],
        )
        .unwrap();

        let plan = build_operation_plan(&conn, 1, "Future cleanup review").unwrap();
        assert!(plan.read_only_preview);
        assert!(plan.external_services_unaffected);
        assert_eq!(plan.schema, "operation_plan/1");
        assert_eq!(plan.recoverable_bytes.total, 100);

        let report = build_risk_report(&plan);
        assert!(report.read_only_preview);
        assert!(report
            .caveats
            .iter()
            .any(|caveat| caveat.contains("Preview only")));
    }

    #[test]
    fn removing_a_model_reports_dependent_workflows_as_dangling() {
        let conn = test_conn();
        conn.execute(
            "INSERT INTO node(id, kind, path, name, present) VALUES(1, 'project', 'fixture://p', 'p', 1)",
            [],
        )
        .unwrap();
        // The model file being removed.
        conn.execute(
            "INSERT INTO node(id, kind, path, name, volume_id, inode_key, link_count, size_apparent, size_allocated, present)
             VALUES(10, 'file', 'models/sd.safetensors', 'sd.safetensors', 'vol', 'model', 1, 4096, 4096, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO nav_item(id, project_id, node_id, path, display_name, item_kind)
             VALUES(110, 1, 10, 'models/sd.safetensors', 'sd.safetensors', 'file')",
            [],
        )
        .unwrap();
        // A workflow that currently resolves to that model.
        conn.execute(
            "INSERT INTO node(id, kind, path, name, present) VALUES(20, 'file', 'workflows/wf.json', 'wf.json', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO nav_item(id, project_id, node_id, path, display_name, item_kind)
             VALUES(120, 1, 20, 'workflows/wf.json', 'wf.json', 'file')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO edge(id, src, dst, kind, confidence, evidence)
             VALUES(1, 20, 10, 'workflow_references_model', 'High', 'sd.safetensors')",
            [],
        )
        .unwrap();

        let plan = build_operation_plan(&conn, 10, "Future cleanup review").unwrap();
        assert!(
            plan.dangling_after
                .iter()
                .any(|d| d.referrer_node_id == 20 && d.missing_path == "models/sd.safetensors"),
            "removing the model must report the dependent workflow as dangling: {:?}",
            plan.dangling_after
        );
        // And the workflow must surface in the Risk Report.
        let report = build_risk_report(&plan);
        assert!(report
            .dangling_after
            .iter()
            .any(|d| d.referrer_node_id == 20));
    }

    #[test]
    fn dangling_after_handles_target_sets_larger_than_the_sql_variable_limit() {
        // Regression: the forward-dangling query used to bind the target node-id set TWICE as
        // inline placeholders (e.dst IN (…) AND e.src NOT IN (…)). A whole-project target has
        // thousands of node ids, so 2×N blew past SQLite's max variable count and "Calculate
        // preview" crashed with "too many SQL variables" on large projects. The set is now staged
        // in a TEMP table, so any size works. Found live via computer-use on the Code Hangar repo.
        let conn = test_conn();
        conn.execute(
            "INSERT INTO node(id, kind, path, name, present) VALUES(1, 'project', 'fixture://p', 'p', 1)",
            [],
        )
        .unwrap();
        // The model being removed, plus a workflow (NOT removed) that resolves to it.
        conn.execute(
            "INSERT INTO node(id, kind, path, name, volume_id, inode_key, link_count, size_apparent, size_allocated, present)
             VALUES(10, 'file', 'models/sd.safetensors', 'sd.safetensors', 'vol', 'model', 1, 4096, 4096, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO nav_item(id, project_id, node_id, path, display_name, item_kind)
             VALUES(110, 1, 10, 'models/sd.safetensors', 'sd.safetensors', 'file')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO node(id, kind, path, name, present) VALUES(20, 'file', 'workflows/wf.json', 'wf.json', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO nav_item(id, project_id, node_id, path, display_name, item_kind)
             VALUES(120, 1, 20, 'workflows/wf.json', 'wf.json', 'file')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO edge(id, src, dst, kind, confidence, evidence)
             VALUES(1, 20, 10, 'workflow_references_model', 'High', 'sd.safetensors')",
            [],
        )
        .unwrap();

        // A target set far larger than SQLite's hard variable cap (32766): the old 2×N binding
        // would exceed it; the temp-table form must not. Includes the model (10) but never the
        // workflow (20), so the un-removed workflow is reported as newly dangling.
        let mut target: std::collections::HashSet<i64> = (1_000..21_000).collect();
        target.insert(10);
        assert!(!target.contains(&20));

        let (dangling, _truncated) = dangling_after(&conn, 1, &target, None)
            .expect("dangling_after must handle target sets larger than the SQL variable limit");
        assert!(
            dangling
                .iter()
                .any(|d| d.referrer_node_id == 20 && d.missing_path == "models/sd.safetensors"),
            "the un-removed workflow must be reported as dangling: {dangling:?}"
        );
        // The workflow lives in the same project (1) as the model, so it is NOT cross-project.
        assert!(dangling
            .iter()
            .find(|d| d.referrer_node_id == 20)
            .is_some_and(|d| !d.cross_project));
    }

    #[test]
    fn cross_project_dangling_is_flagged_with_project_and_kind() {
        // A model in project A is referenced by a workflow in a DIFFERENT project B. Removing the
        // model must report B's workflow as dangling AND mark it cross-project (the higher-risk
        // "breaks something the user isn't looking at" case) with B's id/name + the dependency kind.
        let conn = test_conn();
        conn.execute(
            "INSERT INTO node(id, kind, path, name, present) VALUES(1, 'project', 'fixture://a', 'Project A', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO node(id, kind, path, name, present) VALUES(2, 'project', 'fixture://b', 'Project B', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO node(id, kind, path, name, volume_id, inode_key, link_count, size_apparent, size_allocated, present)
             VALUES(10, 'file', 'a/models/sd.safetensors', 'sd.safetensors', 'vol', 'model', 1, 4096, 4096, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO nav_item(id, project_id, node_id, path, display_name, item_kind)
             VALUES(110, 1, 10, 'a/models/sd.safetensors', 'sd.safetensors', 'file')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO node(id, kind, path, name, present) VALUES(20, 'file', 'b/workflows/wf.json', 'wf.json', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO nav_item(id, project_id, node_id, path, display_name, item_kind)
             VALUES(120, 2, 20, 'b/workflows/wf.json', 'wf.json', 'file')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO edge(id, src, dst, kind, confidence, evidence)
             VALUES(1, 20, 10, 'workflow_references_model', 'High', 'sd.safetensors')",
            [],
        )
        .unwrap();

        let mut target = std::collections::HashSet::new();
        target.insert(10);
        let (dangling, truncated) =
            dangling_after(&conn, 1, &target, None).expect("dangling_after must succeed");
        assert!(!truncated);
        let hit = dangling
            .iter()
            .find(|d| d.referrer_node_id == 20)
            .expect("the cross-project workflow must be reported as dangling");
        assert!(
            hit.cross_project,
            "a workflow in another project must be flagged cross-project: {hit:?}"
        );
        assert_eq!(hit.project_id, Some(2));
        assert_eq!(hit.project_name.as_deref(), Some("Project B"));
        assert_eq!(hit.dependency_kind, "workflow");
    }

    #[test]
    fn workflow_registered_in_multiple_projects_is_flagged_cross_project() {
        // The referrer workflow node is registered in BOTH the removed project (A=1) and another
        // project (B=2) — nav_item is unique on (project_id, path), not node_id. It must collapse
        // to ONE entry and be flagged cross-project: if ANY registration is outside the removed
        // project, removing the model breaks work the user isn't looking at. (Regression: the
        // naive per-row dedup could keep the removed project's row and falsely say "same project".)
        let conn = test_conn();
        for (id, name, uri) in [
            (1, "Project A", "fixture://a"),
            (2, "Project B", "fixture://b"),
        ] {
            conn.execute(
                "INSERT INTO node(id, kind, path, name, present) VALUES(?1, 'project', ?2, ?3, 1)",
                rusqlite::params![id, uri, name],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO node(id, kind, path, name, volume_id, inode_key, link_count, size_apparent, size_allocated, present)
             VALUES(10, 'file', 'a/models/sd.safetensors', 'sd.safetensors', 'vol', 'model', 1, 4096, 4096, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO nav_item(id, project_id, node_id, path, display_name, item_kind)
             VALUES(110, 1, 10, 'a/models/sd.safetensors', 'sd.safetensors', 'file')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO node(id, kind, path, name, present) VALUES(20, 'file', 'shared/wf.json', 'wf.json', 1)",
            [],
        )
        .unwrap();
        // The SAME workflow node registered under project A (removed) AND project B.
        conn.execute(
            "INSERT INTO nav_item(id, project_id, node_id, path, display_name, item_kind)
             VALUES(120, 1, 20, 'shared/wf.json', 'wf.json', 'file')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO nav_item(id, project_id, node_id, path, display_name, item_kind)
             VALUES(121, 2, 20, 'shared/wf.json', 'wf.json', 'file')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO edge(id, src, dst, kind, confidence, evidence)
             VALUES(1, 20, 10, 'workflow_references_model', 'High', 'sd.safetensors')",
            [],
        )
        .unwrap();

        let mut target = std::collections::HashSet::new();
        target.insert(10);
        let (dangling, _truncated) =
            dangling_after(&conn, 1, &target, None).expect("dangling_after must succeed");
        let hits: Vec<_> = dangling
            .iter()
            .filter(|d| d.referrer_node_id == 20)
            .collect();
        assert_eq!(
            hits.len(),
            1,
            "the multi-registered workflow must collapse to one entry: {dangling:?}"
        );
        assert!(
            hits[0].cross_project,
            "a workflow also registered in another project must be flagged cross-project: {:?}",
            hits[0]
        );
        assert_eq!(hits[0].project_id, Some(2));
        assert_eq!(hits[0].project_name.as_deref(), Some("Project B"));
    }

    #[test]
    fn plan_recognizes_dependency_extent_and_prepares_backup_delete() {
        // End-to-end: a project targeted for "Back up, then delete" whose model is
        // depended on by an EXTERNAL workflow and that holds a secret. The single
        // preview must (a) recognise the dependency extent, (b) prepare the backup+
        // delete action over recoverable bytes, and (c) surface the Protected-Zone
        // secret — all without executing anything.
        let conn = test_conn();
        conn.execute(
            "INSERT INTO node(id, kind, path, name, present) VALUES(1, 'project', 'fixture://proj', 'proj', 1)",
            [],
        )
        .unwrap();
        // Recoverable model owned by the target project.
        conn.execute(
            "INSERT INTO node(id, kind, path, name, volume_id, inode_key, link_count, size_apparent, size_allocated, present)
             VALUES(10, 'file', 'proj/models/sdxl.safetensors', 'sdxl.safetensors', 'vol', 'model', 1, 8192, 8192, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO nav_item(id, project_id, node_id, path, display_name, item_kind)
             VALUES(110, 1, 10, 'proj/models/sdxl.safetensors', 'sdxl.safetensors', 'file')",
            [],
        )
        .unwrap();
        // A plain recoverable build artifact.
        conn.execute(
            "INSERT INTO node(id, kind, path, name, volume_id, inode_key, link_count, size_apparent, size_allocated, present)
             VALUES(40, 'file', 'proj/build/output.bin', 'output.bin', 'vol', 'out', 1, 4096, 4096, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO nav_item(id, project_id, node_id, path, display_name, item_kind)
             VALUES(140, 1, 40, 'proj/build/output.bin', 'output.bin', 'file')",
            [],
        )
        .unwrap();
        // A secret in the target — must be caught before any delete.
        conn.execute(
            "INSERT INTO node(id, kind, path, name, volume_id, inode_key, link_count, size_apparent, size_allocated, present)
             VALUES(30, 'file', 'proj/.env', '.env', 'vol', 'env', 1, 200, 200, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO nav_item(id, project_id, node_id, path, display_name, item_kind, is_sensitive)
             VALUES(130, 1, 30, 'proj/.env', '.env', 'file', 1)",
            [],
        )
        .unwrap();
        // An EXTERNAL workflow (a different project) that depends on the model.
        conn.execute(
            "INSERT INTO node(id, kind, path, name, present) VALUES(20, 'file', 'other/wf.json', 'wf.json', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO nav_item(id, project_id, node_id, path, display_name, item_kind)
             VALUES(220, 2, 20, 'other/wf.json', 'wf.json', 'file')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO edge(id, src, dst, kind, confidence, evidence)
             VALUES(1, 20, 10, 'workflow_references_model', 'High', 'sdxl.safetensors')",
            [],
        )
        .unwrap();

        let plan = build_operation_plan(&conn, 1, "Back up, then delete").unwrap();

        // (Gate 1) Nothing runs.
        assert!(plan.read_only_preview);
        assert!(plan.external_services_unaffected);
        // The backup+delete action is prepared over recoverable bytes.
        assert_eq!(plan.action_label, "Back up, then delete");
        assert!(!plan.items.is_empty());
        assert!(plan
            .items
            .iter()
            .all(|item| item.action_label == "Back up, then delete"));
        assert!(
            plan.recoverable_bytes.total >= 4096,
            "recoverable bytes: {:?}",
            plan.recoverable_bytes
        );
        // The extent of dependencies is recognised: the external workflow would dangle.
        assert!(
            plan.dangling_after
                .iter()
                .any(|dangling| dangling.referrer_node_id == 20),
            "must recognise the dependent external workflow: {:?}",
            plan.dangling_after
        );
        // The Protected Zone catches the secret before any delete.
        assert!(
            plan.sensitive_files
                .iter()
                .any(|file| file.path.contains(".env")),
            "must flag the sensitive .env: {:?}",
            plan.sensitive_files
        );

        // The Risk Report projects the same safety signals.
        let report = build_risk_report(&plan);
        assert!(report.read_only_preview);
        assert!(report
            .dangling_after
            .iter()
            .any(|dangling| dangling.referrer_node_id == 20));
        assert!(report
            .sensitive_files
            .iter()
            .any(|file| file.path.contains(".env")));
        assert!(!report.recommended_action.is_empty());
        assert!(
            report
                .risk_counts
                .iter()
                .map(|count| count.count)
                .sum::<u64>()
                >= 1
        );
    }

    #[test]
    fn risk_classifier_assigns_expected_tiers() {
        // Strong/protected/sensitive always Black.
        assert_eq!(
            classify_path("project/.ssh/id_rsa", "file", false, None, false, false),
            RiskTier::Black
        );
        assert_eq!(
            classify_path("app/.env", "file", true, None, false, false),
            RiskTier::Black
        );
        assert_eq!(
            classify_path(
                "app/secret.json",
                "file",
                false,
                Some("no_preview"),
                false,
                false
            ),
            RiskTier::Black
        );
        // Shared or model-like assets are Red.
        assert_eq!(
            classify_path("models/sd.safetensors", "file", false, None, false, false),
            RiskTier::Red
        );
        assert_eq!(
            classify_path("notes/idea.md", "file", false, None, true, false),
            RiskTier::Red
        );
        // Project-specific data is Orange, rebuildable is Yellow, transient is Green.
        assert_eq!(
            classify_path("prompts/system.md", "file", false, None, false, false),
            RiskTier::Orange
        );
        assert_eq!(
            classify_path(
                "app/node_modules/x/index.js",
                "file",
                false,
                None,
                false,
                false
            ),
            RiskTier::Yellow
        );
        assert_eq!(
            classify_path("app/.cache/blob", "file", false, None, false, false),
            RiskTier::Green
        );
        // Fallbacks: unknown file -> Orange, unknown directory -> Yellow.
        assert_eq!(
            classify_path("app/readme.txt", "file", false, None, false, false),
            RiskTier::Orange
        );
        assert_eq!(
            classify_path("app/subdir", "directory", false, None, false, false),
            RiskTier::Yellow
        );
    }

    #[test]
    fn project_target_uses_one_compact_recursive_dir_item() {
        let conn = test_conn();
        conn.execute(
            "INSERT INTO node(id, kind, path, name, present) VALUES(1, 'project', 'fixture://p', 'p', 1)",
            [],
        )
        .unwrap();
        for (nav, node, path, inode) in [
            (100, 10, "a.bin", "a"),
            (101, 11, "b.bin", "b"),
            (102, 12, "c.bin", "c"),
        ] {
            conn.execute(
                "INSERT INTO node(id, kind, path, name, volume_id, inode_key, link_count, size_apparent, size_allocated, present)
                 VALUES(?1, 'file', ?2, ?2, 'vol', ?3, 1, 100, 100, 1)",
                params![node, path, inode],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO nav_item(id, project_id, node_id, path, display_name, item_kind)
                 VALUES(?1, 1, ?2, ?3, ?3, 'file')",
                params![nav, node, path],
            )
            .unwrap();
        }

        let plan = build_operation_plan(&conn, 1, "Future backup, move, or delete review").unwrap();
        // A whole project never enumerates per-file: one compact recursive_dir item.
        assert_eq!(plan.items.len(), 1);
        assert!(plan.items[0].recursive_dir);
        assert_eq!(plan.items[0].child_count, 3);
        assert_eq!(plan.recoverable_bytes.total, 300);
    }

    #[test]
    fn fingerprint_is_stable_and_changes_with_recoverable() {
        let zero = RecoverableBytes {
            owned: 0,
            orphaned_on_removal: 0,
            total: 0,
            partial: false,
        };
        let five = RecoverableBytes {
            owned: 5,
            orphaned_on_removal: 0,
            total: 5,
            partial: false,
        };
        assert_eq!(
            target_fingerprint(1, &[], &zero, false, None).unwrap(),
            target_fingerprint(1, &[], &zero, false, None).unwrap()
        );
        assert_ne!(
            target_fingerprint(1, &[], &zero, false, None).unwrap(),
            target_fingerprint(1, &[], &five, false, None).unwrap()
        );
    }

    #[test]
    fn operation_plan_honors_cancel_token() {
        let conn = test_conn();
        conn.execute(
            "INSERT INTO node(id, kind, path, name, present) VALUES(1, 'project', 'fixture://one', 'one', 1)",
            [],
        )
        .unwrap();
        let cancel = AtomicBool::new(true);

        let result = build_operation_plan_with_cancel(&conn, 1, "Future cleanup review", &cancel);

        assert!(matches!(result, Err(PlanError::Cancelled)));
    }
}
