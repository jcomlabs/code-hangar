//! Recoverable-space accounting that feeds the **preview-only** Operation Plan.
//!
//! Classifies assets reachable from a target as owned / shared / orphaned-on-
//! removal and computes `recoverable_bytes`, deduplicating hardlink groups by
//! `(volume_id, inode_key)` and **excluding** sensitive, protected, externally
//! shared, and reparse-point (symlink/junction) entries. Read-only: nothing is
//! moved or deleted — these numbers only describe what a future backup,
//! quarantine/move, or delete operation could recover.
use hangar_core::{RecoverableBytes, RecoverableSummary, SharedAsset, SharedAssetRef};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug)]
pub enum AccountingError {
    Sqlite(rusqlite::Error),
    Cancelled,
}

impl From<rusqlite::Error> for AccountingError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

pub type AccountingCalcResult<T> = Result<T, AccountingError>;

#[derive(Debug, Clone)]
pub struct AccountingCandidate {
    pub nav_id: i64,
    pub node_id: i64,
    pub project_id: i64,
    pub path: String,
    pub display_name: String,
    pub item_kind: String,
    pub size_apparent: u64,
    pub physical_bytes: u64,
    pub identity_key: String,
    pub link_count: Option<u64>,
    pub is_sensitive: bool,
    pub protected_level: Option<String>,
    pub is_reparse: bool,
    /// Scanner classification for a reparse/cloud-backed entry. Cloud Files are
    /// mutation blockers even when their bytes are fully materialized locally;
    /// `include_protected` must never turn them into unlink candidates.
    pub reparse_kind: Option<String>,
    pub partial: bool,
}

#[derive(Debug, Clone)]
pub struct AccountingResult {
    pub summary: RecoverableSummary,
    /// True only when every present project's Markdown and workflow evidence is
    /// membership-aware and ready. Empty relationship lists are not evidence of
    /// absence when this is false.
    pub relationship_evidence_complete: bool,
    pub target_project_id: i64,
    pub target_node_ids: HashSet<i64>,
    /// Exact navigation memberships covered by the target. A physical node may
    /// legitimately belong to more than one project, so relationship consumers
    /// must not reconstruct this set by joining on `node_id`.
    pub target_nav_ids: HashSet<i64>,
    pub candidates: Vec<AccountingCandidate>,
    pub recoverable_node_ids: HashSet<i64>,
    /// Locally owned nodes after shared-cache, cross-project reference and external-hardlink
    /// checks, but before sensitive/protected/reparse exclusions. Mutation may use this only
    /// after an explicit protected-content opt-in; preview byte totals remain conservative.
    pub mutation_owned_node_ids: HashSet<i64>,
    pub shared_assets: Vec<SharedAsset>,
}

#[derive(Debug, Clone)]
struct TargetScope {
    node_id: i64,
    project_id: i64,
    path: String,
    kind: String,
    nav_id: Option<i64>,
}

#[derive(Debug, Clone)]
struct ProjectRef {
    project_id: i64,
    project_name: String,
    source_count: u64,
}

#[derive(Debug, Clone)]
struct OwnershipFacts {
    refs: Vec<ProjectRef>,
    has_external_ref: bool,
    external_hardlink: bool,
    shared_cache: bool,
}

pub fn project_recoverable_summary(
    conn: &Connection,
    project_id: i64,
) -> rusqlite::Result<RecoverableSummary> {
    recoverable_for_target(conn, project_id).map(|result| result.summary)
}

pub fn node_recoverable_summary(
    conn: &Connection,
    node_id: i64,
) -> rusqlite::Result<RecoverableSummary> {
    recoverable_for_target(conn, node_id).map(|result| result.summary)
}

pub fn recoverable_for_target(
    conn: &Connection,
    target_node_id: i64,
) -> rusqlite::Result<AccountingResult> {
    recoverable_for_target_checked(conn, target_node_id, None).map_err(accounting_error_to_sqlite)
}

pub fn recoverable_for_target_cancellable(
    conn: &Connection,
    target_node_id: i64,
    cancel: &AtomicBool,
) -> AccountingCalcResult<AccountingResult> {
    recoverable_for_target_checked(conn, target_node_id, Some(cancel))
}

