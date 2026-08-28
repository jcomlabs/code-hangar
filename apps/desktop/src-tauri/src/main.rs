#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(windows))]
compile_error!("code-hangar-desktop is a Windows-only application; portable crates remain cross-target testable.");

mod resident;
mod shell_open;
mod windows_shell;

use hangar_api::{
    AppState, DocumentSearchRequest, DuplicateSearchRequest, LostProjectRequest, OrphanAssetRequest,
};
use hangar_core::{
    AdapterSummary, Comment, ContextFile, DashboardSummary, DocumentSearchResult,
    DuplicateCandidates, DuplicateConfirmStatus, DuplicateConfirmation, ExportResult,
    FileChangeEvent, FilePreview, FolderExplanation, FolderInvestigation, GitRepoSummary, GraphMap,
    InvestigationHandle, LostProjectCandidates, NavChildrenPage, NavItem, NodeRelationships,
    OpenTargetInspection, OpenTargetPreparation, OpenTargetScanStart, OperationPlan,
    OrphanCandidates, OrphanStatus, PinnedItem, PlanPreviewStatus, PreviewMode, PreviewPolicy,
    ProcessResourceUsage, ProjectContextSummary, ProjectDetail, ProjectDiscoveryReport,
    ProjectReviewCheckpoint, ProjectSummary, QuickOpenResult, RecentItem, RecoverableSummary,
    ReviewLedgerEntry, RiskReport, SafeManageAnalysisRun, SafeManageDecision,
    SafeManageDecisionKind, SafeManageDecisionRequest, SafeManageFirstRunPreference,
    SafeManageOperationPlanRequest, SafeManageOverview, SafeManageRegenerableScanRequest,
    SafeManageRegenerableTarget, ScanRoot, ScanStatus, SecurityStatus, SessionChangeSet,
    SessionPreview, ShellIntegrationStatus, StartupStatus, SystemResourceProfile, WatcherStatus,
};
#[cfg(feature = "agent_automation")]
use hangar_core::{
    AutomationActivityEntry, AutomationAgentSummary, AutomationCredential, AutomationReadGrant,
    AutomationStatus,
};
#[cfg(feature = "mutation")]
use hangar_core::{
    MutationActivityLog, MutationBackupSummary, MutationFinalRemoveSummary, MutationLockInspection,
    MutationMoveSummary, MutationProtectedPreview, MutationRestoreSummary, MutationTokenResult,
    RecoveryPending, RecoveryResolveResult,
};
use tauri::{AppHandle, Manager, State};

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LostProjectFilters {
    min_size_bytes: Option<u64>,
    project_id: Option<i64>,
    stale_preset: Option<String>,
    signals: Option<Vec<String>>,
    keyword: Option<String>,
    include_partial: Option<bool>,
    limit: Option<usize>,
    include_fixture_projects: Option<bool>,
    performance_mode: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DocumentSearchFilters {
    query: String,
    project_id: Option<i64>,
    indexed_kind: Option<String>,
    path_filter: Option<String>,
    name_filter: Option<String>,
    limit: Option<usize>,
    include_fixture_projects: Option<bool>,
    performance_mode: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct OrphanAssetFilters {
    min_size_bytes: Option<u64>,
    project_id: Option<i64>,
    asset_kind: Option<String>,
    min_confidence: Option<String>,
    include_partial: Option<bool>,
    limit: Option<usize>,
    include_fixture_projects: Option<bool>,
    performance_mode: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DuplicateSearchFilters {
    min_size_bytes: Option<u64>,
    project_id: Option<i64>,
    file_kind: Option<String>,
    current_file_node_id: Option<i64>,
    limit: Option<usize>,
    include_fixture_projects: Option<bool>,
    performance_mode: Option<String>,
}

#[cfg(feature = "agent_automation")]
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AiExplainChangeCommand {
    project_id: i64,
    session_paths: Vec<String>,
    source_mode: String,
    file_path: String,
    edit_index: usize,
    level: String,
    model: String,
    preview_id: String,
}

async fn run_blocking<T: Send + 'static>(
    task: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|err| format!("Background task failed: {err}"))?
}

#[tauri::command]
fn startup_status(state: State<'_, AppState>) -> StartupStatus {
    hangar_api::startup_status(state.inner())
}

#[tauri::command]
fn resident_started_hidden(control: State<'_, resident::ResidentControl>) -> bool {
    control.started_hidden()
}

#[tauri::command]
fn resident_window_visible(app: AppHandle) -> bool {
    app.get_webview_window("main")
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false)
}

async fn wait_for_resident_window_if_needed(
    app: AppHandle,
    started_hidden: bool,
) -> Result<(), String> {
    if !started_hidden {
        return Ok(());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let started = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(15);
        loop {
            let visible = app
                .get_webview_window("main")
                .and_then(|window| window.is_visible().ok())
                .unwrap_or(false);
            if visible {
                return Ok(());
            }
            if started.elapsed() >= timeout {
                return Err(
                    "The resident Code Hangar window did not become visible within 15 seconds."
                        .to_string(),
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    })
    .await
    .map_err(|error| format!("Resident UI activation gate failed: {error}"))?
}

#[tauri::command]
async fn projects_list(
    app: AppHandle,
    control: State<'_, resident::ResidentControl>,
    state: State<'_, AppState>,
) -> Result<Vec<ProjectSummary>, String> {
    let started_hidden = control.started_hidden();
    let app_state = state.inner().clone();
    wait_for_resident_window_if_needed(app, started_hidden).await?;
    run_blocking(move || hangar_api::projects_list(&app_state)).await
}

#[tauri::command]
async fn projects_list_lite(
    app: AppHandle,
    control: State<'_, resident::ResidentControl>,
    state: State<'_, AppState>,
) -> Result<Vec<ProjectSummary>, String> {
    let started_hidden = control.started_hidden();
    let app_state = state.inner().clone();
    wait_for_resident_window_if_needed(app, started_hidden).await?;
    run_blocking(move || hangar_api::projects_list_lite(&app_state)).await
}

#[tauri::command]
fn detect_installed_apps() -> Vec<hangar_core::InstalledApp> {
    hangar_api::detect_installed_apps()
}

#[tauri::command]
fn wsl_scan_enabled(state: State<'_, AppState>) -> bool {
    hangar_api::wsl_scan_enabled(state.inner())
}

#[tauri::command]
fn set_wsl_scan_enabled(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    hangar_api::set_wsl_scan_enabled(state.inner(), enabled)
}

#[tauri::command]
async fn projects_cached_snapshot(
    state: State<'_, AppState>,
) -> Result<Vec<ProjectSummary>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || Ok(hangar_api::projects_cached_snapshot(&app_state))).await
}

#[tauri::command]
async fn cache_discovery_snapshot(
    state: State<'_, AppState>,
    snapshot: String,
) -> Result<(), String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::cache_discovery_snapshot(&app_state, snapshot);
        Ok(())
    })
    .await
}

#[tauri::command]
async fn read_discovery_snapshot(state: State<'_, AppState>) -> Result<Option<String>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || Ok(hangar_api::read_discovery_snapshot(&app_state))).await
}

#[tauri::command]
async fn project_get(
    state: State<'_, AppState>,
    project_id: i64,
) -> Result<Option<ProjectDetail>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::project_get(&app_state, project_id)).await
}

/// A local, no-network "what this project does" summary from its README + manifests.
/// Read-only; available in every edition.
#[tauri::command]
async fn project_context_summary(
    state: State<'_, AppState>,
    project_id: i64,
) -> Result<ProjectContextSummary, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        let project = hangar_api::project_get(&app_state, project_id)?
            .ok_or_else(|| "That project is no longer registered in Code Hangar.".to_string())?;
        Ok(hangar_api::project_context_summary(&project.path))
    })
    .await
}

#[tauri::command]
async fn project_nav_tree(
    state: State<'_, AppState>,
    project_id: i64,
) -> Result<Vec<NavItem>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::project_nav_tree(&app_state, project_id)).await
}

#[tauri::command]
async fn project_nav_children(
    state: State<'_, AppState>,
    project_id: i64,
    parent_nav_id: Option<i64>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<NavChildrenPage, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::project_nav_children(&app_state, project_id, parent_nav_id, limit, offset)
    })
    .await
}

#[tauri::command]
async fn project_nav_path(
    state: State<'_, AppState>,
    project_id: i64,
    node_id: i64,
) -> Result<Vec<NavItem>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::project_nav_path(&app_state, project_id, node_id)).await
}

#[tauri::command]
async fn project_git_status(
    state: State<'_, AppState>,
    project_id: i64,
) -> Result<GitRepoSummary, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::project_git_status(&app_state, project_id)).await
}

#[tauri::command]
async fn folder_explanation(
    state: State<'_, AppState>,
    nav_id: i64,
) -> Result<Option<FolderExplanation>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::folder_explanation(&app_state, nav_id)).await
}

#[tauri::command]
async fn investigate_folder(
    state: State<'_, AppState>,
    path: String,
    performance_mode: Option<String>,
) -> Result<InvestigationHandle, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::investigate_folder(&app_state, path, performance_mode)).await
}

#[tauri::command]
async fn investigation_report(
    state: State<'_, AppState>,
    root_id: i64,
) -> Result<FolderInvestigation, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::investigation_report(&app_state, root_id)).await
}

#[tauri::command]
async fn discard_investigation(state: State<'_, AppState>, root_id: i64) -> Result<(), String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::discard_investigation(&app_state, root_id)).await
}

#[tauri::command]
async fn node_full_path(state: State<'_, AppState>, node_id: i64) -> Result<String, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::node_full_path(&app_state, node_id)).await
}

#[tauri::command]
async fn open_node_external(state: State<'_, AppState>, node_id: i64) -> Result<(), String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::open_node_external(&app_state, node_id)).await
}

#[tauri::command]
async fn reveal_node_external(state: State<'_, AppState>, node_id: i64) -> Result<(), String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::reveal_node_external(&app_state, node_id)).await
}

#[tauri::command]
async fn reveal_project_external(
    state: State<'_, AppState>,
    project_id: i64,
) -> Result<(), String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::reveal_project_external(&app_state, project_id)).await
}

#[tauri::command]
async fn reveal_session_external(path: String) -> Result<(), String> {
    run_blocking(move || hangar_api::reveal_session_external(path)).await
}

#[tauri::command]
async fn dashboard_summary(
    state: State<'_, AppState>,
    include_fixture_projects: Option<bool>,
) -> Result<DashboardSummary, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::dashboard_summary_filtered(&app_state, include_fixture_projects.unwrap_or(true))
    })
    .await
}

