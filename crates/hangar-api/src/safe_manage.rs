use super::{project_review, to_message, AppState};
use hangar_core::{
    assess_safe_manage_project, safe_manage_portfolio_counts, safe_manage_portfolio_counts_include,
    ProjectDiscoveryReport, ProjectSummary, SafeManageAnalysisRun, SafeManageClassificationContext,
    SafeManageDecision, SafeManageDecisionKind, SafeManageDecisionRequest,
    SafeManageFirstRunPreference, SafeManageObjectiveInput, SafeManageOperationPlanRequest,
    SafeManageOverview, SafeManagePortfolioCounts, SafeManageRegenerableScanRequest,
    SafeManageRegenerableTarget, SAFE_MANAGE_RULESET_VERSION,
};
use hangar_jobs::RunningJobAdmission;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SAFE_MANAGE_DISCOVERY_SESSION_LIMIT: usize = 5_000;
const SAFE_MANAGE_DISCOVERY_BUDGET: Duration = Duration::from_secs(20);

/// Selects the source used for Safe Manage's session-correlation pass.
///
/// Normal application states always use `Dedicated`, which performs a fresh,
/// bounded discovery with explicit provenance options. Debug-only integration
/// states may carry one immutable report so native E2E tests never enumerate a
/// developer's real coding-tool stores.
#[derive(Clone, Default)]
pub(crate) enum SafeManageDiscoverySource {
    #[default]
    Dedicated,
    #[cfg(debug_assertions)]
    Fixture(Arc<ProjectDiscoveryReport>),
}

impl SafeManageDiscoverySource {
    fn load_report(
        &self,
        load_dedicated: impl FnOnce() -> Result<ProjectDiscoveryReport, String>,
    ) -> Result<ProjectDiscoveryReport, String> {
        match self {
            Self::Dedicated => load_dedicated(),
            #[cfg(debug_assertions)]
            Self::Fixture(report) => Ok(report.as_ref().clone()),
        }
    }

    #[cfg(debug_assertions)]
    pub(crate) fn fixture(report: ProjectDiscoveryReport) -> Self {
        Self::Fixture(Arc::new(report))
    }
}

#[derive(Clone, Default)]
pub(crate) struct SafeManageJobStore {
    active: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
    next_id: Arc<AtomicU64>,
}

impl SafeManageJobStore {
    fn reserve(&self) -> Result<(String, Arc<AtomicBool>), String> {
        let mut active = self
            .active
            .lock()
            .map_err(|_| "Safe Manage job state is unavailable.".to_string())?;
        if !active.is_empty() {
            return Err("A Safe Manage analysis is already active.".to_string());
        }
        let sequence = self.next_id.fetch_add(1, Ordering::Relaxed);
        let created = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "The system clock is unavailable for the analysis id.".to_string())?
            .as_nanos();
        let id = format!(
            "safe-manage-{:x}-{:x}-{:x}",
            std::process::id(),
            created,
            sequence
        );
        let cancel = Arc::new(AtomicBool::new(false));
        active.insert(id.clone(), Arc::clone(&cancel));
        Ok((id, cancel))
    }

    fn cancel(&self, id: &str) -> Result<bool, String> {
        let active = self
            .active
            .lock()
            .map_err(|_| "Safe Manage job state is unavailable.".to_string())?;
        let Some(cancel) = active.get(id) else {
            return Ok(false);
        };
        cancel.store(true, Ordering::Release);
        Ok(true)
    }

    fn cancellation_requested(&self, id: &str) -> Result<bool, String> {
        let active = self
            .active
            .lock()
            .map_err(|_| "Safe Manage job state is unavailable.".to_string())?;
        Ok(active
            .get(id)
            .is_some_and(|cancel| cancel.load(Ordering::Acquire)))
    }

    fn finish(&self, id: &str) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(id);
        }
    }
}

type SafeManageAnalysisWorker = Box<dyn FnOnce() -> Result<(), String> + Send + 'static>;
type SafeManageThreadTask = Box<dyn FnOnce() + Send + 'static>;

/// Start an admitted analysis without leaving a queued/running record forever
/// if the OS refuses a thread or the worker panics. The injected spawner makes
/// both lifecycle failures deterministic in unit tests.
fn spawn_analysis_worker_with(
    state: &AppState,
    id: &str,
    worker: SafeManageAnalysisWorker,
    spawn: impl FnOnce(SafeManageThreadTask) -> std::io::Result<()>,
) -> Result<(), String> {
    let guarded_state = state.clone();
    let guarded_id = id.to_string();
    let guarded: SafeManageThreadTask = Box::new(move || {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(worker)) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => fail_analysis(&guarded_state, &guarded_id, error),
            Err(_) => fail_analysis(
                &guarded_state,
                &guarded_id,
                "Safe Manage analysis stopped after an unexpected internal panic.".to_string(),
            ),
        }
        guarded_state.safe_manage_jobs.finish(&guarded_id);
    });

    if let Err(error) = spawn(guarded) {
        let message =
            format!("Safe Manage analysis worker could not start; no analysis was run: {error}");
        fail_analysis(state, id, message.clone());
        state.safe_manage_jobs.finish(id);
        return Err(message);
    }
    Ok(())
}

fn spawn_analysis_worker(
    state: &AppState,
    id: &str,
    worker: SafeManageAnalysisWorker,
) -> Result<(), String> {
    spawn_analysis_worker_with(state, id, worker, |task| {
        thread::Builder::new()
            .name("code-hangar-safe-manage".to_string())
            .spawn(task)
            .map(|_| ())
    })
}

#[derive(Default)]
struct SessionEvidence {
    counts: HashMap<i64, u64>,
    last_activity_ms: HashMap<i64, i64>,
    apps: HashMap<i64, HashSet<String>>,
    complete: bool,
}

pub fn analysis_start(state: &AppState) -> Result<String, String> {
    let (id, cancel) = state.safe_manage_jobs.reserve()?;
    let created_at = chrono::Utc::now().to_rfc3339();
    let queued = SafeManageAnalysisRun {
        id: id.clone(),
        state: "queued".to_string(),
        ruleset_version: SAFE_MANAGE_RULESET_VERSION.to_string(),
        catalog_revision: "pending-local-snapshot".to_string(),
        created_at,
        started_at: None,
        completed_at: None,
        processed_projects: 0,
        total_projects: 0,
        counts: SafeManagePortfolioCounts::default(),
        message: "Queued local portfolio analysis.".to_string(),
        error: None,
        assessments: Vec::new(),
    };
    if let Err(error) = state
        .db()?
        .safe_manage_analysis_header_save(&queued)
        .map_err(to_message)
    {
        state.safe_manage_jobs.finish(&id);
        return Err(error);
    }

    let worker_state = state.clone();
    let worker_id = id.clone();
    spawn_analysis_worker(
        state,
        &id,
        Box::new(move || run_analysis(&worker_state, &worker_id, &cancel)),
    )?;
    Ok(id)
}

/// Admit an OperationPlan only through a current, persisted owner decision.
/// The database resolves the authoritative target; the caller cannot provide a
/// free-form action label or substitute a whole project for a regenerable
/// folder. The shared plan worker revalidates this same binding around the
/// plan/risk snapshot before it can publish a completed preview.
pub fn operation_plan_start(
    state: &AppState,
    request: SafeManageOperationPlanRequest,
    performance_mode: Option<String>,
) -> Result<String, String> {
    let target_node_id = {
        let _inventory_guard = state
            .inventory_mutation_gate
            .read()
            .map_err(|_| "Inventory coordination is unavailable.".to_string())?;
        require_current_project_revision(state, request.project_id, &request.evidence_revision)?;
        state
            .db()?
            .safe_manage_operation_plan_target(&request)
            .map_err(to_message)?
    };
    let action_label = match request.decision {
        SafeManageDecisionKind::Archive => "Safe Manage project archive review",
        SafeManageDecisionKind::CleanRegenerables => {
            "Safe Manage exact regenerable-folder cleanup review"
        }
        SafeManageDecisionKind::PrepareRemoval => "Safe Manage project removal preparation review",
        _ => {
            return Err(
                "This Safe Manage decision does not authorize an OperationPlan.".to_string(),
            )
        }
    }
    .to_string();

    super::operation_plan_start_with_safe_manage_binding(
        state,
        target_node_id,
        action_label,
        performance_mode,
        Some(request),
    )
}