fn recoverable_for_target_checked(
    conn: &Connection,
    target_node_id: i64,
    cancel: Option<&AtomicBool>,
) -> AccountingCalcResult<AccountingResult> {
    check_cancelled(cancel)?;
    let scope = load_scope(conn, target_node_id)?;
    check_cancelled(cancel)?;
    let candidates = load_candidates(conn, &scope, cancel)?;
    let target_node_ids = candidates
        .iter()
        .map(|candidate| candidate.node_id)
        .collect::<HashSet<_>>();
    let target_nav_ids = candidates
        .iter()
        .map(|candidate| candidate.nav_id)
        .collect::<HashSet<_>>();
    check_cancelled(cancel)?;
    let relationships_complete = relationship_index_complete(conn)?;
    let identity_counts =
        candidates
            .iter()
            .fold(HashMap::<String, u64>::new(), |mut acc, candidate| {
                *acc.entry(candidate.identity_key.clone()).or_default() += 1;
                acc
            });
    let referrers_by_node = referrer_projects_by_node(conn, cancel)?;
    check_cancelled(cancel)?;
    let external_refs_by_identity =
        external_identity_refs_by_key(conn, &identity_counts, scope.project_id, cancel)?;

    let mut ownership_facts = Vec::with_capacity(candidates.len());
    for (index, candidate) in candidates.iter().enumerate() {
        if index % 1024 == 0 {
            check_cancelled(cancel)?;
        }
        let mut refs = referrers_by_node
            .get(&candidate.node_id)
            .cloned()
            .unwrap_or_default();
        if let Some(external_refs) = external_refs_by_identity.get(&candidate.identity_key) {
            merge_project_refs(&mut refs, external_refs);
        }
        ownership_facts.push(OwnershipFacts {
            has_external_ref: refs
                .iter()
                .any(|project| project.project_id != scope.project_id),
            external_hardlink: candidate
                .link_count
                .is_some_and(|links| links > identity_counts[&candidate.identity_key]),
            shared_cache: is_within_shared_cache(&candidate.path),
            refs,
        });
    }
    let mutation_blocked_node_ids = candidates
        .iter()
        .zip(&ownership_facts)
        .filter_map(|(candidate, facts)| {
            (!relationships_complete
                || facts.shared_cache
                || facts.has_external_ref
                || facts.external_hardlink
                || is_cloud_file(candidate))
            .then_some(candidate.node_id)
        })
        .collect::<HashSet<_>>();
    let recoverable_blocked_node_ids = candidates
        .iter()
        .filter_map(|candidate| {
            (candidate.is_sensitive
                || candidate.protected_level.is_some()
                || candidate.is_reparse
                || is_cloud_file(candidate)
                || mutation_blocked_node_ids.contains(&candidate.node_id))
            .then_some(candidate.node_id)
        })
        .collect::<HashSet<_>>();

    let mut seen_owned = HashSet::new();
    let mut seen_orphaned = HashSet::new();
    let mut recoverable_node_ids = HashSet::new();
    let mutation_owned_node_ids = candidates
        .iter()
        .map(|candidate| candidate.node_id)
        .filter(|node_id| !mutation_blocked_node_ids.contains(node_id))
        .collect::<HashSet<_>>();
    let mut owned = 0_u64;
    let mut orphaned_on_removal = 0_u64;
    let mut shared_count = 0_u64;
    let mut protected_count = 0_u64;
    let mut sensitive_count = 0_u64;
    let mut partial_footprint = false;
    let mut shared_assets = Vec::new();

    for (index, (candidate, facts)) in candidates.iter().zip(&ownership_facts).enumerate() {
        if index % 1024 == 0 {
            check_cancelled(cancel)?;
        }
        partial_footprint |= candidate.partial;
        // Machine-wide shared caches (HuggingFace/Ollama/pip/npm/torch/uv/...) belong to
        // the tool, not this project. Their bytes must never be counted as recoverable
        // or proposed for deletion (Gate 2) just because a project/scan-root happens to
        // contain the cache subtree. Mirrors hangar-graph::cache_category_is_shared_by_default.
        // Preserve conservative preview accounting: protected bytes and links stay out of
        // recoverable totals. The separate owned set above lets an explicitly confirmed
        // mutation include them without bypassing ownership/shared-cache checks.
        if candidate.is_sensitive {
            sensitive_count += 1;
            continue;
        }
        if candidate.protected_level.is_some() || candidate.is_reparse || is_cloud_file(candidate) {
            protected_count += 1;
            continue;
        }
        if facts.shared_cache {
            shared_count += 1;
            continue;
        }

        // Absence of an edge is ownership evidence only after every live project
        // has completed both membership-aware relationship families. Pending,
        // failed, missing, or legacy provenance is unknown and therefore cannot
        // authorize a future mutation or inflate recoverable bytes.
        if !relationships_complete {
            partial_footprint = true;
            continue;
        }

        if recoverable_blocked_node_ids.contains(&candidate.node_id)
            && !mutation_blocked_node_ids.contains(&candidate.node_id)
        {
            // Another exact membership of this same physical node is protected.
            // Keep it eligible only for the explicit protected mutation path.
            continue;
        }

        if mutation_blocked_node_ids.contains(&candidate.node_id) {
            shared_count += 1;
            if facts.has_external_ref || facts.external_hardlink {
                shared_assets.push(shared_asset_for_candidate(candidate, facts.refs.clone()));
            }
            continue;
        }

        if seen_owned.insert(candidate.identity_key.clone()) {
            owned += candidate.physical_bytes;
        }
        recoverable_node_ids.insert(candidate.node_id);
    }

    let orphaned = if relationships_complete {
        orphaned_assets_on_removal(conn, &scope, &target_nav_ids, &target_node_ids, cancel)?
    } else {
        Vec::new()
    };
    for (index, orphan) in orphaned.into_iter().enumerate() {
        if index % 1024 == 0 {
            check_cancelled(cancel)?;
        }
        if orphan.is_sensitive {
            sensitive_count += 1;
            continue;
        }
        if orphan.protected_level.is_some() || orphan.is_reparse || is_cloud_file(&orphan) {
            protected_count += 1;
            continue;
        }
        if is_within_shared_cache(&orphan.path) {
            shared_count += 1;
            continue;
        }
        // Mirror the owned loop's external-hardlink guard: an orphan that is ALSO hardlinked
        // outside the in-scope reference set keeps its inode alive even after the target is
        // removed, so its bytes are not actually reclaimable — count it as shared, not freed.
        let external_hardlink = orphan.link_count.is_some_and(|links| {
            links
                > identity_counts
                    .get(&orphan.identity_key)
                    .copied()
                    .unwrap_or(1)
        });
        if external_hardlink {
            shared_count += 1;
            continue;
        }
        if seen_owned.contains(&orphan.identity_key) || !seen_orphaned.insert(orphan.identity_key) {
            continue;
        }
        orphaned_on_removal += orphan.physical_bytes;
    }

    Ok(AccountingResult {
        summary: RecoverableSummary {
            target_node_id: scope.node_id,
            project_id: scope.project_id,
            target_path: scope.path,
            target_kind: scope.kind,
            recoverable_bytes: RecoverableBytes {
                owned,
                orphaned_on_removal,
                total: owned + orphaned_on_removal,
                partial: partial_footprint,
            },
            shared_count,
            protected_count,
            sensitive_count,
            partial_footprint,
        },
        relationship_evidence_complete: relationships_complete,
        target_project_id: scope.project_id,
        target_node_ids,
        target_nav_ids,
        candidates,
        recoverable_node_ids,
        mutation_owned_node_ids,
        shared_assets,
    })
}