#[tauri::command]
async fn adapters_list(state: State<'_, AppState>) -> Result<Vec<AdapterSummary>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::adapters_list(&app_state)).await
}

#[tauri::command]
async fn project_context_files(
    state: State<'_, AppState>,
    project_id: i64,
) -> Result<Vec<ContextFile>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::project_context_files(&app_state, project_id)).await
}

#[tauri::command]
async fn file_preview(
    state: State<'_, AppState>,
    node_id: i64,
    project_id: Option<i64>,
    mode: PreviewMode,
    record_recent: Option<bool>,
    policy: Option<PreviewPolicy>,
) -> Result<FilePreview, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::file_preview(&app_state, node_id, project_id, mode, record_recent, policy)
    })
    .await
}

#[tauri::command]
async fn file_reveal(
    state: State<'_, AppState>,
    node_id: i64,
    project_id: Option<i64>,
    mode: PreviewMode,
    policy: Option<PreviewPolicy>,
) -> Result<FilePreview, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::file_reveal(&app_state, node_id, project_id, mode, policy))
        .await
}

#[tauri::command]
async fn quick_open(
    state: State<'_, AppState>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<QuickOpenResult>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::quick_open(&app_state, query, limit)).await
}

#[tauri::command]
fn performance_set_mode(mode: Option<String>) -> Result<(), String> {
    hangar_api::performance_set_mode(mode)
}

#[tauri::command]
fn system_resource_profile() -> SystemResourceProfile {
    hangar_api::system_resource_profile()
}

#[tauri::command]
fn process_resource_usage() -> ProcessResourceUsage {
    hangar_api::process_resource_usage()
}

#[tauri::command]
async fn session_preview(
    path: String,
    reveal: bool,
    max_bytes: Option<u64>,
    load_full: Option<bool>,
) -> Result<SessionPreview, String> {
    run_blocking(move || {
        hangar_api::session_preview_window(path, reveal, max_bytes, load_full.unwrap_or(false))
    })
    .await
}

#[tauri::command]
async fn session_change_set(path: String) -> Result<SessionChangeSet, String> {
    run_blocking(move || hangar_api::session_change_set(path)).await
}

#[tauri::command]
async fn project_session_change_set(
    state: State<'_, AppState>,
    project_id: i64,
    path: String,
) -> Result<SessionChangeSet, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::project_session_change_set(&app_state, project_id, path)).await
}

#[tauri::command]
async fn project_git_change_set(
    state: State<'_, AppState>,
    project_id: i64,
) -> Result<SessionChangeSet, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::project_git_change_set(&app_state, project_id)).await
}

#[tauri::command]
async fn project_review_checkpoint(
    state: State<'_, AppState>,
    project_id: i64,
) -> Result<Option<ProjectReviewCheckpoint>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::project_review_checkpoint(&app_state, project_id)).await
}

#[tauri::command]
async fn project_review_checkpoints(
    state: State<'_, AppState>,
) -> Result<Vec<ProjectReviewCheckpoint>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::project_review_checkpoints(&app_state)).await
}

#[tauri::command]
async fn mark_project_reviewed(
    state: State<'_, AppState>,
    project_id: i64,
    session_cutoff_ms: i64,
    undated_session_fingerprint: Option<String>,
) -> Result<ProjectReviewCheckpoint, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::mark_project_reviewed(
            &app_state,
            project_id,
            session_cutoff_ms,
            undated_session_fingerprint.as_deref(),
        )
    })
    .await
}

#[tauri::command]
async fn project_review_ledger(
    state: State<'_, AppState>,
    project_id: i64,
    limit: Option<usize>,
) -> Result<Vec<ReviewLedgerEntry>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::project_review_ledger(&app_state, project_id, limit)).await
}

#[tauri::command]
async fn project_recap(
    state: State<'_, AppState>,
    project_id: i64,
    session_paths: Vec<String>,
) -> Result<SessionChangeSet, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::project_recap(&app_state, project_id, session_paths)).await
}

#[tauri::command]
async fn project_review_receipt_export(
    state: State<'_, AppState>,
    project_id: i64,
    session_paths: Vec<String>,
    scope: String,
    path: String,
) -> Result<ExportResult, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::project_review_receipt_export(
            &app_state,
            project_id,
            session_paths,
            scope,
            path,
        )
    })
    .await
}

#[tauri::command]
async fn watcher_status(
    state: State<'_, AppState>,
    focused_project_id: Option<i64>,
    current_node_id: Option<i64>,
) -> Result<WatcherStatus, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::watcher_status(&app_state, focused_project_id, current_node_id)
    })
    .await
}

#[tauri::command]
async fn recent_file_changes(
    state: State<'_, AppState>,
    project_id: Option<i64>,
    limit: Option<usize>,
) -> Result<Vec<FileChangeEvent>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::recent_file_changes(&app_state, project_id, limit)).await
}

#[tauri::command]
async fn search_documents(
    state: State<'_, AppState>,
    filters: DocumentSearchFilters,
) -> Result<DocumentSearchResult, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::search_documents(
            &app_state,
            DocumentSearchRequest {
                query: filters.query,
                project_id: filters.project_id,
                indexed_kind: filters.indexed_kind,
                path_filter: filters.path_filter,
                name_filter: filters.name_filter,
                limit: filters.limit,
                include_fixture_projects: filters.include_fixture_projects.unwrap_or(false),
                performance_mode: filters.performance_mode,
            },
        )
    })
    .await
}

#[tauri::command]
async fn resolve_local_link(
    state: State<'_, AppState>,
    project_id: i64,
    from_node_id: i64,
    target: String,
) -> Result<Option<i64>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::resolve_local_link(&app_state, project_id, from_node_id, target)
    })
    .await
}

#[tauri::command]
async fn node_relationships(
    state: State<'_, AppState>,
    project_id: i64,
    node_id: i64,
) -> Result<NodeRelationships, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::node_relationships(&app_state, project_id, node_id)).await
}

#[tauri::command]
async fn project_graph_map(
    state: State<'_, AppState>,
    project_id: i64,
    limit: Option<usize>,
) -> Result<GraphMap, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::project_graph_map(&app_state, project_id, limit)).await
}

#[tauri::command]
async fn graph_orphans(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<OrphanCandidates, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::graph_orphans(&app_state, limit)).await
}

#[tauri::command]
async fn orphan_asset_candidates(
    state: State<'_, AppState>,
    filters: OrphanAssetFilters,
) -> Result<OrphanCandidates, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::orphan_asset_candidates(
            &app_state,
            OrphanAssetRequest {
                min_size_bytes: filters.min_size_bytes,
                project_id: filters.project_id,
                asset_kind: filters.asset_kind,
                min_confidence: filters.min_confidence,
                include_partial: filters.include_partial,
                limit: filters.limit,
                include_fixture_projects: filters.include_fixture_projects.unwrap_or(false),
                performance_mode: filters.performance_mode,
            },
        )
    })
    .await
}

#[tauri::command]
async fn node_orphan_status(
    state: State<'_, AppState>,
    project_id: i64,
    node_id: i64,
) -> Result<OrphanStatus, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::node_orphan_status(&app_state, project_id, node_id)).await
}

#[tauri::command]
async fn lost_project_candidates(
    state: State<'_, AppState>,
    filters: LostProjectFilters,
) -> Result<LostProjectCandidates, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::lost_project_candidates(
            &app_state,
            LostProjectRequest {
                min_size_bytes: filters.min_size_bytes,
                project_id: filters.project_id,
                stale_preset: filters.stale_preset,
                signals: filters.signals.unwrap_or_default(),
                keyword: filters.keyword,
                include_partial: filters.include_partial.unwrap_or(false),
                limit: filters.limit.unwrap_or(50),
                include_fixture_projects: filters.include_fixture_projects.unwrap_or(false),
                performance_mode: filters.performance_mode,
            },
        )
    })
    .await
}

#[tauri::command]
async fn duplicate_candidates(
    state: State<'_, AppState>,
    filters: DuplicateSearchFilters,
) -> Result<DuplicateCandidates, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::duplicate_candidates(
            &app_state,
            DuplicateSearchRequest {
                min_size_bytes: filters.min_size_bytes,
                project_id: filters.project_id,
                file_kind: filters.file_kind,
                current_file_node_id: filters.current_file_node_id,
                limit: filters.limit,
                include_fixture_projects: filters.include_fixture_projects.unwrap_or(false),
                performance_mode: filters.performance_mode,
            },
        )
    })
    .await
}

#[tauri::command]
async fn confirm_duplicate_group(
    state: State<'_, AppState>,
    node_id: i64,
) -> Result<DuplicateConfirmation, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::confirm_duplicate_group(&app_state, node_id)).await
}

#[tauri::command]
async fn project_recoverable_summary(
    state: State<'_, AppState>,
    project_id: i64,
) -> Result<RecoverableSummary, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::project_recoverable_summary(&app_state, project_id)).await
}

#[tauri::command]
async fn node_recoverable_summary(
    state: State<'_, AppState>,
    node_id: i64,
) -> Result<RecoverableSummary, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::node_recoverable_summary(&app_state, node_id)).await
}

#[tauri::command]
fn safe_manage_analysis_start(state: State<'_, AppState>) -> Result<String, String> {
    hangar_api::safe_manage_analysis_start(state.inner())
}

#[tauri::command]
fn safe_manage_analysis_status(
    state: State<'_, AppState>,
    job_id: String,
) -> Result<SafeManageAnalysisRun, String> {
    hangar_api::safe_manage_analysis_status(state.inner(), &job_id)
}

#[tauri::command]
fn safe_manage_analysis_cancel(
    state: State<'_, AppState>,
    job_id: String,
) -> Result<SafeManageAnalysisRun, String> {
    hangar_api::safe_manage_analysis_cancel(state.inner(), &job_id)
}

#[tauri::command]
fn safe_manage_overview(state: State<'_, AppState>) -> Result<SafeManageOverview, String> {
    hangar_api::safe_manage_overview(state.inner())
}

#[tauri::command]
fn safe_manage_first_run_get(
    state: State<'_, AppState>,
) -> Result<SafeManageFirstRunPreference, String> {
    hangar_api::safe_manage_first_run_preference(state.inner())
}

#[tauri::command]
fn safe_manage_first_run_set(
    state: State<'_, AppState>,
    suggest_after_discovery: bool,
    prompt_state: String,
    mark_prompted_now: bool,
) -> Result<SafeManageFirstRunPreference, String> {
    hangar_api::safe_manage_first_run_preference_set(
        state.inner(),
        suggest_after_discovery,
        &prompt_state,
        mark_prompted_now,
    )
}

