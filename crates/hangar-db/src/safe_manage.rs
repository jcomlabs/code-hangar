use super::{
    ensure_column, now, project_footprint_summaries, project_relationships_ready, set_setting,
    setting_value, Db, DbError, DbResult, DbWriteSession, SubtreeScanTarget,
};
use hangar_core::{
    display_path_for_path, normalize_path, ProjectSummary, SafeManageAnalysisRun,
    SafeManageConfidence, SafeManageDecision, SafeManageDecisionKind, SafeManageDecisionRequest,
    SafeManageDuplicateEvidence, SafeManageEvidenceCoverage, SafeManageFileKindCount,
    SafeManageFileKindProfile, SafeManageFirstRunPreference, SafeManageImportantFile,
    SafeManageLifecycle, SafeManageObjectiveInput, SafeManageOperationPlanRequest,
    SafeManageProjectAssessment, SafeManageRecommendation, SafeManageRegenerableScanRequest,
    SafeManageRegenerableTarget, SafeManageRiskRelation,
};
use hangar_protect::{is_heavy_or_protected_container_path, regenerable_container_kind};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

const PROMPT_ENABLED_KEY: &str = "safe_manage.first_run.suggest_after_discovery";
const PROMPT_STATE_KEY: &str = "safe_manage.first_run.prompt_state";
const PROMPT_LAST_AT_KEY: &str = "safe_manage.first_run.last_prompted_at";

// The fast Safe Manage pass must stay predictable on very large local
// portfolios. These limits cap catalog rows and pairwise work; hitting one
// degrades absence claims to partial/unavailable evidence rather than silently
// manufacturing a zero.
const SAFE_MANAGE_PROFILE_MAX_FILES_PER_PROJECT: usize = 4_096;
const SAFE_MANAGE_PROFILE_MAX_FILES_PER_RUN: usize = 100_000;
const SAFE_MANAGE_SIMILAR_NAME_GROUP_MAX: usize = 32;
const SAFE_MANAGE_MATERIAL_MIN_FILES: usize = 8;
const SAFE_MANAGE_COPY_MIN_BYTES: u64 = 1_024;
const SAFE_MANAGE_RELATED_PROJECT_IDS_MAX: usize = 64;

fn ensure_safe_manage_not_cancelled(cancel: Option<&AtomicBool>) -> DbResult<()> {
    if cancel.is_some_and(|flag| flag.load(Ordering::Acquire)) {
        return Err(DbError::InvalidInput(
            "Safe Manage analysis was cancelled.".to_string(),
        ));
    }
    Ok(())
}

pub(super) fn ensure_safe_manage_schema(conn: &Connection) -> DbResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS safe_manage_analysis_run (
           id TEXT PRIMARY KEY,
           state TEXT NOT NULL CHECK(state IN (
             'queued', 'running', 'cancelling', 'completed', 'partial', 'cancelled', 'failed'
           )),
           ruleset_version TEXT NOT NULL,
           catalog_revision TEXT NOT NULL,
           created_at TEXT NOT NULL,
           completed_at TEXT,
           run_json TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_safe_manage_run_latest
           ON safe_manage_analysis_run(created_at DESC, id DESC);

         CREATE TABLE IF NOT EXISTS safe_manage_project_assessment (
           analysis_run_id TEXT NOT NULL REFERENCES safe_manage_analysis_run(id) ON DELETE CASCADE,
           project_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
           evidence_revision TEXT NOT NULL,
           lifecycle TEXT NOT NULL,
           recommendation TEXT NOT NULL,
           ordinal INTEGER NOT NULL DEFAULT 0,
           assessment_json TEXT NOT NULL,
           PRIMARY KEY(analysis_run_id, project_id)
         );
         CREATE INDEX IF NOT EXISTS idx_safe_manage_assessment_project
           ON safe_manage_project_assessment(project_id, analysis_run_id);

         CREATE TABLE IF NOT EXISTS safe_manage_decision (
           id INTEGER PRIMARY KEY,
           project_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
           analysis_run_id TEXT NOT NULL REFERENCES safe_manage_analysis_run(id) ON DELETE RESTRICT,
           decision TEXT NOT NULL CHECK(decision IN (
             'keep', 'ignore', 'request_deeper_review', 'archive',
             'clean_regenerables', 'prepare_removal'
           )),
           evidence_revision TEXT NOT NULL,
           decided_by TEXT NOT NULL DEFAULT 'local_user',
           decided_at TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_safe_manage_decision_project
           ON safe_manage_decision(project_id, id DESC);

         CREATE TABLE IF NOT EXISTS safe_manage_regenerable_expansion (
           project_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
           nav_id INTEGER NOT NULL REFERENCES nav_item(id) ON DELETE CASCADE,
           node_id INTEGER NOT NULL REFERENCES node(id) ON DELETE CASCADE,
           path TEXT NOT NULL,
           analysis_run_id TEXT NOT NULL REFERENCES safe_manage_analysis_run(id) ON DELETE RESTRICT,
           evidence_revision TEXT NOT NULL,
           state TEXT NOT NULL CHECK(state IN ('running', 'completed', 'partial', 'cancelled', 'failed')),
           item_count INTEGER NOT NULL DEFAULT 0,
           observed_bytes INTEGER,
           started_at TEXT NOT NULL,
           completed_at TEXT,
           error TEXT,
           PRIMARY KEY(project_id, nav_id)
         );",
    )?;
    ensure_column(
        conn,
        "safe_manage_project_assessment",
        "ordinal",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_column(
        conn,
        "safe_manage_decision",
        "decided_by",
        "TEXT NOT NULL DEFAULT 'local_user'",
    )?;
    Ok(())
}

/// A process died before a queued/running analysis reached a terminal state.
/// Preserve every completed assessment, but never resume or call it successful:
/// the next explicit run will re-read current evidence.
pub(super) fn reconcile_interrupted_safe_manage_runs(conn: &Connection) -> DbResult<()> {
    let mut stmt = conn.prepare(
        "SELECT id, run_json FROM safe_manage_analysis_run
         WHERE state IN ('queued', 'running', 'cancelling')
         ORDER BY created_at, id",
    )?;
    let pending = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);
    for (id, json) in pending {
        let mut run = load_run_with_assessments(conn, &json)?;
        run.state = "partial".to_string();
        run.processed_projects = run.assessments.len() as u64;
        run.counts = hangar_core::safe_manage_portfolio_counts(&run.assessments);
        run.completed_at = Some(now());
        run.message = "The previous analysis was interrupted. Its completed findings were preserved, but current evidence must be analyzed again."
            .to_string();
        run.error = Some("Analysis interrupted before completion.".to_string());
        let mut header = run;
        header.assessments.clear();
        let updated =
            serde_json::to_string(&header).map_err(|error| DbError::FileRead(error.to_string()))?;
        conn.execute(
            "UPDATE safe_manage_analysis_run
             SET state = 'partial', completed_at = ?2, run_json = ?3
             WHERE id = ?1 AND state IN ('queued', 'running', 'cancelling')",
            params![id, header.completed_at, updated],
        )?;
    }
    // An explicit regenerable expansion replaces an opaque subtree
    // incrementally. If the process died mid-walk, preserve whatever concrete
    // rows reached SQLite but make both the target and receipt unambiguously
    // incomplete; it can never become OperationPlan-eligible by restart alone.
    conn.execute(
        "UPDATE nav_item
         SET fully_scanned = 0,
             scan_error = 'Regenerable expansion was interrupted before completion.'
         WHERE id IN (
           SELECT nav_id FROM safe_manage_regenerable_expansion WHERE state = 'running'
         )",
        [],
    )?;
    conn.execute(
        "UPDATE safe_manage_regenerable_expansion
         SET state = 'failed', completed_at = ?1,
             error = 'Regenerable expansion was interrupted before completion.'
         WHERE state = 'running'",
        params![now()],
    )?;
    Ok(())
}

#[derive(Debug, Clone)]
struct ComparableFileEvidence {
    project_id: i64,
    relative_path: String,
    kind: String,
    size: Option<u64>,
    identity_key: String,
    content_hash: Option<String>,
}

#[derive(Debug)]
struct LoadedProjectProfile {
    rows: Vec<ComparableFileEvidence>,
    file_kind_profile: SafeManageFileKindProfile,
    size_comparison_complete: bool,
}

#[derive(Debug)]
struct ProjectComparisonEvidence {
    file_kind_profile: SafeManageFileKindProfile,
    duplicate_evidence: SafeManageDuplicateEvidence,
    comparison_evidence_revision: String,
    materially_similar_project_count: Option<u64>,
    risk_relations: Vec<SafeManageRiskRelation>,
}

#[derive(Debug, Default)]
struct SimilarProjectGroups {
    by_key: HashMap<String, Vec<i64>>,
    project_key: HashMap<i64, String>,
}

impl SimilarProjectGroups {
    #[cfg(test)]
    fn from_projects(projects: &[ProjectSummary]) -> Self {
        Self::from_projects_interruptible(projects, None)
            .expect("an analysis without a cancellation flag cannot be cancelled")
    }

    fn from_projects_interruptible(
        projects: &[ProjectSummary],
        cancel: Option<&AtomicBool>,
    ) -> DbResult<Self> {
        let mut groups = Self::default();
        for project in projects {
            ensure_safe_manage_not_cancelled(cancel)?;
            let key = similar_project_key(&project.name);
            if key.is_empty() {
                continue;
            }
            groups.project_key.insert(project.id, key.clone());
            groups.by_key.entry(key).or_default().push(project.id);
        }
        for ids in groups.by_key.values_mut() {
            ensure_safe_manage_not_cancelled(cancel)?;
            ids.sort_unstable();
            ids.dedup();
        }
        Ok(groups)
    }

    fn related(&self, project_id: i64) -> (u64, Vec<i64>) {
        let Some(key) = self.project_key.get(&project_id) else {
            return (0, Vec::new());
        };
        let Some(ids) = self.by_key.get(key) else {
            return (0, Vec::new());
        };
        let count = ids.len().saturating_sub(1) as u64;
        let related = ids
            .iter()
            .copied()
            .filter(|candidate| *candidate != project_id)
            .take(SAFE_MANAGE_RELATED_PROJECT_IDS_MAX)
            .collect();
        (count, related)
    }

