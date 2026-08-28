use serde::{Deserialize, Serialize};

pub const SAFE_MANAGE_RULESET_VERSION: &str = "safe-manage-objective-v2";
pub const SAFE_MANAGE_ACTIVE_DAYS: i64 = 30;
pub const SAFE_MANAGE_DORMANT_DAYS: i64 = 90;
pub const SAFE_MANAGE_ARCHIVE_DAYS: i64 = 180;
pub const SAFE_MANAGE_REMOVAL_DAYS: i64 = 365;
pub const SAFE_MANAGE_REGENERABLE_MIN_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SafeManageLifecycle {
    Active,
    Dormant,
    ArchiveCandidate,
    CleanupCandidate,
    NeedsReview,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SafeManageRecommendation {
    Keep,
    Review,
    Archive,
    CleanRegenerables,
    RemovalCandidate,
    DoNotTouch,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SafeManageConfidence {
    High,
    Medium,
    Low,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SafeManageSignalState {
    Present,
    Absent,
    Unknown,
}

/// Whether a bounded Safe Manage evidence family covered its declared local
/// scope. `Partial` can carry useful positive lower bounds, but an omitted kind
/// or a zero count is not evidence of absence. Older persisted assessments
/// deserialize to `Unavailable` rather than being silently upgraded to a
/// complete negative result.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum SafeManageEvidenceCoverage {
    Complete,
    Partial,
    #[default]
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SafeManageFileKindCount {
    pub kind: String,
    pub label: String,
    pub file_count: u64,
}

/// Positive-only file-kind counts from the bounded, non-sensitive,
/// non-protected and non-collapsed catalog scope. Missing categories are never
/// serialized as zero. When coverage is partial the counts are lower bounds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct SafeManageFileKindProfile {
    pub coverage: SafeManageEvidenceCoverage,
    pub inspected_file_count: u64,
    pub counts: Vec<SafeManageFileKindCount>,
}

/// Conservative duplicate/copy evidence assembled without opening new file
/// bodies. Metadata candidates are only same-relative-path + same-size hints.
/// `confirmed_indexed_text_copy_count` reuses full BLAKE3 hashes already stored
/// by the safe text index and therefore applies only to that explicitly named
/// scope, not to every file in the project.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct SafeManageDuplicateEvidence {
    pub coverage: SafeManageEvidenceCoverage,
    pub inspected_file_count: u64,
    pub possible_copy_file_count: u64,
    pub indexed_text_file_count: u64,
    pub confirmed_indexed_text_copy_count: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SafeManageDecisionKind {
    Keep,
    Ignore,
    RequestDeeperReview,
    Archive,
    CleanRegenerables,
    PrepareRemoval,
}

/// One project-bound owner decision. Group recording accepts only a non-empty,
/// duplicate-free set whose members all point at the same completed analysis
/// run; validation happens before the first row is written.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SafeManageDecisionRequest {
    pub project_id: i64,
    pub analysis_run_id: String,
    pub decision: SafeManageDecisionKind,
    pub evidence_revision: String,
}

/// Exact project-local directory that may be expanded for a clean-regenerables
/// review. This is never a project-root authorization. `operation_plan_eligible`
/// becomes true only after a complete explicit expansion produced concrete
/// inventory beneath this exact node.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SafeManageRegenerableTarget {
    pub project_id: i64,
    pub analysis_run_id: String,
    pub evidence_revision: String,
    pub nav_id: i64,
    pub node_id: i64,
    pub path: String,
    pub kind: String,
    pub bytes: Option<u64>,
    /// opaque_measured | opaque_partial | expanded_complete | expanded_partial
    pub evidence_state: String,
    pub operation_plan_eligible: bool,
    pub scan_error: Option<String>,
}

/// All identity fields returned by target enumeration are echoed into the
/// explicit scan request. The DB resolves them again as one tuple; mixing a
/// nav id, node id or path from another project is rejected.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SafeManageRegenerableScanRequest {
    pub project_id: i64,
    pub analysis_run_id: String,
    pub evidence_revision: String,
    pub nav_id: i64,
    pub node_id: i64,
    pub path: String,
}

/// Exact regenerable-container identity selected for a Clean regenerables
/// OperationPlan. Project/run/revision identity stays on the envelope so the
/// caller cannot mix bindings from two targets.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SafeManageOperationTargetIdentity {
    pub nav_id: i64,
    pub node_id: i64,
    pub path: String,
}