#[tauri::command]
async fn safe_manage_decision_record(
    state: State<'_, AppState>,
    project_id: i64,
    analysis_run_id: String,
    decision: SafeManageDecisionKind,
    evidence_revision: String,
) -> Result<SafeManageDecision, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::safe_manage_decision_record(
            &app_state,
            project_id,
            &analysis_run_id,
            decision,
            &evidence_revision,
        )
    })
    .await
}

#[tauri::command]
async fn safe_manage_decisions_record_atomic(
    state: State<'_, AppState>,
    requests: Vec<SafeManageDecisionRequest>,
) -> Result<Vec<SafeManageDecision>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::safe_manage_decisions_record_atomic(&app_state, requests))
        .await
}

#[tauri::command]
async fn safe_manage_regenerable_targets(
    state: State<'_, AppState>,
    project_id: i64,
    analysis_run_id: String,
    evidence_revision: String,
) -> Result<Vec<SafeManageRegenerableTarget>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::safe_manage_regenerable_targets(
            &app_state,
            project_id,
            &analysis_run_id,
            &evidence_revision,
        )
    })
    .await
}

#[tauri::command]
async fn safe_manage_regenerable_scan_start(
    state: State<'_, AppState>,
    request: SafeManageRegenerableScanRequest,
) -> Result<String, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::safe_manage_regenerable_scan_start(&app_state, request)).await
}

#[tauri::command]
fn safe_manage_operation_plan_start(
    state: State<'_, AppState>,
    request: SafeManageOperationPlanRequest,
    performance_mode: Option<String>,
) -> Result<String, String> {
    hangar_api::safe_manage_operation_plan_start(state.inner(), request, performance_mode)
}

#[tauri::command]
async fn operation_plan_build(
    state: State<'_, AppState>,
    target_node_id: i64,
    action_label: String,
    performance_mode: Option<String>,
) -> Result<OperationPlan, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::operation_plan_build(&app_state, target_node_id, action_label, performance_mode)
    })
    .await
}

#[tauri::command]
fn operation_plan_start(
    state: State<'_, AppState>,
    target_node_id: i64,
    action_label: String,
    performance_mode: Option<String>,
) -> Result<String, String> {
    hangar_api::operation_plan_start(
        state.inner(),
        target_node_id,
        action_label,
        performance_mode,
    )
}

#[tauri::command]
fn operation_plan_status(
    state: State<'_, AppState>,
    job_id: String,
) -> Result<PlanPreviewStatus, String> {
    hangar_api::operation_plan_status(state.inner(), job_id)
}

#[tauri::command]
fn operation_plan_cancel(state: State<'_, AppState>, job_id: String) -> Result<(), String> {
    hangar_api::operation_plan_cancel(state.inner(), job_id)
}

#[tauri::command]
fn confirm_duplicate_group_start(
    state: State<'_, AppState>,
    node_id: i64,
) -> Result<String, String> {
    hangar_api::confirm_duplicate_group_start(state.inner(), node_id)
}

#[tauri::command]
fn confirm_duplicate_group_status(
    state: State<'_, AppState>,
    job_id: String,
) -> Result<DuplicateConfirmStatus, String> {
    hangar_api::confirm_duplicate_group_status(state.inner(), job_id)
}

#[tauri::command]
fn confirm_duplicate_group_cancel(
    state: State<'_, AppState>,
    job_id: String,
) -> Result<(), String> {
    hangar_api::confirm_duplicate_group_cancel(state.inner(), job_id)
}

#[tauri::command]
async fn risk_report_build(
    state: State<'_, AppState>,
    plan: OperationPlan,
    performance_mode: Option<String>,
) -> Result<RiskReport, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::risk_report_build(&app_state, plan, performance_mode)).await
}

#[tauri::command]
async fn risk_report_build_for_target(
    state: State<'_, AppState>,
    target_node_id: i64,
    action_label: String,
    performance_mode: Option<String>,
) -> Result<RiskReport, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::risk_report_build_for_target(
            &app_state,
            target_node_id,
            action_label,
            performance_mode,
        )
    })
    .await
}

#[tauri::command]
fn risk_report_export(report: RiskReport, path: String) -> Result<ExportResult, String> {
    hangar_api::risk_report_export(report, path)
}

#[tauri::command]
async fn diagnostics_export(
    state: State<'_, AppState>,
    path: String,
) -> Result<ExportResult, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::diagnostics_export(&app_state, path)).await
}

#[tauri::command]
async fn recent_items_list(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<RecentItem>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::recent_items_list(&app_state, limit)).await
}

#[tauri::command]
async fn pinned_items_list(state: State<'_, AppState>) -> Result<Vec<PinnedItem>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::pinned_items_list(&app_state)).await
}

#[tauri::command]
fn pin_item(
    state: State<'_, AppState>,
    node_id: i64,
    item_kind: String,
    project_id: Option<i64>,
) -> Result<(), String> {
    hangar_api::pin_item(state.inner(), node_id, item_kind, project_id)
}

#[tauri::command]
fn unpin_item(
    state: State<'_, AppState>,
    node_id: i64,
    item_kind: String,
    project_id: Option<i64>,
) -> Result<(), String> {
    hangar_api::unpin_item(state.inner(), node_id, item_kind, project_id)
}

#[tauri::command]
async fn comments_for_node(
    state: State<'_, AppState>,
    node_id: i64,
) -> Result<Vec<Comment>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::comments_for_node(&app_state, node_id)).await
}

#[tauri::command]
async fn comments_count_for_node(state: State<'_, AppState>, node_id: i64) -> Result<i64, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::comments_count_for_node(&app_state, node_id)).await
}

#[tauri::command]
fn comment_add(state: State<'_, AppState>, node_id: i64, body: String) -> Result<Comment, String> {
    // The app UI always writes as the local user. Agent identities only ever come from
    // the connected-app server, never the frontend, so source can't be spoofed here.
    hangar_api::comment_add(
        state.inner(),
        node_id,
        body,
        Some("user".to_string()),
        Some("user".to_string()),
    )
}

#[tauri::command]
fn comment_edit(
    state: State<'_, AppState>,
    comment_id: i64,
    body: String,
) -> Result<Comment, String> {
    // The app UI always acts as the local user; the human/AI boundary lives in the DB.
    hangar_api::comment_edit(state.inner(), comment_id, body, "user")
}

#[tauri::command]
fn comment_delete(state: State<'_, AppState>, comment_id: i64) -> Result<(), String> {
    hangar_api::comment_delete(state.inner(), comment_id, "user")
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
fn comment_write_enabled(state: State<'_, AppState>) -> Result<bool, String> {
    hangar_api::comment_write_enabled(state.inner())
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
fn set_comment_write_enabled(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    hangar_api::set_comment_write_enabled(state.inner(), enabled)
}

// The MCP total-control / read-only toggles are meaningful only to the connector server, which
// exists solely in the agent_automation edition. Gating the commands (and their registrations)
// keeps the base build's IPC command table free of any MCP/connector vocabulary; the frontend
// reaches them via optionalCommand, which falls back (false / no-op) when they are absent.
#[cfg(feature = "agent_automation")]
#[tauri::command]
fn mcp_full_control_enabled(state: State<'_, AppState>) -> Result<bool, String> {
    hangar_api::mcp_full_control_enabled(state.inner())
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
fn set_mcp_full_control_enabled(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    hangar_api::set_mcp_full_control_enabled(state.inner(), enabled)
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
fn mcp_read_only_mode(state: State<'_, AppState>) -> Result<bool, String> {
    hangar_api::mcp_read_only_mode(state.inner())
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
fn set_mcp_read_only_mode(state: State<'_, AppState>, enabled: bool) -> Result<(), String> {
    hangar_api::set_mcp_read_only_mode(state.inner(), enabled)
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn mcp_appconfig_status(
    state: State<'_, AppState>,
) -> Result<Vec<hangar_api::HostStatus>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::mcp_appconfig_status(&app_state)).await
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn mcp_appconfig_register(
    state: State<'_, AppState>,
    host_id: String,
    project_ids: Vec<i64>,
    include_history_search: Option<bool>,
    include_mutation_requests: Option<bool>,
) -> Result<hangar_api::HostStatus, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::mcp_appconfig_register(
            &app_state,
            host_id,
            project_ids,
            include_history_search.unwrap_or(false),
            include_mutation_requests.unwrap_or(false),
        )
    })
    .await
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn mcp_appconfig_remove(
    state: State<'_, AppState>,
    host_id: String,
) -> Result<hangar_api::HostStatus, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::mcp_appconfig_remove(&app_state, host_id)).await
}

/// Revoke the durable credential even when the owner-controlled external config
/// is missing, malformed or no longer contains Code Hangar. This command never
/// parses or writes that external config.
#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn mcp_appconfig_revoke_orphan(
    state: State<'_, AppState>,
    host_id: String,
) -> Result<hangar_api::HostStatus, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::mcp_appconfig_revoke_orphan(&app_state, host_id)).await
}

/// Forget an already-revoked durable registry row without reading or editing
/// the external app config. The authenticated host/path binding is retained.
#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn mcp_appconfig_forget_orphan(
    state: State<'_, AppState>,
    host_id: String,
) -> Result<hangar_api::HostStatus, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::mcp_appconfig_forget_orphan(&app_state, host_id)).await
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn agent_requests_pending(
    state: State<'_, AppState>,
) -> Result<Vec<hangar_core::AgentActionRequest>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::agent_requests_pending(&app_state)).await
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn agent_request_resolve(
    state: State<'_, AppState>,
    request_id: i64,
    approve: bool,
    inputs: hangar_api::ResolveInputs,
) -> Result<hangar_core::AgentActionRequest, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::agent_request_resolve(&app_state, request_id, approve, inputs))
        .await
}

/// AI Assist (connector edition): read-only preview of an "explain this file" send — what
/// blocks it (sensitive path / secret content) and its size/cost. Nothing leaves the machine.
#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_explain_preview(
    state: State<'_, AppState>,
    node_id: i64,
) -> Result<hangar_api::AiExplainPreview, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::ai_explain_preview(&app_state, node_id)).await
}

/// AI Assist: credential-free literal disclosure of the exact request body. The prompt is rebuilt
/// from fresh gated bytes; the webview supplies only the lens/options and an optional selection.
#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_send_disclosure(
    state: State<'_, AppState>,
    node_id: i64,
    snippet: Option<String>,
    lens: String,
    level: String,
    model: String,
) -> Result<hangar_core::AiSendDisclosure, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::ai_send_disclosure(
            &app_state,
            node_id,
            snippet.as_deref(),
            &lens,
            &level,
            &model,
        )
    })
    .await
}

