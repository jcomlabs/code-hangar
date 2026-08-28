#![cfg(all(windows, feature = "mutation"))]

use hangar_api::{
    mutation_activity_log, mutation_backup_start, mutation_final_remove_confirm,
    mutation_final_remove_preview, mutation_move_start, mutation_restore_start,
    mutation_token_issue, operation_plan_status, projects_list, roots_add,
    safe_manage_analysis_start, safe_manage_analysis_status, safe_manage_decision_record,
    safe_manage_operation_plan_start, safe_manage_regenerable_scan_start,
    safe_manage_regenerable_targets, scan_start, scan_status, set_final_remove_enabled,
    startup_status, AppState, FinalRemoveScope,
};
use hangar_core::{
    MutationBackupSummary, MutationStoredEntry, OperationPlan, ProjectDiscoveryReport,
    ProjectSummary, RiskReport, SafeManageAnalysisRun, SafeManageDecisionKind,
    SafeManageObjectiveInput, SafeManageOperationPlanRequest, SafeManageOperationTargetIdentity,
    SafeManageProjectAssessment, SafeManageRecommendation, SafeManageRegenerableScanRequest,
    SafeManageRegenerableTarget, ScanRoot, ScanStatus,
};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

const POLL_INTERVAL: Duration = Duration::from_millis(10);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const JOB_TIMEOUT: Duration = Duration::from_secs(60);
const FINAL_REMOVE_ACKNOWLEDGEMENT: &str = "ENABLE PERMANENT REMOVAL";

struct NativeProject {
    temp: TempDir,
    db_path: PathBuf,
    project_path: PathBuf,
    backup_path: PathBuf,
    holding_path: PathBuf,
    payload_path: PathBuf,
}

impl NativeProject {
    fn regenerable(name: &str, partial_expansion: bool) -> Self {
        let temp = tempfile::Builder::new()
            .prefix(name)
            .tempdir()
            .expect("native Safe Manage tempdir");
        let project_path = temp.path().join("project");
        let dependency_path = project_path.join("node_modules").join("fixture-package");
        fs::create_dir_all(&dependency_path).expect("regenerable fixture directory");
        fs::write(
            project_path.join("package.json"),
            br#"{"name":"safe-manage-native-e2e","private":true}"#,
        )
        .expect("fixture manifest");
        let payload_path = dependency_path.join("index.js");
        fs::write(&payload_path, b"module.exports = 'native-e2e';\n")
            .expect("regenerable fixture payload");

        if partial_expansion {
            let forbidden = dependency_path.join("vendor").join("private");
            fs::create_dir_all(&forbidden).expect("forbidden nested fixture");
            fs::write(forbidden.join("source.rs"), b"not derived evidence\n")
                .expect("forbidden nested payload");
        }

        let db_path = temp.path().join("data").join("catalog.sqlite3");
        let backup_path = temp.path().join("backup");
        let holding_path = temp.path().join("holding");
        Self {
            temp,
            db_path,
            project_path,
            backup_path,
            holding_path,
            payload_path,
        }
    }

    fn archive(name: &str) -> Self {
        let temp = tempfile::Builder::new()
            .prefix(name)
            .tempdir()
            .expect("native Safe Manage tempdir");
        let project_path = temp.path().join("project");
        fs::create_dir_all(&project_path).expect("archive fixture directory");
        fs::write(
            project_path.join("README.md"),
            b"# Disposable native fixture\n",
        )
        .expect("archive fixture README");
        let payload_path = project_path.join("artifact.txt");
        fs::write(&payload_path, b"disposable native final-remove fixture\n")
            .expect("archive fixture payload");

        let db_path = temp.path().join("data").join("catalog.sqlite3");
        let backup_path = temp.path().join("backup");
        let holding_path = temp.path().join("holding");
        Self {
            temp,
            db_path,
            project_path,
            backup_path,
            holding_path,
            payload_path,
        }
    }