fn check_cancelled(cancel: Option<&AtomicBool>) -> AccountingCalcResult<()> {
    if cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
        Err(AccountingError::Cancelled)
    } else {
        Ok(())
    }
}

fn accounting_error_to_sqlite(error: AccountingError) -> rusqlite::Error {
    match error {
        AccountingError::Sqlite(error) => error,
        AccountingError::Cancelled => rusqlite::Error::InvalidQuery,
    }
}

fn is_cloud_file(candidate: &AccountingCandidate) -> bool {
    matches!(
        candidate.reparse_kind.as_deref(),
        Some("cloud_local" | "cloud_placeholder")
    )
}

fn load_scope(conn: &Connection, target_node_id: i64) -> rusqlite::Result<TargetScope> {
    let node = conn
        .query_row(
            "SELECT id, kind, COALESCE(path, name, ''), COALESCE(name, path, '')
             FROM node
             WHERE id = ?1",
            params![target_node_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?
        .ok_or(rusqlite::Error::QueryReturnedNoRows)?;

    if node.1 == "project" {
        return Ok(TargetScope {
            node_id: node.0,
            project_id: node.0,
            path: node.2,
            kind: node.1,
            nav_id: None,
        });
    }

    conn.query_row(
        "SELECT id, project_id, path, item_kind
         FROM nav_item
         WHERE node_id = ?1
         ORDER BY id
         LIMIT 1",
        params![target_node_id],
        |row| {
            Ok(TargetScope {
                node_id: target_node_id,
                project_id: row.get(1)?,
                path: row.get(2)?,
                kind: row.get(3)?,
                nav_id: Some(row.get(0)?),
            })
        },
    )
}

/// Whether a candidate path falls inside a machine-wide, tool-owned shared cache.
/// Works on the file/dir path itself (unlike `hangar_graph::cache_category`, which
/// classifies the cache *directory* node) so that the blobs *inside* the cache are
/// also excluded from a single project's recoverable bytes (Gate 2). The marker set
/// mirrors `hangar_graph::cache_category_is_shared_by_default`.
fn is_within_shared_cache(path: &str) -> bool {
    let normalized = format!(
        "/{}",
        path.replace('\\', "/")
            .trim_start_matches('/')
            .to_ascii_lowercase()
    );
    [
        // `/huggingface/` (not just `/huggingface/hub`) so a cache relocated via
        // HF_HOME/HF_HUB_CACHE — whose datasets/, assets/, modules/ sit beside hub/ —
        // is also excluded, matching hangar_graph::cache_category's `/huggingface` rule.
        "/huggingface/",
        "/.cache/transformers",
        "/transformers/",
        "/ollama/models/",
        "/.ollama/models/",
        "/.cache/torch",
        "/torch/hub",
        "/torch_extensions/",
        "/.cache/pip",
        "/pip/cache/",
        "/.cache/uv",
        "/uv/cache/",
        "/npm-cache/",
        "/.npm/",
        // Non-`.cache` machine-wide content stores (the `/.cache/` catch-all below already covers
        // tool caches that live under ~/.cache, e.g. yarn/poetry/deno/bun/go-build).
        "/.cargo/registry/",
        "/.cargo/git/",
        "/go/pkg/mod/",
        "/.pnpm-store/",
        "/pnpm/store/",
    ]
    .iter()
    .any(|marker| normalized.contains(marker))
        // The scanner collapses a `.cache` directory into a SINGLE node (size measured, children
        // not indexed), so the per-tool markers above (e.g. `/.cache/pip`) no longer match the
        // node path `…/.cache`. A `.cache` directory is regenerable and routinely holds
        // machine-wide shared caches (HF, pip, torch, uv, …); conservatively treat the whole thing
        // as shared so it is never attributed as one project's recoverable bytes (Gate-2).
        || normalized.ends_with("/.cache")
        || normalized.contains("/.cache/")
}

fn load_candidates(
    conn: &Connection,
    scope: &TargetScope,
    cancel: Option<&AtomicBool>,
) -> AccountingCalcResult<Vec<AccountingCandidate>> {
    check_cancelled(cancel)?;
    match scope.nav_id {
        Some(nav_id) => {
            let mut stmt = conn.prepare(
                "WITH RECURSIVE subtree(id) AS (
                   SELECT id FROM nav_item WHERE id = ?1
                   UNION ALL
                   SELECT child.id
                   FROM nav_item child
                   JOIN subtree parent ON child.parent_nav_id = parent.id
                 )
                 SELECT ni.id, ni.node_id, ni.project_id, ni.path, ni.display_name, ni.item_kind,
                        ni.is_sensitive, ni.protected_level, ni.fully_scanned, ni.scan_error,
                        ni.aggregate_bytes_partial, COALESCE(n.size_apparent, 0),
                        n.size_allocated, n.volume_id, n.inode_key, n.link_count,
                        n.is_reparse, n.reparse_kind
                 FROM nav_item ni
                 JOIN subtree st ON st.id = ni.id
                 JOIN node n ON n.id = ni.node_id
                 WHERE ni.node_id IS NOT NULL
                   AND n.present = 1",
            )?;
            let rows = stmt.query_map(params![nav_id], candidate_from_row)?;
            collect_candidates(rows, cancel)
        }
        None => {
            let mut stmt = conn.prepare(
                "SELECT ni.id, ni.node_id, ni.project_id, ni.path, ni.display_name, ni.item_kind,
                        ni.is_sensitive, ni.protected_level, ni.fully_scanned, ni.scan_error,
                        ni.aggregate_bytes_partial, COALESCE(n.size_apparent, 0),
                        n.size_allocated, n.volume_id, n.inode_key, n.link_count,
                        n.is_reparse, n.reparse_kind
                 FROM nav_item ni
                 JOIN node n ON n.id = ni.node_id
                 WHERE ni.project_id = ?1
                   AND ni.node_id IS NOT NULL
                   AND n.present = 1",
            )?;
            let rows = stmt.query_map(params![scope.project_id], candidate_from_row)?;
            collect_candidates(rows, cancel)
        }
    }
}

fn candidate_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AccountingCandidate> {
    let nav_id = row.get::<_, i64>(0)?;
    let node_id = row.get::<_, i64>(1)?;
    let size_apparent = row.get::<_, i64>(11)?.max(0) as u64;
    let size_allocated = row
        .get::<_, Option<i64>>(12)?
        .map(|value| value.max(0) as u64);
    let volume_id = row.get::<_, Option<String>>(13)?;
    let inode_key = row.get::<_, Option<String>>(14)?;
    Ok(AccountingCandidate {
        nav_id,
        node_id,
        project_id: row.get(2)?,
        path: row.get(3)?,
        display_name: row.get(4)?,
        item_kind: row.get(5)?,
        is_sensitive: row.get::<_, i64>(6)? == 1,
        protected_level: row.get(7)?,
        partial: row.get::<_, i64>(8)? == 0
            || row.get::<_, Option<String>>(9)?.is_some()
            || row.get::<_, i64>(10)? == 1,
        size_apparent,
        physical_bytes: size_allocated.unwrap_or(size_apparent),
        identity_key: physical_identity_key(
            nav_id,
            node_id,
            volume_id.as_deref(),
            inode_key.as_deref(),
        ),
        link_count: row
            .get::<_, Option<i64>>(15)?
            .map(|value| value.max(0) as u64),
        is_reparse: row.get::<_, i64>(16)? == 1,
        reparse_kind: row.get(17)?,
    })
}

fn collect_candidates<F>(
    rows: rusqlite::MappedRows<'_, F>,
    cancel: Option<&AtomicBool>,
) -> AccountingCalcResult<Vec<AccountingCandidate>>
where
    F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<AccountingCandidate>,
{
    let mut values = Vec::new();
    for (index, row) in rows.enumerate() {
        if index % 1024 == 0 {
            check_cancelled(cancel)?;
        }
        values.push(row?);
    }
    Ok(values)
}

fn physical_identity_key(
    nav_id: i64,
    node_id: i64,
    volume_id: Option<&str>,
    inode_key: Option<&str>,
) -> String {
    match (volume_id, inode_key) {
        (Some(volume_id), Some(inode_key)) if !volume_id.is_empty() && !inode_key.is_empty() => {
            format!("inode:{volume_id}:{inode_key}")
        }
        _ => format!("node:{node_id}:nav:{nav_id}"),
    }
}

fn relationship_index_complete(conn: &Connection) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT NOT EXISTS (
           SELECT 1
           FROM node project
           WHERE project.kind = 'project'
             AND project.present = 1
             AND 2 <> (
               SELECT COUNT(DISTINCT state.family)
               FROM relationship_index_state state
               WHERE state.project_id = project.id
                 AND state.schema_version = 2
                 AND state.state = 'ready'
                 AND state.family IN ('markdown', 'workflow')
             )
         )",
        [],
        |row| row.get::<_, i64>(0).map(|ready| ready == 1),
    )
}