/// AI Assist: bounded text deltas for the primary reading lenses. Local providers stream; remote
/// APIs retain the single-response behavior and emit that response as one channel message.
#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_read_stream(
    state: State<'_, AppState>,
    preview_id: String,
    on_event: tauri::ipc::Channel<String>,
) -> Result<String, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::ai_read_stream(&app_state, &preview_id, |delta| {
            on_event
                .send(delta.to_string())
                .map_err(|error| format!("AI stream could not update the window: {error}"))
        })
    })
    .await
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_walkthrough_preview(
    state: State<'_, AppState>,
    node_id: i64,
) -> Result<hangar_core::AiWalkthroughPreview, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::ai_walkthrough_preview(&app_state, node_id)).await
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_walkthrough_file(
    state: State<'_, AppState>,
    node_id: i64,
    section_ids: Vec<String>,
    level: String,
    model: String,
    preview_id: String,
) -> Result<String, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::ai_walkthrough_file(
            &app_state,
            node_id,
            section_ids,
            &level,
            &model,
            &preview_id,
        )
    })
    .await
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_walkthrough_disclosure(
    state: State<'_, AppState>,
    node_id: i64,
    section_ids: Vec<String>,
    level: String,
    model: String,
) -> Result<hangar_core::AiSendDisclosure, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::ai_walkthrough_disclosure(&app_state, node_id, section_ids, &level, &model)
    })
    .await
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_follow_up_preview(
    state: State<'_, AppState>,
    node_id: i64,
    section_id: String,
    conversation_id: Option<String>,
    question: String,
) -> Result<hangar_api::AiExplainPreview, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::ai_follow_up_preview(
            &app_state,
            node_id,
            &section_id,
            conversation_id.as_deref(),
            &question,
        )
    })
    .await
}

// Tauri exposes these as the existing flat camelCase IPC arguments. Wrapping
// them would silently change the frontend command contract, so keep the narrow
// adapter explicit and pass one structured request into the API immediately.
#[cfg(feature = "agent_automation")]
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn ai_follow_up(
    state: State<'_, AppState>,
    node_id: i64,
    section_id: String,
    conversation_id: Option<String>,
    question: String,
    level: String,
    model: String,
    preview_id: String,
) -> Result<hangar_core::AiFollowUpResult, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::ai_follow_up(
            &app_state,
            hangar_api::AiFollowUpRequest {
                node_id,
                section_id: &section_id,
                conversation_id: conversation_id.as_deref(),
                question: &question,
                level: &level,
                model: &model,
                preview_id: &preview_id,
            },
        )
    })
    .await
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_follow_up_disclosure(
    state: State<'_, AppState>,
    node_id: i64,
    section_id: String,
    conversation_id: Option<String>,
    question: String,
    level: String,
    model: String,
) -> Result<hangar_core::AiSendDisclosure, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::ai_follow_up_disclosure(
            &app_state,
            node_id,
            &section_id,
            conversation_id.as_deref(),
            &question,
            &level,
            &model,
        )
    })
    .await
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_glossary_state(
    state: State<'_, AppState>,
) -> Result<hangar_core::AiGlossaryState, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::ai_glossary_state(&app_state)).await
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn set_ai_glossary_enabled(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<hangar_core::AiGlossaryState, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::set_ai_glossary_enabled(&app_state, enabled)).await
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_glossary_record(
    state: State<'_, AppState>,
    terms: Vec<String>,
) -> Result<hangar_core::AiGlossaryState, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::ai_glossary_record(&app_state, terms)).await
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_annotations_for_node(
    state: State<'_, AppState>,
    node_id: i64,
) -> Result<Vec<hangar_core::CodeAnnotation>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::ai_annotations_for_node(&app_state, node_id)).await
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_annotation_add(
    state: State<'_, AppState>,
    node_id: i64,
    snippet: String,
    note: String,
) -> Result<hangar_core::CodeAnnotation, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::ai_annotation_add(&app_state, node_id, &snippet, &note)).await
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_annotation_delete(
    state: State<'_, AppState>,
    node_id: i64,
    annotation_id: i64,
) -> Result<bool, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::ai_annotation_delete(&app_state, node_id, annotation_id)).await
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_change_set_preview(
    state: State<'_, AppState>,
    project_id: i64,
    session_paths: Vec<String>,
    source_mode: String,
    file_path: Option<String>,
    edit_index: Option<usize>,
) -> Result<hangar_api::AiExplainPreview, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::ai_change_set_preview(
            &app_state,
            project_id,
            session_paths,
            &source_mode,
            file_path.as_deref(),
            edit_index,
        )
    })
    .await
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_narrate_session_changes(
    state: State<'_, AppState>,
    project_id: i64,
    session_paths: Vec<String>,
    source_mode: String,
    level: String,
    model: String,
    preview_id: String,
) -> Result<String, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::ai_narrate_session_changes(
            &app_state,
            project_id,
            session_paths,
            &source_mode,
            &level,
            &model,
            &preview_id,
        )
    })
    .await
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn ai_change_disclosure(
    state: State<'_, AppState>,
    project_id: i64,
    session_paths: Vec<String>,
    source_mode: String,
    lens: String,
    file_path: Option<String>,
    edit_index: Option<usize>,
    level: String,
    model: String,
) -> Result<hangar_core::AiSendDisclosure, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::ai_change_disclosure(
            &app_state,
            project_id,
            session_paths,
            &source_mode,
            &lens,
            file_path.as_deref(),
            edit_index,
            &level,
            &model,
        )
    })
    .await
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_explain_change(
    state: State<'_, AppState>,
    request: AiExplainChangeCommand,
) -> Result<String, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::ai_explain_change(
            &app_state,
            hangar_api::AiExplainChangeRequest {
                project_id: request.project_id,
                session_paths: request.session_paths,
                source_mode: &request.source_mode,
                edit: hangar_api::AiRecordedEditSelector {
                    file_path: &request.file_path,
                    edit_index: request.edit_index,
                },
                level: &request.level,
                model: &request.model,
                preview_id: &request.preview_id,
            },
        )
    })
    .await
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_review_change_set(
    state: State<'_, AppState>,
    project_id: i64,
    session_paths: Vec<String>,
    source_mode: String,
    level: String,
    model: String,
    preview_id: String,
) -> Result<String, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::ai_review_change_set(
            &app_state,
            project_id,
            session_paths,
            &source_mode,
            &level,
            &model,
            &preview_id,
        )
    })
    .await
}

/// Local edition: write manually edited UTF-8 text back to an inventoried file. Reuses the same
/// protected-file gate and writes atomically.
/// Available only in editions with local disk actions (mutation); the caller keeps the prior
/// content for Undo.
#[cfg(feature = "mutation")]
#[tauri::command]
async fn file_edit_preview(
    state: State<'_, AppState>,
    node_id: i64,
    content: String,
    expected_content: Option<String>,
) -> Result<hangar_core::FileEditPreview, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::file_edit_preview(&app_state, node_id, &content, expected_content.as_deref())
    })
    .await
}

#[cfg(feature = "mutation")]
#[tauri::command]
async fn write_file_content(
    state: State<'_, AppState>,
    node_id: i64,
    content: String,
    origin: Option<String>,
    expected_content: Option<String>,
    reviewed_after_hash: Option<String>,
) -> Result<String, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::write_reviewed_file_content(
            &app_state,
            node_id,
            &content,
            origin.as_deref().unwrap_or("manual"),
            expected_content.as_deref(),
            reviewed_after_hash.as_deref(),
        )
    })
    .await
}

#[cfg(feature = "mutation")]
#[tauri::command]
async fn edit_snapshots_for_node(
    state: State<'_, AppState>,
    node_id: i64,
    limit: usize,
) -> Result<Vec<hangar_core::EditSnapshotSummary>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::edit_snapshots_for_node(&app_state, node_id, limit)).await
}

#[cfg(feature = "mutation")]
#[tauri::command]
async fn edit_snapshot_restore(
    state: State<'_, AppState>,
    snapshot_id: i64,
) -> Result<hangar_core::EditSnapshotRestoreResult, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::edit_snapshot_restore(&app_state, snapshot_id)).await
}

#[cfg(feature = "mutation")]
#[tauri::command]
async fn edit_snapshot_compare(
    state: State<'_, AppState>,
    snapshot_id: i64,
) -> Result<hangar_core::EditSnapshotComparison, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::edit_snapshot_compare(&app_state, snapshot_id)).await
}

#[cfg(feature = "mutation")]
#[tauri::command]
async fn editable_values(
    state: State<'_, AppState>,
    node_id: i64,
) -> Result<hangar_core::EditableValueSet, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::editable_values(&app_state, node_id)).await
}

#[cfg(feature = "mutation")]
#[tauri::command]
async fn apply_value_edit(
    state: State<'_, AppState>,
    node_id: i64,
    request: hangar_core::ValueEditRequest,
    reviewed_after_hash: String,
) -> Result<hangar_core::ValueEditResult, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::apply_reviewed_value_edit(&app_state, node_id, &request, &reviewed_after_hash)
    })
    .await
}

#[cfg(feature = "mutation")]
#[tauri::command]
async fn preview_value_edit(
    state: State<'_, AppState>,
    node_id: i64,
    request: hangar_core::ValueEditRequest,
) -> Result<hangar_core::FileEditPreview, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::preview_value_edit(&app_state, node_id, &request)).await
}

#[cfg(feature = "mutation")]
#[tauri::command]
async fn static_correction_check(
    state: State<'_, AppState>,
    node_id: i64,
) -> Result<hangar_core::CorrectionStaticCheckReport, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::static_correction_check(&app_state, node_id)).await
}

#[cfg(feature = "mutation")]
#[tauri::command]
async fn project_checks_detect(
    state: State<'_, AppState>,
    project_id: i64,
) -> Result<Vec<hangar_core::ProjectCheckDefinition>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::project_checks_detect(&app_state, project_id)).await
}

#[cfg(feature = "mutation")]
#[tauri::command]
async fn project_check_approve(
    state: State<'_, AppState>,
    project_id: i64,
    check_id: String,
    fingerprint: String,
) -> Result<hangar_core::ProjectCheckDefinition, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::project_check_approve(&app_state, project_id, &check_id, &fingerprint)
    })
    .await
}

#[cfg(feature = "mutation")]
#[tauri::command]
async fn project_check_revoke(
    state: State<'_, AppState>,
    project_id: i64,
    check_id: String,
) -> Result<bool, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::project_check_revoke(&app_state, project_id, &check_id)).await
}

#[cfg(feature = "mutation")]
#[tauri::command]
async fn project_check_run(
    state: State<'_, AppState>,
    project_id: i64,
    node_id: i64,
    check_id: String,
    fingerprint: String,
) -> Result<hangar_core::ControlledCheckRun, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::project_check_run(&app_state, project_id, node_id, &check_id, &fingerprint)
    })
    .await
}