    fn assert_owned_temp_paths(&self) {
        for path in [
            &self.db_path,
            &self.project_path,
            &self.backup_path,
            &self.holding_path,
            &self.payload_path,
        ] {
            assert!(
                path.starts_with(self.temp.path()),
                "the native journey may touch only its disposable tempdir: {}",
                path.display()
            );
        }
    }
}

fn open_ready_state(db_path: &Path) -> AppState {
    fs::create_dir_all(db_path.parent().expect("database parent"))
        .expect("database parent directory");
    let state = AppState::open_with_safe_manage_discovery_fixture_for_test(
        db_path,
        ProjectDiscoveryReport {
            candidates: Vec::new(),
            sessions: Vec::new(),
            searched_locations: Vec::new(),
            duration_ms: 0,
            total_candidates: 0,
            total_sessions: 0,
        },
    )
    .expect("open persistent native inventory with hermetic session evidence");
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        let status = startup_status(&state);
        match status.state.as_str() {
            "ready" => return state,
            "failed" => panic!("persistent inventory failed to open: {}", status.message),
            _ if Instant::now() >= deadline => {
                panic!(
                    "persistent inventory did not open in time: {}",
                    status.message
                )
            }
            _ => thread::sleep(POLL_INTERVAL),
        }
    }
}

fn wait_for_scan(state: &AppState, job_id: &str) -> ScanStatus {
    let deadline = Instant::now() + JOB_TIMEOUT;
    loop {
        let status = scan_status(state, job_id.to_string()).expect("scan status");
        if matches!(
            status.state.as_str(),
            "completed" | "partial" | "cancelled" | "failed"
        ) {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "scan {job_id} timed out in {}: {}",
            status.state,
            status.message
        );
        thread::sleep(POLL_INTERVAL);
    }
}

fn register_and_scan(state: &AppState, project_path: &Path) -> (ScanRoot, ProjectSummary) {
    let root = roots_add(state, project_path.to_string_lossy().into_owned())
        .expect("register disposable project root");
    let job_id = scan_start(state, Some(vec![root.id]), Some("background".to_string()))
        .expect("start native project scan");
    let status = wait_for_scan(state, &job_id);
    assert_eq!(
        status.state, "completed",
        "normal fixture inventory must be complete: {:?}",
        status.error
    );
    assert!(
        !status.partial,
        "normal fixture inventory cannot be partial"
    );

    let project = projects_list(state)
        .expect("list scanned projects")
        .into_iter()
        .find(|project| project.scan_root_id == Some(root.id))
        .expect("registered root must own one project");
    (root, project)
}

fn wait_for_analysis(state: &AppState, job_id: &str) -> SafeManageAnalysisRun {
    let deadline = Instant::now() + JOB_TIMEOUT;
    loop {
        let run = safe_manage_analysis_status(state, job_id).expect("Safe Manage status");
        if matches!(
            run.state.as_str(),
            "completed" | "partial" | "cancelled" | "failed"
        ) {
            return run;
        }
        assert!(
            Instant::now() < deadline,
            "Safe Manage analysis {job_id} timed out in {}: {}",
            run.state,
            run.message
        );
        thread::sleep(POLL_INTERVAL);
    }
}

fn analyze_and_decide(
    state: &AppState,
    project_id: i64,
    decision: SafeManageDecisionKind,
) -> (SafeManageAnalysisRun, SafeManageProjectAssessment) {
    let job_id = safe_manage_analysis_start(state).expect("start Safe Manage analysis");
    let run = wait_for_analysis(state, &job_id);
    assert_eq!(
        run.state, "completed",
        "fixture analysis must complete: {:?}",
        run.error
    );
    let assessment = run
        .assessments
        .iter()
        .find(|assessment| assessment.project_id == project_id)
        .cloned()
        .expect("analysis must include the scanned project");
    assert_ne!(
        assessment.recommendation,
        SafeManageRecommendation::DoNotTouch,
        "the disposable fixture must remain eligible for an explicit owner decision"
    );
    safe_manage_decision_record(
        state,
        project_id,
        &run.id,
        decision,
        &assessment.evidence_revision,
    )
    .expect("record current owner decision");
    (run, assessment)
}