pub fn analysis_status(state: &AppState, id: &str) -> Result<SafeManageAnalysisRun, String> {
    let mut run = state
        .db()?
        .safe_manage_analysis_get(id)
        .map_err(to_message)?
        .ok_or_else(|| "That Safe Manage analysis was not found.".to_string())?;
    if matches!(run.state.as_str(), "queued" | "running")
        && state.safe_manage_jobs.cancellation_requested(id)?
    {
        // Cancellation intent is process-local until the worker reaches its next
        // evidence boundary. Persisting `cancelling` here would race the worker's
        // queued -> running transition; the terminal cancelled/partial state is
        // the only durable result.
        run.state = "cancelling".to_string();
        run.message = "Stopping after the current local evidence check.".to_string();
    }
    Ok(run)
}

pub fn analysis_cancel(state: &AppState, id: &str) -> Result<SafeManageAnalysisRun, String> {
    state.safe_manage_jobs.cancel(id)?;
    analysis_status(state, id)
}

pub fn overview(state: &AppState) -> Result<SafeManageOverview, String> {
    let db = state.db()?;
    Ok(SafeManageOverview {
        latest_run: db.safe_manage_analysis_latest().map_err(to_message)?,
        last_complete_run: db
            .safe_manage_analysis_latest_complete()
            .map_err(to_message)?,
        decisions: db.safe_manage_decisions_latest().map_err(to_message)?,
        first_run: db.safe_manage_first_run_preference().map_err(to_message)?,
    })
}

pub fn first_run_preference(state: &AppState) -> Result<SafeManageFirstRunPreference, String> {
    state
        .db()?
        .safe_manage_first_run_preference()
        .map_err(to_message)
}

pub fn first_run_preference_set(
    state: &AppState,
    suggest_after_discovery: bool,
    prompt_state: &str,
    mark_prompted_now: bool,
) -> Result<SafeManageFirstRunPreference, String> {
    state
        .db()?
        .safe_manage_first_run_preference_set(
            suggest_after_discovery,
            prompt_state,
            mark_prompted_now,
        )
        .map_err(to_message)
}

pub fn decision_record(
    state: &AppState,
    project_id: i64,
    analysis_run_id: &str,
    decision: SafeManageDecisionKind,
    evidence_revision: &str,
) -> Result<SafeManageDecision, String> {
    let request = SafeManageDecisionRequest {
        project_id,
        analysis_run_id: analysis_run_id.to_string(),
        decision,
        evidence_revision: evidence_revision.to_string(),
    };
    decisions_record_atomic(state, vec![request])?
        .pop()
        .ok_or_else(|| "The Safe Manage decision was not recorded.".to_string())
}

pub fn decisions_record_atomic(
    state: &AppState,
    requests: Vec<SafeManageDecisionRequest>,
) -> Result<Vec<SafeManageDecision>, String> {
    if requests.is_empty() || requests.len() > 1_000 {
        return Err("Select between 1 and 1,000 Safe Manage projects.".to_string());
    }
    let mut projects = HashSet::with_capacity(requests.len());
    let expected_run = requests[0].analysis_run_id.as_str();
    if expected_run.trim().is_empty()
        || requests.iter().any(|request| {
            request.analysis_run_id != expected_run || !projects.insert(request.project_id)
        })
    {
        return Err(
            "Grouped Safe Manage decisions must be duplicate-free and belong to one analysis run."
                .to_string(),
        );
    }
    let _inventory_guard = state
        .inventory_mutation_gate
        .read()
        .map_err(|_| "Inventory coordination is unavailable.".to_string())?;
    for request in &requests {
        require_current_project_revision(state, request.project_id, &request.evidence_revision)?;
    }
    state
        .db()?
        .safe_manage_decisions_record_atomic(&requests)
        .map_err(to_message)
}

pub fn regenerable_targets(
    state: &AppState,
    project_id: i64,
    analysis_run_id: &str,
    evidence_revision: &str,
) -> Result<Vec<SafeManageRegenerableTarget>, String> {
    let _inventory_guard = state
        .inventory_mutation_gate
        .read()
        .map_err(|_| "Inventory coordination is unavailable.".to_string())?;
    require_current_project_revision(state, project_id, evidence_revision)?;
    state
        .db()?
        .safe_manage_regenerable_targets(project_id, analysis_run_id, evidence_revision)
        .map_err(to_message)
}