/// AI Assist: return one staged replacement proposal for a unique selected passage. This provider
/// command is read-only; a separate explicit local command applies the opaque proposal.
#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_rewrite_text(
    state: State<'_, AppState>,
    node_id: i64,
    snippet: String,
    instruction: String,
    level: String,
    model: String,
    preview_id: String,
) -> Result<hangar_core::AiRewriteProposal, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::ai_rewrite_text(
            &app_state,
            node_id,
            &snippet,
            &instruction,
            &level,
            &model,
            &preview_id,
        )
    })
    .await
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_rewrite_disclosure(
    state: State<'_, AppState>,
    node_id: i64,
    snippet: String,
    instruction: String,
    level: String,
    model: String,
) -> Result<hangar_core::AiSendDisclosure, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::ai_rewrite_disclosure(
            &app_state,
            node_id,
            &snippet,
            &instruction,
            &level,
            &model,
        )
    })
    .await
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn apply_ai_suggestion(
    state: State<'_, AppState>,
    proposal_id: String,
) -> Result<hangar_core::AiSuggestionApplyResult, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::apply_ai_suggestion(&app_state, &proposal_id)).await
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_edit_sessions_for_node(
    state: State<'_, AppState>,
    node_id: i64,
    limit: usize,
) -> Result<Vec<hangar_core::AiEditSessionSummary>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::ai_edit_sessions_for_node(&app_state, node_id, limit)).await
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn undo_ai_edit_session(
    state: State<'_, AppState>,
    node_id: i64,
    session_id: String,
) -> Result<hangar_core::EditSnapshotRestoreResult, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::undo_ai_edit_session(&app_state, node_id, &session_id)).await
}

/// AI Assist: an optional AI-enriched project summary, built from the same local context the
/// no-network summary uses and sent through the secret send-gate to the configured provider.
#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_summarize_project(
    state: State<'_, AppState>,
    preview_id: String,
) -> Result<hangar_core::AiProjectSummary, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::ai_summarize_project(&app_state, &preview_id)).await
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_summarize_project_preview(
    state: State<'_, AppState>,
    project_id: i64,
    level: String,
) -> Result<hangar_api::AiExplainPreview, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::ai_summarize_project_preview(&app_state, project_id, &level))
        .await
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_summarize_project_disclosure(
    state: State<'_, AppState>,
    project_id: i64,
    level: String,
    model: String,
) -> Result<hangar_core::AiSendDisclosure, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::ai_summarize_project_disclosure(&app_state, project_id, &level, &model)
    })
    .await
}

/// Connector-only backend-curated context choices for one immutable ambiguous
/// Safe Manage assessment. Selectors are random and contain no path or body.
#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_safe_manage_context_candidates(
    state: State<'_, AppState>,
    project_id: i64,
    analysis_run_id: String,
    evidence_revision: String,
) -> Result<Vec<hangar_core::AiSafeManageContextCandidate>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::ai_safe_manage_context_candidates(
            &app_state,
            project_id,
            &analysis_run_id,
            &evidence_revision,
        )
    })
    .await
}

/// Connector-only exact disclosure for explicitly selected, backend-resolved
/// context. It persists only a fingerprint/count receipt before staging a
/// one-shot provider request; no source path or body is persisted.
#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_safe_manage_advisory_disclosure(
    state: State<'_, AppState>,
    project_id: i64,
    analysis_run_id: String,
    evidence_revision: String,
    selected_context_ids: Vec<String>,
    model: String,
) -> Result<hangar_core::AiSendDisclosure, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::ai_safe_manage_advisory_disclosure(
            &app_state,
            project_id,
            &analysis_run_id,
            &evidence_revision,
            &selected_context_ids,
            &model,
        )
    })
    .await
}

/// Connector-only advisory send. It can return explanation text plus its
/// fingerprint-only receipt, but cannot record a Safe Manage decision, prepare
/// a plan, authorize cleanup, or call any mutation executor.
#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_safe_manage_advisory(
    state: State<'_, AppState>,
    project_id: i64,
    analysis_run_id: String,
    evidence_revision: String,
    model: String,
    preview_id: String,
) -> Result<hangar_core::AiSafeManageAdvisoryResult, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::ai_safe_manage_advisory(
            &app_state,
            project_id,
            &analysis_run_id,
            &evidence_revision,
            &model,
            &preview_id,
        )
    })
    .await
}

/// Connector-only audit history. Rows contain hashes, counts and typed opaque
/// selectors only — never provider/source bodies, paths, keys or model config.
#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_safe_manage_advisory_receipts(
    state: State<'_, AppState>,
    project_id: i64,
    limit: Option<usize>,
) -> Result<Vec<hangar_core::AiSafeManageAdvisoryReceipt>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::ai_safe_manage_advisory_receipts(&app_state, project_id, limit)
    })
    .await
}

/// AI Assist: bind the user's key to the already-saved API provider origin. The secret remains in
/// Credential Manager; encrypted settings retain only origin/version/fingerprint metadata.
#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_key_set(state: State<'_, AppState>, key: String) -> Result<(), String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::ai_key_set(&app_state, &key)).await
}

/// AI Assist: whether a key is present and validly bound to the active API origin.
#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_key_status(state: State<'_, AppState>) -> Result<bool, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::ai_key_status(&app_state)).await
}

/// AI Assist: revoke the origin binding, then remove current and legacy key entries.
#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_key_clear(state: State<'_, AppState>) -> Result<(), String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::ai_key_clear(&app_state)).await
}

/// AI Assist: the configured provider (mode/base_url/model/format). Never returns the API key.
#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_provider_get(
    state: State<'_, AppState>,
) -> Result<hangar_api::AiProviderConfig, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::ai_provider_get(&app_state)).await
}

/// AI Assist: persist the provider configuration. A local provider's endpoint is loopback-
/// validated before it is stored.
#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_provider_set(
    state: State<'_, AppState>,
    mode: String,
    base_url: String,
    model: String,
    format: String,
) -> Result<(), String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::ai_provider_set(&app_state, &mode, &base_url, &model, &format))
        .await
}

/// AI Assist: prepare the exact fixed reachability ping for a provider DRAFT. This validates the
/// supplied fields without persisting or sending them and returns a short-lived one-shot preview.
#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_provider_test_disclosure(
    state: State<'_, AppState>,
    mode: String,
    base_url: String,
    model: String,
    format: String,
) -> Result<hangar_core::AiSendDisclosure, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::ai_provider_test_disclosure(&app_state, &mode, &base_url, &model, &format)
    })
    .await
}

/// AI Assist: consume a reviewed provider-test preview. Endpoint fields are intentionally absent,
/// so confirmation cannot redirect or rewrite the already disclosed request.
#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_provider_test(
    state: State<'_, AppState>,
    preview_id: String,
) -> Result<String, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::ai_provider_test(&app_state, &preview_id)).await
}

/// AI Assist: prepare the exact best-effort model-list GET for a provider DRAFT. Read-only and
/// non-persisting; no provider transport occurs before the user reviews the disclosure.
#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_provider_models_disclosure(
    state: State<'_, AppState>,
    mode: String,
    base_url: String,
    model: String,
    format: String,
) -> Result<hangar_core::AiSendDisclosure, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::ai_provider_models_disclosure(&app_state, &mode, &base_url, &model, &format)
    })
    .await
}

/// AI Assist: consume a reviewed model-list preview. Draft endpoint fields are intentionally
/// absent from this confirm command.
#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_provider_models(
    state: State<'_, AppState>,
    preview_id: String,
) -> Result<Vec<String>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::ai_provider_models(&app_state, &preview_id)).await
}

/// AI Assist: explicit loopback-only discovery. No probe runs until the user presses the button.
#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_local_discover() -> Result<Vec<hangar_core::AiLocalProviderCandidate>, String> {
    run_blocking(move || Ok(hangar_api::ai_local_discover())).await
}

/// AI Assist: in-memory token estimates for this app session. No prompts or response bodies are
/// retained; the optional projection is read-only and only drives the soft-cap warning.
#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_usage_status(
    projected_input_tokens: Option<u64>,
    projected_output_tokens: Option<u64>,
) -> Result<hangar_api::AiUsageStatus, String> {
    run_blocking(move || {
        Ok(hangar_api::ai_usage_status(
            projected_input_tokens,
            projected_output_tokens,
        ))
    })
    .await
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_usage_set_soft_cap(
    soft_cap_tokens: Option<u64>,
) -> Result<hangar_api::AiUsageStatus, String> {
    run_blocking(move || hangar_api::ai_usage_set_soft_cap(soft_cap_tokens)).await
}

#[cfg(feature = "agent_automation")]
#[tauri::command]
async fn ai_usage_reset() -> Result<hangar_api::AiUsageStatus, String> {
    run_blocking(move || Ok(hangar_api::ai_usage_reset())).await
}

#[tauri::command]
async fn roots_list(state: State<'_, AppState>) -> Result<Vec<ScanRoot>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::roots_list(&app_state)).await
}

#[tauri::command]
fn roots_add(state: State<'_, AppState>, path: String) -> Result<ScanRoot, String> {
    hangar_api::roots_add(state.inner(), path)
}

#[tauri::command]
async fn project_discovery_report(
    state: State<'_, AppState>,
    limit: Option<usize>,
    session_limit: Option<usize>,
    include_loose_sessions: Option<bool>,
    include_agents: Option<bool>,
    include_technical_candidates: Option<bool>,
) -> Result<ProjectDiscoveryReport, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::project_discovery_report(
            &app_state,
            limit,
            session_limit,
            include_loose_sessions,
            include_agents,
            include_technical_candidates,
        )
    })
    .await
}

#[tauri::command]
async fn project_discovery_deep_scan(
    state: State<'_, AppState>,
    root_path: String,
    limit: Option<usize>,
    session_limit: Option<usize>,
    include_loose_sessions: Option<bool>,
    include_agents: Option<bool>,
    include_technical_candidates: Option<bool>,
) -> Result<ProjectDiscoveryReport, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::project_discovery_deep_scan(
            &app_state,
            root_path,
            limit,
            session_limit,
            include_loose_sessions,
            include_agents,
            include_technical_candidates,
        )
    })
    .await
}

#[tauri::command]
fn roots_set_enabled(
    state: State<'_, AppState>,
    root_id: i64,
    enabled: bool,
) -> Result<ScanRoot, String> {
    hangar_api::roots_set_enabled(state.inner(), root_id, enabled)
}

#[tauri::command]
fn roots_unregister(state: State<'_, AppState>, root_id: i64) -> Result<(), String> {
    hangar_api::roots_unregister(state.inner(), root_id)
}