fn target_ending_with(
    targets: Vec<SafeManageRegenerableTarget>,
    suffix: &str,
) -> SafeManageRegenerableTarget {
    let suffix = suffix.replace('\\', "/").to_ascii_lowercase();
    targets
        .into_iter()
        .find(|target| {
            let path = target.path.replace('\\', "/").to_ascii_lowercase();
            path == suffix || path.ends_with(&format!("/{suffix}"))
        })
        .unwrap_or_else(|| panic!("expected exact regenerable target ending with {suffix}"))
}

fn objective_input_at(db_path: &Path, project_id: i64) -> SafeManageObjectiveInput {
    let db = hangar_db::Db::open(db_path).expect("open native inventory for evidence assertion");
    let projects = db.projects_list().expect("list native evidence projects");
    db.safe_manage_objective_inputs(&projects, &HashMap::from([(project_id, 0_u64)]))
        .expect("load native objective evidence")
        .into_iter()
        .find(|input| input.project_id == project_id)
        .expect("native objective evidence must include the project")
}

fn operation_request(
    assessment: &SafeManageProjectAssessment,
    decision: SafeManageDecisionKind,
    target: Option<&SafeManageRegenerableTarget>,
) -> SafeManageOperationPlanRequest {
    SafeManageOperationPlanRequest {
        project_id: assessment.project_id,
        analysis_run_id: assessment.analysis_run_id.clone(),
        evidence_revision: assessment.evidence_revision.clone(),
        decision,
        target: target.map(|target| SafeManageOperationTargetIdentity {
            nav_id: target.nav_id,
            node_id: target.node_id,
            path: target.path.clone(),
        }),
    }
}

fn wait_for_bound_plan(
    state: &AppState,
    request: SafeManageOperationPlanRequest,
) -> (OperationPlan, RiskReport) {
    let job_id = safe_manage_operation_plan_start(state, request, Some("background".to_string()))
        .expect("admit decision-bound OperationPlan");
    let deadline = Instant::now() + JOB_TIMEOUT;
    loop {
        let status = operation_plan_status(state, job_id.clone()).expect("OperationPlan status");
        match status.state.as_str() {
            "completed" => {
                let plan = status
                    .plan
                    .expect("completed job must contain its immutable plan");
                let report = status
                    .report
                    .expect("completed job must contain its Risk Report");
                assert!(plan.read_only_preview);
                assert!(report.read_only_preview);
                assert_eq!(report.target, plan.target);
                assert_eq!(report.action_label, plan.action_label);
                return (plan, report);
            }
            "failed" | "cancelled" => panic!(
                "decision-bound OperationPlan ended {} while `{}`: {}",
                status.state,
                status.message,
                status
                    .error
                    .unwrap_or_else(|| "no structured error".to_string())
            ),
            _ if Instant::now() >= deadline => {
                panic!("decision-bound OperationPlan timed out: {}", status.message)
            }
            _ => thread::sleep(POLL_INTERVAL),
        }
    }
}

fn verified_backup_and_holding(
    state: &AppState,
    plan: OperationPlan,
    backup_path: &Path,
    holding_path: &Path,
) -> (MutationBackupSummary, MutationStoredEntry) {
    let token = mutation_token_issue(state, "enter_mutation_mode".to_string())
        .expect("issue backup confirmation token")
        .token;
    let backup = mutation_backup_start(
        state,
        plan.clone(),
        backup_path.to_string_lossy().into_owned(),
        "standard".to_string(),
        Some(true),
        false,
        token,
    )
    .expect("create and verify native backup");
    assert!(backup.verified);
    assert!(backup.item_count > 0);
    assert!(Path::new(&backup.manifest_path).exists());

    let token = mutation_token_issue(state, "enter_mutation_mode".to_string())
        .expect("issue holding confirmation token")
        .token;
    let moved = mutation_move_start(
        state,
        plan,
        holding_path.to_string_lossy().into_owned(),
        backup.backup_id,
        false,
        token,
    )
    .expect("move exact plan target to holding");
    assert!(moved.moved > 0, "the exact target must enter holding");
    assert_eq!(moved.failed, 0);

    let stored = mutation_activity_log(state, Some(100))
        .expect("read mutation journal")
        .stored_entries
        .into_iter()
        .find(|entry| entry.status == "quarantined")
        .expect("holding move must persist a recoverable entry");
    assert!(Path::new(&stored.stored_path).exists());
    (backup, stored)
}