fn referrer_projects(
    conn: &Connection,
    node_id: i64,
    cancel: Option<&AtomicBool>,
) -> AccountingCalcResult<Vec<ProjectRef>> {
    check_cancelled(cancel)?;
    let mut stmt = conn.prepare(
        "SELECT src_nav.project_id, COALESCE(project.name, src_nav.project_id),
                COUNT(DISTINCT e.source_nav_id)
         FROM edge e
         JOIN nav_item src_nav ON src_nav.id = e.source_nav_id
         JOIN node project ON project.id = src_nav.project_id
         WHERE e.target_node_id = ?1
           AND e.kind IN ('markdown_links_to', 'workflow_references_model', 'depends_on', 'referenced_by', 'stored_in', 'configured_in', 'generated_by')
         GROUP BY src_nav.project_id, project.name",
    )?;
    let rows = stmt.query_map(params![node_id], |row| {
        Ok(ProjectRef {
            project_id: row.get(0)?,
            project_name: row.get(1)?,
            source_count: row.get::<_, i64>(2)?.max(0) as u64,
        })
    })?;
    let mut refs = Vec::new();
    for (index, row) in rows.enumerate() {
        if index % 1024 == 0 {
            check_cancelled(cancel)?;
        }
        refs.push(row?);
    }
    Ok(refs)
}

fn referrer_projects_by_node(
    conn: &Connection,
    cancel: Option<&AtomicBool>,
) -> AccountingCalcResult<HashMap<i64, Vec<ProjectRef>>> {
    check_cancelled(cancel)?;
    let mut stmt = conn.prepare(
        "SELECT e.target_node_id, src_nav.project_id,
                COALESCE(project.name, src_nav.project_id),
                COUNT(DISTINCT e.source_nav_id)
         FROM edge e
         JOIN nav_item src_nav ON src_nav.id = e.source_nav_id
         JOIN node project ON project.id = src_nav.project_id
         WHERE e.kind IN ('markdown_links_to', 'workflow_references_model', 'depends_on', 'referenced_by', 'stored_in', 'configured_in', 'generated_by')
         GROUP BY e.target_node_id, src_nav.project_id, project.name",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            ProjectRef {
                project_id: row.get(1)?,
                project_name: row.get(2)?,
                source_count: row.get::<_, i64>(3)?.max(0) as u64,
            },
        ))
    })?;
    let mut by_node = HashMap::<i64, Vec<ProjectRef>>::new();
    for (index, row) in rows.enumerate() {
        if index % 1024 == 0 {
            check_cancelled(cancel)?;
        }
        let (node_id, project_ref) = row?;
        by_node.entry(node_id).or_default().push(project_ref);
    }
    Ok(by_node)
}