#[tauri::command]
fn projects_unregister(state: State<'_, AppState>, project_id: i64) -> Result<(), String> {
    hangar_api::projects_unregister(state.inner(), project_id)
}

#[tauri::command]
fn reset_all_projects(state: State<'_, AppState>) -> Result<u64, String> {
    hangar_api::reset_all_projects(state.inner())
}

#[tauri::command]
fn compact_database(state: State<'_, AppState>) -> Result<hangar_api::DbMaintenanceReport, String> {
    hangar_api::compact_database(state.inner())
}

#[tauri::command]
fn restart_app() {
    // Relaunch: spawn a fresh instance, then hard-exit this one so it releases
    // the database file handle. (AppHandle::restart() does not reliably terminate
    // the outgoing process here, which would leave the file locked.) The new
    // instance wipes the now-unlocked database at startup, reclaiming its space.
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe)
            .env(
                "CODEHANGAR_RESTART_AFTER_PID",
                std::process::id().to_string(),
            )
            .spawn();
    }
    std::process::exit(0);
}

#[tauri::command]
fn shell_open_take_pending(inbox: State<'_, shell_open::ShellOpenInbox>) -> Vec<String> {
    inbox.take_pending()
}

#[tauri::command]
async fn inspect_open_target(
    state: State<'_, AppState>,
    path: String,
) -> Result<OpenTargetInspection, String> {
    let app_state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || hangar_api::inspect_open_target(&app_state, path))
        .await
        .map_err(|err| format!("Open-target inspection worker failed: {err}"))?
}

#[tauri::command]
async fn open_local_file_preview(
    path: String,
    mode: PreviewMode,
    policy: Option<PreviewPolicy>,
) -> Result<Option<hangar_api::DirectFilePreview>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        hangar_api::open_local_file_preview(path, mode, policy)
    })
    .await
    .map_err(|err| format!("Direct local preview worker failed: {err}"))?
}

#[tauri::command]
async fn open_local_file_preview_full(
    path: String,
    mode: PreviewMode,
    policy: Option<PreviewPolicy>,
) -> Result<Option<hangar_api::DirectFilePreview>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        hangar_api::open_local_file_preview_full(path, mode, policy)
    })
    .await
    .map_err(|err| format!("Full direct local preview worker failed: {err}"))?
}

#[tauri::command]
async fn prepare_open_target(
    state: State<'_, AppState>,
    path: String,
    open_mode: String,
    manual_root: Option<String>,
    performance_mode: Option<String>,
) -> Result<OpenTargetPreparation, String> {
    let app_state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        hangar_api::prepare_open_target(&app_state, path, open_mode, manual_root, performance_mode)
    })
    .await
    .map_err(|err| format!("Open-target worker failed: {err}"))?
}

#[tauri::command]
async fn start_open_target_scan(
    state: State<'_, AppState>,
    root_id: i64,
    performance_mode: Option<String>,
) -> Result<OpenTargetScanStart, String> {
    let app_state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        hangar_api::start_open_target_scan(&app_state, root_id, performance_mode)
    })
    .await
    .map_err(|err| format!("Open-target scan worker failed: {err}"))?
}

#[tauri::command]
async fn resolve_open_target(
    state: State<'_, AppState>,
    project_id: i64,
    path: String,
) -> Result<Option<i64>, String> {
    let app_state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        hangar_api::resolve_open_target(&app_state, project_id, path)
    })
    .await
    .map_err(|err| format!("Open-target resolver failed: {err}"))?
}

#[tauri::command]
async fn open_target_preview(
    state: State<'_, AppState>,
    project_id: i64,
    path: String,
    mode: PreviewMode,
    policy: Option<PreviewPolicy>,
) -> Result<FilePreview, String> {
    let app_state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        hangar_api::open_target_preview(&app_state, project_id, path, mode, policy)
    })
    .await
    .map_err(|err| format!("Direct preview worker failed: {err}"))?
}

#[tauri::command]
fn shell_integration_status() -> Result<ShellIntegrationStatus, String> {
    windows_shell::status()
}

#[tauri::command]
fn shell_integration_set(
    markdown_registered: bool,
    context_menu_registered: bool,
) -> Result<ShellIntegrationStatus, String> {
    windows_shell::set(markdown_registered, context_menu_registered)
}

#[tauri::command]
fn background_startup_set(enabled: bool) -> Result<ShellIntegrationStatus, String> {
    windows_shell::set_run_at_login(enabled)
}

#[tauri::command]
fn background_refresh_now(state: State<'_, AppState>) -> Result<Option<String>, String> {
    hangar_api::background_refresh_once(state.inner(), true)
}

#[tauri::command]
fn shell_default_guide_dismiss() -> Result<ShellIntegrationStatus, String> {
    windows_shell::dismiss_default_guide()
}

#[tauri::command]
fn shell_open_default_apps() -> Result<(), String> {
    windows_shell::open_default_apps()
}

#[tauri::command]
fn scan_start(
    state: State<'_, AppState>,
    root_ids: Option<Vec<i64>>,
    performance_mode: Option<String>,
) -> Result<String, String> {
    hangar_api::scan_start(state.inner(), root_ids, performance_mode)
}

#[tauri::command]
fn scan_resume_subtree(
    state: State<'_, AppState>,
    nav_id: i64,
    performance_mode: Option<String>,
) -> Result<String, String> {
    hangar_api::scan_resume_subtree(state.inner(), nav_id, performance_mode)
}

#[tauri::command]
fn scan_cancel(state: State<'_, AppState>, job_id: String) -> Result<(), String> {
    hangar_api::scan_cancel(state.inner(), job_id)
}

#[tauri::command]
fn scan_status(state: State<'_, AppState>, job_id: String) -> Result<ScanStatus, String> {
    hangar_api::scan_status(state.inner(), job_id)
}

#[tauri::command]
async fn zones_list(state: State<'_, AppState>) -> Result<Vec<hangar_core::ProtectedZone>, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::zones_list(&app_state)).await
}

#[tauri::command]
fn security_status() -> Result<SecurityStatus, String> {
    hangar_api::security_status()
}

#[tauri::command]
#[cfg(feature = "mutation")]
fn mutation_mode_status() -> Result<bool, String> {
    hangar_api::mutation_mode_status()
}

#[tauri::command]
#[cfg(feature = "mutation")]
fn mutation_final_remove_enabled(state: State<'_, AppState>) -> bool {
    hangar_api::mutation_final_remove_enabled(state.inner())
}

#[tauri::command]
#[cfg(feature = "mutation")]
async fn mutation_set_final_remove_enabled(
    state: State<'_, AppState>,
    enabled: bool,
    acknowledgement: Option<String>,
) -> Result<(), String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::set_final_remove_enabled(&app_state, enabled, acknowledgement))
        .await
}

#[tauri::command]
#[cfg(feature = "mutation")]
async fn recovery_pending(state: State<'_, AppState>) -> Result<RecoveryPending, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::recovery_pending(&app_state)).await
}

#[tauri::command]
#[cfg(feature = "mutation")]
async fn recovery_resolve(
    state: State<'_, AppState>,
    decision: String,
) -> Result<RecoveryResolveResult, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::recovery_resolve(&app_state, decision)).await
}

#[tauri::command]
#[cfg(feature = "mutation")]
fn mutation_token_issue(
    state: State<'_, AppState>,
    action: String,
) -> Result<MutationTokenResult, String> {
    hangar_api::mutation_token_issue(state.inner(), action)
}

#[tauri::command]
#[cfg(feature = "mutation")]
async fn mutation_backup_start(
    state: State<'_, AppState>,
    plan: OperationPlan,
    destination_root: String,
    level: String,
    allow_same_volume: Option<bool>,
    include_protected: bool,
    token: String,
) -> Result<MutationBackupSummary, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::mutation_backup_start(
            &app_state,
            plan,
            destination_root,
            level,
            allow_same_volume,
            include_protected,
            token,
        )
    })
    .await
}

#[tauri::command]
#[cfg(feature = "mutation")]
async fn mutation_move_start(
    state: State<'_, AppState>,
    plan: OperationPlan,
    holding_root: String,
    verified_backup_id: i64,
    include_protected: bool,
    token: String,
) -> Result<MutationMoveSummary, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::mutation_move_start(
            &app_state,
            plan,
            holding_root,
            verified_backup_id,
            include_protected,
            token,
        )
    })
    .await
}

/// Remove a project from the AI apps that register it (Antigravity for now), backing up
/// each app's registration first so it can be restored. The backup lives in a managed
/// folder under the app's data dir, so the user does not have to pick a location. The
/// removal is also persisted so it can always be recovered from the Recover view, even
/// after navigation or an app restart (not only via the in-session Undo).
#[tauri::command]
#[cfg(feature = "mutation")]
async fn remove_project_from_apps(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    project_id: i64,
) -> Result<Option<hangar_api::PersistedAppRemoval>, String> {
    let data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
    let backup_dir = data_dir.join("app-removal-backups");
    let app_state = state.inner().clone();
    run_blocking(move || {
        let project = hangar_api::project_get(&app_state, project_id)?
            .ok_or_else(|| "That project is no longer registered in Code Hangar.".to_string())?;
        // The Hermes step enumerates WSL state DBs behind the gated wsl_distros();
        // sync the persisted opt-in first so an opted-in user's removal covers WSL
        // even when this is the first attribution-bearing call of the process.
        hangar_api::sync_wsl_scan_flag(&app_state);
        let outcome = hangar_api::remove_project_app_registrations(
            &project.path,
            &backup_dir.to_string_lossy(),
        )?;
        // Persist whatever was actually changed on disk FIRST, so a partial failure (one app
        // could not be updated, e.g. it was running and held its config) still leaves every
        // completed change recoverable from the Recover view rather than silently lost.
        let persisted =
            hangar_api::record_app_removal(&backup_dir, &project.name, &outcome.records)?;
        if outcome.warnings.is_empty() {
            Ok(persisted)
        } else {
            Err(format!(
                "Removed and recorded {} change(s); some apps could not be updated: {}. What was removed is recoverable from Recover.",
                outcome.records.len(),
                outcome.warnings.join("; ")
            ))
        }
    })
    .await
}

/// Every "remove from AI apps" still pending recovery, for the Recover view.
#[tauri::command]
#[cfg(feature = "mutation")]
fn app_removals_list(
    app: tauri::AppHandle,
) -> Result<Vec<hangar_api::PersistedAppRemoval>, String> {
    let data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
    let backup_dir = data_dir.join("app-removal-backups");
    Ok(hangar_api::list_app_removals(&backup_dir))
}