fn exact_regenerable_target(
    state: &AppState,
    assessment: &SafeManageProjectAssessment,
) -> SafeManageRegenerableTarget {
    let opaque = target_ending_with(
        safe_manage_regenerable_targets(
            state,
            assessment.project_id,
            &assessment.analysis_run_id,
            &assessment.evidence_revision,
        )
        .expect("list bounded regenerable targets"),
        "node_modules",
    );
    assert!(!opaque.operation_plan_eligible);

    let job_id = safe_manage_regenerable_scan_start(
        state,
        SafeManageRegenerableScanRequest {
            project_id: assessment.project_id,
            analysis_run_id: assessment.analysis_run_id.clone(),
            evidence_revision: assessment.evidence_revision.clone(),
            nav_id: opaque.nav_id,
            node_id: opaque.node_id,
            path: opaque.path,
        },
    )
    .expect("start exact regenerable metadata scan");
    let status = wait_for_scan(state, &job_id);
    assert_eq!(
        status.state, "completed",
        "clean exact expansion must complete: {:?}",
        status.error
    );

    let expanded = target_ending_with(
        safe_manage_regenerable_targets(
            state,
            assessment.project_id,
            &assessment.analysis_run_id,
            &assessment.evidence_revision,
        )
        .expect("reload expanded regenerable target"),
        "node_modules",
    );
    assert_eq!(expanded.evidence_state, "expanded_complete");
    assert!(expanded.operation_plan_eligible);
    expanded
}

#[test]
fn current_decision_reaches_risk_backup_holding_restart_restore_and_then_goes_stale() {
    let fixture = NativeProject::regenerable("codehangar-safe-manage-roundtrip", false);
    fixture.assert_owned_temp_paths();
    let state = open_ready_state(&fixture.db_path);
    let (root, project) = register_and_scan(&state, &fixture.project_path);
    let (_run, assessment) = analyze_and_decide(
        &state,
        project.id,
        SafeManageDecisionKind::CleanRegenerables,
    );
    let target = exact_regenerable_target(&state, &assessment);
    let request = operation_request(
        &assessment,
        SafeManageDecisionKind::CleanRegenerables,
        Some(&target),
    );
    let (plan, report) = wait_for_bound_plan(&state, request.clone());
    assert_eq!(plan.target.node_id, target.node_id);
    assert_eq!(plan.target.project_id, project.id);
    assert_eq!(report.target.node_id, target.node_id);

    let original_payload = fs::read(&fixture.payload_path).expect("read original payload");
    let (_backup, stored) =
        verified_backup_and_holding(&state, plan, &fixture.backup_path, &fixture.holding_path);
    assert!(
        !fixture.payload_path.exists(),
        "the exact regenerable payload must be held, not left at the source"
    );

    // Drop every live database handle, then reopen the real persisted inventory.
    drop(state);
    let reopened = open_ready_state(&fixture.db_path);
    let persisted = mutation_activity_log(&reopened, Some(100))
        .expect("read holding state after restart")
        .stored_entries
        .into_iter()
        .find(|entry| entry.id == stored.id)
        .expect("held entry must survive AppState restart");
    assert_eq!(persisted.status, "quarantined");
    assert!(Path::new(&persisted.stored_path).exists());

    let token = mutation_token_issue(&reopened, "enter_mutation_mode".to_string())
        .expect("issue restore confirmation token after restart")
        .token;
    let restored = mutation_restore_start(&reopened, persisted.id, token)
        .expect("restore held entry after restart");
    assert_eq!(restored.outcome, "restored");
    assert_eq!(
        fs::read(&fixture.payload_path).expect("read restored payload"),
        original_payload
    );

    // A later inventory change invalidates the old decision and its once-current
    // expansion receipt before a second preview job can even be admitted.
    fs::write(
        &fixture.payload_path,
        b"module.exports = 'changed-after-owner-decision';\n",
    )
    .expect("change disposable target after restore");
    let rescan_id = scan_start(
        &reopened,
        Some(vec![root.id]),
        Some("background".to_string()),
    )
    .expect("rescan changed disposable project");
    let rescan = wait_for_scan(&reopened, &rescan_id);
    assert_eq!(rescan.state, "completed");
    let stale =
        safe_manage_operation_plan_start(&reopened, request, Some("background".to_string()))
            .expect_err("changed evidence must invalidate the old decision/receipt");
    assert!(
        stale.contains("changed after the recommendation"),
        "stale decision must fail with a re-analysis requirement: {stale}"
    );
}