    fn material_comparison_bounded(&self, project_id: i64) -> bool {
        self.project_key
            .get(&project_id)
            .and_then(|key| self.by_key.get(key))
            .is_none_or(|ids| ids.len() <= SAFE_MANAGE_SIMILAR_NAME_GROUP_MAX)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ComparisonMember {
    project_id: i64,
    file_key: String,
    identity_key: String,
}

type CrossProjectGroupEvidence = (HashMap<i64, BTreeSet<String>>, HashMap<i64, BTreeSet<i64>>);

fn load_bounded_portfolio_comparison(
    conn: &Connection,
    projects: &[ProjectSummary],
    aggregates: &HashMap<i64, ProjectAggregates>,
    similar_groups: &SimilarProjectGroups,
    cancel: Option<&AtomicBool>,
) -> DbResult<HashMap<i64, ProjectComparisonEvidence>> {
    load_bounded_portfolio_comparison_with_limits_interruptible(
        conn,
        projects,
        aggregates,
        similar_groups,
        SAFE_MANAGE_PROFILE_MAX_FILES_PER_PROJECT,
        SAFE_MANAGE_PROFILE_MAX_FILES_PER_RUN,
        cancel,
    )
}

#[cfg(test)]
fn load_bounded_portfolio_comparison_with_limits(
    conn: &Connection,
    projects: &[ProjectSummary],
    aggregates: &HashMap<i64, ProjectAggregates>,
    similar_groups: &SimilarProjectGroups,
    max_files_per_project: usize,
    max_files_per_run: usize,
) -> DbResult<HashMap<i64, ProjectComparisonEvidence>> {
    load_bounded_portfolio_comparison_with_limits_interruptible(
        conn,
        projects,
        aggregates,
        similar_groups,
        max_files_per_project,
        max_files_per_run,
        None,
    )
}

fn load_bounded_portfolio_comparison_with_limits_interruptible(
    conn: &Connection,
    projects: &[ProjectSummary],
    aggregates: &HashMap<i64, ProjectAggregates>,
    similar_groups: &SimilarProjectGroups,
    max_files_per_project: usize,
    max_files_per_run: usize,
    cancel: Option<&AtomicBool>,
) -> DbResult<HashMap<i64, ProjectComparisonEvidence>> {
    ensure_safe_manage_not_cancelled(cancel)?;
    if projects.is_empty() {
        return Ok(HashMap::new());
    }

    // Share the global row budget deterministically by project id. This avoids
    // an early large project starving every later project, and makes coverage
    // independent of the caller's display ordering.
    let mut ordered_projects = projects.iter().collect::<Vec<_>>();
    ordered_projects.sort_by_key(|project| project.id);
    let base_budget = max_files_per_run / ordered_projects.len();
    let extra_budget = max_files_per_run % ordered_projects.len();
    let mut loaded = HashMap::<i64, LoadedProjectProfile>::new();

    for (index, project) in ordered_projects.into_iter().enumerate() {
        ensure_safe_manage_not_cancelled(cancel)?;
        let budget = base_budget
            .saturating_add(usize::from(index < extra_budget))
            .min(max_files_per_project);
        let mut policy_mismatch = false;
        let mut rows = if budget == 0 {
            Vec::new()
        } else {
            load_comparable_file_rows(conn, project.id, budget.saturating_add(1))?
        };
        rows.retain(|row| {
            let allowed = !is_heavy_or_protected_container_path(&row.relative_path);
            policy_mismatch |= !allowed;
            allowed
        });
        let truncated = rows.len() > budget;
        rows.truncate(budget);

        let scan_complete = comparison_catalog_scan_complete(conn, project)?
            && aggregates
                .get(&project.id)
                .is_some_and(|value| value.comparison_scan_error_count == 0);
        let local_complete = budget > 0 && scan_complete && !truncated && !policy_mismatch;
        let coverage = if local_complete {
            SafeManageEvidenceCoverage::Complete
        } else if rows.is_empty() {
            SafeManageEvidenceCoverage::Unavailable
        } else {
            SafeManageEvidenceCoverage::Partial
        };
        let file_kind_profile = file_kind_profile_from_rows(&rows, coverage);
        let size_comparison_complete = local_complete && rows.iter().all(|row| row.size.is_some());
        loaded.insert(
            project.id,
            LoadedProjectProfile {
                rows,
                file_kind_profile,
                size_comparison_complete,
            },
        );
    }

    // Duplicate absence is a portfolio statement, so it is complete only if
    // every requested project's comparable metadata was complete. Positive
    // matches remain useful lower bounds when any project was truncated.
    let portfolio_size_comparison_complete = projects.iter().all(|project| {
        loaded
            .get(&project.id)
            .is_some_and(|profile| profile.size_comparison_complete)
    });
    let mut metadata_groups = BTreeMap::<(String, u64), Vec<ComparisonMember>>::new();
    let mut indexed_text_groups = BTreeMap::<(String, u64), Vec<ComparisonMember>>::new();
    let mut indexed_text_counts = HashMap::<i64, u64>::new();

    for profile in loaded.values() {
        ensure_safe_manage_not_cancelled(cancel)?;
        for row in &profile.rows {
            ensure_safe_manage_not_cancelled(cancel)?;
            let indexed_content_hash = row
                .content_hash
                .as_deref()
                .filter(|hash| is_full_blake3(hash));
            if indexed_content_hash.is_some() {
                *indexed_text_counts.entry(row.project_id).or_default() += 1;
            }
            let Some(size) = row.size.filter(|size| *size >= SAFE_MANAGE_COPY_MIN_BYTES) else {
                continue;
            };
            let member = ComparisonMember {
                project_id: row.project_id,
                file_key: format!("{}\u{0}{}", row.relative_path, row.identity_key),
                identity_key: row.identity_key.clone(),
            };
            metadata_groups
                .entry((row.relative_path.clone(), size))
                .or_default()
                .push(member.clone());
            if let Some(content_hash) = indexed_content_hash {
                indexed_text_groups
                    .entry((content_hash.to_ascii_lowercase(), size))
                    .or_default()
                    .push(member);
            }
        }
    }

    let (possible_files, possible_related) =
        cross_project_group_evidence_interruptible(metadata_groups, cancel)?;
    let (confirmed_text_files, confirmed_text_related) =
        cross_project_group_evidence_interruptible(indexed_text_groups, cancel)?;

    let mut exact_material_counts = HashMap::<i64, u64>::new();
    let mut exact_material_related = HashMap::<i64, BTreeSet<i64>>::new();
    let mut exact_shape_by_project = HashMap::<i64, String>::new();
    let mut exact_shapes = BTreeMap::<String, Vec<i64>>::new();
    for (project_id, profile) in &loaded {
        ensure_safe_manage_not_cancelled(cancel)?;
        if let Some(digest) = exact_material_shape_digest(profile) {
            exact_shape_by_project.insert(*project_id, digest.clone());
            exact_shapes.entry(digest).or_default().push(*project_id);
        }
    }
    for mut ids in exact_shapes.into_values().filter(|ids| ids.len() > 1) {
        ensure_safe_manage_not_cancelled(cancel)?;
        ids.sort_unstable();
        ids.dedup();
        record_bounded_exact_material_group(
            &ids,
            &mut exact_material_counts,
            &mut exact_material_related,
            cancel,
        )?;
    }

    // A bounded structural overlap is only attempted inside a version-like
    // name group. Exact metadata-shape matches above remain O(n) for projects
    // with unrelated names; this pairwise branch is capped at 32 members.
    let mut near_material_related = HashMap::<i64, BTreeSet<i64>>::new();
    for ids in similar_groups
        .by_key
        .values()
        .filter(|ids| ids.len() > 1 && ids.len() <= SAFE_MANAGE_SIMILAR_NAME_GROUP_MAX)
    {
        ensure_safe_manage_not_cancelled(cancel)?;
        for left_index in 0..ids.len() {
            ensure_safe_manage_not_cancelled(cancel)?;
            for right_index in (left_index + 1)..ids.len() {
                ensure_safe_manage_not_cancelled(cancel)?;
                let left_id = ids[left_index];
                let right_id = ids[right_index];
                let Some(left) = loaded.get(&left_id) else {
                    continue;
                };
                let Some(right) = loaded.get(&right_id) else {
                    continue;
                };
                if materially_similar_profiles(left, right) {
                    near_material_related
                        .entry(left_id)
                        .or_default()
                        .insert(right_id);
                    near_material_related
                        .entry(right_id)
                        .or_default()
                        .insert(left_id);
                }
            }
        }
    }

    let mut result = HashMap::new();
    for project in projects {
        ensure_safe_manage_not_cancelled(cancel)?;
        let Some(profile) = loaded.get(&project.id) else {
            continue;
        };
        let duplicate_coverage = if portfolio_size_comparison_complete {
            SafeManageEvidenceCoverage::Complete
        } else if profile.rows.is_empty() {
            SafeManageEvidenceCoverage::Unavailable
        } else {
            SafeManageEvidenceCoverage::Partial
        };
        let duplicate_evidence = SafeManageDuplicateEvidence {
            coverage: duplicate_coverage,
            inspected_file_count: profile.rows.len() as u64,
            possible_copy_file_count: possible_files
                .get(&project.id)
                .map_or(0, |files| files.len() as u64),
            indexed_text_file_count: indexed_text_counts.get(&project.id).copied().unwrap_or(0),
            confirmed_indexed_text_copy_count: confirmed_text_files
                .get(&project.id)
                .map_or(0, |files| files.len() as u64),
        };
        let near_ids = near_material_related
            .get(&project.id)
            .cloned()
            .unwrap_or_default();
        let own_exact_shape = exact_shape_by_project.get(&project.id);
        let near_extra_count = near_ids
            .iter()
            .filter(|related_id| {
                own_exact_shape.is_none()
                    || exact_shape_by_project.get(related_id) != own_exact_shape
            })
            .count() as u64;
        let material_count = exact_material_counts
            .get(&project.id)
            .copied()
            .unwrap_or(0)
            .saturating_add(near_extra_count);
        let mut material_ids = exact_material_related
            .get(&project.id)
            .cloned()
            .unwrap_or_default();
        extend_bounded_related_ids(&mut material_ids, near_ids);
        let material_comparison_complete = portfolio_size_comparison_complete
            && similar_groups.material_comparison_bounded(project.id);
        let materially_similar_project_count = if material_count == 0 {
            material_comparison_complete.then_some(0)
        } else {
            Some(material_count)
        };

        let possible_ids = possible_related
            .get(&project.id)
            .cloned()
            .unwrap_or_default();
        let confirmed_ids = confirmed_text_related
            .get(&project.id)
            .cloned()
            .unwrap_or_default();
        let mut risk_relations = Vec::new();
        if !possible_ids.is_empty() {
            risk_relations.push(SafeManageRiskRelation {
                kind: "possible_file_copies".to_string(),
                label: format!(
                    "{} other project(s) contain metadata-only same-path and same-size copy candidates; file bodies were not opened.",
                    bounded_related_project_count_label(&possible_ids)
                ),
                confidence: SafeManageConfidence::Low,
                related_project_ids: capped_ids(&possible_ids),
            });
        }
        if !confirmed_ids.is_empty() {
            risk_relations.push(SafeManageRiskRelation {
                kind: "indexed_text_duplicates".to_string(),
                label: format!(
                    "{} other project(s) contain byte-identical safe text already covered by full local BLAKE3 index hashes. This does not prove project redundancy.",
                    bounded_related_project_count_label(&confirmed_ids)
                ),
                confidence: SafeManageConfidence::High,
                related_project_ids: capped_ids(&confirmed_ids),
            });
        }
        if !material_ids.is_empty() {
            risk_relations.push(SafeManageRiskRelation {
                kind: "materially_similar_inventory".to_string(),
                label: format!(
                    "{} other project(s) match the bounded metadata inventory rules. This is review evidence, not byte identity or cleanup authorization.",
                    material_count
                ),
                confidence: SafeManageConfidence::Low,
                related_project_ids: capped_ids(&material_ids),
            });
        }
        let comparison_evidence_revision = comparison_evidence_revision(
            &profile.file_kind_profile,
            &duplicate_evidence,
            materially_similar_project_count,
            &possible_ids,
            &confirmed_ids,
            &material_ids,
        )?;
        result.insert(
            project.id,
            ProjectComparisonEvidence {
                file_kind_profile: profile.file_kind_profile.clone(),
                duplicate_evidence,
                comparison_evidence_revision,
                materially_similar_project_count,
                risk_relations,
            },
        );
    }
    Ok(result)
}

fn load_comparable_file_rows(
    conn: &Connection,
    project_id: i64,
    limit: usize,
) -> DbResult<Vec<ComparableFileEvidence>> {
    let mut stmt = conn.prepare(
        "SELECT ni.node_id, ni.path, n.path, n.size_apparent,
                n.volume_id, n.inode_key, ni.is_context,
                CASE WHEN di.preview_safe = 1 THEN di.content_hash END
         FROM nav_item ni
         JOIN node n ON n.id = ni.node_id
         LEFT JOIN document_index di
           ON di.project_id = ni.project_id AND di.node_id = ni.node_id
         WHERE ni.project_id = ?1
           AND ni.item_kind = 'file'
           AND ni.is_sensitive = 0
           AND ni.protected_level IS NULL
           AND ni.collapse_default = 0
           AND n.present = 1
           AND (n.is_reparse = 0 OR n.reparse_kind = 'cloud_local')
           AND COALESCE(n.reparse_kind, '') <> 'cloud_placeholder'
         ORDER BY ni.path COLLATE NOCASE, ni.id
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(
        params![project_id, i64::try_from(limit).unwrap_or(i64::MAX)],
        |row| {
            let node_id = row.get::<_, i64>(0)?;
            let relative_path = normalize_path(&row.get::<_, String>(1)?).to_ascii_lowercase();
            let absolute_path = row.get::<_, Option<String>>(2)?;
            let size = row
                .get::<_, Option<i64>>(3)?
                .and_then(|value| u64::try_from(value).ok());
            let volume_id = row.get::<_, Option<String>>(4)?;
            let inode_key = row.get::<_, Option<String>>(5)?;
            let is_context = row.get::<_, i64>(6)? != 0;
            let identity_key = match (volume_id, inode_key) {
                (Some(volume), Some(inode)) => format!("inode:{volume}:{inode}"),
                _ => absolute_path
                    .filter(|path| !path.is_empty())
                    .map(|path| format!("path:{}", normalize_path(&path).to_ascii_lowercase()))
                    .unwrap_or_else(|| format!("node:{node_id}")),
            };
            Ok(ComparableFileEvidence {
                project_id,
                kind: safe_manage_file_kind(&relative_path, is_context).to_string(),
                relative_path,
                size,
                identity_key,
                content_hash: row.get::<_, Option<String>>(7)?,
            })
        },
    )?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn file_kind_profile_from_rows(
    rows: &[ComparableFileEvidence],
    coverage: SafeManageEvidenceCoverage,
) -> SafeManageFileKindProfile {
    let mut counts = BTreeMap::<&str, u64>::new();
    for row in rows {
        *counts.entry(row.kind.as_str()).or_default() += 1;
    }
    let ordered = [
        ("manifest_config", "Manifest and configuration files"),
        ("documentation", "Documentation and context files"),
        ("source", "Source files"),
        ("model", "Model files"),
        ("media", "Media files"),
        ("data_archive", "Data and archive files"),
        ("binary", "Binary files"),
        ("other", "Other files"),
    ];
    let counts = ordered
        .into_iter()
        .filter_map(|(kind, label)| {
            counts
                .get(kind)
                .copied()
                .filter(|count| *count > 0)
                .map(|file_count| SafeManageFileKindCount {
                    kind: kind.to_string(),
                    label: label.to_string(),
                    file_count,
                })
        })
        .collect();
    SafeManageFileKindProfile {
        coverage,
        inspected_file_count: rows.len() as u64,
        counts,
    }
}

fn safe_manage_file_kind(path: &str, is_context: bool) -> &'static str {
    let normalized = normalize_path(path).to_ascii_lowercase();
    let name = normalized.rsplit('/').next().unwrap_or(normalized.as_str());
    let extension = name.rsplit_once('.').map(|(_, extension)| extension);
    if matches!(
        name,
        "cargo.toml"
            | "cargo.lock"
            | "package.json"
            | "package-lock.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "bun.lock"
            | "bun.lockb"
            | "pyproject.toml"
            | "poetry.lock"
            | "requirements.txt"
            | "go.mod"
            | "go.sum"
            | "dockerfile"
            | "compose.yaml"
            | "compose.yml"
            | "makefile"
            | "justfile"
    ) || matches!(
        extension,
        Some("sln" | "csproj" | "vcxproj" | "props" | "targets")
    ) {
        "manifest_config"
    } else if is_context || matches!(extension, Some("md" | "mdx" | "rst" | "adoc" | "txt")) {
        "documentation"
    } else if matches!(
        extension,
        Some(
            "rs" | "ts"
                | "tsx"
                | "js"
                | "jsx"
                | "mjs"
                | "cjs"
                | "py"
                | "go"
                | "c"
                | "cc"
                | "cpp"
                | "h"
                | "hpp"
                | "cs"
                | "java"
                | "kt"
                | "kts"
                | "swift"
                | "gd"
                | "lua"
                | "rb"
                | "php"
                | "sh"
                | "bash"
                | "zsh"
                | "ps1"
                | "css"
                | "scss"
                | "sass"
                | "less"
                | "html"
                | "htm"
                | "vue"
                | "svelte"
                | "sql"
        )
    ) {
        "source"
    } else if matches!(
        extension,
        Some("safetensors" | "ckpt" | "pt" | "pth" | "gguf" | "onnx" | "engine" | "plan")
    ) {
        "model"
    } else if matches!(
        extension,
        Some(
            "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "webp"
                | "svg"
                | "bmp"
                | "tif"
                | "tiff"
                | "ico"
                | "mp3"
                | "wav"
                | "flac"
                | "ogg"
                | "m4a"
                | "mp4"
                | "mov"
                | "mkv"
                | "webm"
                | "avi"
                | "blend"
                | "fbx"
                | "gltf"
                | "glb"
                | "obj"
        )
    ) {
        "media"
    } else if matches!(
        extension,
        Some(
            "json"
                | "jsonl"
                | "yaml"
                | "yml"
                | "toml"
                | "xml"
                | "csv"
                | "tsv"
                | "parquet"
                | "sqlite"
                | "sqlite3"
                | "db"
                | "zip"
                | "7z"
                | "rar"
                | "tar"
                | "gz"
                | "bz2"
                | "xz"
        )
    ) {
        "data_archive"
    } else if matches!(
        extension,
        Some("exe" | "dll" | "so" | "dylib" | "lib" | "a" | "o" | "obj" | "wasm" | "bin")
    ) {
        "binary"
    } else {
        "other"
    }
}

fn is_full_blake3(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
fn cross_project_group_evidence(
    groups: BTreeMap<(String, u64), Vec<ComparisonMember>>,
) -> CrossProjectGroupEvidence {
    cross_project_group_evidence_interruptible(groups, None)
        .expect("an analysis without a cancellation flag cannot be cancelled")
}

fn cross_project_group_evidence_interruptible(
    groups: BTreeMap<(String, u64), Vec<ComparisonMember>>,
    cancel: Option<&AtomicBool>,
) -> DbResult<CrossProjectGroupEvidence> {
    let mut participating_files = HashMap::<i64, BTreeSet<String>>::new();
    let mut related_projects = HashMap::<i64, BTreeSet<i64>>::new();
    for mut members in groups.into_values() {
        ensure_safe_manage_not_cancelled(cancel)?;
        members.sort();
        members.dedup();
        let physical = members
            .iter()
            .map(|member| member.identity_key.as_str())
            .collect::<BTreeSet<_>>();
        let project_ids = members
            .iter()
            .map(|member| member.project_id)
            .collect::<BTreeSet<_>>();
        if physical.len() < 2 || project_ids.len() < 2 {
            continue;
        }
        for member in &members {
            ensure_safe_manage_not_cancelled(cancel)?;
            participating_files
                .entry(member.project_id)
                .or_default()
                .insert(member.file_key.clone());
        }
        // Relationship samples are aggregated once per project, never once per
        // member. Only the first bounded deterministic candidate window is
        // inspected, so even a very large duplicate group cannot become O(n²).
        for project_id in &project_ids {
            ensure_safe_manage_not_cancelled(cancel)?;
            let related = related_projects.entry(*project_id).or_default();
            if related.len() >= SAFE_MANAGE_RELATED_PROJECT_IDS_MAX {
                continue;
            }
            for candidate in project_ids
                .iter()
                .take(SAFE_MANAGE_RELATED_PROJECT_IDS_MAX.saturating_add(1))
            {
                ensure_safe_manage_not_cancelled(cancel)?;
                if candidate != project_id {
                    related.insert(*candidate);
                    if related.len() >= SAFE_MANAGE_RELATED_PROJECT_IDS_MAX {
                        break;
                    }
                }
            }
        }
    }
    Ok((participating_files, related_projects))
}

fn exact_material_shape_digest(profile: &LoadedProjectProfile) -> Option<String> {
    if !profile.size_comparison_complete || profile.rows.len() < SAFE_MANAGE_MATERIAL_MIN_FILES {
        return None;
    }
    let mut shape = profile
        .rows
        .iter()
        .map(|row| {
            format!(
                "{}\u{0}{}\u{0}{}",
                row.relative_path,
                row.kind,
                row.size.expect("complete size profile")
            )
        })
        .collect::<Vec<_>>();
    shape.sort();
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"safe-manage-material-shape-v1");
    for value in shape {
        hasher.update(&(value.len() as u64).to_le_bytes());
        hasher.update(value.as_bytes());
    }
    Some(hasher.finalize().to_hex().to_string())
}

fn materially_similar_profiles(left: &LoadedProjectProfile, right: &LoadedProjectProfile) -> bool {
    if !left.size_comparison_complete || !right.size_comparison_complete {
        return false;
    }
    let left_features = left
        .rows
        .iter()
        .map(|row| format!("{}\u{0}{}", row.relative_path, row.kind))
        .collect::<BTreeSet<_>>();
    let right_features = right
        .rows
        .iter()
        .map(|row| format!("{}\u{0}{}", row.relative_path, row.kind))
        .collect::<BTreeSet<_>>();
    if left_features.len() < SAFE_MANAGE_MATERIAL_MIN_FILES
        || right_features.len() < SAFE_MANAGE_MATERIAL_MIN_FILES
    {
        return false;
    }
    let intersection = left_features.intersection(&right_features).count() as u128;
    let union = left_features.union(&right_features).count() as u128;
    intersection.saturating_mul(100) >= union.saturating_mul(80)
}

fn capped_ids(ids: &BTreeSet<i64>) -> Vec<i64> {
    ids.iter()
        .copied()
        .take(SAFE_MANAGE_RELATED_PROJECT_IDS_MAX)
        .collect()
}

fn bounded_related_project_count_label(ids: &BTreeSet<i64>) -> String {
    if ids.len() >= SAFE_MANAGE_RELATED_PROJECT_IDS_MAX {
        format!("At least {}", ids.len())
    } else {
        ids.len().to_string()
    }
}

fn extend_bounded_related_ids<I>(target: &mut BTreeSet<i64>, ids: I)
where
    I: IntoIterator<Item = i64>,
{
    if target.len() >= SAFE_MANAGE_RELATED_PROJECT_IDS_MAX {
        return;
    }
    for id in ids.into_iter().take(SAFE_MANAGE_RELATED_PROJECT_IDS_MAX) {
        target.insert(id);
        if target.len() >= SAFE_MANAGE_RELATED_PROJECT_IDS_MAX {
            break;
        }
    }
}

fn record_bounded_exact_material_group(
    ids: &[i64],
    counts: &mut HashMap<i64, u64>,
    related: &mut HashMap<i64, BTreeSet<i64>>,
    cancel: Option<&AtomicBool>,
) -> DbResult<()> {
    let related_count = ids.len().saturating_sub(1) as u64;
    for project_id in ids {
        ensure_safe_manage_not_cancelled(cancel)?;
        counts.insert(*project_id, related_count);
        extend_bounded_related_ids(
            related.entry(*project_id).or_default(),
            ids.iter()
                .copied()
                .filter(|candidate| candidate != project_id),
        );
    }
    Ok(())
}

fn comparison_evidence_revision(
    file_kind_profile: &SafeManageFileKindProfile,
    duplicate_evidence: &SafeManageDuplicateEvidence,
    materially_similar_project_count: Option<u64>,
    possible_ids: &BTreeSet<i64>,
    confirmed_ids: &BTreeSet<i64>,
    material_ids: &BTreeSet<i64>,
) -> DbResult<String> {
    let possible_ids = capped_ids(possible_ids);
    let confirmed_ids = capped_ids(confirmed_ids);
    let material_ids = capped_ids(material_ids);
    let encoded = serde_json::to_vec(&(
        file_kind_profile,
        duplicate_evidence,
        materially_similar_project_count,
        &possible_ids,
        &confirmed_ids,
        &material_ids,
    ))
    .map_err(|error| DbError::FileRead(error.to_string()))?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"safe-manage-comparison-evidence-v2");
    hasher.update(&(encoded.len() as u64).to_le_bytes());
    hasher.update(&encoded);
    Ok(hasher.finalize().to_hex().to_string())
}

fn safe_manage_objective_inputs_from_conn(
    conn: &Connection,
    projects: &[ProjectSummary],
    session_counts: &HashMap<i64, u64>,
    cancel: Option<&AtomicBool>,
) -> DbResult<Vec<SafeManageObjectiveInput>> {
    ensure_safe_manage_not_cancelled(cancel)?;
    let catalog_evidence_epoch = safe_manage_catalog_evidence_epoch(conn)?;
    ensure_safe_manage_not_cancelled(cancel)?;
    let similar_groups = SimilarProjectGroups::from_projects_interruptible(projects, cancel)?;
    let footprints = project_footprint_summaries(conn, usize::MAX, true)?
        .into_iter()
        .map(|summary| (summary.project_id, summary))
        .collect::<HashMap<_, _>>();
    let mut project_aggregates = HashMap::with_capacity(projects.len());
    for project in projects {
        ensure_safe_manage_not_cancelled(cancel)?;
        project_aggregates.insert(project.id, load_project_aggregates(conn, project.id)?);
    }
    let mut comparisons = load_bounded_portfolio_comparison(
        conn,
        projects,
        &project_aggregates,
        &similar_groups,
        cancel,
    )?;
    let mut inputs = Vec::with_capacity(projects.len());
    for project in projects {
        ensure_safe_manage_not_cancelled(cancel)?;
        let aggregates = project_aggregates.get(&project.id).ok_or_else(|| {
            DbError::FileRead("Safe Manage project aggregates disappeared.".to_string())
        })?;
        let comparison = comparisons.remove(&project.id).ok_or_else(|| {
            DbError::FileRead("Safe Manage comparison profile disappeared.".to_string())
        })?;
        let git = load_git_evidence(conn, project.id)?;
        let footprint = footprints.get(&project.id);
        let relationship_evidence_complete = project_relationships_ready(conn, project.id)?;
        let related_project_ids = load_shared_project_ids(conn, project.id)?;
        let relationship_evidence_revision = safe_manage_relationship_evidence_revision(
            conn,
            project.id,
            aggregates.shared_reference_count,
            aggregates.relationship_issue_count,
            &related_project_ids,
        )?;
        let shared_reference_count =
            relationship_evidence_complete.then_some(aggregates.shared_reference_count);
        let relationship_issue_count =
            relationship_evidence_complete.then_some(aggregates.relationship_issue_count);
        let mut risk_relations = if related_project_ids.is_empty() {
            Vec::new()
        } else {
            vec![SafeManageRiskRelation {
                kind: "shared_inventory_nodes".to_string(),
                label: format!(
                    "{} other project(s) share catalog objects with this project.",
                    related_project_ids.len()
                ),
                confidence: SafeManageConfidence::High,
                related_project_ids,
            }]
        };
        let (similar_project_count, similar_project_ids) = similar_groups.related(project.id);
        if !similar_project_ids.is_empty() {
            risk_relations.push(SafeManageRiskRelation {
                kind: "similar_project_name".to_string(),
                label: format!(
                    "{} registered project(s) have a similar version-like name.",
                    similar_project_count
                ),
                confidence: SafeManageConfidence::Medium,
                related_project_ids: similar_project_ids,
            });
        }
        risk_relations.extend(comparison.risk_relations);
        inputs.push(SafeManageObjectiveInput {
            project_id: project.id,
            project_name: project.name.clone(),
            project_path: project.path.clone(),
            source: project.source.clone(),
            catalog_evidence_epoch: catalog_evidence_epoch.clone(),
            relationship_evidence_revision,
            apps: project.apps.clone(),
            is_current: project.is_current,
            session_count: session_counts.get(&project.id).copied(),
            last_activity_ms: aggregates.last_activity_ms,
            last_activity_source: aggregates
                .last_activity_ms
                .map(|_| "scanned file modification time".to_string()),
            scan_complete: project.scan_state == "scanned" && aggregates.scan_error_count == 0,
            scan_error_count: aggregates.scan_error_count,
            file_count: aggregates.file_count,
            context_file_count: aggregates.context_file_count,
            substantive_file_count: aggregates.substantive_file_count,
            file_kind_profile: comparison.file_kind_profile,
            duplicate_evidence: comparison.duplicate_evidence,
            comparison_evidence_revision: comparison.comparison_evidence_revision,
            materially_similar_project_count: comparison.materially_similar_project_count,
            apparent_bytes: footprint.map(|value| value.apparent_bytes),
            physical_bytes: footprint.and_then(|value| value.physical_bytes),
            footprint_partial: footprint
                .map(|value| value.footprint_partial)
                .unwrap_or(true),
            has_git: git.has_git,
            git_has_remote: git.has_git.then_some(git.has_remote),
            // Current working-tree state is deliberately supplied later by the
            // API's bounded local Git probe. Stored HEAD/config metadata cannot
            // prove a clean worktree.
            git_uncommitted: None,
            git_evidence_error: git.error,
            regenerable_bytes: (!aggregates.regenerable_partial)
                .then_some(aggregates.regenerable_bytes),
            similar_project_count: Some(similar_project_count),
            shared_reference_count,
            relationship_issue_count,
            relationship_evidence_complete,
            sensitive_file_count: aggregates.sensitive_file_count,
            protected_file_count: aggregates.protected_file_count,
            root_protected: project.protected_level.is_some(),
            important_files: load_important_files(conn, project.id)?,
            risk_relations,
        });
    }
    Ok(inputs)
}

/// Stable portfolio epoch: real scan generations, registered project identity
/// and the protection policy. It deliberately excludes navigation rows and
/// relationship readiness because exact regenerable materialization and a
/// no-op relationship rebuild may change those implementation details without
/// changing the decision the owner reviewed.
fn safe_manage_catalog_evidence_epoch(conn: &Connection) -> DbResult<String> {
    let scan_roots = {
        let mut stmt = conn.prepare(
            "SELECT id, path, enabled, scan_generation,
                    CASE WHEN last_scanned_at IS NULL THEN 0 ELSE 1 END
             FROM scan_root
             WHERE COALESCE(adhoc, 0) = 0
             ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let projects = {
        let mut stmt = conn.prepare(
            "SELECT project.id, project.name, project.path,
                    COALESCE(json_extract(project.attributes, '$.source'), 'fixture'),
                    project.protected_level
             FROM node project
             LEFT JOIN scan_root root ON root.path = project.path
             WHERE project.kind = 'project' AND project.present = 1
               AND COALESCE(root.adhoc, 0) = 0
             ORDER BY project.id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let protected_zones = {
        let mut stmt = conn.prepare(
            "SELECT id, pattern_type, pattern, level, source
             FROM protected_zone ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let encoded = serde_json::to_vec(&(scan_roots, projects, protected_zones))
        .map_err(|error| DbError::FileRead(error.to_string()))?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"safe-manage-catalog-evidence-v1");
    hasher.update(&(encoded.len() as u64).to_le_bytes());
    hasher.update(&encoded);
    Ok(hasher.finalize().to_hex().to_string())
}

/// Digest semantic relationships independently of the relationship index's
/// pending/ready state. Paths and relationship kinds are stable across a no-op
/// rebuild; changed links, unresolved targets or shared memberships are not.
fn safe_manage_relationship_evidence_revision(
    conn: &Connection,
    project_id: i64,
    shared_reference_count: u64,
    relationship_issue_count: u64,
    shared_project_ids: &[i64],
) -> DbResult<String> {
    let edges = {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT
                    CASE WHEN source.project_id = ?1 THEN 'outgoing' ELSE 'incoming' END,
                    source.project_id,
                    source.path,
                    COALESCE(target.path, ''),
                    relation.kind,
                    relation.confidence
             FROM edge relation
             JOIN nav_item source ON source.id = relation.source_nav_id
             JOIN node target ON target.id = relation.target_node_id
             WHERE source.project_id = ?1
                OR EXISTS (
                  SELECT 1 FROM nav_item membership
                  WHERE membership.node_id = relation.target_node_id
                    AND membership.project_id = ?1
                )
             ORDER BY 1, 2, 3, 4, 5, 6",
        )?;
        let rows = stmt.query_map(params![project_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let issues = {
        let mut stmt = conn.prepare(
            "SELECT source.path, issue.kind, issue.confidence, issue.target
             FROM relationship_issue issue
             JOIN nav_item source ON source.id = issue.source_nav_id
             WHERE source.project_id = ?1
             ORDER BY source.path, issue.kind, issue.confidence, issue.target",
        )?;
        let rows = stmt.query_map(params![project_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    let encoded = serde_json::to_vec(&(
        shared_reference_count,
        relationship_issue_count,
        shared_project_ids,
        edges,
        issues,
    ))
    .map_err(|error| DbError::FileRead(error.to_string()))?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"safe-manage-relationship-evidence-v1");
    hasher.update(&(encoded.len() as u64).to_le_bytes());
    hasher.update(&encoded);
    Ok(hasher.finalize().to_hex().to_string())
}

impl Db {
    /// Assemble bounded local catalog facts. App/session enrichment is supplied
    /// by the API because app registries and complete session discovery are not
    /// persisted in SQLite. A missing count stays unknown and cannot authorize a
    /// cleanup recommendation.
    pub fn safe_manage_objective_inputs(
        &self,
        projects: &[ProjectSummary],
        session_counts: &HashMap<i64, u64>,
    ) -> DbResult<Vec<SafeManageObjectiveInput>> {
        self.with_read_conn(|conn| {
            safe_manage_objective_inputs_from_conn(conn, projects, session_counts, None)
        })
    }

    /// Close every pending relationship family and capture the objective
    /// portfolio snapshot on the same SQLite writer connection. The API holds
    /// the inventory/mutation write gate around this call, so a scan cannot
    /// invalidate relationships between preparation and hashing.
    pub fn safe_manage_prepared_objective_inputs(
        &self,
        projects: &[ProjectSummary],
        session_counts: &HashMap<i64, u64>,
        cancel: &AtomicBool,
    ) -> DbResult<Vec<SafeManageObjectiveInput>> {
        self.with_writer(|conn| {
            super::rebuild_all_pending_relationships_interruptible(conn, Some(cancel))?;
            safe_manage_objective_inputs_from_conn(conn, projects, session_counts, Some(cancel))
        })
    }

    pub fn safe_manage_analysis_save(&self, run: &SafeManageAnalysisRun) -> DbResult<()> {
        validate_run(run)?;
        self.with_writer(|conn| {
            ensure_safe_manage_schema(conn)?;
            let tx = conn.transaction()?;
            let prior_state = tx
                .query_row(
                    "SELECT state FROM safe_manage_analysis_run WHERE id = ?1",
                    params![run.id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            validate_run_transition(prior_state.as_deref(), &run.state)?;
            let stored = load_assessments_for_run(&tx, &run.id)?;
            if stored.len() > run.assessments.len()
                || stored
                    .iter()
                    .zip(&run.assessments)
                    .any(|(left, right)| left != right)
            {
                return Err(DbError::InvalidInput(
                    "Safe Manage analysis history is append-only and cannot be replaced or rewound."
                        .to_string(),
                ));
            }
            for (ordinal, assessment) in run.assessments.iter().enumerate().skip(stored.len()) {
                insert_assessment(&tx, &run.id, ordinal, assessment)?;
            }
            write_analysis_header(&tx, run)?;
            tx.commit()?;
            Ok(())
        })
    }

    /// Persist a queued/running header before any assessment has been appended.
    /// Progress rows use `safe_manage_analysis_assessment_append`; this method
    /// refuses to hide or replace an existing assessment prefix.
    pub fn safe_manage_analysis_header_save(&self, run: &SafeManageAnalysisRun) -> DbResult<()> {
        validate_run_metadata(run)?;
        if !matches!(run.state.as_str(), "queued" | "running")
            || !run.assessments.is_empty()
            || run.processed_projects != 0
            || run.counts != Default::default()
        {
            return Err(DbError::InvalidInput(
                "A Safe Manage analysis header cannot contain assessments or terminal state."
                    .to_string(),
            ));
        }
        self.with_writer(|conn| {
            ensure_safe_manage_schema(conn)?;
            let tx = conn.transaction()?;
            let prior_state = tx
                .query_row(
                    "SELECT state FROM safe_manage_analysis_run WHERE id = ?1",
                    params![run.id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            validate_run_transition(prior_state.as_deref(), &run.state)?;
            if !load_assessments_for_run(&tx, &run.id)?.is_empty() {
                return Err(DbError::InvalidInput(
                    "A Safe Manage analysis header cannot replace appended assessments."
                        .to_string(),
                ));
            }
            write_analysis_header(&tx, run)?;
            tx.commit()?;
            Ok(())
        })
    }

    /// Atomically append exactly one assessment and its matching progress
    /// header. The prior prefix is neither read back nor rewritten, keeping a
    /// portfolio run linear in the number of projects.
    pub fn safe_manage_analysis_assessment_append(
        &self,
        run: &SafeManageAnalysisRun,
        assessment: &SafeManageProjectAssessment,
    ) -> DbResult<()> {
        validate_run_metadata(run)?;
        if run.state != "running"
            || run.completed_at.is_some()
            || run.assessments.last() != Some(assessment)
            || run.processed_projects != run.assessments.len() as u64
            || assessment.analysis_run_id != run.id
            || assessment.ruleset_version != run.ruleset_version
            || assessment.evidence_revision.trim().is_empty()
        {
            return Err(DbError::InvalidInput(
                "The appended Safe Manage assessment is not the next item in this running analysis."
                    .to_string(),
            ));
        }
        self.with_writer(|conn| {
            ensure_safe_manage_schema(conn)?;
            let tx = conn.transaction()?;
            let previous = load_analysis_header(&tx, &run.id)?.ok_or_else(|| {
                DbError::InvalidInput(
                    "Create the Safe Manage analysis header before appending findings.".to_string(),
                )
            })?;
            validate_run_transition(Some(&previous.state), &run.state)?;
            require_same_analysis_identity(&previous, run)?;
            if previous.state != "running"
                || previous.processed_projects.saturating_add(1) != run.processed_projects
                || previous.total_projects != run.total_projects
                || previous.catalog_revision != run.catalog_revision
            {
                return Err(DbError::InvalidInput(
                    "Safe Manage assessments must be appended once, in order, to the current running header."
                        .to_string(),
                ));
            }
            let mut expected_counts = previous.counts.clone();
            hangar_core::safe_manage_portfolio_counts_include(
                &mut expected_counts,
                assessment,
            );
            if expected_counts != run.counts {
                return Err(DbError::InvalidInput(
                    "Safe Manage progress counts do not match the appended assessment.".to_string(),
                ));
            }
            let ordinal = usize::try_from(previous.processed_projects).map_err(|_| {
                DbError::InvalidInput("Safe Manage assessment ordinal is too large.".to_string())
            })?;
            insert_assessment(&tx, &run.id, ordinal, assessment)?;
            write_analysis_header(&tx, run)?;
            tx.commit()?;
            Ok(())
        })
    }

    /// Commit only the terminal header after proving it describes the exact
    /// append-only assessment prefix already stored in SQLite.
    pub fn safe_manage_analysis_finalize(&self, run: &SafeManageAnalysisRun) -> DbResult<()> {
        validate_run(run)?;
        if !matches!(
            run.state.as_str(),
            "completed" | "partial" | "cancelled" | "failed"
        ) || run.completed_at.is_none()
        {
            return Err(DbError::InvalidInput(
                "Safe Manage finalization requires a terminal state and completion time."
                    .to_string(),
            ));
        }
        self.with_writer(|conn| {
            ensure_safe_manage_schema(conn)?;
            let tx = conn.transaction()?;
            let previous = load_analysis_header(&tx, &run.id)?.ok_or_else(|| {
                DbError::InvalidInput("The Safe Manage analysis header does not exist.".to_string())
            })?;
            validate_run_transition(Some(&previous.state), &run.state)?;
            require_same_analysis_identity(&previous, run)?;
            let stored = load_assessments_for_run(&tx, &run.id)?;
            if stored != run.assessments
                || run.processed_projects != stored.len() as u64
                || run.counts != hangar_core::safe_manage_portfolio_counts(&stored)
            {
                return Err(DbError::InvalidInput(
                    "The terminal Safe Manage header does not match its durable assessment prefix."
                        .to_string(),
                ));
            }
            write_analysis_header(&tx, run)?;
            tx.commit()?;
            Ok(())
        })
    }

    pub fn safe_manage_analysis_latest(&self) -> DbResult<Option<SafeManageAnalysisRun>> {
        self.with_read_conn(|conn| {
            let run_json = conn
                .query_row(
                    "SELECT run_json FROM safe_manage_analysis_run
                     ORDER BY created_at DESC, id DESC LIMIT 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            run_json
                .map(|json| load_run_with_assessments(conn, &json))
                .transpose()
        })
    }

    pub fn safe_manage_analysis_latest_complete(&self) -> DbResult<Option<SafeManageAnalysisRun>> {
        self.with_read_conn(|conn| {
            let run_json = conn
                .query_row(
                    "SELECT run_json FROM safe_manage_analysis_run
                     WHERE state = 'completed'
                     ORDER BY created_at DESC, id DESC LIMIT 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            run_json
                .map(|json| load_run_with_assessments(conn, &json))
                .transpose()
        })
    }

    pub fn safe_manage_analysis_get(
        &self,
        run_id: &str,
    ) -> DbResult<Option<SafeManageAnalysisRun>> {
        self.with_read_conn(|conn| {
            let run_json = conn
                .query_row(
                    "SELECT run_json FROM safe_manage_analysis_run WHERE id = ?1",
                    params![run_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            run_json
                .map(|json| load_run_with_assessments(conn, &json))
                .transpose()
        })
    }

    pub fn safe_manage_decision_record(
        &self,
        project_id: i64,
        analysis_run_id: &str,
        decision: SafeManageDecisionKind,
        evidence_revision: &str,
    ) -> DbResult<SafeManageDecision> {
        let request = SafeManageDecisionRequest {
            project_id,
            analysis_run_id: analysis_run_id.to_string(),
            decision,
            evidence_revision: evidence_revision.to_string(),
        };
        self.safe_manage_decisions_record_atomic(std::slice::from_ref(&request))?
            .pop()
            .ok_or_else(|| {
                DbError::InvalidInput("The Safe Manage decision was not recorded.".to_string())
            })
    }

    /// Validate a whole owner selection before writing any member. Every
    /// project must belong to the same completed analysis run and carry the
    /// exact stored evidence revision. SQLite commits the group once, so an
    /// ineligible/stale member leaves no partial decision rows behind.
    pub fn safe_manage_decisions_record_atomic(
        &self,
        requests: &[SafeManageDecisionRequest],
    ) -> DbResult<Vec<SafeManageDecision>> {
        if requests.is_empty() {
            return Err(DbError::InvalidInput(
                "A Safe Manage decision group cannot be empty.".to_string(),
            ));
        }
        if requests.len() > 1_000 {
            return Err(DbError::InvalidInput(
                "A Safe Manage decision group exceeds the 1,000-project limit.".to_string(),
            ));
        }
        let analysis_run_id = requests[0].analysis_run_id.trim();
        if analysis_run_id.is_empty()
            || requests
                .iter()
                .any(|request| request.analysis_run_id.trim() != analysis_run_id)
        {
            return Err(DbError::InvalidInput(
                "Every grouped Safe Manage decision must belong to the same analysis run."
                    .to_string(),
            ));
        }
        let mut unique_projects = HashSet::with_capacity(requests.len());
        if requests
            .iter()
            .any(|request| !unique_projects.insert(request.project_id))
        {
            return Err(DbError::InvalidInput(
                "A Safe Manage decision group contains a duplicate project.".to_string(),
            ));
        }
        if requests
            .iter()
            .any(|request| request.evidence_revision.trim().is_empty())
        {
            return Err(DbError::InvalidInput(
                "Every Safe Manage decision requires an evidence revision.".to_string(),
            ));
        }

        self.with_writer(|conn| {
            ensure_safe_manage_schema(conn)?;
            let tx = conn.transaction()?;
            // First pass is validation only. No INSERT occurs until every
            // member has passed, preserving all-or-nothing semantics.
            for request in requests {
                let recommendation = validate_analysis_project_binding(
                    &tx,
                    request.project_id,
                    analysis_run_id,
                    &request.evidence_revision,
                )?;
                if recommendation == SafeManageRecommendation::DoNotTouch
                    && decision_changes_disk_intent(request.decision)
                {
                    return Err(DbError::InvalidInput(
                        "This analysis marked a selected project Do not touch. Review its protection evidence instead."
                            .to_string(),
                    ));
                }
            }

            let decided_at = now();
            let mut results = Vec::with_capacity(requests.len());
            for request in requests {
                tx.execute(
                    "INSERT INTO safe_manage_decision(
                       project_id, analysis_run_id, decision, evidence_revision, decided_by, decided_at
                     ) VALUES(?1, ?2, ?3, ?4, 'local_user', ?5)",
                    params![
                        request.project_id,
                        analysis_run_id,
                        decision_name(request.decision),
                        request.evidence_revision,
                        decided_at,
                    ],
                )?;
                results.push(SafeManageDecision {
                    id: tx.last_insert_rowid(),
                    project_id: request.project_id,
                    analysis_run_id: analysis_run_id.to_string(),
                    decision: request.decision,
                    evidence_revision: request.evidence_revision.clone(),
                    decided_by: "local_user".to_string(),
                    decided_at: decided_at.clone(),
                    evidence_stale: false,
                });
            }
            tx.commit()?;
            Ok(results)
        })
    }

    pub fn safe_manage_decisions_latest(&self) -> DbResult<Vec<SafeManageDecision>> {
        self.with_read_conn(|conn| {
            let mut stmt = conn.prepare(
                "WITH latest AS (
                   SELECT project_id, MAX(id) AS id
                   FROM safe_manage_decision GROUP BY project_id
                 ), latest_assessment AS (
                   SELECT assessment.project_id, assessment.evidence_revision
                   FROM safe_manage_project_assessment assessment
                   JOIN safe_manage_analysis_run run ON run.id = assessment.analysis_run_id
                   WHERE run.id = (
                     SELECT newest.id FROM safe_manage_analysis_run newest
                     WHERE newest.state = 'completed'
                     ORDER BY newest.created_at DESC, newest.id DESC LIMIT 1
                   )
                 )
                 SELECT decision.id, decision.project_id, decision.analysis_run_id,
                        decision.decision, decision.evidence_revision, decision.decided_by,
                        decision.decided_at,
                        CASE WHEN latest_assessment.evidence_revision = decision.evidence_revision
                             THEN 0 ELSE 1 END
                 FROM latest
                 JOIN safe_manage_decision decision ON decision.id = latest.id
                 LEFT JOIN latest_assessment ON latest_assessment.project_id = decision.project_id
                 ORDER BY decision.id DESC",
            )?;
            let rows = stmt.query_map([], |row| {
                let stored_decision = row.get::<_, String>(3)?;
                let decision = parse_decision(&stored_decision).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
                Ok(SafeManageDecision {
                    id: row.get(0)?,
                    project_id: row.get(1)?,
                    analysis_run_id: row.get(2)?,
                    decision,
                    evidence_revision: row.get(4)?,
                    decided_by: row.get(5)?,
                    decided_at: row.get(6)?,
                    evidence_stale: row.get::<_, i64>(7)? != 0,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>().map_err(DbError::from)
        })
    }

    /// Enumerate only exact, outermost, project-local regenerable containers.
    /// The analysis binding is checked in the same read snapshot as the target
    /// identities. Protected/shared/overlapping-project rows are omitted rather
    /// than widened to a project-root action.
    pub fn safe_manage_regenerable_targets(
        &self,
        project_id: i64,
        analysis_run_id: &str,
        evidence_revision: &str,
    ) -> DbResult<Vec<SafeManageRegenerableTarget>> {
        self.with_read_conn(|conn| {
            Ok(load_regenerable_candidates(
                conn,
                project_id,
                Some((analysis_run_id, evidence_revision)),
            )?
            .candidates
            .into_iter()
            .map(|candidate| candidate.target)
            .collect())
        })
    }

    /// Resolve every caller-supplied identity field back to one current target.
    /// A current clean-regenerables owner decision bound to the same run and
    /// revision is required before the explicit inventory job can start.
    pub fn safe_manage_regenerable_scan_target(
        &self,
        request: &SafeManageRegenerableScanRequest,
    ) -> DbResult<(SubtreeScanTarget, SafeManageRegenerableTarget)> {
        self.with_read_conn(|conn| {
            let set = load_regenerable_candidates(
                conn,
                request.project_id,
                Some((&request.analysis_run_id, &request.evidence_revision)),
            )?;
            let candidate = set
                .candidates
                .into_iter()
                .find(|candidate| {
                    candidate.target.nav_id == request.nav_id
                        && candidate.target.node_id == request.node_id
                        && path_key(&candidate.target.path) == path_key(&request.path)
                })
                .ok_or_else(|| {
                    DbError::InvalidInput(
                        "The selected nav id, node id and path are not one current regenerable target in this project."
                            .to_string(),
                    )
                })?;
            require_current_clean_regenerables_decision(conn, request)?;
            Ok((candidate.subtree, candidate.target))
        })
    }

    /// Resolve the only object an OperationPlan may receive from a current
    /// Safe Manage decision. Project-level archive/removal intents return the
    /// project id; Clean regenerables returns the exact expanded node id.
    pub fn safe_manage_operation_plan_target(
        &self,
        request: &SafeManageOperationPlanRequest,
    ) -> DbResult<i64> {
        self.with_read_conn(|conn| {
            let recommendation = validate_analysis_project_binding(
                conn,
                request.project_id,
                &request.analysis_run_id,
                &request.evidence_revision,
            )?;
            if recommendation == SafeManageRecommendation::DoNotTouch {
                return Err(DbError::InvalidInput(
                    "This project is marked Do not touch and cannot feed an OperationPlan."
                        .to_string(),
                ));
            }
            if !matches!(
                request.decision,
                SafeManageDecisionKind::Archive
                    | SafeManageDecisionKind::CleanRegenerables
                    | SafeManageDecisionKind::PrepareRemoval
            ) {
                return Err(DbError::InvalidInput(
                    "Only Archive, Clean regenerables or Prepare removal can feed an OperationPlan."
                        .to_string(),
                ));
            }
            require_current_safe_manage_decision(
                conn,
                request.project_id,
                &request.analysis_run_id,
                &request.evidence_revision,
                request.decision,
            )?;

            match request.decision {
                SafeManageDecisionKind::Archive | SafeManageDecisionKind::PrepareRemoval => {
                    if request.target.is_some() {
                        return Err(DbError::InvalidInput(
                            "Project-level Safe Manage actions cannot carry a regenerable target."
                                .to_string(),
                        ));
                    }
                    Ok(request.project_id)
                }
                SafeManageDecisionKind::CleanRegenerables => {
                    let identity = request.target.as_ref().ok_or_else(|| {
                        DbError::InvalidInput(
                            "Clean regenerables requires one exact expanded target.".to_string(),
                        )
                    })?;
                    let candidates = load_regenerable_candidates(
                        conn,
                        request.project_id,
                        Some((&request.analysis_run_id, &request.evidence_revision)),
                    )?;
                    let target = candidates
                        .candidates
                        .into_iter()
                        .map(|candidate| candidate.target)
                        .find(|target| {
                            target.nav_id == identity.nav_id
                                && target.node_id == identity.node_id
                                && path_key(&target.path) == path_key(&identity.path)
                        })
                        .ok_or_else(|| {
                            DbError::InvalidInput(
                                "The Clean regenerables target is not one current exact project target."
                                    .to_string(),
                            )
                        })?;
                    if target.evidence_state != "expanded_complete"
                        || !target.operation_plan_eligible
                    {
                        return Err(DbError::InvalidInput(
                            "The Clean regenerables target needs a current complete expansion receipt before an OperationPlan can be built."
                                .to_string(),
                        ));
                    }
                    Ok(target.node_id)
                }
                _ => unreachable!("non-operation decisions were rejected above"),
            }
        })
    }

    pub fn safe_manage_first_run_preference(&self) -> DbResult<SafeManageFirstRunPreference> {
        self.with_read_conn(|conn| {
            Ok(SafeManageFirstRunPreference {
                suggest_after_discovery: setting_value(conn, PROMPT_ENABLED_KEY)?.as_deref()
                    != Some("0"),
                prompt_state: setting_value(conn, PROMPT_STATE_KEY)?
                    .unwrap_or_else(|| "pending".to_string()),
                last_prompted_at: setting_value(conn, PROMPT_LAST_AT_KEY)?,
            })
        })
    }

    pub fn safe_manage_first_run_preference_set(
        &self,
        suggest_after_discovery: bool,
        prompt_state: &str,
        mark_prompted_now: bool,
    ) -> DbResult<SafeManageFirstRunPreference> {
        if !matches!(
            prompt_state,
            "pending" | "postponed" | "completed" | "suppressed"
        ) {
            return Err(DbError::InvalidInput(
                "The Safe Manage first-run prompt state is invalid.".to_string(),
            ));
        }
        if prompt_state == "suppressed" && suggest_after_discovery {
            return Err(DbError::InvalidInput(
                "A suppressed first-run suggestion cannot remain enabled.".to_string(),
            ));
        }
        self.with_writer(|conn| {
            set_setting(
                conn,
                PROMPT_ENABLED_KEY,
                if suggest_after_discovery { "1" } else { "0" },
            )?;
            set_setting(conn, PROMPT_STATE_KEY, prompt_state)?;
            if mark_prompted_now {
                set_setting(conn, PROMPT_LAST_AT_KEY, &now())?;
            }
            Ok(())
        })?;
        self.safe_manage_first_run_preference()
    }
}

impl DbWriteSession {
    /// Persist the exact identity tuple before replacing an opaque target with
    /// concrete inventory. This receipt is evidence only; it never authorizes a
    /// mutation and cannot name a project root.
    pub fn safe_manage_regenerable_expansion_begin(
        &mut self,
        request: &SafeManageRegenerableScanRequest,
    ) -> DbResult<()> {
        ensure_safe_manage_schema(&self.conn)?;
        let exact = self.conn.query_row(
            "SELECT EXISTS(
               SELECT 1 FROM nav_item ni
               JOIN node n ON n.id = ni.node_id AND n.present = 1
               WHERE ni.project_id = ?1 AND ni.id = ?2 AND ni.node_id = ?3
                 AND ni.path = ?4 AND ni.item_kind = 'directory'
             )",
            params![
                request.project_id,
                request.nav_id,
                request.node_id,
                request.path
            ],
            |row| row.get::<_, i64>(0),
        )? != 0;
        if !exact || regenerable_container_kind(&request.path).is_none() {
            return Err(DbError::InvalidInput(
                "The regenerable expansion identity changed before the scan began.".to_string(),
            ));
        }
        self.conn.execute(
            "INSERT INTO safe_manage_regenerable_expansion(
               project_id, nav_id, node_id, path, analysis_run_id, evidence_revision,
               state, item_count, observed_bytes, started_at, completed_at, error
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, 'running', 0, NULL, ?7, NULL, NULL)
             ON CONFLICT(project_id, nav_id) DO UPDATE SET
               node_id = excluded.node_id,
               path = excluded.path,
               analysis_run_id = excluded.analysis_run_id,
               evidence_revision = excluded.evidence_revision,
               state = 'running',
               item_count = 0,
               observed_bytes = NULL,
               started_at = excluded.started_at,
               completed_at = NULL,
               error = NULL",
            params![
                request.project_id,
                request.nav_id,
                request.node_id,
                request.path,
                request.analysis_run_id,
                request.evidence_revision,
                now(),
            ],
        )?;
        Ok(())
    }

    pub fn safe_manage_regenerable_expansion_finish(
        &mut self,
        request: &SafeManageRegenerableScanRequest,
        state: &str,
        item_count: u64,
        error: Option<&str>,
    ) -> DbResult<()> {
        if !matches!(state, "completed" | "partial" | "cancelled" | "failed") {
            return Err(DbError::InvalidInput(
                "The regenerable expansion terminal state is invalid.".to_string(),
            ));
        }
        let observed_bytes = self
            .conn
            .query_row(
                "SELECT COALESCE(ni.aggregate_physical_bytes, ni.aggregate_allocated_bytes,
                                 ni.aggregate_apparent_bytes, n.size_allocated, n.size_apparent)
                 FROM nav_item ni JOIN node n ON n.id = ni.node_id
                 WHERE ni.project_id = ?1 AND ni.id = ?2 AND ni.node_id = ?3 AND ni.path = ?4",
                params![
                    request.project_id,
                    request.nav_id,
                    request.node_id,
                    request.path
                ],
                |row| row.get::<_, Option<i64>>(0),
            )?
            .and_then(|value| u64::try_from(value).ok());
        let changed = self.conn.execute(
            "UPDATE safe_manage_regenerable_expansion
             SET state = ?5, item_count = ?6, observed_bytes = ?7,
                 completed_at = ?8, error = ?9
             WHERE project_id = ?1 AND nav_id = ?2 AND node_id = ?3 AND path = ?4
               AND analysis_run_id = ?10 AND evidence_revision = ?11 AND state = 'running'",
            params![
                request.project_id,
                request.nav_id,
                request.node_id,
                request.path,
                state,
                i64::try_from(item_count).unwrap_or(i64::MAX),
                observed_bytes.map(|bytes| i64::try_from(bytes).unwrap_or(i64::MAX)),
                now(),
                error,
                request.analysis_run_id,
                request.evidence_revision,
            ],
        )?;
        if changed != 1 {
            return Err(DbError::InvalidInput(
                "The regenerable expansion receipt no longer matches the running scan.".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug)]
struct ProjectAggregates {
    file_count: u64,
    context_file_count: u64,
    substantive_file_count: u64,
    last_activity_ms: Option<i64>,
    scan_error_count: u64,
    comparison_scan_error_count: u64,
    sensitive_file_count: u64,
    protected_file_count: u64,
    regenerable_bytes: u64,
    regenerable_partial: bool,
    shared_reference_count: u64,
    relationship_issue_count: u64,
}

#[derive(Debug)]
struct StoredGitEvidence {
    has_git: bool,
    has_remote: bool,
    error: Option<String>,
}

#[derive(Debug)]
struct RegenerableCandidateSet {
    candidates: Vec<RegenerableCandidate>,
    blocked_evidence: bool,
}

#[derive(Debug)]
struct RegenerableCandidate {
    target: SafeManageRegenerableTarget,
    subtree: SubtreeScanTarget,
}

#[derive(Debug)]
struct ExpansionEvidence {
    analysis_run_id: String,
    evidence_revision: String,
    state: String,
    item_count: u64,
    observed_bytes: Option<u64>,
    error: Option<String>,
}

#[allow(clippy::too_many_arguments)]
fn load_regenerable_candidates(
    conn: &Connection,
    project_id: i64,
    analysis_binding: Option<(&str, &str)>,
) -> DbResult<RegenerableCandidateSet> {
    let (analysis_run_id, evidence_revision) = if let Some((run_id, revision)) = analysis_binding {
        let recommendation = validate_analysis_project_binding(conn, project_id, run_id, revision)?;
        if recommendation == SafeManageRecommendation::DoNotTouch {
            return Err(DbError::InvalidInput(
                "This project is marked Do not touch; regenerable targets are not available."
                    .to_string(),
            ));
        }
        (run_id.to_string(), revision.to_string())
    } else {
        (String::new(), String::new())
    };

    let registered_root = conn
        .query_row(
            "SELECT project.path, root.id
             FROM node project
             JOIN scan_root root ON root.path = project.path
             WHERE project.id = ?1 AND project.kind = 'project' AND project.present = 1
               AND root.enabled = 1 AND root.adhoc = 0",
            params![project_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    let Some((root_path, root_id)) = registered_root else {
        if analysis_binding.is_none() {
            // Built-in fixtures and legacy catalog-only projects legitimately
            // have no enabled disk scan root. They simply contribute zero
            // regenerable targets to objective analysis.
            return Ok(RegenerableCandidateSet {
                candidates: Vec::new(),
                blocked_evidence: false,
            });
        }
        return Err(DbError::InvalidInput(
            "The selected project is not an enabled registered scan root.".to_string(),
        ));
    };
    let other_project_paths = {
        let mut stmt = conn.prepare(
            "SELECT path FROM node
             WHERE kind = 'project' AND present = 1 AND id <> ?1 AND path IS NOT NULL",
        )?;
        let rows = stmt.query_map(params![project_id], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    let mut stmt = conn.prepare(
        "SELECT ni.id, ni.node_id, ni.path, n.path, ni.child_count,
                ni.fully_scanned, ni.scan_error, ni.aggregate_bytes_partial,
                COALESCE(ni.aggregate_physical_bytes, ni.aggregate_allocated_bytes,
                         ni.aggregate_apparent_bytes, n.size_allocated, n.size_apparent),
                n.is_reparse, n.reparse_kind, ni.is_sensitive, ni.protected_level,
                expansion.state, expansion.item_count, expansion.observed_bytes,
                expansion.error, expansion.analysis_run_id, expansion.evidence_revision
         FROM nav_item ni
         JOIN node n ON n.id = ni.node_id AND n.present = 1
         LEFT JOIN safe_manage_regenerable_expansion expansion
           ON expansion.project_id = ni.project_id
          AND expansion.nav_id = ni.id
          AND expansion.node_id = ni.node_id
          AND expansion.path = ni.path
         WHERE ni.project_id = ?1 AND ni.item_kind = 'directory'
         ORDER BY length(ni.path), ni.path COLLATE NOCASE, ni.id",
    )?;
    let rows = stmt.query_map(params![project_id], |row| {
        let expansion_state = row.get::<_, Option<String>>(13)?;
        let expansion_item_count = row
            .get::<_, Option<i64>>(14)?
            .and_then(|value| u64::try_from(value).ok())
            .unwrap_or_default();
        let expansion_observed_bytes = row
            .get::<_, Option<i64>>(15)?
            .and_then(|value| u64::try_from(value).ok());
        let expansion_error = row.get::<_, Option<String>>(16)?;
        let expansion_run_id = row.get::<_, Option<String>>(17)?;
        let expansion_revision = row.get::<_, Option<String>>(18)?;
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            nonnegative_u64(row.get::<_, Option<i64>>(4)?),
            row.get::<_, i64>(5)? != 0,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, i64>(7)? != 0,
            row.get::<_, Option<i64>>(8)?
                .and_then(|value| u64::try_from(value).ok()),
            row.get::<_, i64>(9)? != 0,
            row.get::<_, Option<String>>(10)?,
            row.get::<_, i64>(11)? != 0,
            row.get::<_, Option<String>>(12)?,
            expansion_state.and_then(|state| {
                Some(ExpansionEvidence {
                    analysis_run_id: expansion_run_id?,
                    evidence_revision: expansion_revision?,
                    state,
                    item_count: expansion_item_count,
                    observed_bytes: expansion_observed_bytes,
                    error: expansion_error,
                })
            }),
        ))
    })?;

    let mut candidates = Vec::new();
    let mut blocked_evidence = false;
    for row in rows {
        let (
            nav_id,
            node_id,
            relative_path,
            absolute_path,
            child_count,
            fully_scanned,
            scan_error,
            aggregate_partial,
            bytes,
            is_reparse,
            reparse_kind,
            is_sensitive,
            protected_level,
            expansion,
        ) = row?;
        let Some(kind) = regenerable_container_kind(&relative_path) else {
            continue;
        };

        let expected_absolute = Path::new(&root_path).join(&relative_path);
        let exact_path = path_key(&absolute_path) == path_key(&expected_absolute.to_string_lossy());
        let shared_node = conn.query_row(
            "SELECT EXISTS(
               SELECT 1 FROM nav_item other
               WHERE other.node_id = ?1 AND other.project_id <> ?2
             )",
            params![node_id, project_id],
            |query| query.get::<_, i64>(0),
        )? != 0;
        let overlaps_other_project = other_project_paths
            .iter()
            .any(|other| paths_overlap_by_boundary(&absolute_path, other));
        if !exact_path
            || shared_node
            || overlaps_other_project
            || is_reparse
            || reparse_kind.is_some()
            || is_sensitive
            || protected_level.is_some()
        {
            blocked_evidence = true;
            continue;
        }

        // Only outermost targets are actionable. Nested node_modules/target
        // rows remain concrete inventory beneath their selected ancestor; they
        // are never returned as overlapping OperationPlan roots.
        if candidates.iter().any(|candidate: &RegenerableCandidate| {
            relative_path_is_descendant(&candidate.target.path, &relative_path)
        }) {
            continue;
        }

        let subtree_count = subtree_inventory_count(conn, nav_id)?;
        let receipt_binding_matches = expansion.as_ref().is_some_and(|stored| {
            analysis_binding.is_none()
                || (stored.analysis_run_id == analysis_run_id
                    && stored.evidence_revision == evidence_revision)
        });
        let stale_receipt =
            analysis_binding.is_some() && expansion.is_some() && !receipt_binding_matches;
        let expansion_complete = expansion.as_ref().is_some_and(|stored| {
            receipt_binding_matches
                && stored.state == "completed"
                && stored.item_count == subtree_count
                && stored.observed_bytes == bytes
                && fully_scanned
                && scan_error.is_none()
                && !aggregate_partial
        });
        let expanded_partial = !expansion_complete
            && (stale_receipt
                || subtree_count > 1
                || expansion.as_ref().is_some_and(|stored| {
                    matches!(
                        stored.state.as_str(),
                        "running" | "partial" | "cancelled" | "failed"
                    ) && (!fully_scanned || scan_error.is_some() || aggregate_partial)
                }));
        let evidence_state = if expansion_complete {
            "expanded_complete"
        } else if expanded_partial {
            "expanded_partial"
        } else if fully_scanned && scan_error.is_none() && !aggregate_partial && bytes.is_some() {
            "opaque_measured"
        } else {
            "opaque_partial"
        };
        let effective_error = scan_error.clone().or_else(|| {
            stale_receipt
                .then(|| {
                    "The stored expansion receipt belongs to another analysis run or evidence revision."
                        .to_string()
                })
                .or_else(|| {
                    expansion
                        .as_ref()
                        .and_then(|stored| stored.error.clone())
                        .filter(|_| expanded_partial)
                })
        });
        candidates.push(RegenerableCandidate {
            target: SafeManageRegenerableTarget {
                project_id,
                analysis_run_id: analysis_run_id.clone(),
                evidence_revision: evidence_revision.clone(),
                nav_id,
                node_id,
                path: normalize_path(&relative_path),
                kind: kind.as_str().to_string(),
                bytes,
                evidence_state: evidence_state.to_string(),
                operation_plan_eligible: expansion_complete,
                scan_error: effective_error,
            },
            subtree: SubtreeScanTarget {
                root_id,
                root_path: root_path.clone(),
                display_root_path: display_path_for_path(&root_path),
                project_id,
                nav_id,
                relative_path: normalize_path(&relative_path),
                absolute_path,
            },
        });
        if child_count == 0 && subtree_count > 1 {
            blocked_evidence = true;
        }
    }
    Ok(RegenerableCandidateSet {
        candidates,
        blocked_evidence,
    })
}

fn subtree_inventory_count(conn: &Connection, nav_id: i64) -> DbResult<u64> {
    let count = conn.query_row(
        "WITH RECURSIVE subtree(id) AS (
           SELECT id FROM nav_item WHERE id = ?1
           UNION ALL
           SELECT child.id FROM nav_item child JOIN subtree parent ON child.parent_nav_id = parent.id
         ) SELECT COUNT(*) FROM subtree",
        params![nav_id],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(count.max(0) as u64)
}

fn path_key(path: &str) -> String {
    let normalized = normalize_path(path).trim_end_matches('/').to_string();
    if cfg!(windows) {
        normalized.to_ascii_lowercase()
    } else {
        normalized
    }
}

fn paths_overlap_by_boundary(left: &str, right: &str) -> bool {
    let left = path_key(left);
    let right = path_key(right);
    left == right
        || left
            .strip_prefix(&right)
            .is_some_and(|suffix| suffix.starts_with('/'))
        || right
            .strip_prefix(&left)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn relative_path_is_descendant(parent: &str, candidate: &str) -> bool {
    let parent = path_key(parent);
    let candidate = path_key(candidate);
    candidate != parent
        && candidate
            .strip_prefix(&parent)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn require_current_clean_regenerables_decision(
    conn: &Connection,
    request: &SafeManageRegenerableScanRequest,
) -> DbResult<()> {
    require_current_safe_manage_decision(
        conn,
        request.project_id,
        &request.analysis_run_id,
        &request.evidence_revision,
        SafeManageDecisionKind::CleanRegenerables,
    )
    .map_err(|_| {
        DbError::InvalidInput(
            "Record a current Clean regenerables decision before expanding this exact target."
                .to_string(),
        )
    })
}

fn require_current_safe_manage_decision(
    conn: &Connection,
    project_id: i64,
    analysis_run_id: &str,
    evidence_revision: &str,
    decision: SafeManageDecisionKind,
) -> DbResult<()> {
    let latest = conn
        .query_row(
            "SELECT analysis_run_id, decision, evidence_revision
             FROM safe_manage_decision
             WHERE project_id = ?1 ORDER BY id DESC LIMIT 1",
            params![project_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    if !latest.is_some_and(|(run_id, stored_decision, revision)| {
        run_id == analysis_run_id
            && stored_decision == decision_name(decision)
            && revision == evidence_revision
    }) {
        return Err(DbError::InvalidInput(
            "The latest Safe Manage decision does not match this run, revision and action."
                .to_string(),
        ));
    }
    Ok(())
}

fn load_project_aggregates(conn: &Connection, project_id: i64) -> DbResult<ProjectAggregates> {
    let mut value = conn.query_row(
        "SELECT
           SUM(CASE WHEN ni.item_kind = 'file' THEN 1 ELSE 0 END),
           SUM(CASE WHEN ni.item_kind = 'file' AND ni.is_context = 1 THEN 1 ELSE 0 END),
           SUM(CASE WHEN ni.item_kind = 'file'
                     AND ni.is_context = 0
                     AND lower(ni.path) NOT LIKE '.git/%'
                     AND lower(ni.path) NOT LIKE '%/.git/%'
                     AND lower(ni.path) NOT LIKE 'node_modules/%'
                     AND lower(ni.path) NOT LIKE '%/node_modules/%'
                     AND lower(ni.path) NOT LIKE 'target/%'
                     AND lower(ni.path) NOT LIKE '%/target/%'
                     AND lower(ni.path) NOT LIKE 'dist/%'
                     AND lower(ni.path) NOT LIKE '%/dist/%'
                     AND lower(ni.path) NOT LIKE 'build/%'
                     AND lower(ni.path) NOT LIKE '%/build/%'
                     AND lower(ni.path) NOT LIKE '.venv/%'
                     AND lower(ni.path) NOT LIKE '%/.venv/%'
                     AND lower(ni.path) NOT LIKE '%/__pycache__/%'
                    THEN 1 ELSE 0 END),
           MAX(CASE WHEN ni.item_kind = 'file' THEN CAST(n.mtime AS INTEGER) * 1000 END),
           SUM(CASE WHEN ni.fully_scanned = 0 OR ni.scan_error IS NOT NULL THEN 1 ELSE 0 END),
           SUM(CASE WHEN ni.item_kind = 'file' AND ni.is_sensitive = 1 THEN 1 ELSE 0 END),
           SUM(CASE WHEN ni.item_kind = 'file' AND ni.protected_level IS NOT NULL
                     AND lower(ni.path) NOT LIKE '.git/%'
                     AND lower(ni.path) NOT LIKE '%/.git/%'
                    THEN 1 ELSE 0 END),
           SUM(CASE WHEN ni.item_kind = 'file' AND (
                         lower(ni.path) LIKE 'node_modules/%'
                      OR lower(ni.path) LIKE '%/node_modules/%'
                      OR lower(ni.path) LIKE 'target/%'
                      OR lower(ni.path) LIKE '%/target/%'
                      OR lower(ni.path) LIKE 'dist/%'
                      OR lower(ni.path) LIKE '%/dist/%'
                      OR lower(ni.path) LIKE 'build/%'
                      OR lower(ni.path) LIKE '%/build/%'
                      OR lower(ni.path) LIKE '.venv/%'
                      OR lower(ni.path) LIKE '%/.venv/%'
                      OR lower(ni.path) LIKE '%/__pycache__/%'
                    ) THEN COALESCE(n.size_allocated, n.size_apparent, 0) ELSE 0 END),
           MAX(CASE WHEN ni.item_kind = 'file' AND (
                         lower(ni.path) LIKE 'node_modules/%'
                      OR lower(ni.path) LIKE '%/node_modules/%'
                      OR lower(ni.path) LIKE 'target/%'
                      OR lower(ni.path) LIKE '%/target/%'
                      OR lower(ni.path) LIKE 'dist/%'
                      OR lower(ni.path) LIKE '%/dist/%'
                      OR lower(ni.path) LIKE 'build/%'
                      OR lower(ni.path) LIKE '%/build/%'
                      OR lower(ni.path) LIKE '.venv/%'
                      OR lower(ni.path) LIKE '%/.venv/%'
                      OR lower(ni.path) LIKE '%/__pycache__/%'
                    ) AND (n.size_allocated IS NULL OR ni.aggregate_bytes_partial = 1)
                    THEN 1 ELSE 0 END)
         FROM nav_item ni
         LEFT JOIN node n ON n.id = ni.node_id
         WHERE ni.project_id = ?1",
        params![project_id],
        |row| {
            Ok(ProjectAggregates {
                file_count: nonnegative_u64(row.get::<_, Option<i64>>(0)?),
                context_file_count: nonnegative_u64(row.get::<_, Option<i64>>(1)?),
                substantive_file_count: nonnegative_u64(row.get::<_, Option<i64>>(2)?),
                last_activity_ms: row.get::<_, Option<i64>>(3)?.filter(|value| *value > 0),
                scan_error_count: nonnegative_u64(row.get::<_, Option<i64>>(4)?),
                comparison_scan_error_count: 0,
                sensitive_file_count: nonnegative_u64(row.get::<_, Option<i64>>(5)?),
                protected_file_count: nonnegative_u64(row.get::<_, Option<i64>>(6)?),
                regenerable_bytes: nonnegative_u64(row.get::<_, Option<i64>>(7)?),
                regenerable_partial: row.get::<_, Option<i64>>(8)?.unwrap_or(0) != 0,
                shared_reference_count: 0,
                relationship_issue_count: 0,
            })
        },
    )?;
    value.comparison_scan_error_count = comparison_scope_scan_error_count(conn, project_id)?;
    // The legacy SQL above identifies generated *files* by path and therefore
    // omitted the single opaque container node used by normal scans; after an
    // explicit expansion it could also sum nested generated paths more than
    // once. Replace that provisional value with the shared narrow allowlist and
    // outermost-target aggregates. Each exact container contributes once,
    // whether currently opaque or concretely expanded.
    let regenerables = load_regenerable_candidates(conn, project_id, None)?;
    value.regenerable_bytes = 0;
    value.regenerable_partial = regenerables.blocked_evidence;
    for candidate in regenerables.candidates {
        match candidate.target.bytes {
            Some(bytes) => {
                value.regenerable_bytes = value.regenerable_bytes.saturating_add(bytes);
            }
            None => value.regenerable_partial = true,
        }
        value.regenerable_partial |= candidate.target.evidence_state.ends_with("partial");
    }
    value.shared_reference_count = conn
        .query_row(
            "SELECT COUNT(DISTINCT own.node_id)
         FROM nav_item own
         WHERE own.project_id = ?1 AND own.node_id IS NOT NULL
           AND EXISTS (
             SELECT 1 FROM nav_item other
             WHERE other.node_id = own.node_id AND other.project_id <> own.project_id
           )",
            params![project_id],
            |row| row.get::<_, i64>(0),
        )?
        .max(0) as u64;
    value.relationship_issue_count = conn
        .query_row(
            "SELECT COUNT(*)
         FROM relationship_issue issue
         JOIN nav_item source ON source.id = issue.source_nav_id
         WHERE source.project_id = ?1",
            params![project_id],
            |row| row.get::<_, i64>(0),
        )?
        .max(0) as u64;
    Ok(value)
}

/// Errors inside a deliberately collapsed heavy/protected container belong to
/// the exact Safe Manage materialisation receipt, not to the bounded portfolio
/// comparison. In particular, a cancelled `node_modules` expansion must remain
/// partial and ineligible without making the owner's pre-expansion decision
/// appear stale. Errors in the ordinary comparable catalog still make the
/// profile partial and fail closed.
fn comparison_scope_scan_error_count(conn: &Connection, project_id: i64) -> DbResult<u64> {
    let mut stmt = conn.prepare(
        "SELECT path
         FROM nav_item
         WHERE project_id = ?1
           AND (fully_scanned = 0 OR scan_error IS NOT NULL)
           AND collapse_default = 0
           AND is_sensitive = 0
           AND protected_level IS NULL
         ORDER BY id",
    )?;
    let paths = stmt.query_map(params![project_id], |row| row.get::<_, String>(0))?;
    let mut count = 0_u64;
    for path in paths {
        let path = path?;
        if !is_heavy_or_protected_container_path(&path) {
            count = count.saturating_add(1);
        }
    }
    Ok(count)
}

/// A real root scan owns the stable catalog epoch and marks `last_scanned_at`
/// null until it completes. An exact Safe Manage subtree materialisation does
/// neither; it may temporarily make the navigation-derived `ProjectSummary`
/// look outdated, which must not rewrite the bounded comparison snapshot.
fn comparison_catalog_scan_complete(conn: &Connection, project: &ProjectSummary) -> DbResult<bool> {
    let Some(scan_root_id) = project.scan_root_id else {
        // Synthetic/legacy fixture projects have no registered root. Their
        // existing summary state remains the only available completeness fact.
        return Ok(project.scan_state == "scanned");
    };
    conn.query_row(
        "SELECT enabled = 1 AND adhoc = 0 AND last_scanned_at IS NOT NULL
         FROM scan_root
         WHERE id = ?1",
        params![scan_root_id],
        |row| row.get::<_, bool>(0),
    )
    .optional()
    .map(|value| value.unwrap_or(false))
    .map_err(Into::into)
}

fn load_git_evidence(conn: &Connection, project_id: i64) -> DbResult<StoredGitEvidence> {
    conn.query_row(
        "SELECT origin_url, metadata_error FROM git_repo WHERE project_id = ?1",
        params![project_id],
        |row| {
            Ok(StoredGitEvidence {
                has_git: true,
                has_remote: row
                    .get::<_, Option<String>>(0)?
                    .is_some_and(|value| !value.trim().is_empty()),
                error: row.get(1)?,
            })
        },
    )
    .optional()
    .map(|value| {
        value.unwrap_or(StoredGitEvidence {
            has_git: false,
            has_remote: false,
            error: None,
        })
    })
    .map_err(DbError::from)
}

fn load_important_files(
    conn: &Connection,
    project_id: i64,
) -> DbResult<Vec<SafeManageImportantFile>> {
    let mut stmt = conn.prepare(
        "SELECT ni.node_id, ni.path, ni.display_name, ni.is_sensitive,
                ni.protected_level, ni.priority
         FROM nav_item ni
         WHERE ni.project_id = ?1 AND ni.item_kind = 'file'
           AND (ni.is_context = 1 OR ni.is_sensitive = 1 OR ni.protected_level IS NOT NULL)
         ORDER BY ni.is_sensitive DESC, ni.protected_level IS NOT NULL DESC,
                  ni.priority DESC, ni.path COLLATE NOCASE
         LIMIT 12",
    )?;
    let rows = stmt.query_map(params![project_id], |row| {
        let sensitive = row.get::<_, i64>(3)? != 0;
        let protected = row.get::<_, Option<String>>(4)?.is_some();
        let priority = row.get::<_, i64>(5)?;
        Ok(SafeManageImportantFile {
            node_id: row.get(0)?,
            path: row.get(1)?,
            display_name: row.get(2)?,
            reason: if sensitive {
                "Sensitive-file marker".to_string()
            } else if protected {
                "Protected Zone marker".to_string()
            } else if priority > 0 {
                "High-priority project context".to_string()
            } else {
                "Project context".to_string()
            },
            protected_or_sensitive: sensitive || protected,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(DbError::from)
}

fn load_shared_project_ids(conn: &Connection, project_id: i64) -> DbResult<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT other.project_id
         FROM nav_item own
         JOIN nav_item other ON other.node_id = own.node_id
         WHERE own.project_id = ?1 AND other.project_id <> own.project_id
         ORDER BY other.project_id LIMIT 64",
    )?;
    let rows = stmt.query_map(params![project_id], |row| row.get::<_, i64>(0))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(DbError::from)
}

fn validate_run_metadata(run: &SafeManageAnalysisRun) -> DbResult<()> {
    if run.id.trim().is_empty() || run.id.len() > 128 {
        return Err(DbError::InvalidInput(
            "The Safe Manage analysis id is invalid.".to_string(),
        ));
    }
    if run.ruleset_version.trim().is_empty() || run.catalog_revision.trim().is_empty() {
        return Err(DbError::InvalidInput(
            "The Safe Manage analysis must identify its rules and catalog revision.".to_string(),
        ));
    }
    if !matches!(
        run.state.as_str(),
        "queued" | "running" | "cancelling" | "completed" | "partial" | "cancelled" | "failed"
    ) {
        return Err(DbError::InvalidInput(
            "The Safe Manage analysis state is invalid.".to_string(),
        ));
    }
    if run.processed_projects > run.total_projects {
        return Err(DbError::InvalidInput(
            "Safe Manage progress exceeds the project count.".to_string(),
        ));
    }
    Ok(())
}

fn validate_run(run: &SafeManageAnalysisRun) -> DbResult<()> {
    validate_run_metadata(run)?;
    if run.processed_projects != run.assessments.len() as u64
        || run.counts != hangar_core::safe_manage_portfolio_counts(&run.assessments)
    {
        return Err(DbError::InvalidInput(
            "Safe Manage progress does not match its assessment prefix.".to_string(),
        ));
    }
    if run.state == "completed"
        && (run.processed_projects != run.total_projects || run.counts.total != run.total_projects)
    {
        return Err(DbError::InvalidInput(
            "A complete Safe Manage analysis must contain every project exactly once.".to_string(),
        ));
    }
    let mut projects = HashSet::new();
    for assessment in &run.assessments {
        if assessment.analysis_run_id != run.id
            || assessment.ruleset_version != run.ruleset_version
            || !projects.insert(assessment.project_id)
        {
            return Err(DbError::InvalidInput(
                "Safe Manage assessments are duplicated or bound to another run/ruleset."
                    .to_string(),
            ));
        }
    }
    Ok(())
}

fn analysis_header(run: &SafeManageAnalysisRun) -> SafeManageAnalysisRun {
    SafeManageAnalysisRun {
        id: run.id.clone(),
        state: run.state.clone(),
        ruleset_version: run.ruleset_version.clone(),
        catalog_revision: run.catalog_revision.clone(),
        created_at: run.created_at.clone(),
        started_at: run.started_at.clone(),
        completed_at: run.completed_at.clone(),
        processed_projects: run.processed_projects,
        total_projects: run.total_projects,
        counts: run.counts.clone(),
        message: run.message.clone(),
        error: run.error.clone(),
        assessments: Vec::new(),
    }
}

fn write_analysis_header(conn: &Connection, run: &SafeManageAnalysisRun) -> DbResult<()> {
    let run_json = serde_json::to_string(&analysis_header(run))
        .map_err(|error| DbError::FileRead(error.to_string()))?;
    conn.execute(
        "INSERT INTO safe_manage_analysis_run(
           id, state, ruleset_version, catalog_revision, created_at, completed_at, run_json
         ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(id) DO UPDATE SET
           state = excluded.state,
           ruleset_version = excluded.ruleset_version,
           catalog_revision = excluded.catalog_revision,
           completed_at = excluded.completed_at,
           run_json = excluded.run_json",
        params![
            run.id,
            run.state,
            run.ruleset_version,
            run.catalog_revision,
            run.created_at,
            run.completed_at,
            run_json,
        ],
    )?;
    Ok(())
}

fn load_analysis_header(
    conn: &Connection,
    run_id: &str,
) -> DbResult<Option<SafeManageAnalysisRun>> {
    let json = conn
        .query_row(
            "SELECT run_json FROM safe_manage_analysis_run WHERE id = ?1",
            params![run_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    json.map(|json| {
        let header: SafeManageAnalysisRun =
            serde_json::from_str(&json).map_err(|error| DbError::FileRead(error.to_string()))?;
        if !header.assessments.is_empty() {
            return Err(DbError::FileRead(
                "Stored Safe Manage header contains embedded assessments.".to_string(),
            ));
        }
        Ok(header)
    })
    .transpose()
}

fn load_assessments_for_run(
    conn: &Connection,
    run_id: &str,
) -> DbResult<Vec<SafeManageProjectAssessment>> {
    let mut stmt = conn.prepare(
        "SELECT assessment_json FROM safe_manage_project_assessment
         WHERE analysis_run_id = ?1 ORDER BY ordinal, project_id",
    )?;
    let rows = stmt.query_map(params![run_id], |row| row.get::<_, String>(0))?;
    rows.map(|row| {
        row.map_err(DbError::from).and_then(|json| {
            serde_json::from_str(&json).map_err(|error| DbError::FileRead(error.to_string()))
        })
    })
    .collect()
}

fn insert_assessment(
    conn: &Connection,
    run_id: &str,
    ordinal: usize,
    assessment: &SafeManageProjectAssessment,
) -> DbResult<()> {
    let ordinal = i64::try_from(ordinal)
        .map_err(|_| DbError::InvalidInput("Safe Manage ordinal is too large.".to_string()))?;
    let assessment_json =
        serde_json::to_string(assessment).map_err(|error| DbError::FileRead(error.to_string()))?;
    conn.execute(
        "INSERT INTO safe_manage_project_assessment(
           analysis_run_id, project_id, evidence_revision, lifecycle,
           recommendation, ordinal, assessment_json
         ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            run_id,
            assessment.project_id,
            assessment.evidence_revision,
            lifecycle_name(assessment.lifecycle),
            recommendation_name(assessment.recommendation),
            ordinal,
            assessment_json,
        ],
    )?;
    Ok(())
}

fn require_same_analysis_identity(
    previous: &SafeManageAnalysisRun,
    next: &SafeManageAnalysisRun,
) -> DbResult<()> {
    if previous.id != next.id
        || previous.ruleset_version != next.ruleset_version
        || previous.created_at != next.created_at
        || previous.started_at != next.started_at
        || previous.catalog_revision != next.catalog_revision
        || previous.total_projects != next.total_projects
    {
        return Err(DbError::InvalidInput(
            "Safe Manage analysis identity changed after assessment persistence began.".to_string(),
        ));
    }
    Ok(())
}

fn validate_run_transition(previous: Option<&str>, next: &str) -> DbResult<()> {
    let valid = match previous {
        None => matches!(next, "queued" | "running"),
        Some("queued") => matches!(next, "queued" | "running" | "cancelled" | "failed"),
        Some("running") => matches!(
            next,
            "running" | "cancelling" | "completed" | "partial" | "cancelled" | "failed"
        ),
        Some("cancelling") => matches!(
            next,
            "cancelling" | "completed" | "partial" | "cancelled" | "failed"
        ),
        Some(terminal) => terminal == next,
    };
    if valid {
        Ok(())
    } else {
        Err(DbError::InvalidInput(format!(
            "Invalid Safe Manage analysis transition from {} to {next}.",
            previous.unwrap_or("new")
        )))
    }
}

fn load_run_with_assessments(conn: &Connection, json: &str) -> DbResult<SafeManageAnalysisRun> {
    let mut run: SafeManageAnalysisRun =
        serde_json::from_str(json).map_err(|error| DbError::FileRead(error.to_string()))?;
    run.assessments = load_assessments_for_run(conn, &run.id)?;
    Ok(run)
}

fn lifecycle_name(value: SafeManageLifecycle) -> &'static str {
    match value {
        SafeManageLifecycle::Active => "active",
        SafeManageLifecycle::Dormant => "dormant",
        SafeManageLifecycle::ArchiveCandidate => "archive_candidate",
        SafeManageLifecycle::CleanupCandidate => "cleanup_candidate",
        SafeManageLifecycle::NeedsReview => "needs_review",
    }
}

fn recommendation_name(value: SafeManageRecommendation) -> &'static str {
    match value {
        SafeManageRecommendation::Keep => "keep",
        SafeManageRecommendation::Review => "review",
        SafeManageRecommendation::Archive => "archive",
        SafeManageRecommendation::CleanRegenerables => "clean_regenerables",
        SafeManageRecommendation::RemovalCandidate => "removal_candidate",
        SafeManageRecommendation::DoNotTouch => "do_not_touch",
    }
}

fn parse_recommendation(value: &str) -> DbResult<SafeManageRecommendation> {
    match value {
        "keep" => Ok(SafeManageRecommendation::Keep),
        "review" => Ok(SafeManageRecommendation::Review),
        "archive" => Ok(SafeManageRecommendation::Archive),
        "clean_regenerables" => Ok(SafeManageRecommendation::CleanRegenerables),
        "removal_candidate" => Ok(SafeManageRecommendation::RemovalCandidate),
        "do_not_touch" => Ok(SafeManageRecommendation::DoNotTouch),
        _ => Err(DbError::FileRead(
            "Stored Safe Manage recommendation is invalid.".to_string(),
        )),
    }
}

fn validate_analysis_project_binding(
    conn: &Connection,
    project_id: i64,
    analysis_run_id: &str,
    evidence_revision: &str,
) -> DbResult<SafeManageRecommendation> {
    if analysis_run_id.trim().is_empty() || evidence_revision.trim().is_empty() {
        return Err(DbError::InvalidInput(
            "Safe Manage requires an analysis run and evidence revision.".to_string(),
        ));
    }
    let (state, stored_revision, recommendation_json) = conn
        .query_row(
            "SELECT run.state, assessment.evidence_revision, assessment.recommendation
             FROM safe_manage_analysis_run run
             JOIN safe_manage_project_assessment assessment
               ON assessment.analysis_run_id = run.id
             WHERE run.id = ?1 AND assessment.project_id = ?2",
            params![analysis_run_id, project_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| {
            DbError::InvalidInput(
                "The selected project is not part of that Safe Manage analysis.".to_string(),
            )
        })?;
    if state != "completed" {
        return Err(DbError::InvalidInput(
            "Only a complete Safe Manage analysis can accept this request.".to_string(),
        ));
    }
    if stored_revision != evidence_revision {
        return Err(DbError::InvalidInput(
            "The project evidence changed. Analyze it again before continuing.".to_string(),
        ));
    }
    parse_recommendation(&recommendation_json)
}

fn decision_changes_disk_intent(decision: SafeManageDecisionKind) -> bool {
    matches!(
        decision,
        SafeManageDecisionKind::Archive
            | SafeManageDecisionKind::CleanRegenerables
            | SafeManageDecisionKind::PrepareRemoval
    )
}

fn decision_name(value: SafeManageDecisionKind) -> &'static str {
    match value {
        SafeManageDecisionKind::Keep => "keep",
        SafeManageDecisionKind::Ignore => "ignore",
        SafeManageDecisionKind::RequestDeeperReview => "request_deeper_review",
        SafeManageDecisionKind::Archive => "archive",
        SafeManageDecisionKind::CleanRegenerables => "clean_regenerables",
        SafeManageDecisionKind::PrepareRemoval => "prepare_removal",
    }
}

#[derive(Debug)]
struct StoredDecisionError;

impl std::fmt::Display for StoredDecisionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Stored Safe Manage decision is invalid.")
    }
}

impl std::error::Error for StoredDecisionError {}

fn parse_decision(value: &str) -> Result<SafeManageDecisionKind, StoredDecisionError> {
    match value {
        "keep" => Ok(SafeManageDecisionKind::Keep),
        "ignore" => Ok(SafeManageDecisionKind::Ignore),
        "request_deeper_review" => Ok(SafeManageDecisionKind::RequestDeeperReview),
        "archive" => Ok(SafeManageDecisionKind::Archive),
        "clean_regenerables" => Ok(SafeManageDecisionKind::CleanRegenerables),
        "prepare_removal" => Ok(SafeManageDecisionKind::PrepareRemoval),
        _ => Err(StoredDecisionError),
    }
}

fn nonnegative_u64(value: Option<i64>) -> u64 {
    value.unwrap_or_default().max(0) as u64
}

fn similar_project_key(name: &str) -> String {
    let mut tokens = name
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(|token| token.to_lowercase())
        .collect::<Vec<_>>();
    while tokens.last().is_some_and(|token| {
        matches!(
            token.as_str(),
            "copy" | "backup" | "archive" | "archived" | "old" | "clone" | "draft"
        ) || token.chars().all(|character| character.is_ascii_digit())
            || token.strip_prefix('v').is_some_and(|version| {
                !version.is_empty() && version.chars().all(|character| character.is_ascii_digit())
            })
    }) {
        tokens.pop();
    }
    let key = tokens.join(" ");
    if key.chars().count() >= 3 {
        key
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hangar_core::{
        assess_safe_manage_project, safe_manage_portfolio_counts, SafeManageClassificationContext,
        SafeManageDecisionRequest, SafeManageOperationPlanRequest,
        SafeManageOperationTargetIdentity, SafeManagePortfolioCounts,
        SafeManageRegenerableScanRequest, SAFE_MANAGE_RULESET_VERSION,
    };
    use std::fs;
    use tempfile::tempdir;

    fn empty_run(id: &str, state: &str, total: u64) -> SafeManageAnalysisRun {
        SafeManageAnalysisRun {
            id: id.to_string(),
            state: state.to_string(),
            ruleset_version: SAFE_MANAGE_RULESET_VERSION.to_string(),
            catalog_revision: "catalog-1".to_string(),
            created_at: "2026-08-27T10:00:00Z".to_string(),
            started_at: (state != "queued").then(|| "2026-08-27T10:00:01Z".to_string()),
            completed_at: None,
            processed_projects: 0,
            total_projects: total,
            counts: SafeManagePortfolioCounts::default(),
            message: "Queued".to_string(),
            error: None,
            assessments: Vec::new(),
        }
    }

    fn save_completed_run(
        db: &Db,
        id: &str,
        assessments: Vec<hangar_core::SafeManageProjectAssessment>,
    ) {
        let mut run = empty_run(id, "queued", assessments.len() as u64);
        db.safe_manage_analysis_header_save(&run).unwrap();
        run.state = "running".to_string();
        run.started_at = Some("2026-08-27T10:00:01Z".to_string());
        run.message = "Running".to_string();
        db.safe_manage_analysis_header_save(&run).unwrap();
        for assessment in assessments {
            hangar_core::safe_manage_portfolio_counts_include(&mut run.counts, &assessment);
            run.assessments.push(assessment);
            run.processed_projects = run.assessments.len() as u64;
            db.safe_manage_analysis_assessment_append(&run, run.assessments.last().unwrap())
                .unwrap();
        }
        run.state = "completed".to_string();
        run.completed_at = Some("2026-08-27T10:00:02Z".to_string());
        run.message = "Complete".to_string();
        db.safe_manage_analysis_finalize(&run).unwrap();
    }

    fn assessments_for(
        db: &Db,
        run_id: &str,
        projects: &[ProjectSummary],
        recommendations: &[SafeManageRecommendation],
    ) -> Vec<hangar_core::SafeManageProjectAssessment> {
        let counts = projects
            .iter()
            .map(|project| (project.id, 0_u64))
            .collect::<HashMap<_, _>>();
        let inputs = db.safe_manage_objective_inputs(projects, &counts).unwrap();
        inputs
            .iter()
            .enumerate()
            .map(|(index, input)| {
                let mut assessment = assess_safe_manage_project(
                    input,
                    &SafeManageClassificationContext {
                        now_ms: 1_800_000_000_000,
                        analysis_run_id: run_id.to_string(),
                        evidence_revision: format!("{run_id}-rev-{}", input.project_id),
                        observed_at: Some("2026-08-27T10:00:01Z".to_string()),
                    },
                );
                assessment.recommendation = recommendations[index];
                assessment
            })
            .collect()
    }

    fn load_normal_inventory(db: &Db, root: &Path) -> i64 {
        let root_text = root.to_string_lossy().into_owned();
        db.roots_add(&root_text).unwrap();
        let mut files = Vec::new();
        hangar_fs::scan_inventory_stream(
            root,
            None,
            hangar_fs::ScanLimits::root_scan(),
            None,
            || false,
            |_, _, _| {},
            |batch| {
                files.extend(batch);
                Ok(())
            },
        )
        .unwrap();
        db.load_scanned_root(&root_text, &files, None).unwrap();
        db.project_id_for_root_path(&root_text).unwrap().unwrap()
    }

    fn write_comparable_project(root: &Path, source_byte: u8) {
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("README.md"), vec![b'R'; 2_048]).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"bounded-profile\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        for index in 0..6 {
            fs::write(
                root.join("src").join(format!("module_{index}.rs")),
                vec![source_byte; 1_200 + index],
            )
            .unwrap();
        }
        fs::write(root.join("hero.png"), vec![source_byte; 1_400]).unwrap();
        fs::write(root.join("model.gguf"), vec![source_byte; 1_500]).unwrap();
    }

    fn project_summaries_for(db: &Db, project_ids: &[i64]) -> Vec<ProjectSummary> {
        let wanted = project_ids.iter().copied().collect::<HashSet<_>>();
        let mut projects = db
            .projects_list()
            .unwrap()
            .into_iter()
            .filter(|project| wanted.contains(&project.id))
            .collect::<Vec<_>>();
        projects.sort_by_key(|project| project.id);
        projects
    }

    #[test]
    fn prompt_is_optional_and_suppression_is_persistent() {
        let db = Db::open_memory().unwrap();
        let initial = db.safe_manage_first_run_preference().unwrap();
        assert!(initial.suggest_after_discovery);
        assert_eq!(initial.prompt_state, "pending");

        let suppressed = db
            .safe_manage_first_run_preference_set(false, "suppressed", true)
            .unwrap();
        assert!(!suppressed.suggest_after_discovery);
        assert_eq!(suppressed.prompt_state, "suppressed");
        assert!(suppressed.last_prompted_at.is_some());
    }

    #[test]
    fn objective_input_keeps_unavailable_session_and_git_state_unknown() {
        let db = Db::open_memory().unwrap();
        let projects = db.projects_list().unwrap();
        let inputs = db
            .safe_manage_objective_inputs(&projects, &HashMap::new())
            .unwrap();
        assert!(!inputs.is_empty());
        assert!(inputs.iter().all(|input| input.session_count.is_none()));
        assert!(inputs
            .iter()
            .filter(|input| input.has_git)
            .all(|input| input.git_uncommitted.is_none()));
    }

    #[test]
    fn completed_run_round_trips_assessments_and_decision_binding() {
        let db = Db::open_memory().unwrap();
        let projects = db.projects_list().unwrap();
        let mut counts = HashMap::new();
        for project in &projects {
            counts.insert(project.id, 0);
        }
        let inputs = db.safe_manage_objective_inputs(&projects, &counts).unwrap();
        let mut queued = empty_run("run-1", "queued", inputs.len() as u64);
        db.safe_manage_analysis_save(&queued).unwrap();
        queued.state = "running".to_string();
        queued.started_at = Some("2026-08-27T10:00:01Z".to_string());
        queued.message = "Running".to_string();
        db.safe_manage_analysis_save(&queued).unwrap();

        let assessments = inputs
            .iter()
            .map(|input| {
                assess_safe_manage_project(
                    input,
                    &SafeManageClassificationContext {
                        now_ms: 1_800_000_000_000,
                        analysis_run_id: "run-1".to_string(),
                        evidence_revision: format!("rev-{}", input.project_id),
                        observed_at: Some("2026-08-27T10:00:01Z".to_string()),
                    },
                )
            })
            .collect::<Vec<_>>();
        let complete = SafeManageAnalysisRun {
            state: "completed".to_string(),
            completed_at: Some("2026-08-27T10:00:02Z".to_string()),
            processed_projects: assessments.len() as u64,
            counts: safe_manage_portfolio_counts(&assessments),
            message: "Complete".to_string(),
            assessments,
            ..queued
        };
        db.safe_manage_analysis_save(&complete).unwrap();
        let loaded = db.safe_manage_analysis_latest().unwrap().unwrap();
        assert_eq!(loaded, complete);

        let assessment = &loaded.assessments[0];
        let decision = db
            .safe_manage_decision_record(
                assessment.project_id,
                &loaded.id,
                SafeManageDecisionKind::Keep,
                &assessment.evidence_revision,
            )
            .unwrap();
        assert!(!decision.evidence_stale);
        assert_eq!(decision.decided_by, "local_user");
        assert_eq!(db.safe_manage_decisions_latest().unwrap().len(), 1);
    }

    #[test]
    fn grouped_decisions_commit_once_for_one_completed_run() {
        let db = Db::open_memory().unwrap();
        let projects = db
            .projects_list()
            .unwrap()
            .into_iter()
            .take(2)
            .collect::<Vec<_>>();
        let assessments = assessments_for(
            &db,
            "group-ok",
            &projects,
            &[
                SafeManageRecommendation::Keep,
                SafeManageRecommendation::Review,
            ],
        );
        save_completed_run(&db, "group-ok", assessments.clone());
        let requests = assessments
            .iter()
            .map(|assessment| SafeManageDecisionRequest {
                project_id: assessment.project_id,
                analysis_run_id: "group-ok".to_string(),
                decision: SafeManageDecisionKind::Keep,
                evidence_revision: assessment.evidence_revision.clone(),
            })
            .collect::<Vec<_>>();

        let recorded = db.safe_manage_decisions_record_atomic(&requests).unwrap();
        assert_eq!(recorded.len(), 2);
        assert_eq!(db.safe_manage_decisions_latest().unwrap().len(), 2);
    }

    #[test]
    fn grouped_decisions_reject_ineligible_stale_duplicate_and_mixed_without_partial_writes() {
        let make = |run_id: &str, recommendations: &[SafeManageRecommendation]| {
            let db = Db::open_memory().unwrap();
            let projects = db
                .projects_list()
                .unwrap()
                .into_iter()
                .take(2)
                .collect::<Vec<_>>();
            let assessments = assessments_for(&db, run_id, &projects, recommendations);
            save_completed_run(&db, run_id, assessments.clone());
            (db, assessments)
        };

        let (db, assessments) = make(
            "group-protected",
            &[
                SafeManageRecommendation::Keep,
                SafeManageRecommendation::DoNotTouch,
            ],
        );
        let ineligible = assessments
            .iter()
            .map(|assessment| SafeManageDecisionRequest {
                project_id: assessment.project_id,
                analysis_run_id: "group-protected".to_string(),
                decision: SafeManageDecisionKind::CleanRegenerables,
                evidence_revision: assessment.evidence_revision.clone(),
            })
            .collect::<Vec<_>>();
        assert!(db.safe_manage_decisions_record_atomic(&ineligible).is_err());
        assert!(db.safe_manage_decisions_latest().unwrap().is_empty());

        let (db, assessments) = make(
            "group-stale",
            &[
                SafeManageRecommendation::Keep,
                SafeManageRecommendation::Review,
            ],
        );
        let mut stale = assessments
            .iter()
            .map(|assessment| SafeManageDecisionRequest {
                project_id: assessment.project_id,
                analysis_run_id: "group-stale".to_string(),
                decision: SafeManageDecisionKind::Keep,
                evidence_revision: assessment.evidence_revision.clone(),
            })
            .collect::<Vec<_>>();
        stale[1].evidence_revision = "forged-stale-revision".to_string();
        assert!(db.safe_manage_decisions_record_atomic(&stale).is_err());
        assert!(db.safe_manage_decisions_latest().unwrap().is_empty());

        let (db, assessments) = make(
            "group-duplicate",
            &[
                SafeManageRecommendation::Keep,
                SafeManageRecommendation::Review,
            ],
        );
        let duplicate = vec![
            SafeManageDecisionRequest {
                project_id: assessments[0].project_id,
                analysis_run_id: "group-duplicate".to_string(),
                decision: SafeManageDecisionKind::Keep,
                evidence_revision: assessments[0].evidence_revision.clone(),
            },
            SafeManageDecisionRequest {
                project_id: assessments[0].project_id,
                analysis_run_id: "group-duplicate".to_string(),
                decision: SafeManageDecisionKind::Ignore,
                evidence_revision: assessments[0].evidence_revision.clone(),
            },
        ];
        assert!(db.safe_manage_decisions_record_atomic(&duplicate).is_err());
        assert!(db.safe_manage_decisions_latest().unwrap().is_empty());

        let (db, assessments) = make(
            "group-mixed-a",
            &[
                SafeManageRecommendation::Keep,
                SafeManageRecommendation::Review,
            ],
        );
        let mixed = vec![
            SafeManageDecisionRequest {
                project_id: assessments[0].project_id,
                analysis_run_id: "group-mixed-a".to_string(),
                decision: SafeManageDecisionKind::Keep,
                evidence_revision: assessments[0].evidence_revision.clone(),
            },
            SafeManageDecisionRequest {
                project_id: assessments[1].project_id,
                analysis_run_id: "another-run".to_string(),
                decision: SafeManageDecisionKind::Keep,
                evidence_revision: assessments[1].evidence_revision.clone(),
            },
        ];
        assert!(db.safe_manage_decisions_record_atomic(&mixed).is_err());
        assert!(db.safe_manage_decisions_latest().unwrap().is_empty());
    }

    #[test]
    fn regenerable_targets_are_narrow_project_bound_and_reject_forged_identity() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("project");
        fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        fs::create_dir_all(root.join("vendor/lib")).unwrap();
        fs::create_dir_all(root.join(".git/objects")).unwrap();
        fs::create_dir_all(root.join("shared/node_modules/pkg")).unwrap();
        fs::write(root.join("node_modules/pkg/index.js"), "derived").unwrap();
        fs::write(root.join("vendor/lib/source.rs"), "source").unwrap();
        fs::write(root.join(".git/objects/one"), "git").unwrap();
        fs::write(root.join("shared/node_modules/pkg/index.js"), "shared").unwrap();

        let db = Db::open(directory.path().join("catalog.db")).unwrap();
        let project_id = load_normal_inventory(&db, &root);
        let project = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|project| project.id == project_id)
            .unwrap();
        let assessments = assessments_for(
            &db,
            "regen-targets",
            std::slice::from_ref(&project),
            &[SafeManageRecommendation::CleanRegenerables],
        );
        save_completed_run(&db, "regen-targets", assessments.clone());
        let revision = assessments[0].evidence_revision.clone();

        let targets = db
            .safe_manage_regenerable_targets(project_id, "regen-targets", &revision)
            .unwrap();
        assert_eq!(
            targets.len(),
            1,
            "only node_modules is eligible: {targets:?}"
        );
        assert_eq!(targets[0].path, "node_modules");
        assert_eq!(targets[0].evidence_state, "opaque_measured");
        assert!(!targets[0].operation_plan_eligible);
        assert!(targets[0].bytes.is_some_and(|bytes| bytes > 0));

        db.safe_manage_decision_record(
            project_id,
            "regen-targets",
            SafeManageDecisionKind::CleanRegenerables,
            &revision,
        )
        .unwrap();
        let exact = SafeManageRegenerableScanRequest {
            project_id,
            analysis_run_id: "regen-targets".to_string(),
            evidence_revision: revision.clone(),
            nav_id: targets[0].nav_id,
            node_id: targets[0].node_id,
            path: targets[0].path.clone(),
        };
        assert!(db.safe_manage_regenerable_scan_target(&exact).is_ok());

        let mut forged = exact.clone();
        forged.node_id += 1;
        assert!(db.safe_manage_regenerable_scan_target(&forged).is_err());
        forged = exact.clone();
        forged.nav_id += 1;
        assert!(db.safe_manage_regenerable_scan_target(&forged).is_err());
        forged = exact;
        forged.path = ".git".to_string();
        assert!(db.safe_manage_regenerable_scan_target(&forged).is_err());
        assert!(db
            .safe_manage_regenerable_targets(
                project_id,
                "regen-targets",
                "stale-or-forged-revision",
            )
            .is_err());

        // Only the latest exact decision can open the scan boundary. A later
        // Keep decision supersedes the earlier Clean decision.
        db.safe_manage_decision_record(
            project_id,
            "regen-targets",
            SafeManageDecisionKind::Keep,
            &revision,
        )
        .unwrap();
        let superseded = SafeManageRegenerableScanRequest {
            project_id,
            analysis_run_id: "regen-targets".to_string(),
            evidence_revision: revision,
            nav_id: targets[0].nav_id,
            node_id: targets[0].node_id,
            path: targets[0].path.clone(),
        };
        assert!(db.safe_manage_regenerable_scan_target(&superseded).is_err());
    }

    #[test]
    fn regenerable_target_overlapping_another_registered_project_is_excluded() {
        let directory = tempdir().unwrap();
        let outer = directory.path().join("outer");
        let inner = outer.join("node_modules/owned-project");
        fs::create_dir_all(&inner).unwrap();
        fs::write(inner.join("README.md"), "owned elsewhere").unwrap();

        let db = Db::open_memory().unwrap();
        let outer_id = load_normal_inventory(&db, &outer);
        load_normal_inventory(&db, &inner);
        let project = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|project| project.id == outer_id)
            .unwrap();
        let assessments = assessments_for(
            &db,
            "regen-overlap",
            std::slice::from_ref(&project),
            &[SafeManageRecommendation::CleanRegenerables],
        );
        save_completed_run(&db, "regen-overlap", assessments.clone());
        let targets = db
            .safe_manage_regenerable_targets(
                outer_id,
                "regen-overlap",
                &assessments[0].evidence_revision,
            )
            .unwrap();
        assert!(targets.is_empty());
    }

    #[test]
    fn explicit_expansion_receipt_enables_exact_target_without_double_counting() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("project");
        fs::create_dir_all(root.join("node_modules/pkg/node_modules/child")).unwrap();
        fs::write(root.join("node_modules/pkg/index.js"), [1_u8; 13]).unwrap();
        fs::write(
            root.join("node_modules/pkg/node_modules/child/index.js"),
            [2_u8; 17],
        )
        .unwrap();

        let db = Db::open(directory.path().join("expansion-catalog.db")).unwrap();
        let project_id = load_normal_inventory(&db, &root);
        let project = db
            .projects_list()
            .unwrap()
            .into_iter()
            .find(|project| project.id == project_id)
            .unwrap();
        let assessments = assessments_for(
            &db,
            "regen-expand",
            std::slice::from_ref(&project),
            &[SafeManageRecommendation::CleanRegenerables],
        );
        save_completed_run(&db, "regen-expand", assessments.clone());
        let revision = assessments[0].evidence_revision.clone();
        let mut counts = HashMap::new();
        counts.insert(project_id, 0);
        let stable_catalog_epoch = db
            .safe_manage_objective_inputs(std::slice::from_ref(&project), &counts)
            .unwrap()
            .remove(0)
            .catalog_evidence_epoch;
        let target = db
            .safe_manage_regenerable_targets(project_id, "regen-expand", &revision)
            .unwrap()
            .remove(0);
        db.safe_manage_decision_record(
            project_id,
            "regen-expand",
            SafeManageDecisionKind::CleanRegenerables,
            &revision,
        )
        .unwrap();
        let request = SafeManageRegenerableScanRequest {
            project_id,
            analysis_run_id: "regen-expand".to_string(),
            evidence_revision: revision.clone(),
            nav_id: target.nav_id,
            node_id: target.node_id,
            path: target.path.clone(),
        };
        let operation_request = SafeManageOperationPlanRequest {
            project_id,
            analysis_run_id: "regen-expand".to_string(),
            evidence_revision: revision.clone(),
            decision: SafeManageDecisionKind::CleanRegenerables,
            target: Some(SafeManageOperationTargetIdentity {
                nav_id: target.nav_id,
                node_id: target.node_id,
                path: target.path.clone(),
            }),
        };
        assert!(db
            .safe_manage_operation_plan_target(&operation_request)
            .is_err());
        let (subtree, _) = db.safe_manage_regenerable_scan_target(&request).unwrap();
        let mut writer = db.open_write_session().unwrap();
        writer
            .begin_safe_manage_regenerable_scan(project_id, target.nav_id)
            .unwrap();
        writer
            .safe_manage_regenerable_expansion_begin(&request)
            .unwrap();
        let summary = hangar_fs::scan_regenerable_inventory_stream(
            &root,
            &target.path,
            hangar_fs::ScanLimits::regenerable_expansion(),
            || false,
            |_, _, _| {},
            |batch| {
                writer
                    .persist_batch(project_id, &batch)
                    .map_err(|error| error.to_string())?;
                Ok(())
            },
        )
        .unwrap();
        assert!(!summary.partial);
        writer
            .finish_subtree_scan(project_id, subtree.nav_id, None)
            .unwrap();
        writer
            .safe_manage_regenerable_expansion_finish(
                &request,
                "completed",
                summary.scanned_files,
                None,
            )
            .unwrap();
        drop(writer);

        let expanded = db
            .safe_manage_regenerable_targets(project_id, "regen-expand", &revision)
            .unwrap();
        assert_eq!(expanded.len(), 1);
        assert_eq!(expanded[0].evidence_state, "expanded_complete");
        assert!(expanded[0].operation_plan_eligible);
        assert_eq!(
            db.safe_manage_operation_plan_target(&operation_request)
                .unwrap(),
            target.node_id
        );
        let mut forged_operation = operation_request.clone();
        forged_operation.target.as_mut().unwrap().node_id += 1;
        assert!(db
            .safe_manage_operation_plan_target(&forged_operation)
            .is_err());
        forged_operation = operation_request.clone();
        forged_operation.target.as_mut().unwrap().path = "vendor".to_string();
        assert!(db
            .safe_manage_operation_plan_target(&forged_operation)
            .is_err());
        let input = db
            .safe_manage_objective_inputs(std::slice::from_ref(&project), &counts)
            .unwrap()
            .remove(0);
        assert_eq!(input.catalog_evidence_epoch, stable_catalog_epoch);
        assert_eq!(input.regenerable_bytes, expanded[0].bytes);

        // A complete receipt transplanted from another evidence revision is
        // not reusable even when all target bytes and ids still match.
        db.with_writer(|conn| {
            conn.execute(
                "UPDATE safe_manage_regenerable_expansion
                 SET evidence_revision = 'substituted-revision'
                 WHERE project_id = ?1 AND nav_id = ?2",
                params![project_id, target.nav_id],
            )?;
            Ok(())
        })
        .unwrap();
        let substituted = db
            .safe_manage_regenerable_targets(project_id, "regen-expand", &revision)
            .unwrap();
        assert_eq!(substituted[0].evidence_state, "expanded_partial");
        assert!(!substituted[0].operation_plan_eligible);
        assert!(db
            .safe_manage_operation_plan_target(&operation_request)
            .is_err());
        db.with_writer(|conn| {
            conn.execute(
                "UPDATE safe_manage_regenerable_expansion
                 SET evidence_revision = ?3
                 WHERE project_id = ?1 AND nav_id = ?2",
                params![project_id, target.nav_id, revision],
            )?;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            db.safe_manage_operation_plan_target(&operation_request)
                .unwrap(),
            target.node_id
        );

        // A corrupt/substituted receipt cannot keep the target plan-eligible,
        // even when the nav/node/path tuple itself still exists.
        db.with_writer(|conn| {
            conn.execute(
                "UPDATE safe_manage_regenerable_expansion
                 SET item_count = item_count + 1
                 WHERE project_id = ?1 AND nav_id = ?2",
                params![project_id, target.nav_id],
            )?;
            Ok(())
        })
        .unwrap();
        let corrupt = db
            .safe_manage_regenerable_targets(project_id, "regen-expand", &revision)
            .unwrap();
        assert_eq!(corrupt[0].evidence_state, "expanded_partial");
        assert!(!corrupt[0].operation_plan_eligible);
        assert!(db
            .safe_manage_operation_plan_target(&operation_request)
            .is_err());
    }

    #[test]
    fn operation_plan_requires_latest_exact_disk_intent_and_project_binding() {
        let db = Db::open_memory().unwrap();
        let project = db.projects_list().unwrap().remove(0);
        let assessments = assessments_for(
            &db,
            "operation-binding",
            std::slice::from_ref(&project),
            &[SafeManageRecommendation::Review],
        );
        save_completed_run(&db, "operation-binding", assessments.clone());
        let revision = assessments[0].evidence_revision.clone();
        db.safe_manage_decision_record(
            project.id,
            "operation-binding",
            SafeManageDecisionKind::Archive,
            &revision,
        )
        .unwrap();
        let archive = SafeManageOperationPlanRequest {
            project_id: project.id,
            analysis_run_id: "operation-binding".to_string(),
            evidence_revision: revision.clone(),
            decision: SafeManageDecisionKind::Archive,
            target: None,
        };
        assert_eq!(
            db.safe_manage_operation_plan_target(&archive).unwrap(),
            project.id
        );

        let mut wrong_kind = archive.clone();
        wrong_kind.decision = SafeManageDecisionKind::PrepareRemoval;
        assert!(db.safe_manage_operation_plan_target(&wrong_kind).is_err());
        let mut stale = archive.clone();
        stale.evidence_revision = "stale-revision".to_string();
        assert!(db.safe_manage_operation_plan_target(&stale).is_err());
        let mut forged_target = archive.clone();
        forged_target.target = Some(SafeManageOperationTargetIdentity {
            nav_id: 1,
            node_id: project.id,
            path: "node_modules".to_string(),
        });
        assert!(db
            .safe_manage_operation_plan_target(&forged_target)
            .is_err());

        db.safe_manage_decision_record(
            project.id,
            "operation-binding",
            SafeManageDecisionKind::PrepareRemoval,
            &revision,
        )
        .unwrap();
        assert!(db.safe_manage_operation_plan_target(&archive).is_err());
        wrong_kind.decision = SafeManageDecisionKind::PrepareRemoval;
        assert_eq!(
            db.safe_manage_operation_plan_target(&wrong_kind).unwrap(),
            project.id
        );
        wrong_kind.decision = SafeManageDecisionKind::Keep;
        assert!(db.safe_manage_operation_plan_target(&wrong_kind).is_err());
    }

    #[test]
    fn complete_run_cannot_omit_projects() {
        let db = Db::open_memory().unwrap();
        let mut run = empty_run("run-bad", "completed", 1);
        run.completed_at = Some("2026-08-27T10:00:02Z".to_string());
        run.processed_projects = 1;
        run.counts.total = 1;
        assert!(db.safe_manage_analysis_save(&run).is_err());
    }

    #[test]
    fn append_only_analysis_rejects_duplicate_bad_counts_and_terminal_append() {
        let db = Db::open_memory().unwrap();
        let projects = db
            .projects_list()
            .unwrap()
            .into_iter()
            .take(2)
            .collect::<Vec<_>>();
        assert_eq!(projects.len(), 2);
        let assessments = assessments_for(
            &db,
            "append-linear",
            &projects,
            &[
                SafeManageRecommendation::Keep,
                SafeManageRecommendation::Review,
            ],
        );
        let mut run = empty_run("append-linear", "queued", 2);
        db.safe_manage_analysis_header_save(&run).unwrap();
        run.state = "running".to_string();
        run.started_at = Some("2026-08-27T10:00:01Z".to_string());
        db.safe_manage_analysis_header_save(&run).unwrap();

        hangar_core::safe_manage_portfolio_counts_include(&mut run.counts, &assessments[0]);
        run.assessments.push(assessments[0].clone());
        run.processed_projects = 1;
        db.safe_manage_analysis_assessment_append(&run, &assessments[0])
            .unwrap();
        assert!(db
            .safe_manage_analysis_assessment_append(&run, &assessments[0])
            .is_err());
        let after_duplicate = db
            .safe_manage_analysis_get("append-linear")
            .unwrap()
            .unwrap();
        assert_eq!(after_duplicate.assessments, vec![assessments[0].clone()]);
        assert_eq!(after_duplicate.processed_projects, 1);

        run.assessments.push(assessments[1].clone());
        run.processed_projects = 2;
        assert!(db
            .safe_manage_analysis_assessment_append(&run, &assessments[1])
            .is_err());
        assert_eq!(
            db.safe_manage_analysis_get("append-linear")
                .unwrap()
                .unwrap()
                .assessments
                .len(),
            1
        );

        hangar_core::safe_manage_portfolio_counts_include(&mut run.counts, &assessments[1]);
        db.safe_manage_analysis_assessment_append(&run, &assessments[1])
            .unwrap();
        run.state = "completed".to_string();
        run.completed_at = Some("2026-08-27T10:00:02Z".to_string());
        db.safe_manage_analysis_finalize(&run).unwrap();
        assert!(db
            .safe_manage_analysis_assessment_append(&run, &assessments[1])
            .is_err());
        let complete = db
            .safe_manage_analysis_get("append-linear")
            .unwrap()
            .unwrap();
        assert_eq!(complete.state, "completed");
        assert_eq!(complete.assessments, assessments);
    }

    #[test]
    fn similar_version_names_are_related_without_claiming_byte_identity() {
        assert_eq!(similar_project_key("Orbit v2"), "orbit");
        assert_eq!(similar_project_key("Orbit-backup"), "orbit");
        let projects = vec![
            ProjectSummary {
                id: 10,
                name: "Orbit v1".to_string(),
                path: r"C:\work\orbit-v1".to_string(),
                source: "test".to_string(),
                context_count: 0,
                pinned: false,
                protected_level: None,
                scan_state: "scanned".to_string(),
                scan_root_id: None,
                antigravity_name: None,
                is_current: false,
                app: None,
                apps: Vec::new(),
            },
            ProjectSummary {
                id: 11,
                name: "Orbit backup".to_string(),
                path: r"C:\work\orbit-backup".to_string(),
                source: "test".to_string(),
                context_count: 0,
                pinned: false,
                protected_level: None,
                scan_state: "scanned".to_string(),
                scan_root_id: None,
                antigravity_name: None,
                is_current: false,
                app: None,
                apps: Vec::new(),
            },
        ];
        let related = SimilarProjectGroups::from_projects(&projects);
        assert_eq!(related.related(10), (1, vec![11]));
        assert_eq!(related.related(11), (1, vec![10]));
    }

    #[test]
    fn exact_material_groups_keep_exact_counts_but_cap_related_ids() {
        let ids = (1_i64..=100).collect::<Vec<_>>();
        let mut counts = HashMap::new();
        let mut related = HashMap::new();
        record_bounded_exact_material_group(&ids, &mut counts, &mut related, None).unwrap();

        assert_eq!(counts.len(), 100);
        assert_eq!(counts.get(&50), Some(&99));
        assert_eq!(related.get(&50).unwrap().len(), 64);
        assert!(!related.get(&50).unwrap().contains(&50));
    }

    #[test]
    fn copy_hints_require_distinct_projects_and_physical_identities() {
        let same_identity = BTreeMap::from([(
            ("readme.md".to_string(), 2_048),
            vec![
                ComparisonMember {
                    project_id: 1,
                    file_key: "one".to_string(),
                    identity_key: "inode:volume:7".to_string(),
                },
                ComparisonMember {
                    project_id: 2,
                    file_key: "two".to_string(),
                    identity_key: "inode:volume:7".to_string(),
                },
            ],
        )]);
        let (files, relations) = cross_project_group_evidence(same_identity);
        assert!(files.is_empty());
        assert!(relations.is_empty());

        let distinct_identity = BTreeMap::from([(
            ("readme.md".to_string(), 2_048),
            vec![
                ComparisonMember {
                    project_id: 1,
                    file_key: "one".to_string(),
                    identity_key: "inode:volume:7".to_string(),
                },
                ComparisonMember {
                    project_id: 2,
                    file_key: "two".to_string(),
                    identity_key: "inode:volume:8".to_string(),
                },
            ],
        )]);
        let (files, relations) = cross_project_group_evidence(distinct_identity);
        assert_eq!(files.get(&1).map(BTreeSet::len), Some(1));
        assert_eq!(relations.get(&1), Some(&BTreeSet::from([2])));
    }

    #[test]
    fn wide_copy_groups_bound_relationship_memory_and_revision_inputs() {
        const PROJECTS: i64 = 4_096;
        let members = (1_i64..=PROJECTS)
            .map(|project_id| ComparisonMember {
                project_id,
                file_key: format!("file-{project_id}"),
                identity_key: format!("inode:volume:{project_id}"),
            })
            .collect::<Vec<_>>();
        let groups = BTreeMap::from([(("readme.md".to_string(), 2_048), members)]);

        let (files, relations) = cross_project_group_evidence(groups);

        assert_eq!(files.len(), PROJECTS as usize);
        assert_eq!(relations.len(), PROJECTS as usize);
        assert!(relations
            .values()
            .all(|ids| ids.len() <= SAFE_MANAGE_RELATED_PROJECT_IDS_MAX));
        assert_eq!(relations.get(&1).unwrap().len(), 64);
        assert_eq!(relations.get(&PROJECTS).unwrap().len(), 64);
        assert_eq!(
            bounded_related_project_count_label(relations.get(&1).unwrap()),
            "At least 64"
        );
        drop(files);
        drop(relations);

        let exact_ids = (1_i64..=PROJECTS).collect::<Vec<_>>();
        let mut exact_counts = HashMap::new();
        let mut exact_related = HashMap::new();
        record_bounded_exact_material_group(
            &exact_ids,
            &mut exact_counts,
            &mut exact_related,
            None,
        )
        .unwrap();
        assert_eq!(exact_counts.get(&1), Some(&(PROJECTS as u64 - 1)));
        assert!(exact_related
            .values()
            .all(|ids| ids.len() <= SAFE_MANAGE_RELATED_PROJECT_IDS_MAX));
        drop(exact_related);

        let profile = SafeManageFileKindProfile {
            coverage: SafeManageEvidenceCoverage::Partial,
            inspected_file_count: 1,
            counts: Vec::new(),
        };
        let duplicate = SafeManageDuplicateEvidence::default();
        let bounded = (1_i64..=64).collect::<BTreeSet<_>>();
        let oversized = (1_i64..=PROJECTS).collect::<BTreeSet<_>>();
        let bounded_revision = comparison_evidence_revision(
            &profile,
            &duplicate,
            Some(PROJECTS as u64 - 1),
            &bounded,
            &bounded,
            &bounded,
        )
        .unwrap();
        let oversized_revision = comparison_evidence_revision(
            &profile,
            &duplicate,
            Some(PROJECTS as u64 - 1),
            &oversized,
            &oversized,
            &oversized,
        )
        .unwrap();
        assert_eq!(oversized_revision, bounded_revision);
    }

    #[test]
    fn comparison_aggregation_honors_cancellation_before_allocating_groups() {
        let groups = BTreeMap::from([(
            ("readme.md".to_string(), 2_048),
            vec![
                ComparisonMember {
                    project_id: 1,
                    file_key: "one".to_string(),
                    identity_key: "inode:volume:1".to_string(),
                },
                ComparisonMember {
                    project_id: 2,
                    file_key: "two".to_string(),
                    identity_key: "inode:volume:2".to_string(),
                },
            ],
        )]);
        let cancel = AtomicBool::new(true);

        let error = cross_project_group_evidence_interruptible(groups, Some(&cancel)).unwrap_err();

        assert!(error.to_string().to_ascii_lowercase().contains("cancel"));
    }

    #[test]
    fn bounded_profiles_report_positive_kinds_and_conservative_copy_evidence() {
        let directory = tempdir().unwrap();
        let first_root = directory.path().join("Orbit v1");
        let second_root = directory.path().join("Orbit backup");
        write_comparable_project(&first_root, b'a');
        write_comparable_project(&second_root, b'b');

        let db = Db::open(directory.path().join("profiles.db")).unwrap();
        let first_id = load_normal_inventory(&db, &first_root);
        let second_id = load_normal_inventory(&db, &second_root);
        let projects = project_summaries_for(&db, &[first_id, second_id]);
        let session_counts = projects
            .iter()
            .map(|project| (project.id, 0_u64))
            .collect::<HashMap<_, _>>();
        let inputs = db
            .safe_manage_objective_inputs(&projects, &session_counts)
            .unwrap();

        assert_eq!(inputs.len(), 2);
        for input in inputs {
            assert_eq!(
                input.file_kind_profile.coverage,
                SafeManageEvidenceCoverage::Complete
            );
            assert!(input.file_kind_profile.inspected_file_count >= 10);
            assert!(input
                .file_kind_profile
                .counts
                .iter()
                .all(|count| count.file_count > 0));
            let kinds = input
                .file_kind_profile
                .counts
                .iter()
                .map(|count| count.kind.as_str())
                .collect::<HashSet<_>>();
            assert!(kinds.contains("manifest_config"));
            assert!(kinds.contains("documentation"));
            assert!(kinds.contains("source"));
            assert!(kinds.contains("model"));
            assert!(kinds.contains("media"));

            assert_eq!(
                input.duplicate_evidence.coverage,
                SafeManageEvidenceCoverage::Complete
            );
            assert!(input.duplicate_evidence.possible_copy_file_count >= 8);
            assert!(input.duplicate_evidence.indexed_text_file_count >= 1);
            assert!(input.duplicate_evidence.confirmed_indexed_text_copy_count >= 1);
            assert_eq!(input.materially_similar_project_count, Some(1));
            assert_eq!(input.comparison_evidence_revision.len(), 64);
            assert!(input.risk_relations.iter().any(|relation| {
                relation.kind == "possible_file_copies"
                    && relation.confidence == SafeManageConfidence::Low
            }));
            assert!(input
                .risk_relations
                .iter()
                .any(|relation| relation.kind == "indexed_text_duplicates"));
            assert!(input
                .risk_relations
                .iter()
                .any(|relation| relation.kind == "materially_similar_inventory"));
        }
    }

    #[test]
    fn bounded_profile_limits_expose_partial_and_unavailable_coverage() {
        let directory = tempdir().unwrap();
        let first_root = directory.path().join("Bounded first");
        let second_root = directory.path().join("Bounded second");
        write_comparable_project(&first_root, b'a');
        write_comparable_project(&second_root, b'b');
        let db = Db::open(directory.path().join("bounded.db")).unwrap();
        let first_id = load_normal_inventory(&db, &first_root);
        let second_id = load_normal_inventory(&db, &second_root);
        let projects = project_summaries_for(&db, &[first_id, second_id]);

        let partial = db
            .with_read_conn(|conn| {
                let mut aggregates = HashMap::new();
                for project in &projects {
                    aggregates.insert(project.id, load_project_aggregates(conn, project.id)?);
                }
                let groups = SimilarProjectGroups::from_projects(&projects);
                load_bounded_portfolio_comparison_with_limits(
                    conn,
                    &projects,
                    &aggregates,
                    &groups,
                    2,
                    4,
                )
            })
            .unwrap();
        for evidence in partial.values() {
            assert_eq!(
                evidence.file_kind_profile.coverage,
                SafeManageEvidenceCoverage::Partial
            );
            assert_eq!(evidence.file_kind_profile.inspected_file_count, 2);
            assert!(evidence
                .file_kind_profile
                .counts
                .iter()
                .all(|count| count.file_count > 0));
            assert_eq!(
                evidence.duplicate_evidence.coverage,
                SafeManageEvidenceCoverage::Partial
            );
            assert_eq!(evidence.materially_similar_project_count, None);
        }

        let unavailable = db
            .with_read_conn(|conn| {
                let mut aggregates = HashMap::new();
                for project in &projects {
                    aggregates.insert(project.id, load_project_aggregates(conn, project.id)?);
                }
                let groups = SimilarProjectGroups::from_projects(&projects);
                load_bounded_portfolio_comparison_with_limits(
                    conn,
                    &projects,
                    &aggregates,
                    &groups,
                    0,
                    0,
                )
            })
            .unwrap();
        for evidence in unavailable.values() {
            assert_eq!(
                evidence.file_kind_profile.coverage,
                SafeManageEvidenceCoverage::Unavailable
            );
            assert!(evidence.file_kind_profile.counts.is_empty());
            assert_eq!(
                evidence.duplicate_evidence.coverage,
                SafeManageEvidenceCoverage::Unavailable
            );
            assert_eq!(evidence.materially_similar_project_count, None);
        }
    }

    #[test]
    fn collapsed_generated_descendants_do_not_change_comparison_revision() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("Stable profile");
        write_comparable_project(&root, b'a');
        let db = Db::open(directory.path().join("stable.db")).unwrap();
        let project_id = load_normal_inventory(&db, &root);
        let projects = project_summaries_for(&db, &[project_id]);
        let counts = HashMap::from([(project_id, 0_u64)]);
        let before = db
            .safe_manage_objective_inputs(&projects, &counts)
            .unwrap()
            .remove(0);

        db.with_writer(|conn| {
            conn.execute(
                "INSERT INTO node(
                   kind, path, name, size_apparent, first_seen_at, last_seen_at, present
                 ) VALUES('file', ?1, 'generated.js', 4096, ?2, ?2, 1)",
                params![
                    root.join("node_modules/pkg/generated.js")
                        .to_string_lossy()
                        .into_owned(),
                    now()
                ],
            )?;
            let node_id = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO nav_item(
                   project_id, node_id, path, display_name, item_kind, sort_key,
                   fully_scanned, collapse_default, scan_error
                 ) VALUES(?1, ?2, 'node_modules/pkg/generated.js', 'generated.js',
                          'file', 'generated.js', 0, 1,
                          'Regenerable expansion stopped before completion.')",
                params![project_id, node_id],
            )?;
            Ok(())
        })
        .unwrap();

        let after = db
            .safe_manage_objective_inputs(&projects, &counts)
            .unwrap()
            .remove(0);
        assert_eq!(after.file_kind_profile, before.file_kind_profile);
        assert_eq!(after.duplicate_evidence, before.duplicate_evidence);
        assert_eq!(
            after.materially_similar_project_count,
            before.materially_similar_project_count
        );
        assert_eq!(
            after.comparison_evidence_revision,
            before.comparison_evidence_revision
        );
    }

    #[test]
    fn reopening_marks_an_interrupted_run_partial_without_replacing_complete_results() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("catalog.db");
        {
            let db = Db::open(&path).unwrap();
            let project = db.projects_list().unwrap().remove(0);
            let assessment = assessments_for(
                &db,
                "interrupted",
                std::slice::from_ref(&project),
                &[SafeManageRecommendation::Review],
            )
            .remove(0);
            let mut run = empty_run("interrupted", "queued", 2);
            db.safe_manage_analysis_header_save(&run).unwrap();
            run.state = "running".to_string();
            run.started_at = Some("2026-08-27T10:00:01Z".to_string());
            db.safe_manage_analysis_header_save(&run).unwrap();
            hangar_core::safe_manage_portfolio_counts_include(&mut run.counts, &assessment);
            run.assessments.push(assessment.clone());
            run.processed_projects = 1;
            db.safe_manage_analysis_assessment_append(&run, &assessment)
                .unwrap();
        }

        let reopened = Db::open(&path).unwrap();
        let recovered = reopened.safe_manage_analysis_latest().unwrap().unwrap();
        assert_eq!(recovered.state, "partial");
        assert_eq!(recovered.processed_projects, 1);
        assert_eq!(recovered.assessments.len(), 1);
        assert!(recovered.completed_at.is_some());
        assert!(recovered.error.is_some());
        assert!(reopened
            .safe_manage_analysis_latest_complete()
            .unwrap()
            .is_none());
    }

    #[test]
    fn reopening_marks_interrupted_regenerable_expansion_failed_and_ineligible() {
        let directory = tempdir().unwrap();
        let catalog_path = directory.path().join("interrupted-expansion.db");
        let root = directory.path().join("project");
        fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        fs::write(root.join("node_modules/pkg/index.js"), "derived").unwrap();

        let (project_id, revision, target) = {
            let db = Db::open(&catalog_path).unwrap();
            let project_id = load_normal_inventory(&db, &root);
            let project = db
                .projects_list()
                .unwrap()
                .into_iter()
                .find(|project| project.id == project_id)
                .unwrap();
            let assessments = assessments_for(
                &db,
                "interrupted-expansion",
                std::slice::from_ref(&project),
                &[SafeManageRecommendation::CleanRegenerables],
            );
            save_completed_run(&db, "interrupted-expansion", assessments.clone());
            let revision = assessments[0].evidence_revision.clone();
            db.safe_manage_decision_record(
                project_id,
                "interrupted-expansion",
                SafeManageDecisionKind::CleanRegenerables,
                &revision,
            )
            .unwrap();
            let target = db
                .safe_manage_regenerable_targets(project_id, "interrupted-expansion", &revision)
                .unwrap()
                .remove(0);
            let request = SafeManageRegenerableScanRequest {
                project_id,
                analysis_run_id: "interrupted-expansion".to_string(),
                evidence_revision: revision.clone(),
                nav_id: target.nav_id,
                node_id: target.node_id,
                path: target.path.clone(),
            };
            let mut writer = db.open_write_session().unwrap();
            writer
                .begin_safe_manage_regenerable_scan(project_id, target.nav_id)
                .unwrap();
            writer
                .safe_manage_regenerable_expansion_begin(&request)
                .unwrap();
            (project_id, revision, target)
        };

        let reopened = Db::open(&catalog_path).unwrap();
        let targets = reopened
            .safe_manage_regenerable_targets(project_id, "interrupted-expansion", &revision)
            .unwrap();
        let reopened_target = targets
            .into_iter()
            .find(|candidate| candidate.nav_id == target.nav_id)
            .unwrap();
        assert_eq!(reopened_target.evidence_state, "expanded_partial");
        assert!(!reopened_target.operation_plan_eligible);
        assert!(reopened_target
            .scan_error
            .as_deref()
            .is_some_and(|error| error.contains("interrupted")));
    }
}