/// Recover a persisted "remove from AI apps" by id: restore the registry files and clear it.
#[tauri::command]
#[cfg(feature = "mutation")]
fn app_removal_restore(app: tauri::AppHandle, id: String) -> Result<(), String> {
    let data_dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
    let backup_dir = data_dir.join("app-removal-backups");
    hangar_api::restore_app_removal_by_id(&backup_dir, &id)
}

#[tauri::command]
#[cfg(feature = "mutation")]
async fn mutation_preview_protected(
    state: State<'_, AppState>,
    plan: OperationPlan,
) -> Result<MutationProtectedPreview, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::mutation_preview_protected(&app_state, plan)).await
}

#[tauri::command]
#[cfg(feature = "mutation")]
async fn mutation_restore_start(
    state: State<'_, AppState>,
    entry_id: i64,
    token: String,
) -> Result<MutationRestoreSummary, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::mutation_restore_start(&app_state, entry_id, token)).await
}

#[tauri::command]
#[cfg(feature = "mutation")]
async fn mutation_restore_to_folder_start(
    state: State<'_, AppState>,
    entry_id: i64,
    destination_root: String,
    token: String,
) -> Result<MutationRestoreSummary, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::mutation_restore_to_folder_start(&app_state, entry_id, destination_root, token)
    })
    .await
}

#[tauri::command]
#[cfg(feature = "mutation")]
async fn mutation_final_remove_start(
    state: State<'_, AppState>,
    entry_id: i64,
    token: String,
) -> Result<MutationFinalRemoveSummary, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::mutation_final_remove_start(&app_state, entry_id, token)).await
}

#[tauri::command]
#[cfg(feature = "mutation")]
async fn mutation_final_remove_preview(
    state: State<'_, AppState>,
    scope: hangar_api::FinalRemoveScope,
) -> Result<hangar_api::FinalRemovePreview, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::mutation_final_remove_preview(&app_state, scope)).await
}

#[tauri::command]
#[cfg(feature = "mutation")]
async fn mutation_final_remove_confirm(
    state: State<'_, AppState>,
    preview_id: String,
    preview_digest: String,
    selected_topology_group_ids: Vec<String>,
) -> Result<hangar_api::FinalRemoveConfirmationToken, String> {
    let app_state = state.inner().clone();
    run_blocking(move || {
        hangar_api::mutation_final_remove_confirm(
            &app_state,
            preview_id,
            preview_digest,
            selected_topology_group_ids,
        )
    })
    .await
}

#[tauri::command]
#[cfg(feature = "mutation")]
async fn mutation_final_remove_batch_start(
    state: State<'_, AppState>,
    request: hangar_api::FinalRemoveBatchStartRequest,
) -> Result<hangar_api::FinalRemoveBatchStartSummary, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::mutation_final_remove_batch_start(&app_state, request)).await
}

#[tauri::command]
#[cfg(feature = "mutation")]
async fn mutation_final_remove_batch_status(
    state: State<'_, AppState>,
    job_id: String,
) -> Result<hangar_api::FinalRemoveBatchStatus, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::mutation_final_remove_batch_status(&app_state, job_id)).await
}

#[tauri::command]
#[cfg(feature = "mutation")]
async fn mutation_final_remove_batch_stop(
    state: State<'_, AppState>,
    job_id: String,
) -> Result<(), String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::mutation_final_remove_batch_stop(&app_state, job_id)).await
}

#[tauri::command]
#[cfg(feature = "mutation")]
async fn mutation_recovery_dashboard(
    state: State<'_, AppState>,
) -> Result<hangar_api::MutationRecoveryDashboard, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::mutation_recovery_dashboard(&app_state)).await
}