#[test]
fn partial_regenerable_receipt_stays_ineligible_across_restart() {
    let fixture = NativeProject::regenerable("codehangar-safe-manage-partial", true);
    fixture.assert_owned_temp_paths();
    let state = open_ready_state(&fixture.db_path);
    let (_root, project) = register_and_scan(&state, &fixture.project_path);
    let (_run, assessment) = analyze_and_decide(
        &state,
        project.id,
        SafeManageDecisionKind::CleanRegenerables,
    );
    let evidence_before_expansion = objective_input_at(&fixture.db_path, project.id);
    let opaque = target_ending_with(
        safe_manage_regenerable_targets(
            &state,
            assessment.project_id,
            &assessment.analysis_run_id,
            &assessment.evidence_revision,
        )
        .expect("list opaque regenerable target"),
        "node_modules",
    );
    let scan_id = safe_manage_regenerable_scan_start(
        &state,
        SafeManageRegenerableScanRequest {
            project_id: assessment.project_id,
            analysis_run_id: assessment.analysis_run_id.clone(),
            evidence_revision: assessment.evidence_revision.clone(),
            nav_id: opaque.nav_id,
            node_id: opaque.node_id,
            path: opaque.path.clone(),
        },
    )
    .expect("start partial exact expansion");
    let partial_status = wait_for_scan(&state, &scan_id);
    assert_eq!(partial_status.state, "partial");
    assert!(partial_status.partial);

    let evidence_after_expansion = objective_input_at(&fixture.db_path, project.id);
    assert_eq!(
        evidence_after_expansion.catalog_evidence_epoch,
        evidence_before_expansion.catalog_evidence_epoch,
        "exact materialisation must not advance the portfolio epoch"
    );
    assert_eq!(
        evidence_after_expansion.relationship_evidence_revision,
        evidence_before_expansion.relationship_evidence_revision,
        "exact materialisation must not alter semantic relationship evidence"
    );
    assert_eq!(
        evidence_after_expansion.file_kind_profile, evidence_before_expansion.file_kind_profile,
        "exact materialisation must not alter the bounded file-kind profile"
    );
    assert_eq!(
        evidence_after_expansion.duplicate_evidence, evidence_before_expansion.duplicate_evidence,
        "exact materialisation must not alter bounded duplicate evidence"
    );
    assert_eq!(
        evidence_after_expansion.materially_similar_project_count,
        evidence_before_expansion.materially_similar_project_count,
        "exact materialisation must not alter bounded material similarity"
    );
    assert_eq!(
        evidence_after_expansion.comparison_evidence_revision,
        evidence_before_expansion.comparison_evidence_revision,
        "exact materialisation must not alter bounded comparison evidence"
    );

    let partial_target = target_ending_with(
        safe_manage_regenerable_targets(
            &state,
            assessment.project_id,
            &assessment.analysis_run_id,
            &assessment.evidence_revision,
        )
        .expect("read persisted partial target"),
        "node_modules",
    );
    assert_eq!(partial_target.evidence_state, "expanded_partial");
    assert!(!partial_target.operation_plan_eligible);
    let request = operation_request(
        &assessment,
        SafeManageDecisionKind::CleanRegenerables,
        Some(&partial_target),
    );
    let error =
        safe_manage_operation_plan_start(&state, request.clone(), Some("background".to_string()))
            .expect_err("partial expansion receipt must not feed an OperationPlan");
    assert!(
        error.contains("complete expansion receipt"),
        "partial receipt refusal must explain the missing proof: {error}"
    );

    drop(state);
    let reopened = open_ready_state(&fixture.db_path);
    let reopened_error =
        safe_manage_operation_plan_start(&reopened, request, Some("background".to_string()))
            .expect_err("restart must not upgrade a partial receipt");
    assert!(
        reopened_error.contains("complete expansion receipt"),
        "partial receipt must remain fail-closed after restart: {reopened_error}"
    );
}