pub fn regenerable_scan_start(
    state: &AppState,
    request: SafeManageRegenerableScanRequest,
) -> Result<String, String> {
    let (initial_target, initial_public) = {
        let _inventory_guard = state
            .inventory_mutation_gate
            .read()
            .map_err(|_| "Inventory coordination is unavailable.".to_string())?;
        require_current_project_revision(state, request.project_id, &request.evidence_revision)?;
        state
            .db()?
            .safe_manage_regenerable_scan_target(&request)
            .map_err(to_message)?
    };
    let admission = state.jobs.admit_running_for_roots_with_estimate(
        format!(
            "Preparing exact regenerable inventory for {}.",
            initial_public.path
        ),
        vec![initial_target.root_id],
        vec![initial_target.display_root_path.clone()],
        None,
    );
    let job_id = match admission {
        RunningJobAdmission::Created(job_id) => job_id,
        RunningJobAdmission::Existing { .. } => {
            return Err(format!(
                "A scan is already running for {}.",
                initial_target.display_root_path
            ));
        }
    };
    state.jobs.set_worker_count(&job_id, 1);

    let worker_state = state.clone();
    let worker_job_id = job_id.clone();
    thread::spawn(move || {
        let jobs = worker_state.jobs.clone();
        let _inventory_guard = match worker_state.inventory_mutation_gate.read() {
            Ok(guard) => guard,
            Err(_) => {
                jobs.fail(
                    &worker_job_id,
                    "Inventory coordination is unavailable.".to_string(),
                );
                return;
            }
        };
        if jobs.is_cancelled(&worker_job_id) {
            jobs.cancel(&worker_job_id, 0, 0);
            return;
        }
        if let Err(error) = require_current_project_revision(
            &worker_state,
            request.project_id,
            &request.evidence_revision,
        ) {
            jobs.fail(&worker_job_id, error);
            return;
        }
        let db = match worker_state.db() {
            Ok(db) => db,
            Err(error) => {
                jobs.fail(&worker_job_id, error);
                return;
            }
        };
        let (target, public_target) = match db.safe_manage_regenerable_scan_target(&request) {
            Ok(target) => target,
            Err(error) => {
                jobs.fail(&worker_job_id, to_message(error));
                return;
            }
        };
        if target.root_id != initial_target.root_id
            || target.project_id != initial_target.project_id
            || target.nav_id != initial_target.nav_id
            || public_target.node_id != initial_public.node_id
            || public_target.path != initial_public.path
        {
            jobs.fail(
                &worker_job_id,
                "The regenerable target identity changed before the worker started.".to_string(),
            );
            return;
        }

        let mut writer = match db.open_write_session() {
            Ok(writer) => writer,
            Err(error) => {
                jobs.fail(&worker_job_id, to_message(error));
                return;
            }
        };
        if !matches!(writer.root_is_enabled(target.root_id), Ok(true)) {
            jobs.fail(
                &worker_job_id,
                "The project scan root is no longer enabled.".to_string(),
            );
            return;
        }
        if let Err(error) =
            writer.begin_safe_manage_regenerable_scan(target.project_id, target.nav_id)
        {
            jobs.fail(&worker_job_id, to_message(error));
            return;
        }
        if let Err(error) = writer.safe_manage_regenerable_expansion_begin(&request) {
            let _ = writer.mark_subtree_scan_incomplete(
                target.nav_id,
                "Regenerable expansion receipt could not be recorded.",
            );
            jobs.fail(&worker_job_id, to_message(error));
            return;
        }

        jobs.update_phase(
            &worker_job_id,
            "scanning",
            Some(public_target.path.clone()),
            "Expanding one allowlisted regenerable target into metadata-only inventory.",
        );
        let mut persisted_scanned = 0_u64;
        let mut persisted_indexed = 0_u64;
        let progress_jobs = jobs.clone();
        let progress_job_id = worker_job_id.clone();
        let cancel_jobs = jobs.clone();
        let cancel_job_id = worker_job_id.clone();
        let batch_jobs = jobs.clone();
        let batch_job_id = worker_job_id.clone();
        let scan_result = hangar_fs::scan_regenerable_inventory_stream(
            Path::new(&target.root_path),
            &target.relative_path,
            hangar_fs::ScanLimits::regenerable_expansion(),
            || cancel_jobs.is_cancelled(&cancel_job_id),
            |scanned, indexed, current_path| {
                progress_jobs.update_progress(
                    &progress_job_id,
                    scanned,
                    indexed,
                    Some(current_path.to_string()),
                    "Reading exact local metadata for one regenerable target.",
                );
            },
            |batch| {
                if !matches!(writer.root_is_enabled(target.root_id), Ok(true)) {
                    return Err("The project scan root is no longer enabled.".to_string());
                }
                let (scanned, indexed) = writer
                    .persist_batch(target.project_id, &batch)
                    .map_err(to_message)?;
                persisted_scanned = persisted_scanned.saturating_add(scanned);
                persisted_indexed = persisted_indexed.saturating_add(indexed);
                batch_jobs.update_progress(
                    &batch_job_id,
                    persisted_scanned,
                    persisted_indexed,
                    None,
                    "Persisting exact regenerable metadata inventory.",
                );
                Ok(())
            },
        );

        let outcome = match scan_result {
            Ok(outcome) => outcome,
            Err(error) => {
                let _ = writer.finish_subtree_scan(target.project_id, target.nav_id, Some(&error));
                let _ = writer.safe_manage_regenerable_expansion_finish(
                    &request,
                    "failed",
                    persisted_scanned,
                    Some(&error),
                );
                jobs.fail(&worker_job_id, error);
                return;
            }
        };

        let partial_error = if outcome.cancelled {
            Some("Cancelled")
        } else {
            outcome.partial_error.as_deref()
        };
        jobs.update_phase(
            &worker_job_id,
            "finalizing",
            None,
            "Finalizing exact regenerable counts and evidence state.",
        );
        let finish = writer.finish_subtree_scan_interruptible_with_progress(
            target.project_id,
            target.nav_id,
            partial_error,
            jobs.cancel_token(&worker_job_id),
            |_| {},
        );
        if let Err(error) = finish {
            let message = to_message(error);
            let _ = writer.mark_subtree_scan_incomplete(target.nav_id, &message);
            let state_name = if jobs.is_cancelled(&worker_job_id) {
                "cancelled"
            } else {
                "failed"
            };
            let _ = writer.safe_manage_regenerable_expansion_finish(
                &request,
                state_name,
                outcome.scanned_files,
                Some(&message),
            );
            if state_name == "cancelled" {
                jobs.cancel(
                    &worker_job_id,
                    outcome.scanned_files,
                    outcome.indexed_documents,
                );
            } else {
                jobs.fail(&worker_job_id, message);
            }
            return;
        }

        let terminal = if outcome.cancelled || jobs.is_cancelled(&worker_job_id) {
            "cancelled"
        } else if outcome.partial {
            "partial"
        } else {
            "completed"
        };
        if let Err(error) = writer.safe_manage_regenerable_expansion_finish(
            &request,
            terminal,
            outcome.scanned_files,
            outcome.partial_error.as_deref(),
        ) {
            jobs.fail(&worker_job_id, to_message(error));
            return;
        }
        match terminal {
            "cancelled" => jobs.cancel(
                &worker_job_id,
                outcome.scanned_files,
                outcome.indexed_documents,
            ),
            "partial" => jobs.complete_partial(
                &worker_job_id,
                outcome.scanned_files,
                outcome.indexed_documents,
                outcome.partial_error.unwrap_or_else(|| {
                    "Regenerable inventory is partial and cannot feed an OperationPlan.".to_string()
                }),
            ),
            _ => jobs.complete(
                &worker_job_id,
                outcome.scanned_files,
                outcome.indexed_documents,
            ),
        }
    });

    Ok(job_id)
}

fn run_analysis(state: &AppState, id: &str, cancel: &Arc<AtomicBool>) -> Result<(), String> {
    run_analysis_with_sources(
        state,
        id,
        cancel.as_ref(),
        || super::projects_list(state),
        |projects| load_session_evidence(state, Some(id), projects, cancel.as_ref()),
        |_| {},
    )
}

/// Execute the real portfolio pipeline with injectable, local evidence sources.
/// Production supplies the desktop catalog/session loaders above. Tests supply
/// disposable synthetic catalogs so large-portfolio and cancellation behavior
/// can be exercised deterministically without inspecting developer data. The
/// progress callback runs only after one complete assessment has committed.
fn run_analysis_with_sources<P, S, H>(
    state: &AppState,
    id: &str,
    cancel: &AtomicBool,
    mut load_projects: P,
    mut load_sessions: S,
    mut after_assessment_persisted: H,
) -> Result<(), String>
where
    P: FnMut() -> Result<Vec<ProjectSummary>, String>,
    S: FnMut(&[ProjectSummary]) -> Result<SessionEvidence, String>,
    H: FnMut(&SafeManageAnalysisRun),
{
    let mut run = analysis_status(state, id)?;
    if cancel.load(Ordering::Acquire) {
        return finish_cancelled(state, run);
    }
    run.state = "running".to_string();
    run.started_at = Some(chrono::Utc::now().to_rfc3339());
    run.message = "Reading the current local project catalog.".to_string();
    save_run_header(state, &run)?;

    if cancel.load(Ordering::Acquire) {
        return finish_cancelled(state, run);
    }

    let projects = {
        let _inventory_guard = state
            .inventory_mutation_gate
            .read()
            .map_err(|_| "Inventory coordination is unavailable.".to_string())?;
        safe_manage_real_projects(load_projects()?)
    };
    run.total_projects = projects.len() as u64;
    run.message = format!(
        "Found {} project(s). Correlating local sessions.",
        run.total_projects
    );
    save_run_header(state, &run)?;

    let sessions = load_sessions(&projects)?;
    if cancel.load(Ordering::Acquire) {
        return finish_cancelled(state, run);
    }

    let (projects, inputs) = {
        let _inventory_guard = state
            .inventory_mutation_gate
            .write()
            .map_err(|_| "Inventory coordination is unavailable.".to_string())?;
        // Refresh the project list after local session discovery, then close
        // pending relationships and capture every objective input under this
        // same exclusive inventory boundary. A root scan cannot invalidate the
        // relation snapshot between preparation and evidence hashing.
        let projects = safe_manage_real_projects(load_projects()?);
        let mut inputs = match state.db()?.safe_manage_prepared_objective_inputs(
            &projects,
            &sessions.counts,
            cancel,
        ) {
            Ok(inputs) => inputs,
            Err(_) if cancel.load(Ordering::Acquire) => return finish_cancelled(state, run),
            Err(error) => return Err(to_message(error)),
        };
        enrich_session_activity(&mut inputs, &sessions);
        for input in &mut inputs {
            if cancel.load(Ordering::Acquire) {
                return finish_cancelled(state, run);
            }
            probe_git_state(input);
        }
        (projects, inputs)
    };
    run.total_projects = projects.len() as u64;
    run.catalog_revision = catalog_revision(&inputs)?;
    run.message = "Evaluating objective evidence project by project.".to_string();
    save_run_header(state, &run)?;

    let observed_at = chrono::Utc::now().to_rfc3339();
    let now_ms = chrono::Utc::now().timestamp_millis();
    for input in inputs {
        if cancel.load(Ordering::Acquire) {
            return finish_cancelled(state, run);
        }
        let evidence_revision = project_evidence_revision(&input)?;
        let assessment = assess_safe_manage_project(
            &input,
            &SafeManageClassificationContext {
                now_ms,
                analysis_run_id: id.to_string(),
                evidence_revision,
                observed_at: Some(observed_at.clone()),
            },
        );
        safe_manage_portfolio_counts_include(&mut run.counts, &assessment);
        run.assessments.push(assessment);
        run.processed_projects = run.assessments.len() as u64;
        run.message = format!(
            "Analyzed {} of {} project(s). No files were changed.",
            run.processed_projects, run.total_projects
        );
        append_latest_assessment(state, &run)?;
        after_assessment_persisted(&run);
    }

    run.state = "completed".to_string();
    run.completed_at = Some(chrono::Utc::now().to_rfc3339());
    run.message = format!(
        "Analyzed {} project(s) from local evidence. Review recommendations before preparing any action.",
        run.total_projects
    );
    finalize_run(state, &run)?;
    // A completed analysis satisfies the one-time first-run suggestion. This is
    // backend-owned so closing the window between completion and the next UI
    // refresh cannot make the prompt recur incorrectly.
    if let Ok(db) = state.db() {
        let _ = db.safe_manage_first_run_preference_set(true, "completed", false);
    }
    Ok(())
}