#[tauri::command]
#[cfg(feature = "mutation")]
async fn mutation_activity_log(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<MutationActivityLog, String> {
    let app_state = state.inner().clone();
    run_blocking(move || hangar_api::mutation_activity_log(&app_state, limit)).await
}

#[tauri::command]
#[cfg(feature = "mutation")]
fn mutation_lock_inspect_path(path: String) -> Result<MutationLockInspection, String> {
    hangar_api::mutation_lock_inspect_path(path)
}

#[tauri::command]
#[cfg(feature = "agent_automation")]
fn automation_status(state: State<'_, AppState>) -> Result<AutomationStatus, String> {
    hangar_api::automation_status(state.inner())
}

#[tauri::command]
#[cfg(feature = "agent_automation")]
fn automation_register(
    state: State<'_, AppState>,
    name: String,
    scopes: Vec<String>,
    project_ids: Vec<i64>,
) -> Result<AutomationCredential, String> {
    hangar_api::automation_register(state.inner(), name, scopes, project_ids)
}

#[tauri::command]
#[cfg(feature = "agent_automation")]
fn automation_agents(state: State<'_, AppState>) -> Result<Vec<AutomationAgentSummary>, String> {
    hangar_api::automation_agents(state.inner())
}

#[tauri::command]
#[cfg(feature = "agent_automation")]
fn automation_revoke(state: State<'_, AppState>, agent_id: i64) -> Result<bool, String> {
    hangar_api::automation_revoke(state.inner(), agent_id)
}

#[tauri::command]
#[cfg(feature = "agent_automation")]
fn automation_forget_revoked(state: State<'_, AppState>, agent_id: i64) -> Result<bool, String> {
    hangar_api::automation_forget_revoked(state.inner(), agent_id)
}

#[tauri::command]
#[cfg(feature = "agent_automation")]
fn automation_grant_read(
    state: State<'_, AppState>,
    agent_id: i64,
    node_id: i64,
    minutes: Option<u64>,
) -> Result<AutomationReadGrant, String> {
    hangar_api::automation_grant_read(state.inner(), agent_id, node_id, minutes)
}

#[tauri::command]
#[cfg(feature = "agent_automation")]
fn automation_activity(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<AutomationActivityEntry>, String> {
    hangar_api::automation_activity(state.inner(), limit)
}

fn main() {
    if let Err(error) = hangar_fs::enable_cloud_placeholder_visibility() {
        eprintln!("Cloud Files safety initialization failed: {error}");
    }
    let initial_shell_paths = shell_open::paths_from_process_args();
    let background_requested = shell_open::background_start_requested();
    let start_hidden = background_requested && initial_shell_paths.is_empty();
    let activate_existing = shell_open::should_activate_existing_instance(
        background_requested,
        !initial_shell_paths.is_empty(),
    );
    match shell_open::become_primary_or_forward(initial_shell_paths.clone(), activate_existing) {
        Ok(true) => {}
        Ok(false) => return,
        Err(error) => {
            eprintln!("{error}");
            return;
        }
    }

    let builder = tauri::Builder::<tauri::Wry>::default()
        .plugin(tauri_plugin_dialog::init::<tauri::Wry>())
        .on_window_event(resident::handle_window_event)
        .setup(move |app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let db_path = data_dir.join("codehangar.sqlite3");
            let state = AppState::open(db_path).map_err(std::io::Error::other)?;
            #[cfg(feature = "agent_automation")]
            hangar_api::start_local_automation(&state).map_err(std::io::Error::other)?;
            app.manage(shell_open::ShellOpenInbox::default());
            app.manage(state.clone());
            resident::setup(app, state, start_hidden)?;
            shell_open::initialize_primary(app.handle().clone(), initial_shell_paths.clone());
            Ok(())
        });

    #[cfg(not(feature = "mutation"))]
    let builder = builder.invoke_handler(tauri::generate_handler![
        startup_status,
        resident_started_hidden,
        resident_window_visible,
        shell_open_take_pending,
        open_local_file_preview,
        open_local_file_preview_full,
        inspect_open_target,
        prepare_open_target,
        start_open_target_scan,
        resolve_open_target,
        open_target_preview,
        shell_integration_status,
        shell_integration_set,
        background_startup_set,
        background_refresh_now,
        shell_default_guide_dismiss,
        shell_open_default_apps,
        projects_list,
        projects_list_lite,
        detect_installed_apps,
        wsl_scan_enabled,
        set_wsl_scan_enabled,
        projects_cached_snapshot,
        cache_discovery_snapshot,
        read_discovery_snapshot,
        project_get,
        project_context_summary,
        project_nav_tree,
        project_nav_children,
        project_nav_path,
        project_git_status,
        folder_explanation,
        investigate_folder,
        investigation_report,
        discard_investigation,
        node_full_path,
        open_node_external,
        reveal_node_external,
        reveal_project_external,
        reveal_session_external,
        dashboard_summary,
        adapters_list,
        project_context_files,
        file_preview,
        file_reveal,
        quick_open,
        performance_set_mode,
        system_resource_profile,
        process_resource_usage,
        session_preview,
        session_change_set,
        project_session_change_set,
        project_git_change_set,
        project_review_checkpoint,
        project_review_checkpoints,
        mark_project_reviewed,
        project_review_ledger,
        project_recap,
        project_review_receipt_export,
        watcher_status,
        recent_file_changes,
        search_documents,
        resolve_local_link,
        node_relationships,
        project_graph_map,
        graph_orphans,
        orphan_asset_candidates,
        node_orphan_status,
        lost_project_candidates,
        duplicate_candidates,
        confirm_duplicate_group,
        confirm_duplicate_group_start,
        confirm_duplicate_group_status,
        confirm_duplicate_group_cancel,
        project_recoverable_summary,
        node_recoverable_summary,
        operation_plan_build,
        operation_plan_start,
        operation_plan_status,
        operation_plan_cancel,
        risk_report_build,
        risk_report_build_for_target,
        risk_report_export,
        diagnostics_export,
        recent_items_list,
        pinned_items_list,
        pin_item,
        unpin_item,
        comments_for_node,
        comments_count_for_node,
        comment_add,
        comment_edit,
        comment_delete,
        project_discovery_report,
        project_discovery_deep_scan,
        roots_list,
        roots_add,
        roots_set_enabled,
        roots_unregister,
        projects_unregister,
        reset_all_projects,
        compact_database,
        restart_app,
        scan_start,
        scan_resume_subtree,
        scan_cancel,
        scan_status,
        zones_list,
        security_status,
        safe_manage_analysis_start,
        safe_manage_analysis_status,
        safe_manage_analysis_cancel,
        safe_manage_overview,
        safe_manage_first_run_get,
        safe_manage_first_run_set,
        safe_manage_decision_record,
        safe_manage_decisions_record_atomic,
        safe_manage_regenerable_targets,
        safe_manage_regenerable_scan_start,
        safe_manage_operation_plan_start
    ]);

    #[cfg(all(feature = "mutation", not(feature = "agent_automation")))]
    let builder = builder.invoke_handler(tauri::generate_handler![
        startup_status,
        resident_started_hidden,
        resident_window_visible,
        shell_open_take_pending,
        open_local_file_preview,
        open_local_file_preview_full,
        inspect_open_target,
        prepare_open_target,
        start_open_target_scan,
        resolve_open_target,
        open_target_preview,
        shell_integration_status,
        shell_integration_set,
        background_startup_set,
        background_refresh_now,
        shell_default_guide_dismiss,
        shell_open_default_apps,
        projects_list,
        projects_list_lite,
        detect_installed_apps,
        wsl_scan_enabled,
        set_wsl_scan_enabled,
        projects_cached_snapshot,
        cache_discovery_snapshot,
        read_discovery_snapshot,
        project_get,
        project_context_summary,
        project_nav_tree,
        project_nav_children,
        project_nav_path,
        project_git_status,
        folder_explanation,
        investigate_folder,
        investigation_report,
        discard_investigation,
        node_full_path,
        open_node_external,
        reveal_node_external,
        reveal_project_external,
        reveal_session_external,
        dashboard_summary,
        adapters_list,
        project_context_files,
        file_preview,
        file_reveal,
        quick_open,
        performance_set_mode,
        system_resource_profile,
        process_resource_usage,
        session_preview,
        session_change_set,
        project_session_change_set,
        project_git_change_set,
        project_review_checkpoint,
        project_review_checkpoints,
        mark_project_reviewed,
        project_review_ledger,
        project_recap,
        project_review_receipt_export,
        watcher_status,
        recent_file_changes,
        search_documents,
        resolve_local_link,
        node_relationships,
        project_graph_map,
        graph_orphans,
        orphan_asset_candidates,
        node_orphan_status,
        lost_project_candidates,
        duplicate_candidates,
        confirm_duplicate_group,
        confirm_duplicate_group_start,
        confirm_duplicate_group_status,
        confirm_duplicate_group_cancel,
        project_recoverable_summary,
        node_recoverable_summary,
        operation_plan_build,
        operation_plan_start,
        operation_plan_status,
        operation_plan_cancel,
        risk_report_build,
        risk_report_build_for_target,
        risk_report_export,
        diagnostics_export,
        recent_items_list,
        pinned_items_list,
        pin_item,
        unpin_item,
        comments_for_node,
        comments_count_for_node,
        comment_add,
        comment_edit,
        comment_delete,
        project_discovery_report,
        project_discovery_deep_scan,
        roots_list,
        roots_add,
        roots_set_enabled,
        roots_unregister,
        projects_unregister,
        reset_all_projects,
        compact_database,
        restart_app,
        scan_start,
        scan_resume_subtree,
        scan_cancel,
        scan_status,
        zones_list,
        security_status,
        safe_manage_analysis_start,
        safe_manage_analysis_status,
        safe_manage_analysis_cancel,
        safe_manage_overview,
        safe_manage_first_run_get,
        safe_manage_first_run_set,
        safe_manage_decision_record,
        safe_manage_decisions_record_atomic,
        safe_manage_regenerable_targets,
        safe_manage_regenerable_scan_start,
        safe_manage_operation_plan_start,
        mutation_mode_status,
        mutation_final_remove_enabled,
        mutation_set_final_remove_enabled,
        recovery_pending,
        recovery_resolve,
        mutation_token_issue,
        mutation_backup_start,
        mutation_move_start,
        remove_project_from_apps,
        app_removals_list,
        app_removal_restore,
        mutation_preview_protected,
        mutation_restore_start,
        mutation_restore_to_folder_start,
        mutation_final_remove_start,
        mutation_final_remove_preview,
        mutation_final_remove_confirm,
        mutation_final_remove_batch_start,
        mutation_final_remove_batch_status,
        mutation_final_remove_batch_stop,
        mutation_recovery_dashboard,
        mutation_activity_log,
        mutation_lock_inspect_path,
        file_edit_preview,
        write_file_content,
        edit_snapshots_for_node,
        edit_snapshot_restore,
        edit_snapshot_compare,
        editable_values,
        preview_value_edit,
        apply_value_edit,
        static_correction_check,
        project_checks_detect,
        project_check_approve,
        project_check_revoke,
        project_check_run
    ]);

    #[cfg(feature = "agent_automation")]
    let builder = builder.invoke_handler(tauri::generate_handler![
        startup_status,
        resident_started_hidden,
        resident_window_visible,
        shell_open_take_pending,
        open_local_file_preview,
        open_local_file_preview_full,
        inspect_open_target,
        prepare_open_target,
        start_open_target_scan,
        resolve_open_target,
        open_target_preview,
        shell_integration_status,
        shell_integration_set,
        background_startup_set,
        background_refresh_now,
        shell_default_guide_dismiss,
        shell_open_default_apps,
        projects_list,
        projects_list_lite,
        detect_installed_apps,
        wsl_scan_enabled,
        set_wsl_scan_enabled,
        projects_cached_snapshot,
        cache_discovery_snapshot,
        read_discovery_snapshot,
        project_get,
        project_context_summary,
        project_nav_tree,
        project_nav_children,
        project_nav_path,
        project_git_status,
        folder_explanation,
        investigate_folder,
        investigation_report,
        discard_investigation,
        node_full_path,
        open_node_external,
        reveal_node_external,
        reveal_project_external,
        reveal_session_external,
        dashboard_summary,
        adapters_list,
        project_context_files,
        file_preview,
        file_reveal,
        quick_open,
        performance_set_mode,
        system_resource_profile,
        process_resource_usage,
        session_preview,
        session_change_set,
        project_session_change_set,
        project_git_change_set,
        project_review_checkpoint,
        project_review_checkpoints,
        mark_project_reviewed,
        project_review_ledger,
        project_recap,
        project_review_receipt_export,
        watcher_status,
        recent_file_changes,
        search_documents,
        resolve_local_link,
        node_relationships,
        project_graph_map,
        graph_orphans,
        orphan_asset_candidates,
        node_orphan_status,
        lost_project_candidates,
        duplicate_candidates,
        confirm_duplicate_group,
        confirm_duplicate_group_start,
        confirm_duplicate_group_status,
        confirm_duplicate_group_cancel,
        project_recoverable_summary,
        node_recoverable_summary,
        operation_plan_build,
        operation_plan_start,
        operation_plan_status,
        operation_plan_cancel,
        risk_report_build,
        risk_report_build_for_target,
        risk_report_export,
        diagnostics_export,
        recent_items_list,
        pinned_items_list,
        pin_item,
        unpin_item,
        comments_for_node,
        comments_count_for_node,
        comment_add,
        comment_edit,
        comment_delete,
        comment_write_enabled,
        set_comment_write_enabled,
        project_discovery_report,
        project_discovery_deep_scan,
        roots_list,
        roots_add,
        roots_set_enabled,
        roots_unregister,
        projects_unregister,
        reset_all_projects,
        compact_database,
        restart_app,
        scan_start,
        scan_resume_subtree,
        scan_cancel,
        scan_status,
        zones_list,
        security_status,
        safe_manage_analysis_start,
        safe_manage_analysis_status,
        safe_manage_analysis_cancel,
        safe_manage_overview,
        safe_manage_first_run_get,
        safe_manage_first_run_set,
        safe_manage_decision_record,
        safe_manage_decisions_record_atomic,
        safe_manage_regenerable_targets,
        safe_manage_regenerable_scan_start,
        safe_manage_operation_plan_start,
        mutation_mode_status,
        mutation_final_remove_enabled,
        mutation_set_final_remove_enabled,
        recovery_pending,
        recovery_resolve,
        mutation_token_issue,
        mutation_backup_start,
        mutation_move_start,
        remove_project_from_apps,
        app_removals_list,
        app_removal_restore,
        mutation_preview_protected,
        mutation_restore_start,
        mutation_restore_to_folder_start,
        mutation_final_remove_start,
        mutation_final_remove_preview,
        mutation_final_remove_confirm,
        mutation_final_remove_batch_start,
        mutation_final_remove_batch_status,
        mutation_final_remove_batch_stop,
        mutation_recovery_dashboard,
        mutation_activity_log,
        mutation_lock_inspect_path,
        file_edit_preview,
        write_file_content,
        edit_snapshots_for_node,
        edit_snapshot_restore,
        edit_snapshot_compare,
        editable_values,
        preview_value_edit,
        apply_value_edit,
        static_correction_check,
        project_checks_detect,
        project_check_approve,
        project_check_revoke,
        project_check_run,
        automation_status,
        automation_register,
        automation_agents,
        automation_revoke,
        automation_forget_revoked,
        automation_grant_read,
        automation_activity,
        mcp_appconfig_status,
        mcp_appconfig_register,
        mcp_appconfig_remove,
        mcp_appconfig_revoke_orphan,
        mcp_appconfig_forget_orphan,
        agent_requests_pending,
        agent_request_resolve,
        mcp_full_control_enabled,
        set_mcp_full_control_enabled,
        mcp_read_only_mode,
        set_mcp_read_only_mode,
        ai_explain_preview,
        ai_send_disclosure,
        ai_read_stream,
        ai_walkthrough_preview,
        ai_walkthrough_disclosure,
        ai_walkthrough_file,
        ai_follow_up_preview,
        ai_follow_up_disclosure,
        ai_follow_up,
        ai_glossary_state,
        set_ai_glossary_enabled,
        ai_glossary_record,
        ai_annotations_for_node,
        ai_annotation_add,
        ai_annotation_delete,
        ai_change_set_preview,
        ai_change_disclosure,
        ai_narrate_session_changes,
        ai_explain_change,
        ai_review_change_set,
        ai_rewrite_disclosure,
        ai_rewrite_text,
        apply_ai_suggestion,
        ai_edit_sessions_for_node,
        undo_ai_edit_session,
        ai_summarize_project,
        ai_summarize_project_preview,
        ai_summarize_project_disclosure,
        ai_safe_manage_context_candidates,
        ai_safe_manage_advisory_disclosure,
        ai_safe_manage_advisory,
        ai_safe_manage_advisory_receipts,
        ai_key_set,
        ai_key_status,
        ai_key_clear,
        ai_provider_get,
        ai_provider_set,
        ai_provider_test_disclosure,
        ai_provider_test,
        ai_provider_models_disclosure,
        ai_provider_models,
        ai_local_discover,
        ai_usage_status,
        ai_usage_set_soft_cap,
        ai_usage_reset
    ]);

    let app = builder
        .build(tauri::generate_context!())
        .expect("error while building Code Hangar");
    app.run(|app_handle, event| {
        if let tauri::RunEvent::ExitRequested { api, .. } = event {
            let control = app_handle.state::<resident::ResidentControl>();
            if !control.is_quitting() {
                // The resident `--background` path intentionally owns no WebView
                // until first use. Closing/destroying the last window must not
                // terminate the native tray service.
                api.prevent_exit();
            }
        }
    });
}