fn external_identity_refs_by_key(
    conn: &Connection,
    identity_counts: &HashMap<String, u64>,
    target_project_id: i64,
    cancel: Option<&AtomicBool>,
) -> AccountingCalcResult<HashMap<String, Vec<ProjectRef>>> {
    check_cancelled(cancel)?;
    if identity_counts.keys().all(|key| !key.starts_with("inode:")) {
        return Ok(HashMap::new());
    }
    let mut stmt = conn.prepare(
        "SELECT DISTINCT other_node.volume_id, other_node.inode_key, other.project_id, COALESCE(project.name, other.project_id)
         FROM nav_item other
         JOIN node other_node ON other_node.id = other.node_id
         JOIN node project ON project.id = other.project_id
         WHERE other_node.volume_id IS NOT NULL
           AND other_node.inode_key IS NOT NULL
           AND other.project_id <> ?1",
    )?;
    let rows = stmt.query_map(params![target_project_id], |row| {
        let volume_id = row.get::<_, String>(0)?;
        let inode_key = row.get::<_, String>(1)?;
        Ok((
            format!("inode:{volume_id}:{inode_key}"),
            ProjectRef {
                project_id: row.get(2)?,
                project_name: row.get(3)?,
                source_count: 1,
            },
        ))
    })?;
    let mut refs_by_key = HashMap::<String, Vec<ProjectRef>>::new();
    for (index, row) in rows.enumerate() {
        if index % 1024 == 0 {
            check_cancelled(cancel)?;
        }
        let (identity_key, project_ref) = row?;
        if identity_counts.contains_key(&identity_key) {
            let refs = refs_by_key.entry(identity_key).or_default();
            if refs
                .iter()
                .all(|existing| existing.project_id != project_ref.project_id)
            {
                refs.push(project_ref);
            }
        }
    }
    Ok(refs_by_key)
}

fn merge_project_refs(refs: &mut Vec<ProjectRef>, additional: &[ProjectRef]) {
    for project_ref in additional {
        if refs
            .iter()
            .all(|existing| existing.project_id != project_ref.project_id)
        {
            refs.push(project_ref.clone());
        }
    }
}

fn shared_asset_for_candidate(
    candidate: &AccountingCandidate,
    refs: Vec<ProjectRef>,
) -> SharedAsset {
    let confidence = if refs.iter().any(|project| project.source_count > 1) {
        "High"
    } else {
        "Medium"
    };
    SharedAsset {
        node_id: candidate.node_id,
        path: candidate.path.clone(),
        display_name: candidate.display_name.clone(),
        physical_bytes: Some(candidate.physical_bytes),
        referenced_by: refs
            .into_iter()
            .map(|project| SharedAssetRef {
                project_id: project.project_id,
                project_name: project.project_name,
            })
            .collect(),
        confidence: confidence.to_string(),
    }
}