fn load_session_evidence(
    state: &AppState,
    analysis_id: Option<&str>,
    projects: &[ProjectSummary],
    cancel: &AtomicBool,
) -> Result<SessionEvidence, String> {
    // The UI discovery cache does not bind the report to its include-loose,
    // include-agent, session-limit and registered-root provenance. It therefore
    // cannot prove a complete portfolio absence. Safe Manage always performs
    // its own bounded local discovery and preserves unknown evidence if that
    // dedicated pass fails or is truncated.
    let report = match state.safe_manage_discovery_source.load_report(|| {
        super::sync_wsl_scan_flag(state);
        let registered_roots = super::registered_roots_for_state(state)?;
        let outcome = hangar_discovery::discover_known_sessions_bounded(
            &registered_roots,
            hangar_discovery::DiscoveryOptions {
                limit: 0,
                session_limit: SAFE_MANAGE_DISCOVERY_SESSION_LIMIT,
                include_loose_sessions: true,
                include_agents: true,
                include_technical_candidates: false,
            },
            SAFE_MANAGE_DISCOVERY_BUDGET,
            || cancel.load(Ordering::Acquire),
            |progress| {
                if let Some(analysis_id) = analysis_id {
                    if let Ok(mut run) = analysis_status(state, analysis_id) {
                        if run.state == "running" {
                            run.message = format!(
                                "Correlating local sessions: {} of {} bounded source(s) checked ({}).",
                                progress.processed_sources,
                                progress.total_sources,
                                progress.source_label
                            );
                            let _ = save_run_header(state, &run);
                        }
                    }
                }
            },
        );
        Ok(outcome.report)
    }) {
        Ok(report) => report,
        Err(_) => return Ok(SessionEvidence::default()),
    };
    session_evidence_from_report(projects, &report)
}

fn safe_manage_real_projects(projects: Vec<ProjectSummary>) -> Vec<ProjectSummary> {
    projects
        .into_iter()
        .filter(|project| !project.source.trim().eq_ignore_ascii_case("fixture"))
        .collect()
}

fn report_has_complete_sessions(report: &ProjectDiscoveryReport) -> bool {
    report.total_sessions == report.sessions.len() as u64
}

fn session_evidence_from_report(
    projects: &[ProjectSummary],
    report: &ProjectDiscoveryReport,
) -> Result<SessionEvidence, String> {
    session_evidence_from_report_at(projects, report, chrono::Utc::now().timestamp_millis())
}

fn session_evidence_from_report_at(
    projects: &[ProjectSummary],
    report: &ProjectDiscoveryReport,
    now_ms: i64,
) -> Result<SessionEvidence, String> {
    if !report_has_complete_sessions(report) {
        return Ok(SessionEvidence::default());
    }
    let mut evidence = SessionEvidence {
        counts: projects.iter().map(|project| (project.id, 0)).collect(),
        complete: true,
        ..SessionEvidence::default()
    };
    let project_keys = projects
        .iter()
        .map(|project| {
            (
                project.id,
                hangar_discovery::project_path_key(&project.path),
            )
        })
        .collect::<Vec<_>>();
    let known_ids = project_keys
        .iter()
        .map(|(project_id, _)| *project_id)
        .collect::<HashSet<_>>();

    for session in &report.sessions {
        let mut matches = session
            .linked_registered_project_ids
            .iter()
            .filter(|project_id| known_ids.contains(project_id))
            .copied()
            .collect::<HashSet<_>>();
        for linked in &session.linked_project_paths {
            let linked_key = hangar_discovery::project_path_key(linked);
            for (project_id, project_key) in &project_keys {
                if path_keys_overlap(&linked_key, project_key) {
                    matches.insert(*project_id);
                }
            }
        }
        for project_id in matches {
            *evidence.counts.entry(project_id).or_default() += 1;
            if !session.source_kind.trim().is_empty() {
                evidence
                    .apps
                    .entry(project_id)
                    .or_default()
                    .insert(session.source_kind.clone());
            }
            if let Some(modified_ms) = session.modified_ms.filter(|value| *value <= now_ms) {
                evidence
                    .last_activity_ms
                    .entry(project_id)
                    .and_modify(|current| *current = (*current).max(modified_ms))
                    .or_insert(modified_ms);
            }
        }
    }
    Ok(evidence)
}

fn path_keys_overlap(left: &str, right: &str) -> bool {
    left == right
        || left
            .strip_prefix(right)
            .is_some_and(|suffix| suffix.starts_with('/') || suffix.starts_with('\\'))
        || right
            .strip_prefix(left)
            .is_some_and(|suffix| suffix.starts_with('/') || suffix.starts_with('\\'))
}

fn enrich_session_activity(inputs: &mut [SafeManageObjectiveInput], sessions: &SessionEvidence) {
    for input in inputs {
        if !sessions.complete {
            input.session_count = None;
            continue;
        }
        if let Some(modified_ms) = sessions.last_activity_ms.get(&input.project_id).copied() {
            if input
                .last_activity_ms
                .is_none_or(|current| modified_ms > current)
            {
                input.last_activity_ms = Some(modified_ms);
                input.last_activity_source =
                    Some("associated local coding-tool session".to_string());
            }
        }
        if let Some(apps) = sessions.apps.get(&input.project_id) {
            input.apps.extend(apps.iter().cloned());
            input.apps.sort();
            input.apps.dedup();
        }
    }
}

fn probe_git_state(input: &mut SafeManageObjectiveInput) {
    let root = Path::new(&input.project_path);
    // The catalog can legitimately pre-date creation of a repository. Upgrade
    // that stale negative from the current filesystem, while retaining a stale
    // positive as unknown if the repository has disappeared since indexing.
    if !input.has_git && root.join(".git").exists() {
        input.has_git = true;
    }
    if !input.has_git {
        input.git_has_remote = None;
        input.git_uncommitted = None;
        input.git_evidence_error = None;
        return;
    }

    let dirty = project_review::project_git_dirty(root);
    let has_remote = project_review::project_git_has_remote(root);
    input.git_uncommitted = dirty.as_ref().ok().copied();
    input.git_has_remote = has_remote.as_ref().ok().copied();

    let mut errors = Vec::new();
    if let Err(error) = dirty {
        errors.push(format!("Working-tree state: {error}"));
    }
    if let Err(error) = has_remote {
        errors.push(format!("Remote state: {error}"));
    }
    input.git_evidence_error = (!errors.is_empty()).then(|| errors.join(" "));
}

fn current_project_evidence_revision(state: &AppState, project_id: i64) -> Result<String, String> {
    let projects = super::projects_list(state)?;
    if !projects.iter().any(|project| project.id == project_id) {
        return Err("That project is no longer in the catalog.".to_string());
    }

    // The analysis revision is portfolio-relative: similarity counts and risk
    // relations depend on the complete catalog. Reconstructing one project in
    // isolation produces a different digest as soon as another project exists,
    // even when the selected project has not changed. Rebuild the same bounded
    // all-project evidence set used by run_analysis, then select the requested
    // project's input for the live comparison.
    let cancel = AtomicBool::new(false);
    let sessions = load_session_evidence(state, None, &projects, &cancel)?;
    let mut inputs = state
        .db()?
        .safe_manage_objective_inputs(&projects, &sessions.counts)
        .map_err(to_message)?;
    enrich_session_activity(&mut inputs, &sessions);
    for input in &mut inputs {
        probe_git_state(input);
    }
    let input = inputs
        .into_iter()
        .find(|input| input.project_id == project_id)
        .ok_or_else(|| "Current project evidence is unavailable.".to_string())?;
    project_evidence_revision(&input)
}