/// Owner intent that must be revalidated immediately before an OperationPlan
/// is built. Archive and Prepare removal are project-bound and reject a target;
/// Clean regenerables requires one exact expanded target with a current receipt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SafeManageOperationPlanRequest {
    pub project_id: i64,
    pub analysis_run_id: String,
    pub evidence_revision: String,
    pub decision: SafeManageDecisionKind,
    pub target: Option<SafeManageOperationTargetIdentity>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SafeManageSignal {
    pub code: String,
    pub label: String,
    pub state: SafeManageSignalState,
    pub detail: String,
    pub source: String,
    pub observed_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SafeManageImportantFile {
    pub node_id: Option<i64>,
    pub path: String,
    pub display_name: String,
    pub reason: String,
    pub protected_or_sensitive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SafeManageRiskRelation {
    pub kind: String,
    pub label: String,
    pub confidence: SafeManageConfidence,
    pub related_project_ids: Vec<i64>,
}

/// A bounded, objective snapshot assembled from the local catalog. Unknown fields
/// remain `None`; the rule engine never treats absence of evidence as a safe fact.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SafeManageObjectiveInput {
    pub project_id: i64,
    pub project_name: String,
    pub project_path: String,
    pub source: String,
    /// Stable, local-only epoch for the registered portfolio. It advances on
    /// real inventory scans and catalog/protection changes, but deliberately
    /// does not advance when Safe Manage merely materializes one previously
    /// opaque regenerable container for an exact operation receipt.
    pub catalog_evidence_epoch: String,
    /// Canonical semantic relationship digest. Readiness is intentionally
    /// separate: rebuilding an unchanged pending index must not stale the
    /// owner's decision, while a changed relation still must.
    pub relationship_evidence_revision: String,
    pub apps: Vec<String>,
    pub is_current: bool,
    pub session_count: Option<u64>,
    pub last_activity_ms: Option<i64>,
    pub last_activity_source: Option<String>,
    pub scan_complete: bool,
    pub scan_error_count: u64,
    pub file_count: u64,
    pub context_file_count: u64,
    pub substantive_file_count: u64,
    #[serde(default)]
    pub file_kind_profile: SafeManageFileKindProfile,
    #[serde(default)]
    pub duplicate_evidence: SafeManageDuplicateEvidence,
    /// Digest of the exact bounded profile, copy groups and material-similarity
    /// relations used for this recommendation. It deliberately excludes every
    /// collapsed `.git` or heavy/regenerable descendant.
    #[serde(default)]
    pub comparison_evidence_revision: String,
    /// `None` means the bounded material-version comparison was incomplete.
    /// A positive value is review evidence only; zero is never used as a reason
    /// to archive or remove a project.
    #[serde(default)]
    pub materially_similar_project_count: Option<u64>,
    pub apparent_bytes: Option<u64>,
    pub physical_bytes: Option<u64>,
    pub footprint_partial: bool,
    pub has_git: bool,
    pub git_has_remote: Option<bool>,
    pub git_uncommitted: Option<bool>,
    pub git_evidence_error: Option<String>,
    pub regenerable_bytes: Option<u64>,
    pub similar_project_count: Option<u64>,
    pub shared_reference_count: Option<u64>,
    pub relationship_issue_count: Option<u64>,
    pub relationship_evidence_complete: bool,
    pub sensitive_file_count: u64,
    pub protected_file_count: u64,
    pub root_protected: bool,
    pub important_files: Vec<SafeManageImportantFile>,
    pub risk_relations: Vec<SafeManageRiskRelation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SafeManageProjectAssessment {
    pub analysis_run_id: String,
    pub project_id: i64,
    pub project_name: String,
    pub project_path: String,
    pub lifecycle: SafeManageLifecycle,
    pub recommendation: SafeManageRecommendation,
    pub confidence: SafeManageConfidence,
    pub reason_code: String,
    pub reason: String,
    pub ruleset_version: String,
    pub evidence_revision: String,
    pub evidence_stale: bool,
    pub last_activity_ms: Option<i64>,
    pub apps: Vec<String>,
    pub session_count: Option<u64>,
    pub has_git: bool,
    pub git_has_remote: Option<bool>,
    pub git_uncommitted: Option<bool>,
    pub apparent_bytes: Option<u64>,
    pub physical_bytes: Option<u64>,
    pub footprint_partial: bool,
    #[serde(default)]
    pub file_kind_profile: SafeManageFileKindProfile,
    #[serde(default)]
    pub duplicate_evidence: SafeManageDuplicateEvidence,
    /// Positive review evidence or a complete zero. `None` means the bounded
    /// portfolio comparison could not establish absence.
    #[serde(default)]
    pub materially_similar_project_count: Option<u64>,
    pub signals: Vec<SafeManageSignal>,
    pub important_files: Vec<SafeManageImportantFile>,
    pub risk_relations: Vec<SafeManageRiskRelation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct SafeManagePortfolioCounts {
    pub total: u64,
    pub active: u64,
    pub dormant: u64,
    pub archive_candidates: u64,
    pub cleanup_candidates: u64,
    pub needs_review: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SafeManageAnalysisRun {
    pub id: String,
    /// queued | running | cancelling | completed | partial | cancelled | failed
    pub state: String,
    pub ruleset_version: String,
    pub catalog_revision: String,
    pub created_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub processed_projects: u64,
    pub total_projects: u64,
    pub counts: SafeManagePortfolioCounts,
    pub message: String,
    pub error: Option<String>,
    pub assessments: Vec<SafeManageProjectAssessment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SafeManageDecision {
    pub id: i64,
    pub project_id: i64,
    pub analysis_run_id: String,
    pub decision: SafeManageDecisionKind,
    pub evidence_revision: String,
    pub decided_by: String,
    pub decided_at: String,
    pub evidence_stale: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SafeManageFirstRunPreference {
    pub suggest_after_discovery: bool,
    /// pending | postponed | completed | suppressed
    pub prompt_state: String,
    pub last_prompted_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SafeManageOverview {
    /// Newest run, including an active, cancelled, failed or partial run.
    pub latest_run: Option<SafeManageAnalysisRun>,
    /// Most recent fully completed portfolio. A partial/cancelled run never
    /// replaces this accepted local result.
    pub last_complete_run: Option<SafeManageAnalysisRun>,
    pub decisions: Vec<SafeManageDecision>,
    pub first_run: SafeManageFirstRunPreference,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SafeManageClassificationContext {
    pub now_ms: i64,
    pub analysis_run_id: String,
    pub evidence_revision: String,
    pub observed_at: Option<String>,
}

pub fn assess_safe_manage_project(
    input: &SafeManageObjectiveInput,
    context: &SafeManageClassificationContext,
) -> SafeManageProjectAssessment {
    let activity_days = input
        .last_activity_ms
        .map(|last| context.now_ms.saturating_sub(last).max(0) / 86_400_000);
    let signals = objective_signals(input, activity_days, context.observed_at.as_deref());

    let relationship_unknown = !input.relationship_evidence_complete
        || input.shared_reference_count.is_none()
        || input.relationship_issue_count.is_none();
    let has_risky_relationship = input.shared_reference_count.unwrap_or(0) > 0
        || input.relationship_issue_count.unwrap_or(0) > 0;
    let has_similar_project = input.similar_project_count.unwrap_or(0) > 0;
    let has_materially_similar_project = input
        .materially_similar_project_count
        .is_some_and(|count| count > 0);
    let has_copy_evidence = input.duplicate_evidence.possible_copy_file_count > 0
        || input.duplicate_evidence.confirmed_indexed_text_copy_count > 0;
    let git_unknown = input.has_git && input.git_uncommitted.is_none();
    let git_dirty = input.git_uncommitted == Some(true);
    let incomplete = !input.scan_complete
        || input.scan_error_count > 0
        || input.footprint_partial
        || relationship_unknown
        || input.similar_project_count.is_none()
        || input.session_count.is_none();
    let important = input.context_file_count > 0
        || input.substantive_file_count > 0
        || input.session_count.unwrap_or(0) > 0;
    // Profile/copy evidence may only narrow retention recommendations. Keeping
    // it separate from `incomplete` lets a fully receipt-bound cleanup of an
    // exact regenerable container remain available, but Archive and Removal
    // can never use an unknown/partial comparison as negative evidence.
    let retention_comparison_incomplete = input.file_kind_profile.coverage
        != SafeManageEvidenceCoverage::Complete
        || input.duplicate_evidence.coverage != SafeManageEvidenceCoverage::Complete
        || input.materially_similar_project_count.is_none();

    let (lifecycle, recommendation, confidence, reason_code, reason) = if input.root_protected {
        (
            SafeManageLifecycle::NeedsReview,
            SafeManageRecommendation::DoNotTouch,
            SafeManageConfidence::High,
            "protected_root",
            "This project root is protected. Safe Manage will not prepare it for cleanup.",
        )
    } else if input.sensitive_file_count > 0 || input.protected_file_count > 0 {
        (
            SafeManageLifecycle::NeedsReview,
            SafeManageRecommendation::Review,
            SafeManageConfidence::High,
            "protected_or_sensitive_content",
            "Protected or sensitive content requires an exact project review before any cleanup decision.",
        )
    } else if git_dirty {
        (
            SafeManageLifecycle::NeedsReview,
            SafeManageRecommendation::Review,
            SafeManageConfidence::High,
            "uncommitted_git_work",
            "The local Git working tree contains uncommitted work, so no cleanup recommendation is safe.",
        )
    } else if has_risky_relationship {
        (
            SafeManageLifecycle::NeedsReview,
            SafeManageRecommendation::Review,
            SafeManageConfidence::High,
            "shared_or_related",
            "Other projects or assets depend on this project, so its relationships need review.",
        )
    } else if has_materially_similar_project || has_copy_evidence {
        (
            SafeManageLifecycle::NeedsReview,
            SafeManageRecommendation::Review,
            if input
                .duplicate_evidence
                .confirmed_indexed_text_copy_count
                > 0
            {
                SafeManageConfidence::Medium
            } else {
                // Name/size/shape evidence is deliberately low-confidence: it
                // never opens a body and therefore cannot establish identity.
                SafeManageConfidence::Low
            },
            "copy_or_material_similarity",
            "Bounded local evidence found possible copied files or a materially similar project inventory. Compare the projects; this evidence never authorizes cleanup.",
        )
    } else if has_similar_project {
        (
            SafeManageLifecycle::NeedsReview,
            SafeManageRecommendation::Review,
            SafeManageConfidence::Medium,
            "similar_project_versions",
            "Another registered project has a similar local name. Compare the versions before archiving or preparing removal.",
        )
    } else if input.is_current || activity_days.is_some_and(|days| days <= SAFE_MANAGE_ACTIVE_DAYS)
    {
        (
            SafeManageLifecycle::Active,
            SafeManageRecommendation::Keep,
            if input.is_current {
                SafeManageConfidence::High
            } else {
                SafeManageConfidence::Medium
            },
            "recent_activity",
            "Recent local activity indicates that this project is still in use.",
        )
    } else if incomplete || git_unknown || input.last_activity_ms.is_none() {
        (
            SafeManageLifecycle::NeedsReview,
            SafeManageRecommendation::Review,
            SafeManageConfidence::Low,
            "incomplete_evidence",
            "Some required local evidence is missing, partial or stale; Code Hangar will not infer that the project is safe to clean.",
        )
    } else if input.regenerable_bytes.unwrap_or(0) >= SAFE_MANAGE_REGENERABLE_MIN_BYTES {
        (
            SafeManageLifecycle::CleanupCandidate,
            SafeManageRecommendation::CleanRegenerables,
            if input.physical_bytes.is_some() {
                SafeManageConfidence::High
            } else {
                SafeManageConfidence::Medium
            },
            "regenerable_footprint",
            "A material amount of locally identified build, cache or dependency data can be regenerated.",
        )
    } else if retention_comparison_incomplete {
        (
            SafeManageLifecycle::NeedsReview,
            SafeManageRecommendation::Review,
            SafeManageConfidence::Low,
            "incomplete_retention_comparison",
            "File-kind or copy/version comparison evidence is partial or unavailable, so Code Hangar will not recommend archive or removal.",
        )
    } else if activity_days.is_some_and(|days| days >= SAFE_MANAGE_REMOVAL_DAYS)
        && !important
        && (!input.has_git || input.git_uncommitted == Some(false))
        && (!input.has_git || input.git_has_remote == Some(true))
    {
        (
            SafeManageLifecycle::CleanupCandidate,
            SafeManageRecommendation::RemovalCandidate,
            SafeManageConfidence::Medium,
            "old_residual_project",
            "The complete local evidence describes an old residual project with no substantial files or recorded sessions. This is only a review candidate, never an authorization to remove it.",
        )
    } else if activity_days.is_some_and(|days| days >= SAFE_MANAGE_ARCHIVE_DAYS)
        && important
        && (!input.has_git || input.git_uncommitted == Some(false))
        && (!input.has_git || input.git_has_remote == Some(true))
    {
        (
            SafeManageLifecycle::ArchiveCandidate,
            SafeManageRecommendation::Archive,
            SafeManageConfidence::Medium,
            "dormant_substantial_project",
            "The project appears substantial but has been inactive for a long time; archiving preserves it without treating it as waste.",
        )
    } else if activity_days.is_some_and(|days| days >= SAFE_MANAGE_DORMANT_DAYS) {
        (
            SafeManageLifecycle::Dormant,
            SafeManageRecommendation::Review,
            SafeManageConfidence::Medium,
            "dormant_project",
            "The project has been inactive long enough to review, but the evidence does not justify a stronger recommendation.",
        )
    } else {
        (
            SafeManageLifecycle::NeedsReview,
            SafeManageRecommendation::Review,
            SafeManageConfidence::Low,
            "ambiguous_project",
            "The objective signals do not support a confident keep, archive or cleanup recommendation.",
        )
    };

    SafeManageProjectAssessment {
        analysis_run_id: context.analysis_run_id.clone(),
        project_id: input.project_id,
        project_name: input.project_name.clone(),
        project_path: input.project_path.clone(),
        lifecycle,
        recommendation,
        confidence,
        reason_code: reason_code.to_string(),
        reason: reason.to_string(),
        ruleset_version: SAFE_MANAGE_RULESET_VERSION.to_string(),
        evidence_revision: context.evidence_revision.clone(),
        evidence_stale: false,
        last_activity_ms: input.last_activity_ms,
        apps: input.apps.clone(),
        session_count: input.session_count,
        has_git: input.has_git,
        git_has_remote: input.git_has_remote,
        git_uncommitted: input.git_uncommitted,
        apparent_bytes: input.apparent_bytes,
        physical_bytes: input.physical_bytes,
        footprint_partial: input.footprint_partial,
        file_kind_profile: input.file_kind_profile.clone(),
        duplicate_evidence: input.duplicate_evidence.clone(),
        materially_similar_project_count: input.materially_similar_project_count,
        signals,
        important_files: input.important_files.clone(),
        risk_relations: input.risk_relations.clone(),
    }
}

pub fn safe_manage_portfolio_counts(
    assessments: &[SafeManageProjectAssessment],
) -> SafeManagePortfolioCounts {
    let mut counts = SafeManagePortfolioCounts::default();
    for assessment in assessments {
        safe_manage_portfolio_counts_include(&mut counts, assessment);
    }
    counts
}

/// Extend portfolio totals with one newly completed assessment. The analysis
/// worker uses this instead of recounting its entire prefix after every project.
pub fn safe_manage_portfolio_counts_include(
    counts: &mut SafeManagePortfolioCounts,
    assessment: &SafeManageProjectAssessment,
) {
    counts.total = counts.total.saturating_add(1);
    match assessment.lifecycle {
        SafeManageLifecycle::Active => counts.active = counts.active.saturating_add(1),
        SafeManageLifecycle::Dormant => counts.dormant = counts.dormant.saturating_add(1),
        SafeManageLifecycle::ArchiveCandidate => {
            counts.archive_candidates = counts.archive_candidates.saturating_add(1)
        }
        SafeManageLifecycle::CleanupCandidate => {
            counts.cleanup_candidates = counts.cleanup_candidates.saturating_add(1)
        }
        SafeManageLifecycle::NeedsReview => {
            counts.needs_review = counts.needs_review.saturating_add(1)
        }
    }
}

fn objective_signals(
    input: &SafeManageObjectiveInput,
    activity_days: Option<i64>,
    observed_at: Option<&str>,
) -> Vec<SafeManageSignal> {
    let observed_at = observed_at.map(str::to_string);
    let mut signals = vec![SafeManageSignal {
        code: "last_activity".to_string(),
        label: "Last activity".to_string(),
        state: if input.last_activity_ms.is_some() {
            SafeManageSignalState::Present
        } else {
            SafeManageSignalState::Unknown
        },
        detail: activity_days
            .map(|days| {
                format!(
                    "{days} days ago via {}",
                    input
                        .last_activity_source
                        .as_deref()
                        .unwrap_or("local catalog")
                )
            })
            .unwrap_or_else(|| "No reliable activity timestamp is available.".to_string()),
        source: input
            .last_activity_source
            .clone()
            .unwrap_or_else(|| "local catalog".to_string()),
        observed_at: observed_at.clone(),
    }];

    signals.push(boolean_signal(
        "scan_complete",
        "Catalog coverage",
        Some(input.scan_complete && input.scan_error_count == 0),
        if input.scan_complete && input.scan_error_count == 0 {
            "The catalog scan is complete.".to_string()
        } else {
            format!(
                "The scan is partial or has {} recorded error(s).",
                input.scan_error_count
            )
        },
        "scanner",
        observed_at.clone(),
    ));
    signals.push(boolean_signal(
        "git_uncommitted",
        "Uncommitted Git work",
        input
            .git_uncommitted
            .or_else(|| (!input.has_git).then_some(false)),
        match input.git_uncommitted {
            Some(true) => "The local working tree contains uncommitted changes.".to_string(),
            Some(false) => "No uncommitted changes were observed at analysis time.".to_string(),
            None if input.has_git => input
                .git_evidence_error
                .clone()
                .unwrap_or_else(|| "Git working-tree state was not established.".to_string()),
            None => "This project has no local Git repository.".to_string(),
        },
        "local Git",
        observed_at.clone(),
    ));
    signals.push(boolean_signal(
        "git_remote",
        "Recorded Git remote",
        input
            .git_has_remote
            .or_else(|| (!input.has_git).then_some(false)),
        match input.git_has_remote {
            Some(true) => "At least one remote name is recorded in local Git configuration. No network request was made.".to_string(),
            Some(false) if input.has_git => {
                "No remote is recorded in the current local Git configuration.".to_string()
            }
            None if input.has_git => input
                .git_evidence_error
                .clone()
                .unwrap_or_else(|| "Git remote state was not established.".to_string()),
            _ => "This project has no local Git repository.".to_string(),
        },
        "local Git",
        observed_at.clone(),
    ));
    signals.push(boolean_signal(
        "protected_or_sensitive",
        "Protected or sensitive content",
        Some(
            input.root_protected
                || input.protected_file_count > 0
                || input.sensitive_file_count > 0,
        ),
        format!(
            "{} protected and {} sensitive catalog item(s).",
            input.protected_file_count, input.sensitive_file_count
        ),
        "protection index",
        observed_at.clone(),
    ));
    signals.push(boolean_signal(
        "shared_relationships",
        "Shared or dependent relationships",
        input
            .shared_reference_count
            .zip(input.relationship_issue_count)
            .map(|(shared, issues)| shared > 0 || issues > 0),
        match (input.shared_reference_count, input.relationship_issue_count) {
            (Some(shared), Some(issues)) => {
                format!("{shared} shared reference(s), {issues} relationship issue(s).")
            }
            _ => "Relationship evidence is incomplete.".to_string(),
        },
        "relationship index",
        observed_at.clone(),
    ));
    signals.push(boolean_signal(
        "similar_projects",
        "Similar registered projects",
        input.similar_project_count.map(|count| count > 0),
        input
            .similar_project_count
            .map(|count| {
                format!("{count} similar-name project candidate(s) in the current catalog.")
            })
            .unwrap_or_else(|| "Similar-project evidence is unavailable.".to_string()),
        "project catalog",
        observed_at.clone(),
    ));
    signals.push(boolean_signal(
        "materially_similar_projects",
        "Materially similar project inventories",
        input
            .materially_similar_project_count
            .map(|count| count > 0),
        input
            .materially_similar_project_count
            .map(|count| {
                format!(
                    "{count} project(s) matched the bounded local inventory-comparison rules. This is not byte identity or cleanup authorization."
                )
            })
            .unwrap_or_else(|| {
                "The bounded material-version comparison was incomplete; no zero result was inferred."
                    .to_string()
            }),
        "local catalog comparison",
        observed_at.clone(),
    ));
    signals.push(SafeManageSignal {
        code: "project_contents".to_string(),
        label: "Project contents".to_string(),
        state: if input.scan_complete && input.scan_error_count == 0 {
            SafeManageSignalState::Present
        } else {
            SafeManageSignalState::Unknown
        },
        detail: if input.scan_complete && input.scan_error_count == 0 {
            format!(
                "{} file(s), {} context file(s), {} substantive file(s), {} recorded session(s).",
                input.file_count,
                input.context_file_count,
                input.substantive_file_count,
                input
                    .session_count
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "unknown".to_string())
            )
        } else {
            format!(
                "At least {} file(s), {} context file(s) and {} substantive file(s) are catalogued; the remaining contents are unknown.",
                input.file_count, input.context_file_count, input.substantive_file_count
            )
        },
        source: "local catalog".to_string(),
        observed_at: observed_at.clone(),
    });
    signals.push(file_kind_signal(input, observed_at.clone()));
    signals.push(duplicate_evidence_signal(input, observed_at.clone()));
    signals.push(SafeManageSignal {
        code: "regenerable_bytes".to_string(),
        label: "Regenerable footprint".to_string(),
        state: input
            .regenerable_bytes
            .map(|bytes| {
                if bytes > 0 {
                    SafeManageSignalState::Present
                } else {
                    SafeManageSignalState::Absent
                }
            })
            .unwrap_or(SafeManageSignalState::Unknown),
        detail: input
            .regenerable_bytes
            .map(|bytes| {
                format!("{bytes} locally accounted byte(s) are classified as regenerable.")
            })
            .unwrap_or_else(|| "Regenerable footprint is not fully accounted.".to_string()),
        source: "local accounting".to_string(),
        observed_at,
    });
    signals
}

fn file_kind_signal(
    input: &SafeManageObjectiveInput,
    observed_at: Option<String>,
) -> SafeManageSignal {
    let profile = &input.file_kind_profile;
    let counts = profile
        .counts
        .iter()
        .filter(|item| item.file_count > 0)
        .map(|item| format!("{} {}", item.file_count, item.label.to_ascii_lowercase()))
        .collect::<Vec<_>>();
    let scope = if counts.is_empty() {
        "No positive file-kind count is available in the bounded comparable scope.".to_string()
    } else {
        counts.join(", ")
    };
    let (state, detail) = match profile.coverage {
        SafeManageEvidenceCoverage::Complete => (
            if profile.inspected_file_count == 0 {
                SafeManageSignalState::Absent
            } else {
                SafeManageSignalState::Present
            },
            format!(
                "{scope}. The complete comparable scope inspected {} file(s); collapsed, sensitive and protected paths are outside it.",
                profile.inspected_file_count
            ),
        ),
        SafeManageEvidenceCoverage::Partial => (
            SafeManageSignalState::Unknown,
            format!(
                "Known lower bounds from {} inspected file(s): {scope}. Missing kinds remain unknown.",
                profile.inspected_file_count
            ),
        ),
        SafeManageEvidenceCoverage::Unavailable => (
            SafeManageSignalState::Unknown,
            "File-kind evidence was unavailable; no missing category was counted as zero."
                .to_string(),
        ),
    };
    SafeManageSignal {
        code: "file_kinds".to_string(),
        label: "Known file kinds".to_string(),
        state,
        detail,
        source: "bounded local catalog profile".to_string(),
        observed_at,
    }
}

fn duplicate_evidence_signal(
    input: &SafeManageObjectiveInput,
    observed_at: Option<String>,
) -> SafeManageSignal {
    let evidence = &input.duplicate_evidence;
    let positive =
        evidence.possible_copy_file_count > 0 || evidence.confirmed_indexed_text_copy_count > 0;
    let state = if positive {
        SafeManageSignalState::Present
    } else if evidence.coverage == SafeManageEvidenceCoverage::Complete {
        SafeManageSignalState::Absent
    } else {
        SafeManageSignalState::Unknown
    };
    let detail = match evidence.coverage {
        SafeManageEvidenceCoverage::Complete => format!(
            "{} metadata-only possible copy file(s) and {} byte-identical already-indexed text file(s) were found among {} comparable file(s). Text identity reuses full local BLAKE3 hashes from the completed index; other file bodies were not opened.",
            evidence.possible_copy_file_count,
            evidence.confirmed_indexed_text_copy_count,
            evidence.inspected_file_count,
        ),
        SafeManageEvidenceCoverage::Partial => format!(
            "At least {} metadata-only possible copy file(s) and {} byte-identical already-indexed text file(s) were found in a partial {}-file comparison. A zero is not implied for the remaining scope.",
            evidence.possible_copy_file_count,
            evidence.confirmed_indexed_text_copy_count,
            evidence.inspected_file_count,
        ),
        SafeManageEvidenceCoverage::Unavailable => {
            "Copy/duplicate evidence was unavailable; Code Hangar did not infer zero duplicates."
                .to_string()
        }
    };
    SafeManageSignal {
        code: "duplicate_copy_evidence".to_string(),
        label: "Copy and duplicate evidence".to_string(),
        state,
        detail,
        source: "catalog metadata and existing safe-text hashes".to_string(),
        observed_at,
    }
}

fn boolean_signal(
    code: &str,
    label: &str,
    value: Option<bool>,
    detail: String,
    source: &str,
    observed_at: Option<String>,
) -> SafeManageSignal {
    SafeManageSignal {
        code: code.to_string(),
        label: label.to_string(),
        state: match value {
            Some(true) => SafeManageSignalState::Present,
            Some(false) => SafeManageSignalState::Absent,
            None => SafeManageSignalState::Unknown,
        },
        detail,
        source: source.to_string(),
        observed_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_input() -> SafeManageObjectiveInput {
        SafeManageObjectiveInput {
            project_id: 7,
            project_name: "Fixture".to_string(),
            project_path: r"C:\fixture".to_string(),
            source: "manual".to_string(),
            catalog_evidence_epoch: "catalog-1".to_string(),
            relationship_evidence_revision: "relations-1".to_string(),
            apps: vec!["codex".to_string()],
            is_current: false,
            session_count: Some(1),
            last_activity_ms: Some(1_700_000_000_000),
            last_activity_source: Some("catalog mtime".to_string()),
            scan_complete: true,
            scan_error_count: 0,
            file_count: 20,
            context_file_count: 2,
            substantive_file_count: 8,
            file_kind_profile: SafeManageFileKindProfile {
                coverage: SafeManageEvidenceCoverage::Complete,
                inspected_file_count: 20,
                counts: vec![SafeManageFileKindCount {
                    kind: "source".to_string(),
                    label: "Source files".to_string(),
                    file_count: 8,
                }],
            },
            duplicate_evidence: SafeManageDuplicateEvidence {
                coverage: SafeManageEvidenceCoverage::Complete,
                inspected_file_count: 20,
                ..SafeManageDuplicateEvidence::default()
            },
            comparison_evidence_revision: "comparison-1".to_string(),
            materially_similar_project_count: Some(0),
            apparent_bytes: Some(200),
            physical_bytes: Some(180),
            footprint_partial: false,
            has_git: true,
            git_has_remote: Some(true),
            git_uncommitted: Some(false),
            git_evidence_error: None,
            regenerable_bytes: Some(0),
            similar_project_count: Some(0),
            shared_reference_count: Some(0),
            relationship_issue_count: Some(0),
            relationship_evidence_complete: true,
            sensitive_file_count: 0,
            protected_file_count: 0,
            root_protected: false,
            important_files: Vec::new(),
            risk_relations: Vec::new(),
        }
    }

    fn context(days_after_last_activity: i64) -> SafeManageClassificationContext {
        SafeManageClassificationContext {
            now_ms: 1_700_000_000_000 + days_after_last_activity * 86_400_000,
            analysis_run_id: "run-1".to_string(),
            evidence_revision: "rev-1".to_string(),
            observed_at: Some("2026-08-27T10:00:00Z".to_string()),
        }
    }

    #[test]
    fn recent_activity_is_keep_not_cleanup() {
        let mut input = base_input();
        input.regenerable_bytes = Some(5 * SAFE_MANAGE_REGENERABLE_MIN_BYTES);
        let result = assess_safe_manage_project(&input, &context(5));
        assert_eq!(result.lifecycle, SafeManageLifecycle::Active);
        assert_eq!(result.recommendation, SafeManageRecommendation::Keep);
    }

    #[test]
    fn unknown_evidence_never_becomes_removal_candidate() {
        let mut input = base_input();
        input.session_count = None;
        input.last_activity_ms = None;
        input.relationship_evidence_complete = false;
        input.shared_reference_count = None;
        input.relationship_issue_count = None;
        input.context_file_count = 0;
        input.substantive_file_count = 0;
        input.has_git = false;
        input.git_has_remote = None;
        input.git_uncommitted = None;
        let result = assess_safe_manage_project(&input, &context(500));
        assert_eq!(result.lifecycle, SafeManageLifecycle::NeedsReview);
        assert_eq!(result.recommendation, SafeManageRecommendation::Review);
        assert_eq!(result.reason_code, "incomplete_evidence");
    }

    #[test]
    fn unknown_session_count_is_not_treated_as_zero() {
        let mut input = base_input();
        input.session_count = None;
        input.context_file_count = 0;
        input.substantive_file_count = 0;
        input.has_git = false;
        input.git_has_remote = None;
        input.git_uncommitted = None;
        let result = assess_safe_manage_project(&input, &context(500));
        assert_eq!(result.recommendation, SafeManageRecommendation::Review);
        assert_eq!(result.reason_code, "incomplete_evidence");
    }

    #[test]
    fn dirty_git_blocks_cleanup_even_when_old() {
        let mut input = base_input();
        input.git_uncommitted = Some(true);
        input.regenerable_bytes = Some(500 * 1024 * 1024);
        let result = assess_safe_manage_project(&input, &context(500));
        assert_eq!(result.recommendation, SafeManageRecommendation::Review);
        assert_eq!(result.reason_code, "uncommitted_git_work");
    }

    #[test]
    fn similar_project_version_requires_comparison_before_archive_or_removal() {
        let mut input = base_input();
        input.similar_project_count = Some(1);
        input.session_count = Some(0);
        input.context_file_count = 0;
        input.substantive_file_count = 0;
        input.has_git = false;
        input.git_has_remote = None;
        input.git_uncommitted = None;
        let result = assess_safe_manage_project(&input, &context(500));
        assert_eq!(result.recommendation, SafeManageRecommendation::Review);
        assert_eq!(result.reason_code, "similar_project_versions");
    }

    #[test]
    fn metadata_copy_candidate_is_low_confidence_review_only() {
        let mut input = base_input();
        input.duplicate_evidence.possible_copy_file_count = 2;
        let result = assess_safe_manage_project(&input, &context(500));
        assert_eq!(result.recommendation, SafeManageRecommendation::Review);
        assert_eq!(result.reason_code, "copy_or_material_similarity");
        assert_eq!(result.confidence, SafeManageConfidence::Low);
        assert_eq!(result.materially_similar_project_count, Some(0));
    }

    #[test]
    fn indexed_text_identity_is_review_evidence_not_cleanup_authority() {
        let mut input = base_input();
        input.duplicate_evidence.confirmed_indexed_text_copy_count = 1;
        let result = assess_safe_manage_project(&input, &context(500));
        assert_eq!(result.recommendation, SafeManageRecommendation::Review);
        assert_eq!(result.reason_code, "copy_or_material_similarity");
        assert_eq!(result.confidence, SafeManageConfidence::Medium);
    }

    #[test]
    fn metadata_only_material_similarity_stays_low_confidence_review() {
        let mut input = base_input();
        input.materially_similar_project_count = Some(1);
        let result = assess_safe_manage_project(&input, &context(500));
        assert_eq!(result.recommendation, SafeManageRecommendation::Review);
        assert_eq!(result.reason_code, "copy_or_material_similarity");
        assert_eq!(result.confidence, SafeManageConfidence::Low);
        assert_eq!(result.materially_similar_project_count, Some(1));
    }

    #[test]
    fn partial_comparison_blocks_archive_without_turning_unknown_into_zero() {
        let mut input = base_input();
        input.file_kind_profile.coverage = SafeManageEvidenceCoverage::Partial;
        input.duplicate_evidence.coverage = SafeManageEvidenceCoverage::Partial;
        input.materially_similar_project_count = None;
        let result = assess_safe_manage_project(&input, &context(240));
        assert_eq!(result.recommendation, SafeManageRecommendation::Review);
        assert_eq!(result.reason_code, "incomplete_retention_comparison");
        assert!(result.signals.iter().any(|signal| {
            signal.code == "file_kinds" && signal.state == SafeManageSignalState::Unknown
        }));
    }

    #[test]
    fn partial_comparison_does_not_hide_exact_regenerable_cleanup_review() {
        let mut input = base_input();
        input.file_kind_profile.coverage = SafeManageEvidenceCoverage::Partial;
        input.duplicate_evidence.coverage = SafeManageEvidenceCoverage::Partial;
        input.materially_similar_project_count = None;
        input.regenerable_bytes = Some(SAFE_MANAGE_REGENERABLE_MIN_BYTES);
        let result = assess_safe_manage_project(&input, &context(240));
        assert_eq!(
            result.recommendation,
            SafeManageRecommendation::CleanRegenerables
        );
        assert_eq!(result.reason_code, "regenerable_footprint");
    }

    #[test]
    fn partial_scan_reports_known_content_as_lower_bound() {
        let mut input = base_input();
        input.scan_complete = false;
        input.scan_error_count = 1;
        let result = assess_safe_manage_project(&input, &context(240));
        let contents = result
            .signals
            .iter()
            .find(|signal| signal.code == "project_contents")
            .unwrap();
        assert_eq!(contents.state, SafeManageSignalState::Unknown);
        assert!(contents.detail.starts_with("At least 20 file(s)"));
    }

    #[test]
    fn persisted_legacy_assessment_defaults_new_evidence_to_unknown() {
        let assessment = assess_safe_manage_project(&base_input(), &context(5));
        let mut encoded = serde_json::to_value(assessment).unwrap();
        let object = encoded.as_object_mut().unwrap();
        object.remove("fileKindProfile");
        object.remove("duplicateEvidence");
        object.remove("materiallySimilarProjectCount");

        let decoded: SafeManageProjectAssessment = serde_json::from_value(encoded).unwrap();
        assert_eq!(
            decoded.file_kind_profile.coverage,
            SafeManageEvidenceCoverage::Unavailable
        );
        assert_eq!(
            decoded.duplicate_evidence.coverage,
            SafeManageEvidenceCoverage::Unavailable
        );
        assert_eq!(decoded.materially_similar_project_count, None);
    }

    #[test]
    fn assessment_comparison_evidence_exposes_counts_not_file_identities() {
        let assessment = assess_safe_manage_project(&base_input(), &context(5));
        let encoded = serde_json::to_string(&assessment).unwrap();
        for forbidden in ["identityKey", "contentHash", "absolutePath", "relativePath"] {
            assert!(!encoded.contains(forbidden), "leaked {forbidden}");
        }
        assert!(encoded.contains("fileKindProfile"));
        assert!(encoded.contains("duplicateEvidence"));
        assert!(encoded.contains("materiallySimilarProjectCount"));
    }

    #[test]
    fn protected_root_is_do_not_touch() {
        let mut input = base_input();
        input.root_protected = true;
        let result = assess_safe_manage_project(&input, &context(500));
        assert_eq!(result.recommendation, SafeManageRecommendation::DoNotTouch);
        assert_eq!(result.confidence, SafeManageConfidence::High);
    }

    #[test]
    fn old_substantial_remote_project_is_archive_candidate() {
        let input = base_input();
        let result = assess_safe_manage_project(&input, &context(240));
        assert_eq!(result.lifecycle, SafeManageLifecycle::ArchiveCandidate);
        assert_eq!(result.recommendation, SafeManageRecommendation::Archive);
    }

    #[test]
    fn only_complete_residual_evidence_can_recommend_removal_review() {
        let mut input = base_input();
        input.session_count = Some(0);
        input.context_file_count = 0;
        input.substantive_file_count = 0;
        input.file_count = 2;
        input.has_git = false;
        input.git_has_remote = None;
        input.git_uncommitted = None;
        let result = assess_safe_manage_project(&input, &context(500));
        assert_eq!(
            result.recommendation,
            SafeManageRecommendation::RemovalCandidate
        );
        assert!(result.reason.contains("never an authorization"));
    }

    #[test]
    fn portfolio_counts_classifications_once() {
        let mut items = Vec::new();
        for days in [5, 120, 240] {
            items.push(assess_safe_manage_project(&base_input(), &context(days)));
        }
        let counts = safe_manage_portfolio_counts(&items);
        assert_eq!(counts.total, 3);
        assert_eq!(counts.active, 1);
        assert_eq!(counts.dormant, 1);
        assert_eq!(counts.archive_candidates, 1);
    }
}