#[test]
fn permanent_removal_requires_exact_activation_and_fresh_preview_confirmation() {
    let fixture = NativeProject::archive("codehangar-safe-manage-final-remove");
    fixture.assert_owned_temp_paths();
    let state = open_ready_state(&fixture.db_path);
    let (_root, project) = register_and_scan(&state, &fixture.project_path);
    let (_run, assessment) =
        analyze_and_decide(&state, project.id, SafeManageDecisionKind::Archive);
    let request = operation_request(&assessment, SafeManageDecisionKind::Archive, None);
    let (plan, report) = wait_for_bound_plan(&state, request);
    assert_eq!(plan.target.node_id, project.id);
    assert_eq!(report.target.node_id, project.id);

    let (backup, stored) =
        verified_backup_and_holding(&state, plan, &fixture.backup_path, &fixture.holding_path);
    assert!(
        !fixture.payload_path.exists(),
        "the disposable project must be in holding before final review"
    );

    let disabled = mutation_final_remove_preview(&state, FinalRemoveScope::AllEligible)
        .expect_err("permanent removal must start disabled");
    assert!(disabled.contains("Permanent removal is off"));
    let wrong_phrase = set_final_remove_enabled(&state, true, Some("ENABLE REMOVAL".to_string()))
        .expect_err("an approximate activation phrase must fail");
    assert!(wrong_phrase.contains(FINAL_REMOVE_ACKNOWLEDGEMENT));
    let still_disabled = mutation_final_remove_preview(&state, FinalRemoveScope::AllEligible)
        .expect_err("a rejected phrase must not enable permanent removal as a side effect");
    assert!(still_disabled.contains("Permanent removal is off"));
    set_final_remove_enabled(&state, true, Some(FINAL_REMOVE_ACKNOWLEDGEMENT.to_string()))
        .expect("the exact owner phrase enables final review");

    let preview = mutation_final_remove_preview(&state, FinalRemoveScope::AllEligible)
        .expect("build fresh immutable final-remove preview");
    assert!(preview
        .objects
        .iter()
        .any(|item| item.entry_id == stored.id));
    assert!(!preview.eligible_topology_group_ids.is_empty());
    let mut wrong_digest = preview.preview_digest.clone();
    let replacement = if wrong_digest.starts_with('0') {
        "1"
    } else {
        "0"
    };
    wrong_digest.replace_range(0..1, replacement);
    mutation_final_remove_confirm(
        &state,
        preview.preview_id.clone(),
        wrong_digest,
        preview.eligible_topology_group_ids.clone(),
    )
    .expect_err("confirmation must be bound to the exact preview digest");

    let confirmation = mutation_final_remove_confirm(
        &state,
        preview.preview_id.clone(),
        preview.preview_digest.clone(),
        preview.eligible_topology_group_ids.clone(),
    )
    .expect("confirm the exact fresh preview and exact groups");
    assert_eq!(confirmation.preview_id, preview.preview_id);
    assert_eq!(confirmation.preview_digest, preview.preview_digest);

    // Confirmation alone remains recommendation/review: no final-delete helper
    // is started in CI, and both the held target and verified backup must survive.
    let after = mutation_activity_log(&state, Some(100)).expect("read post-confirm journal");
    assert!(after
        .stored_entries
        .iter()
        .any(|entry| entry.id == stored.id && entry.status == "quarantined"));
    assert!(Path::new(&stored.stored_path).exists());
    assert!(Path::new(&backup.manifest_path).exists());
}