pub(crate) fn require_current_project_revision(
    state: &AppState,
    project_id: i64,
    expected_revision: &str,
) -> Result<(), String> {
    if expected_revision.trim().is_empty() {
        return Err("Safe Manage requires an evidence revision.".to_string());
    }
    let current_revision = current_project_evidence_revision(state, project_id)?;
    if current_revision != expected_revision {
        return Err(
            "This project changed after the recommendation. Run Safe Manage analysis again before continuing."
                .to_string(),
        );
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StableSafeManageDecisionEvidence<'a> {
    project_id: i64,
    project_name: &'a str,
    project_path: &'a str,
    source: &'a str,
    catalog_evidence_epoch: &'a str,
    relationship_evidence_revision: &'a str,
    comparison_evidence_revision: &'a str,
    apps: Vec<&'a str>,
    is_current: bool,
    session_count: Option<u64>,
    session_last_activity_ms: Option<i64>,
    has_git: bool,
    git_has_remote: Option<bool>,
    git_uncommitted: Option<bool>,
    git_evidence_available: bool,
    similar_project_count: Option<u64>,
    root_protected: bool,
}

fn project_evidence_revision(input: &SafeManageObjectiveInput) -> Result<String, String> {
    let mut apps = input.apps.iter().map(String::as_str).collect::<Vec<_>>();
    apps.sort_unstable();
    apps.dedup();
    let stable = StableSafeManageDecisionEvidence {
        project_id: input.project_id,
        project_name: &input.project_name,
        project_path: &input.project_path,
        source: &input.source,
        catalog_evidence_epoch: &input.catalog_evidence_epoch,
        relationship_evidence_revision: &input.relationship_evidence_revision,
        comparison_evidence_revision: &input.comparison_evidence_revision,
        apps,
        is_current: input.is_current,
        session_count: input.session_count,
        session_last_activity_ms: (input.last_activity_source.as_deref()
            == Some("associated local coding-tool session"))
        .then_some(input.last_activity_ms)
        .flatten(),
        has_git: input.has_git,
        git_has_remote: input.git_has_remote,
        git_uncommitted: input.git_uncommitted,
        git_evidence_available: input.git_evidence_error.is_none(),
        similar_project_count: input.similar_project_count,
        root_protected: input.root_protected,
    };
    let bytes = serde_json::to_vec(&stable)
        .map_err(|error| format!("Could not bind Safe Manage evidence: {error}"))?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(SAFE_MANAGE_RULESET_VERSION.as_bytes());
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(&bytes);
    Ok(hasher.finalize().to_hex().to_string())
}

fn catalog_revision(inputs: &[SafeManageObjectiveInput]) -> Result<String, String> {
    let mut revisions = inputs
        .iter()
        .map(|input| Ok((input.project_id, project_evidence_revision(input)?)))
        .collect::<Result<Vec<_>, String>>()?;
    revisions.sort_by_key(|(project_id, _)| *project_id);
    let mut hasher = blake3::Hasher::new();
    hasher.update(SAFE_MANAGE_RULESET_VERSION.as_bytes());
    for (project_id, revision) in revisions {
        hasher.update(&project_id.to_le_bytes());
        hasher.update(revision.as_bytes());
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn finish_cancelled(state: &AppState, mut run: SafeManageAnalysisRun) -> Result<(), String> {
    run.state = if run.assessments.is_empty() {
        "cancelled"
    } else {
        "partial"
    }
    .to_string();
    run.completed_at = Some(chrono::Utc::now().to_rfc3339());
    run.processed_projects = run.assessments.len() as u64;
    run.counts = safe_manage_portfolio_counts(&run.assessments);
    run.message = if run.assessments.is_empty() {
        "Analysis cancelled before any project completed. The previous complete result is unchanged."
            .to_string()
    } else {
        format!(
            "Analysis stopped after {} of {} project(s). These findings are partial and did not replace the previous complete result.",
            run.processed_projects, run.total_projects
        )
    };
    finalize_run(state, &run)
}

fn fail_analysis(state: &AppState, id: &str, error: String) {
    let Ok(mut run) = analysis_status(state, id) else {
        return;
    };
    if matches!(run.state.as_str(), "completed" | "partial" | "cancelled") {
        return;
    }
    run.state = "failed".to_string();
    run.completed_at = Some(chrono::Utc::now().to_rfc3339());
    run.processed_projects = run.assessments.len() as u64;
    run.counts = safe_manage_portfolio_counts(&run.assessments);
    run.message =
        "Safe Manage analysis failed. Existing complete results remain available.".to_string();
    run.error = Some(error);
    let _ = finalize_run(state, &run);
}

fn save_run_header(state: &AppState, run: &SafeManageAnalysisRun) -> Result<(), String> {
    state
        .db()?
        .safe_manage_analysis_header_save(run)
        .map_err(to_message)
}

fn append_latest_assessment(state: &AppState, run: &SafeManageAnalysisRun) -> Result<(), String> {
    let assessment = run
        .assessments
        .last()
        .ok_or_else(|| "Safe Manage has no completed assessment to append.".to_string())?;
    state
        .db()?
        .safe_manage_analysis_assessment_append(run, assessment)
        .map_err(to_message)
}

fn finalize_run(state: &AppState, run: &SafeManageAnalysisRun) -> Result<(), String> {
    state
        .db()?
        .safe_manage_analysis_finalize(run)
        .map_err(to_message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hangar_core::{
        SafeManageDuplicateEvidence, SafeManageEvidenceCoverage, SafeManageFileKindCount,
        SafeManageFileKindProfile, SafeManageRecommendation, SafeManageSignalState,
        SessionDiscoveryCandidate,
    };
    use std::fs;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    fn project(id: i64, path: &str) -> ProjectSummary {
        ProjectSummary {
            id,
            name: format!("Project {id}"),
            path: path.to_string(),
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
        }
    }

    fn git(root: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("local Git executable");
        assert!(status.success(), "git {args:?} failed");
    }

    fn objective_input(path: String) -> SafeManageObjectiveInput {
        SafeManageObjectiveInput {
            project_id: 1,
            project_name: "Current evidence".to_string(),
            project_path: path,
            source: "test".to_string(),
            catalog_evidence_epoch: "catalog-1".to_string(),
            relationship_evidence_revision: "relations-1".to_string(),
            apps: Vec::new(),
            is_current: false,
            session_count: Some(0),
            last_activity_ms: Some(1),
            last_activity_source: Some("scanned file modification time".to_string()),
            scan_complete: true,
            scan_error_count: 0,
            file_count: 1,
            context_file_count: 0,
            substantive_file_count: 0,
            file_kind_profile: SafeManageFileKindProfile {
                coverage: SafeManageEvidenceCoverage::Complete,
                inspected_file_count: 1,
                counts: vec![SafeManageFileKindCount {
                    kind: "other".to_string(),
                    label: "Other files".to_string(),
                    file_count: 1,
                }],
            },
            duplicate_evidence: SafeManageDuplicateEvidence {
                coverage: SafeManageEvidenceCoverage::Complete,
                inspected_file_count: 1,
                ..SafeManageDuplicateEvidence::default()
            },
            comparison_evidence_revision: "comparison-1".to_string(),
            materially_similar_project_count: Some(0),
            apparent_bytes: Some(1),
            physical_bytes: Some(1),
            footprint_partial: false,
            has_git: false,
            git_has_remote: None,
            git_uncommitted: None,
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

    fn queued_analysis(id: &str, created_at: &str) -> SafeManageAnalysisRun {
        SafeManageAnalysisRun {
            id: id.to_string(),
            state: "queued".to_string(),
            ruleset_version: SAFE_MANAGE_RULESET_VERSION.to_string(),
            catalog_revision: "pending-local-snapshot".to_string(),
            created_at: created_at.to_string(),
            started_at: None,
            completed_at: None,
            processed_projects: 0,
            total_projects: 0,
            counts: SafeManagePortfolioCounts::default(),
            message: "Queued synthetic local portfolio analysis.".to_string(),
            error: None,
            assessments: Vec::new(),
        }
    }

    fn register_synthetic_portfolio(
        state: &AppState,
        parent: &Path,
        project_count: usize,
    ) -> Vec<ProjectSummary> {
        fs::create_dir_all(parent).unwrap();
        let db = state.db().unwrap();
        for index in 0..project_count {
            // The trailing alphabetic character keeps every synthetic basename
            // semantically distinct in the similar-version classifier.
            let path = parent.join(format!("project-{index:04x}x"));
            fs::create_dir(&path).unwrap();
            let readme = path.join("README.md");
            fs::write(&readme, format!("# Synthetic project {index}\n")).unwrap();
            let path_text = path.to_string_lossy().into_owned();
            db.roots_add(&path_text).unwrap();
            let mut files = Vec::new();
            let outcome = hangar_fs::scan_inventory_stream(
                &path,
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
            assert!(!outcome.cancelled);
            assert!(!outcome.partial);
            db.load_scanned_root(&path_text, &files, outcome.git.as_ref())
                .unwrap();
        }
        let projects = db
            .projects_list()
            .unwrap()
            .into_iter()
            .filter(|project| Path::new(&project.path).starts_with(parent))
            .collect::<Vec<_>>();
        assert_eq!(projects.len(), project_count);
        projects
    }

    fn complete_empty_session_evidence(projects: &[ProjectSummary]) -> SessionEvidence {
        SessionEvidence {
            counts: projects.iter().map(|project| (project.id, 0)).collect(),
            complete: true,
            ..SessionEvidence::default()
        }
    }

    fn execute_synthetic_analysis<H>(
        state: &AppState,
        id: &str,
        created_at: &str,
        projects: &[ProjectSummary],
        cancel: &AtomicBool,
        after_assessment_persisted: H,
    ) where
        H: FnMut(&SafeManageAnalysisRun),
    {
        state
            .db()
            .unwrap()
            .safe_manage_analysis_header_save(&queued_analysis(id, created_at))
            .unwrap();
        let project_snapshot = projects.to_vec();
        run_analysis_with_sources(
            state,
            id,
            cancel,
            || Ok(project_snapshot.clone()),
            |loaded_projects| Ok(complete_empty_session_evidence(loaded_projects)),
            after_assessment_persisted,
        )
        .unwrap();
    }

    fn open_ready_test_state(path: &Path) -> AppState {
        let state = AppState::open(path).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let status = state.startup_status();
            match status.state.as_str() {
                "ready" => return state,
                "failed" => panic!("synthetic catalog failed to open: {}", status.message),
                _ if Instant::now() >= deadline => {
                    panic!("synthetic catalog did not open in time: {}", status.message)
                }
                _ => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    }

    #[test]
    fn job_store_allows_one_analysis_and_exposes_cancel_intent() {
        let jobs = SafeManageJobStore::default();
        let (id, _) = jobs.reserve().unwrap();
        assert!(jobs.reserve().is_err());
        assert!(!jobs.cancellation_requested(&id).unwrap());
        assert!(jobs.cancel(&id).unwrap());
        assert!(jobs.cancellation_requested(&id).unwrap());
        jobs.finish(&id);
        assert!(jobs.reserve().is_ok());
    }

    #[test]
    fn analysis_worker_spawn_failure_is_terminal_and_releases_admission() {
        let state = AppState::memory().unwrap();
        let (id, _) = state.safe_manage_jobs.reserve().unwrap();
        state
            .db()
            .unwrap()
            .safe_manage_analysis_header_save(&queued_analysis(&id, "2026-08-28T00:00:00Z"))
            .unwrap();
        let worker_ran = Arc::new(AtomicBool::new(false));
        let worker_ran_in_task = Arc::clone(&worker_ran);

        let error = spawn_analysis_worker_with(
            &state,
            &id,
            Box::new(move || {
                worker_ran_in_task.store(true, Ordering::Release);
                Ok(())
            }),
            |_task| Err(std::io::Error::other("synthetic thread refusal")),
        )
        .unwrap_err();

        assert!(error.contains("could not start"), "{error}");
        assert!(!worker_ran.load(Ordering::Acquire));
        let terminal = analysis_status(&state, &id).unwrap();
        assert_eq!(terminal.state, "failed");
        assert!(terminal
            .error
            .as_deref()
            .is_some_and(|value| value.contains("could not start")));
        assert!(
            state.safe_manage_jobs.reserve().is_ok(),
            "a failed spawn must not leave Safe Manage permanently active"
        );
    }

    #[test]
    fn analysis_worker_panic_is_observed_terminal_and_releases_admission() {
        let state = AppState::memory().unwrap();
        let (id, _) = state.safe_manage_jobs.reserve().unwrap();
        state
            .db()
            .unwrap()
            .safe_manage_analysis_header_save(&queued_analysis(&id, "2026-08-28T00:00:00Z"))
            .unwrap();

        spawn_analysis_worker(
            &state,
            &id,
            Box::new(|| -> Result<(), String> { panic!("synthetic Safe Manage worker panic") }),
        )
        .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let status = analysis_status(&state, &id).unwrap();
            if status.state == "failed" {
                assert!(status
                    .error
                    .as_deref()
                    .is_some_and(|value| value.contains("unexpected internal panic")));
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the panicked analysis worker remained non-terminal"
            );
            std::thread::yield_now();
        }
        assert!(
            state.safe_manage_jobs.reserve().is_ok(),
            "a panicked worker must release Safe Manage admission"
        );
    }

    #[test]
    fn default_discovery_source_runs_the_dedicated_loader_and_fixture_never_does() {
        let dedicated_calls = std::cell::Cell::new(0_u32);
        let empty_report = ProjectDiscoveryReport {
            candidates: Vec::new(),
            sessions: Vec::new(),
            searched_locations: Vec::new(),
            duration_ms: 0,
            total_candidates: 0,
            total_sessions: 0,
        };
        let loaded = SafeManageDiscoverySource::default()
            .load_report(|| {
                dedicated_calls.set(dedicated_calls.get() + 1);
                Ok(empty_report.clone())
            })
            .unwrap();
        assert_eq!(dedicated_calls.get(), 1);
        assert_eq!(loaded, empty_report);
        assert_eq!(SAFE_MANAGE_DISCOVERY_SESSION_LIMIT, 5_000);

        let fixture = SafeManageDiscoverySource::fixture(empty_report.clone());
        let loaded = fixture
            .load_report(|| -> Result<ProjectDiscoveryReport, String> {
                panic!("a fixture-bound AppState must never enumerate real session stores")
            })
            .unwrap();
        assert_eq!(loaded, empty_report);
    }

    #[test]
    fn grouped_decision_shape_rejects_duplicate_and_mixed_runs_before_evidence_reads() {
        let state = AppState::memory().unwrap();
        let request = SafeManageDecisionRequest {
            project_id: 1,
            analysis_run_id: "one-run".to_string(),
            decision: SafeManageDecisionKind::Keep,
            evidence_revision: "revision".to_string(),
        };
        let duplicate =
            decisions_record_atomic(&state, vec![request.clone(), request.clone()]).unwrap_err();
        assert!(duplicate.contains("duplicate-free"));
        assert!(state
            .db()
            .unwrap()
            .safe_manage_decisions_latest()
            .unwrap()
            .is_empty());

        let mut other_run = request.clone();
        other_run.project_id = 2;
        other_run.analysis_run_id = "another-run".to_string();
        let mixed = decisions_record_atomic(&state, vec![request, other_run]).unwrap_err();
        assert!(mixed.contains("one analysis run"));
        assert!(state
            .db()
            .unwrap()
            .safe_manage_decisions_latest()
            .unwrap()
            .is_empty());
    }

    #[test]
    fn complete_session_report_counts_ids_and_path_links_once() {
        let projects = vec![project(1, r"C:\work\one"), project(2, r"C:\work\two")];
        let report = ProjectDiscoveryReport {
            candidates: Vec::new(),
            sessions: vec![SessionDiscoveryCandidate {
                path: "session.jsonl".to_string(),
                display_name: "Session".to_string(),
                source_kind: "codex".to_string(),
                source_label: "Codex".to_string(),
                session_kind: "conversation".to_string(),
                confidence: "High".to_string(),
                linked_project_paths: vec![r"C:\work\one\src".to_string()],
                linked_registered_project_ids: vec![1],
                association: "linked".to_string(),
                modified_ms: Some(123),
            }],
            searched_locations: Vec::new(),
            duration_ms: 1,
            total_candidates: 0,
            total_sessions: 1,
        };
        let evidence = session_evidence_from_report(&projects, &report).unwrap();
        assert!(evidence.complete);
        assert_eq!(evidence.counts.get(&1), Some(&1));
        assert_eq!(evidence.counts.get(&2), Some(&0));
        assert_eq!(evidence.last_activity_ms.get(&1), Some(&123));
        assert!(evidence
            .apps
            .get(&1)
            .is_some_and(|apps| apps.contains("codex")));
    }

    #[test]
    fn truncated_session_report_keeps_counts_unknown_and_blocks_removal() {
        let projects = vec![project(1, r"C:\work\one")];
        let report = ProjectDiscoveryReport {
            candidates: Vec::new(),
            sessions: Vec::new(),
            searched_locations: Vec::new(),
            duration_ms: 1,
            total_candidates: 0,
            total_sessions: 2,
        };
        let evidence = session_evidence_from_report(&projects, &report).unwrap();
        assert!(!evidence.complete);
        assert!(evidence.counts.is_empty());

        let mut inputs = vec![objective_input(r"C:\work\one".to_string())];
        enrich_session_activity(&mut inputs, &evidence);
        assert_eq!(inputs[0].session_count, None);
        let assessment = assess_safe_manage_project(
            &inputs[0],
            &SafeManageClassificationContext {
                now_ms: 2_000_000_000_000,
                analysis_run_id: "truncated-sessions".to_string(),
                evidence_revision: "truncated-sessions-evidence".to_string(),
                observed_at: None,
            },
        );
        assert_ne!(
            assessment.recommendation,
            SafeManageRecommendation::RemovalCandidate
        );
    }

    #[test]
    fn future_session_timestamp_is_not_accepted_as_activity_evidence() {
        let projects = vec![project(1, r"C:\work\one")];
        let report = ProjectDiscoveryReport {
            candidates: Vec::new(),
            sessions: vec![SessionDiscoveryCandidate {
                path: "future-session.jsonl".to_string(),
                display_name: "Future session".to_string(),
                source_kind: "codex".to_string(),
                source_label: "Codex".to_string(),
                session_kind: "conversation".to_string(),
                confidence: "High".to_string(),
                linked_project_paths: Vec::new(),
                linked_registered_project_ids: vec![1],
                association: "linked".to_string(),
                modified_ms: Some(101),
            }],
            searched_locations: Vec::new(),
            duration_ms: 1,
            total_candidates: 0,
            total_sessions: 1,
        };

        let evidence = session_evidence_from_report_at(&projects, &report, 100).unwrap();

        assert!(evidence.complete);
        assert_eq!(evidence.counts.get(&1), Some(&1));
        assert!(!evidence.last_activity_ms.contains_key(&1));
    }

    #[test]
    fn analysis_filters_fixture_projects_before_sessions_and_classification() {
        let directory = tempfile::tempdir().unwrap();
        let state = AppState::memory().unwrap();
        let real_projects =
            register_synthetic_portfolio(&state, &directory.path().join("real-projects"), 1);
        let all_projects = super::super::projects_list(&state).unwrap();
        assert!(all_projects
            .iter()
            .any(|project| project.source == "fixture"));
        assert_eq!(
            all_projects
                .iter()
                .filter(|project| project.source != "fixture")
                .count(),
            1
        );
        state
            .db()
            .unwrap()
            .safe_manage_analysis_header_save(&queued_analysis(
                "fixture-filtered",
                "2026-08-28T10:00:00Z",
            ))
            .unwrap();
        let cancel = AtomicBool::new(false);
        run_analysis_with_sources(
            &state,
            "fixture-filtered",
            &cancel,
            || Ok(all_projects.clone()),
            |loaded_projects| {
                assert!(loaded_projects
                    .iter()
                    .all(|project| !project.source.eq_ignore_ascii_case("fixture")));
                Ok(complete_empty_session_evidence(loaded_projects))
            },
            |_| {},
        )
        .unwrap();

        let completed = analysis_status(&state, "fixture-filtered").unwrap();
        assert_eq!(completed.state, "completed");
        assert_eq!(completed.total_projects, 1);
        assert_eq!(completed.processed_projects, 1);
        assert_eq!(completed.assessments[0].project_id, real_projects[0].id);
    }

    #[test]
    fn current_git_probe_upgrades_stale_catalog_and_reads_remote_without_network() {
        let root = tempfile::tempdir().unwrap();
        git(root.path(), &["init", "--quiet"]);
        git(
            root.path(),
            &[
                "remote",
                "add",
                "origin",
                "https://example.invalid/repository.git",
            ],
        );
        fs::write(root.path().join("untracked.txt"), "local work\n").unwrap();

        let mut input = objective_input(root.path().to_string_lossy().into_owned());

        probe_git_state(&mut input);

        assert!(input.has_git);
        assert_eq!(input.git_uncommitted, Some(true));
        assert_eq!(input.git_has_remote, Some(true));
        assert_eq!(input.git_evidence_error, None);
    }

    #[test]
    fn decision_revision_ignores_exact_materialization_but_binds_real_epochs() {
        assert_eq!(SAFE_MANAGE_RULESET_VERSION, "safe-manage-objective-v2");
        let input = objective_input(r"C:\work\project".to_string());
        let original = project_evidence_revision(&input).unwrap();

        // Exact regenerable expansion is an internal materialization step. Its
        // separate receipt binds concrete bytes; these volatile catalog totals
        // must not make the owner's just-recorded decision impossible to use.
        let mut materialized = input.clone();
        materialized.file_count = 500;
        materialized.last_activity_ms = Some(9_999);
        materialized.scan_complete = false;
        materialized.scan_error_count = 3;
        materialized.apparent_bytes = Some(9_999);
        materialized.physical_bytes = None;
        materialized.footprint_partial = true;
        materialized.regenerable_bytes = Some(8_888);
        materialized.relationship_evidence_complete = false;
        materialized.shared_reference_count = None;
        materialized.relationship_issue_count = None;
        assert_eq!(project_evidence_revision(&materialized).unwrap(), original);

        let mut rescanned = materialized.clone();
        rescanned.catalog_evidence_epoch = "catalog-2".to_string();
        assert_ne!(project_evidence_revision(&rescanned).unwrap(), original);

        let mut relations_changed = materialized;
        relations_changed.relationship_evidence_revision = "relations-2".to_string();
        assert_ne!(
            project_evidence_revision(&relations_changed).unwrap(),
            original
        );

        let mut comparison_changed = input;
        comparison_changed.comparison_evidence_revision = "comparison-2".to_string();
        assert_ne!(
            project_evidence_revision(&comparison_changed).unwrap(),
            original
        );
    }

    #[test]
    fn local_analysis_never_turns_unknown_fixture_evidence_into_removal() {
        let state = AppState::memory().unwrap();
        let projects = super::super::projects_list(&state).unwrap();
        let inputs = state
            .db()
            .unwrap()
            .safe_manage_objective_inputs(&projects, &HashMap::new())
            .unwrap();
        for input in inputs {
            let assessment = assess_safe_manage_project(
                &input,
                &SafeManageClassificationContext {
                    now_ms: 2_000_000_000_000,
                    analysis_run_id: "test".to_string(),
                    evidence_revision: "revision".to_string(),
                    observed_at: None,
                },
            );
            assert_ne!(
                assessment.recommendation,
                SafeManageRecommendation::RemovalCandidate
            );
            assert!(assessment
                .signals
                .iter()
                .any(|signal| signal.state == SafeManageSignalState::Unknown));
        }
    }

    #[test]
    fn large_portfolio_analysis_persists_every_synthetic_project_without_mutation() {
        const LARGE_PORTFOLIO_PROJECTS: usize = 512;

        let directory = tempfile::tempdir().unwrap();
        let portfolio_root = directory.path().join("large-portfolio");
        let state = AppState::memory().unwrap();
        let projects =
            register_synthetic_portfolio(&state, &portfolio_root, LARGE_PORTFOLIO_PROJECTS);
        let cancel = AtomicBool::new(false);

        execute_synthetic_analysis(
            &state,
            "large-portfolio-complete",
            "2026-08-27T12:00:00Z",
            &projects,
            &cancel,
            |_| {},
        );

        let completed = analysis_status(&state, "large-portfolio-complete").unwrap();
        assert_eq!(completed.state, "completed");
        assert_eq!(completed.total_projects, LARGE_PORTFOLIO_PROJECTS as u64);
        assert_eq!(
            completed.processed_projects,
            LARGE_PORTFOLIO_PROJECTS as u64
        );
        assert_eq!(completed.counts.total, LARGE_PORTFOLIO_PROJECTS as u64);
        assert_eq!(completed.assessments.len(), LARGE_PORTFOLIO_PROJECTS);
        assert_eq!(
            completed
                .assessments
                .iter()
                .map(|assessment| assessment.project_id)
                .collect::<HashSet<_>>()
                .len(),
            LARGE_PORTFOLIO_PROJECTS,
            "every synthetic project must produce exactly one persisted assessment"
        );
        assert!(projects.iter().all(|project| {
            let entries = fs::read_dir(&project.path).unwrap().collect::<Vec<_>>();
            entries.len() == 1
                && fs::read_to_string(Path::new(&project.path).join("README.md"))
                    .is_ok_and(|text| text.starts_with("# Synthetic project "))
        }));
        assert_eq!(
            state
                .db()
                .unwrap()
                .safe_manage_analysis_latest_complete()
                .unwrap()
                .unwrap(),
            completed
        );
    }

    #[test]
    fn mid_run_cancellation_preserves_last_complete_and_never_promotes_partial() {
        const PORTFOLIO_PROJECTS: usize = 24;
        const CANCEL_AFTER_PERSISTED: u64 = 7;

        let directory = tempfile::tempdir().unwrap();
        let state = AppState::memory().unwrap();
        let projects = register_synthetic_portfolio(
            &state,
            &directory.path().join("cancel-portfolio"),
            PORTFOLIO_PROJECTS,
        );
        let baseline_cancel = AtomicBool::new(false);
        execute_synthetic_analysis(
            &state,
            "complete-baseline",
            "2026-08-27T13:00:00Z",
            &projects,
            &baseline_cancel,
            |_| {},
        );
        let baseline = analysis_status(&state, "complete-baseline").unwrap();
        assert_eq!(baseline.state, "completed");

        let cancel = AtomicBool::new(false);
        execute_synthetic_analysis(
            &state,
            "cancelled-after-progress",
            "2026-08-27T13:01:00Z",
            &projects,
            &cancel,
            |run| {
                if run.processed_projects == CANCEL_AFTER_PERSISTED {
                    cancel.store(true, Ordering::Release);
                }
            },
        );

        let overview = overview(&state).unwrap();
        let partial = overview.latest_run.unwrap();
        assert_eq!(partial.id, "cancelled-after-progress");
        assert_eq!(partial.state, "partial");
        assert_eq!(partial.processed_projects, CANCEL_AFTER_PERSISTED);
        assert_eq!(partial.counts.total, CANCEL_AFTER_PERSISTED);
        assert!(partial.processed_projects < partial.total_projects);
        assert!(partial.message.contains("did not replace"));
        assert_eq!(overview.last_complete_run.as_ref(), Some(&baseline));
        assert_eq!(
            state
                .db()
                .unwrap()
                .safe_manage_analysis_get("complete-baseline")
                .unwrap()
                .as_ref(),
            Some(&baseline),
            "the previously complete portfolio must remain byte-for-byte unchanged"
        );
        assert_ne!(overview.last_complete_run.unwrap().id, partial.id);
    }

    #[test]
    fn first_run_analyze_later_and_suppress_have_distinct_persisted_outcomes() {
        let directory = tempfile::tempdir().unwrap();

        let analyze_db = directory.path().join("analyze.sqlite3");
        let analyze_state = open_ready_test_state(&analyze_db);
        let analyze_projects = register_synthetic_portfolio(
            &analyze_state,
            &directory.path().join("analyze-projects"),
            2,
        );
        let prompted = first_run_preference_set(&analyze_state, true, "pending", true).unwrap();
        let prompted_at = prompted.last_prompted_at.clone();
        let postponed = first_run_preference_set(&analyze_state, true, "postponed", false).unwrap();
        assert_eq!(postponed.prompt_state, "postponed");
        assert_eq!(postponed.last_prompted_at, prompted_at);
        execute_synthetic_analysis(
            &analyze_state,
            "first-run-analyze-now",
            "2026-08-27T14:00:00Z",
            &analyze_projects,
            &AtomicBool::new(false),
            |_| {},
        );
        assert_eq!(
            first_run_preference(&analyze_state).unwrap().prompt_state,
            "completed"
        );
        assert_eq!(
            overview(&analyze_state)
                .unwrap()
                .last_complete_run
                .unwrap()
                .id,
            "first-run-analyze-now"
        );
        drop(analyze_state);
        let analyze_reopened = open_ready_test_state(&analyze_db);
        let analyze_persisted = first_run_preference(&analyze_reopened).unwrap();
        assert!(analyze_persisted.suggest_after_discovery);
        assert_eq!(analyze_persisted.prompt_state, "completed");
        assert_eq!(analyze_persisted.last_prompted_at, prompted_at);
        assert_eq!(
            overview(&analyze_reopened)
                .unwrap()
                .last_complete_run
                .unwrap()
                .id,
            "first-run-analyze-now"
        );

        let later_db = directory.path().join("later.sqlite3");
        let later_state = open_ready_test_state(&later_db);
        let later_prompted = first_run_preference_set(&later_state, true, "pending", true).unwrap();
        let later = first_run_preference_set(&later_state, true, "postponed", false).unwrap();
        assert!(later.suggest_after_discovery);
        assert_eq!(later.prompt_state, "postponed");
        assert_eq!(later.last_prompted_at, later_prompted.last_prompted_at);
        assert!(overview(&later_state).unwrap().latest_run.is_none());
        drop(later_state);
        let later_reopened = open_ready_test_state(&later_db);
        let later_persisted = first_run_preference(&later_reopened).unwrap();
        assert!(later_persisted.suggest_after_discovery);
        assert_eq!(later_persisted.prompt_state, "postponed");
        assert_eq!(
            later_persisted.last_prompted_at,
            later_prompted.last_prompted_at
        );
        assert!(overview(&later_reopened).unwrap().latest_run.is_none());

        let suppress_db = directory.path().join("suppress.sqlite3");
        let suppress_state = open_ready_test_state(&suppress_db);
        let suppress_prompted =
            first_run_preference_set(&suppress_state, true, "pending", true).unwrap();
        let suppressed =
            first_run_preference_set(&suppress_state, false, "suppressed", false).unwrap();
        assert!(!suppressed.suggest_after_discovery);
        assert_eq!(suppressed.prompt_state, "suppressed");
        assert_eq!(
            suppressed.last_prompted_at,
            suppress_prompted.last_prompted_at
        );
        assert!(overview(&suppress_state).unwrap().latest_run.is_none());
        drop(suppress_state);
        let suppress_reopened = open_ready_test_state(&suppress_db);
        let suppress_persisted = first_run_preference(&suppress_reopened).unwrap();
        assert!(!suppress_persisted.suggest_after_discovery);
        assert_eq!(suppress_persisted.prompt_state, "suppressed");
        assert_eq!(
            suppress_persisted.last_prompted_at,
            suppress_prompted.last_prompted_at
        );
        assert!(overview(&suppress_reopened).unwrap().latest_run.is_none());
    }
}