fn orphaned_assets_on_removal(
    conn: &Connection,
    scope: &TargetScope,
    target_nav_ids: &HashSet<i64>,
    target_node_ids: &HashSet<i64>,
    cancel: Option<&AtomicBool>,
) -> AccountingCalcResult<Vec<AccountingCandidate>> {
    check_cancelled(cancel)?;
    if target_nav_ids.is_empty() || target_node_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(
        "SELECT DISTINCT e.source_nav_id, dst_nav.id, dst_nav.node_id, dst_nav.project_id, dst_nav.path,
                dst_nav.display_name, dst_nav.item_kind, dst_nav.is_sensitive,
                dst_nav.protected_level, dst_nav.fully_scanned, dst_nav.scan_error,
                dst_nav.aggregate_bytes_partial, COALESCE(dst_node.size_apparent, 0),
                dst_node.size_allocated, dst_node.volume_id, dst_node.inode_key,
                dst_node.link_count, dst_node.is_reparse, dst_node.reparse_kind
         FROM edge e
         JOIN nav_item src_nav ON src_nav.id = e.source_nav_id
         JOIN node dst_node ON dst_node.id = e.target_node_id
         JOIN nav_item dst_nav
           ON dst_nav.id = COALESCE(
                e.matched_target_nav_id,
                (
                  SELECT candidate.id
                  FROM nav_item candidate
                  WHERE candidate.node_id = e.target_node_id
                  ORDER BY CASE WHEN candidate.project_id = ?1 THEN 0 ELSE 1 END,
                           candidate.id
                  LIMIT 1
                )
              )
          AND dst_nav.node_id = e.target_node_id
         WHERE src_nav.project_id = ?1
           AND dst_nav.node_id IS NOT NULL
           AND dst_node.present = 1
           AND dst_nav.project_id <> ?1
           AND e.kind IN ('markdown_links_to', 'workflow_references_model', 'depends_on', 'referenced_by', 'stored_in', 'configured_in', 'generated_by')",
    )?;
    let rows = stmt.query_map(params![scope.project_id], |row| {
        let nav_id = row.get::<_, i64>(1)?;
        let node_id = row.get::<_, i64>(2)?;
        let size_apparent = row.get::<_, i64>(12)?.max(0) as u64;
        let size_allocated = row
            .get::<_, Option<i64>>(13)?
            .map(|value| value.max(0) as u64);
        let volume_id = row.get::<_, Option<String>>(14)?;
        let inode_key = row.get::<_, Option<String>>(15)?;
        Ok((
            row.get::<_, i64>(0)?,
            AccountingCandidate {
                nav_id,
                node_id,
                project_id: row.get(3)?,
                path: row.get(4)?,
                display_name: row.get(5)?,
                item_kind: row.get(6)?,
                is_sensitive: row.get::<_, i64>(7)? == 1,
                protected_level: row.get(8)?,
                partial: row.get::<_, i64>(9)? == 0
                    || row.get::<_, Option<String>>(10)?.is_some()
                    || row.get::<_, i64>(11)? == 1,
                size_apparent,
                physical_bytes: size_allocated.unwrap_or(size_apparent),
                identity_key: physical_identity_key(
                    nav_id,
                    node_id,
                    volume_id.as_deref(),
                    inode_key.as_deref(),
                ),
                link_count: row
                    .get::<_, Option<i64>>(16)?
                    .map(|value| value.max(0) as u64),
                is_reparse: row.get::<_, i64>(17)? == 1,
                reparse_kind: row.get(18)?,
            },
        ))
    })?;
    let mut values = Vec::new();
    for (index, row) in rows.enumerate() {
        if index % 512 == 0 {
            check_cancelled(cancel)?;
        }
        let (source_nav_id, candidate) = row?;
        if !target_nav_ids.contains(&source_nav_id) {
            continue;
        }
        let refs = referrer_projects(conn, candidate.node_id, cancel)?;
        if refs
            .iter()
            .all(|project| project.project_id == scope.project_id)
            && !target_node_ids.contains(&candidate.node_id)
        {
            values.push(candidate);
        }
    }
    Ok(values)
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
              source_nav_id INTEGER NOT NULL,
              target_node_id INTEGER NOT NULL,
              matched_target_nav_id INTEGER,
              kind TEXT NOT NULL,
              confidence TEXT NOT NULL,
              evidence TEXT,
              UNIQUE(source_nav_id, target_node_id, kind)
            );
            CREATE TABLE relationship_index_state (
              project_id INTEGER NOT NULL,
              family TEXT NOT NULL,
              schema_version INTEGER NOT NULL,
              state TEXT NOT NULL,
              built_at TEXT,
              error TEXT,
              PRIMARY KEY(project_id, family)
            );
            CREATE TRIGGER test_project_relationship_state
            AFTER INSERT ON node
            WHEN NEW.kind = 'project'
            BEGIN
              INSERT INTO relationship_index_state(project_id, family, schema_version, state)
              VALUES(NEW.id, 'markdown', 2, 'ready');
              INSERT INTO relationship_index_state(project_id, family, schema_version, state)
              VALUES(NEW.id, 'workflow', 2, 'ready');
            END;
            ",
        )
        .unwrap();
        conn
    }

    fn insert_project(conn: &Connection, id: i64, name: &str) {
        conn.execute(
            "INSERT INTO node(id, kind, path, name, present) VALUES(?1, 'project', ?2, ?2, 1)",
            params![id, name],
        )
        .unwrap();
    }

    fn insert_file(
        conn: &Connection,
        nav_id: i64,
        node_id: i64,
        project_id: i64,
        path: &str,
        size: i64,
        inode: &str,
    ) {
        conn.execute(
            "INSERT INTO node(id, kind, path, name, volume_id, inode_key, link_count, size_apparent, size_allocated, present)
             VALUES(?1, 'file', ?2, ?2, 'vol', ?3, 1, ?4, ?4, 1)",
            params![node_id, path, inode, size],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO nav_item(id, project_id, node_id, path, display_name, item_kind)
             VALUES(?1, ?2, ?3, ?4, ?4, 'file')",
            params![nav_id, project_id, node_id, path],
        )
        .unwrap();
    }

    #[test]
    fn hardlinks_count_once() {
        let conn = test_conn();
        insert_project(&conn, 1, "one");
        insert_file(&conn, 10, 100, 1, "a.bin", 50, "same");
        insert_file(&conn, 11, 101, 1, "b.bin", 50, "same");

        let summary = project_recoverable_summary(&conn, 1).unwrap();
        assert_eq!(summary.recoverable_bytes.owned, 50);
    }

    #[test]
    fn external_hardlink_is_not_recoverable() {
        let conn = test_conn();
        insert_project(&conn, 1, "one");
        insert_file(&conn, 10, 100, 1, "a.bin", 50, "same");
        conn.execute("UPDATE node SET link_count = 2 WHERE id = 100", [])
            .unwrap();

        let summary = project_recoverable_summary(&conn, 1).unwrap();
        assert_eq!(summary.recoverable_bytes.total, 0);
        assert_eq!(summary.shared_count, 1);
    }

    #[test]
    fn protected_and_sensitive_are_excluded() {
        let conn = test_conn();
        insert_project(&conn, 1, "one");
        insert_file(&conn, 10, 100, 1, ".env", 50, "secret");
        conn.execute(
            "UPDATE nav_item SET is_sensitive = 1, protected_level = 'no_preview' WHERE node_id = 100",
            [],
        )
        .unwrap();

        let result = recoverable_for_target(&conn, 1).unwrap();
        assert_eq!(result.summary.recoverable_bytes.total, 0);
        assert_eq!(result.summary.sensitive_count, 1);
        assert!(!result.recoverable_node_ids.contains(&100));
        assert!(result.mutation_owned_node_ids.contains(&100));
    }

    #[test]
    fn externally_shared_sensitive_file_is_not_mutation_owned() {
        let conn = test_conn();
        insert_project(&conn, 1, "one");
        insert_project(&conn, 2, "two");
        insert_file(&conn, 10, 100, 1, ".env", 50, "secret");
        insert_file(&conn, 20, 200, 2, "ref.md", 10, "ref");
        conn.execute(
            "UPDATE nav_item SET is_sensitive = 1, protected_level = 'no_preview' WHERE node_id = 100",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO edge(source_nav_id, target_node_id, matched_target_nav_id, kind, confidence)
             VALUES(20, 100, 10, 'markdown_links_to', 'High')",
            [],
        )
        .unwrap();

        let result = recoverable_for_target(&conn, 1).unwrap();
        assert_eq!(result.summary.sensitive_count, 1);
        assert!(!result.recoverable_node_ids.contains(&100));
        assert!(!result.mutation_owned_node_ids.contains(&100));
    }

    #[test]
    fn shared_machine_wide_cache_is_not_recoverable() {
        let conn = test_conn();
        insert_project(&conn, 1, "one");
        insert_file(&conn, 10, 100, 1, "build/output.bin", 4096, "out");
        // A HuggingFace cache blob that happens to live under the project/scan-root.
        insert_file(
            &conn,
            11,
            101,
            1,
            ".cache/huggingface/hub/models--org--m/blobs/abc",
            9_000_000,
            "hfblob",
        );

        let summary = project_recoverable_summary(&conn, 1).unwrap();
        // Only the build artifact is recoverable; the shared cache blob is excluded so
        // it is neither counted as recoverable nor proposed for deletion (Gate 2).
        assert_eq!(summary.recoverable_bytes.owned, 4096);
        assert!(
            summary.shared_count >= 1,
            "the cache blob must be counted as shared, not owned"
        );
    }

    #[test]
    fn shared_cache_marker_covers_relocated_and_default_caches() {
        // Default + HF_HOME-relocated HuggingFace (datasets/ sibling of hub/), plus the
        // other machine-wide tool caches — all excluded from a project's recoverable bytes.
        assert!(is_within_shared_cache(
            ".cache/huggingface/hub/models--o--m/blobs/abc"
        ));
        assert!(is_within_shared_cache(
            "D:/models/huggingface/datasets/foo/data.bin"
        ));
        assert!(is_within_shared_cache(
            "home/.ollama/models/blobs/sha256-abc"
        ));
        assert!(is_within_shared_cache("x/.cache/pip/wheels/ab/cd/whl"));
        assert!(is_within_shared_cache(
            "x/.cache/torch/hub/checkpoints/m.pth"
        ));
        // Non-.cache machine-wide content stores.
        assert!(is_within_shared_cache(
            "C:/Users/me/.cargo/registry/cache/crate-1.0.crate"
        ));
        assert!(is_within_shared_cache(
            "home/go/pkg/mod/github.com/x/y@v1/f.go"
        ));
        assert!(is_within_shared_cache(
            "C:/Users/me/.local/share/pnpm/store/v3/files/00/abc"
        ));
        // A normal project tree is not a machine-wide shared cache.
        assert!(!is_within_shared_cache("my-project/src/main.rs"));
        assert!(!is_within_shared_cache("my-project/dist/bundle.js"));
    }

    #[test]
    fn scoped_node_summary_does_not_use_whole_project_links() {
        let conn = test_conn();
        insert_project(&conn, 1, "one");
        insert_project(&conn, 2, "two");
        insert_file(&conn, 10, 100, 1, "target.md", 50, "target");
        insert_file(&conn, 11, 101, 1, "other.md", 50, "other");
        insert_file(&conn, 20, 200, 2, "asset.bin", 90, "asset");
        conn.execute(
            "INSERT INTO edge(source_nav_id, target_node_id, matched_target_nav_id, kind, confidence)
             VALUES(11, 200, 20, 'markdown_links_to', 'High')",
            [],
        )
        .unwrap();

        let summary = node_recoverable_summary(&conn, 100).unwrap();
        assert_eq!(summary.recoverable_bytes.orphaned_on_removal, 0);
        assert_eq!(summary.recoverable_bytes.total, 50);
    }

    #[test]
    fn asset_referenced_by_another_project_is_shared_not_recoverable() {
        let conn = test_conn();
        insert_project(&conn, 1, "one");
        insert_project(&conn, 2, "two");
        insert_file(&conn, 10, 100, 1, "owned.bin", 40, "owned");
        insert_file(&conn, 11, 101, 1, "shared.bin", 60, "shared");
        insert_file(&conn, 20, 200, 2, "ref.md", 10, "ref");
        // A file in project two references project one's shared.bin.
        conn.execute(
            "INSERT INTO edge(source_nav_id, target_node_id, matched_target_nav_id, kind, confidence)
             VALUES(20, 101, 11, 'markdown_links_to', 'High')",
            [],
        )
        .unwrap();

        let summary = project_recoverable_summary(&conn, 1).unwrap();
        assert_eq!(summary.recoverable_bytes.owned, 40); // only owned.bin
        assert_eq!(summary.recoverable_bytes.total, 40);
        assert_eq!(summary.shared_count, 1);
        // Invariant: recoverable never exceeds owned + orphaned-on-removal.
        assert!(
            summary.recoverable_bytes.total
                <= summary.recoverable_bytes.owned + summary.recoverable_bytes.orphaned_on_removal
        );
    }

    #[test]
    fn shared_source_node_is_attributed_only_to_the_exact_source_membership() {
        let conn = test_conn();
        insert_project(&conn, 1, "Project A");
        insert_project(&conn, 2, "Project B");
        insert_file(&conn, 10, 100, 1, "a/asset.bin", 60, "asset");
        insert_file(&conn, 21, 200, 2, "shared/ref.md", 10, "ref");
        // The same physical source is also visible in A, but the indexed edge was
        // produced specifically by B's membership (nav 21).
        conn.execute(
            "INSERT INTO nav_item(id, project_id, node_id, path, display_name, item_kind)
             VALUES(20, 1, 200, 'shared/ref.md', 'ref.md', 'file')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO edge(source_nav_id, target_node_id, matched_target_nav_id, kind, confidence)
             VALUES(21, 100, 10, 'markdown_links_to', 'High')",
            [],
        )
        .unwrap();

        let result = recoverable_for_target(&conn, 1).unwrap();
        let shared = result
            .shared_assets
            .iter()
            .find(|asset| asset.node_id == 100)
            .expect("B's exact membership must keep A's asset shared");
        let projects = shared
            .referenced_by
            .iter()
            .map(|reference| reference.project_id)
            .collect::<Vec<_>>();
        assert_eq!(projects, vec![2]);
        assert!(!result.mutation_owned_node_ids.contains(&100));
        assert!(!result.recoverable_node_ids.contains(&100));
    }

    #[test]
    fn missing_relationship_freshness_fails_closed_for_ownership() {
        let conn = test_conn();
        insert_project(&conn, 1, "one");
        insert_file(&conn, 10, 100, 1, "owned.bin", 50, "owned");
        conn.execute(
            "DELETE FROM relationship_index_state
             WHERE project_id = 1 AND family = 'workflow'",
            [],
        )
        .unwrap();

        let result = recoverable_for_target(&conn, 1).unwrap();
        assert!(result.mutation_owned_node_ids.is_empty());
        assert!(result.recoverable_node_ids.is_empty());
        assert_eq!(result.summary.recoverable_bytes.total, 0);
        assert!(result.summary.partial_footprint);
    }

    #[test]
    fn mutation_ownership_is_order_independent_and_any_unsafe_membership_wins() {
        for (normal_nav_id, cache_nav_id) in [(10, 11), (11, 10)] {
            let conn = test_conn();
            insert_project(&conn, 1, "one");
            insert_file(
                &conn,
                normal_nav_id,
                100,
                1,
                "project/output.bin",
                50,
                "shared-node",
            );
            conn.execute(
                "INSERT INTO nav_item(id, project_id, node_id, path, display_name, item_kind)
                 VALUES(?1, 1, 100, '.cache/output.bin', 'output.bin', 'file')",
                params![cache_nav_id],
            )
            .unwrap();

            let result = recoverable_for_target(&conn, 1).unwrap();
            assert!(!result.mutation_owned_node_ids.contains(&100));
            assert!(!result.recoverable_node_ids.contains(&100));
        }
    }

    #[test]
    fn protected_membership_blocks_unconfirmed_recoverable_path_in_any_order() {
        for (normal_nav_id, protected_nav_id) in [(10, 11), (11, 10)] {
            let conn = test_conn();
            insert_project(&conn, 1, "one");
            insert_file(
                &conn,
                normal_nav_id,
                100,
                1,
                "project/secret.bin",
                50,
                "protected-node",
            );
            conn.execute(
                "INSERT INTO nav_item(
                   id, project_id, node_id, path, display_name, item_kind, protected_level
                 ) VALUES(?1, 1, 100, 'protected/secret.bin', 'secret.bin', 'file', 'no_preview')",
                params![protected_nav_id],
            )
            .unwrap();

            let result = recoverable_for_target(&conn, 1).unwrap();
            assert!(!result.recoverable_node_ids.contains(&100));
            assert!(result.mutation_owned_node_ids.contains(&100));
        }
    }

    #[test]
    fn reparse_points_are_excluded_from_recoverable() {
        let conn = test_conn();
        insert_project(&conn, 1, "one");
        insert_file(&conn, 10, 100, 1, "junction", 1000, "j");
        conn.execute("UPDATE node SET is_reparse = 1 WHERE id = 100", [])
            .unwrap();

        let result = recoverable_for_target(&conn, 1).unwrap();
        assert_eq!(result.summary.recoverable_bytes.total, 0);
        assert_eq!(result.summary.protected_count, 1);
        assert!(!result.recoverable_node_ids.contains(&100));
        assert!(result.mutation_owned_node_ids.contains(&100));
    }

    #[test]
    fn cloud_placeholders_are_excluded_from_recoverable() {
        // An online-only OneDrive/Dropbox placeholder has is_reparse=0 but
        // reparse_kind='cloud_placeholder' and metadata.len()=full logical size. It must NEVER
        // be counted as recoverable owned bytes (inflated reclaim) nor proposed for deletion
        // (which would hydrate/egress or destroy the local handle to cloud data).
        let conn = test_conn();
        insert_project(&conn, 1, "one");
        insert_file(&conn, 10, 100, 1, "model.safetensors", 5_000_000_000, "ph");
        conn.execute(
            "UPDATE node SET reparse_kind = 'cloud_placeholder' WHERE id = 100",
            [],
        )
        .unwrap();

        let result = recoverable_for_target(&conn, 1).unwrap();
        assert_eq!(
            result.summary.recoverable_bytes.total, 0,
            "a cloud placeholder must never be recoverable or proposed for deletion"
        );
        assert!(result
            .candidates
            .iter()
            .any(|candidate| candidate.node_id == 100
                && candidate.reparse_kind.as_deref() == Some("cloud_placeholder")));
        assert!(!result.recoverable_node_ids.contains(&100));
        assert!(!result.mutation_owned_node_ids.contains(&100));
    }

    #[test]
    fn materialized_cloud_reparse_is_never_mutation_owned() {
        // A fully materialized Cloud File still carries a Cloud Files reparse tag.
        // It is readable without recall, but deleting its local path can propagate to
        // the cloud provider. The protected-content opt-in must not own or unlink it.
        let conn = test_conn();
        insert_project(&conn, 1, "one");
        insert_file(&conn, 10, 100, 1, "local-model.bin", 4096, "cloud");
        conn.execute(
            "UPDATE node SET is_reparse = 1, reparse_kind = 'cloud_local' WHERE id = 100",
            [],
        )
        .unwrap();

        let result = recoverable_for_target(&conn, 1).unwrap();
        assert_eq!(result.summary.recoverable_bytes.total, 0);
        assert_eq!(result.summary.protected_count, 1);
        assert!(!result.recoverable_node_ids.contains(&100));
        assert!(!result.mutation_owned_node_ids.contains(&100));
        let candidate = result
            .candidates
            .iter()
            .find(|candidate| candidate.node_id == 100)
            .expect("cloud-local node remains visible as a blocking candidate");
        assert!(candidate.is_reparse);
        assert_eq!(candidate.reparse_kind.as_deref(), Some("cloud_local"));
    }

    #[test]
    fn recoverable_target_honors_cancel_token() {
        let conn = test_conn();
        insert_project(&conn, 1, "one");
        let cancel = AtomicBool::new(true);

        let result = recoverable_for_target_cancellable(&conn, 1, &cancel);

        assert!(matches!(result, Err(AccountingError::Cancelled)));
    }
}
