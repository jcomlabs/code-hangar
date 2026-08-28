#![cfg_attr(not(windows), allow(dead_code))]

#[cfg(not(all(windows, feature = "p1_ui_e2e")))]
fn main() {
    eprintln!("P1-UI-E2E-01 requires Windows and --features p1_ui_e2e.");
    std::process::exit(2);
}

#[cfg(all(windows, feature = "p1_ui_e2e"))]
mod windows_harness {
    use hangar_api::AppState;
    use serde::{Deserialize, Serialize};
    use std::ffi::OsString;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
    use tauri::{AppHandle, State, WebviewUrl};

    const TEST_ID: &str = "P1-UI-E2E-01";
    const PROJECT_NAME: &str = "p1-ui-project";
    const ORIGINAL: &[u8] = b"{\n  \"enabled\": false,\n  \"label\": \"baseline\"\n}\n";
    const MODIFIED: &[u8] = b"{\n  \"enabled\": true,\n  \"label\": \"baseline\"\n}\n";
    const EXPECTED_PHASES: &[&str] = &[
        "unlock",
        "edit",
        "review-exact-diff",
        "apply",
        "previous-versions",
        "compare",
        "restore",
    ];
    const FRONTEND_DIAGNOSTIC_SCRIPT: &str = r#"
(() => {
  let reported = false;
  const report = (message) => {
    if (reported) return;
    reported = true;
    const send = () => {
      const invoke = window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke;
      if (typeof invoke === 'function') {
        invoke('p1_e2e_complete', { observed: null, error: String(message) });
      }
    };
    window.setTimeout(send, 0);
  };
  window.addEventListener('error', (event) => {
    report(`Uncaught frontend error: ${event.message} at ${event.filename}:${event.lineno}`);
  });
  window.addEventListener('unhandledrejection', (event) => {
    const reason = event.reason && (event.reason.stack || event.reason.message || event.reason);
    report(`Unhandled frontend rejection: ${String(reason)}`);
  });
  window.addEventListener('DOMContentLoaded', () => {
    window.setTimeout(() => {
      const root = document.getElementById('root');
      if (!root || root.childElementCount === 0) {
        report('The embedded UI module did not mount a React tree.');
      }
    }, 3000);
  });
})();
"#;

    type SharedOutcome = Arc<Mutex<Option<Result<String, String>>>>;

    #[derive(Clone)]
    struct ProofState {
        temp_root: PathBuf,
        db_path: PathBuf,
        source_path: PathBuf,
        project_id: i64,
        node_id: i64,
        outcome: SharedOutcome,
    }

    #[derive(Debug, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct HarnessContext {
        project_id: i64,
        node_id: i64,
        project_name: &'static str,
    }

    #[derive(Debug, Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct UiObservation {
        phases: Vec<String>,
        edit_removed: Vec<String>,
        edit_added: Vec<String>,
        compare_removed: Vec<String>,
        compare_added: Vec<String>,
    }

    struct TempWorkspace {
        root: PathBuf,
        canonical_parent: PathBuf,
        expected_leaf: OsString,
        runner_owned: bool,
    }

    impl TempWorkspace {
        fn create() -> Result<Self, String> {
            let temp_root = std::env::temp_dir();
            let temp = temp_root
                .canonicalize()
                .map_err(|error| format!("Could not resolve the OS temporary folder: {error}"))?;
            let mut args = std::env::args_os().skip(1);
            let requested_leaf = args.next();
            let runner_owned = requested_leaf.is_some();
            if args.next().is_some() {
                return Err("P1-UI-E2E-01 accepts at most one exact temp leaf argument.".into());
            }
            let expected_leaf = match requested_leaf {
                Some(leaf) => {
                    let leaf_text = leaf.to_str().ok_or_else(|| {
                        "The requested E2E temp leaf is not valid Unicode.".to_string()
                    })?;
                    if !leaf_text.starts_with("code-hangar-p1-ui-e2e-")
                        || !leaf_text
                            .chars()
                            .all(|character| character.is_ascii_alphanumeric() || character == '-')
                        || Path::new(&leaf).file_name() != Some(leaf.as_os_str())
                    {
                        return Err(
                            "The requested E2E temp leaf is not one safe, exact Code Hangar E2E name."
                                .into(),
                        );
                    }
                    leaf
                }
                None => {
                    let nonce = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_err(|error| format!("System clock is before the Unix epoch: {error}"))?
                        .as_nanos();
                    OsString::from(format!(
                        "code-hangar-p1-ui-e2e-{}-{nonce}",
                        std::process::id()
                    ))
                }
            };
            let root = temp_root.join(&expected_leaf);
            fs::create_dir(&root).map_err(|error| {
                format!("Could not create the isolated temporary folder: {error}")
            })?;
            let canonical = root
                .canonicalize()
                .map_err(|error| format!("Could not verify the temporary folder: {error}"))?;
            let identity = hangar_fs::inspect_path_identity(&root);
            let expected_canonical = temp.join(&expected_leaf);
            if identity.inaccessible
                || identity.is_symlink
                || identity.is_reparse
                || canonical != expected_canonical
                || canonical.parent() != Some(temp.as_path())
                || canonical.file_name() != Some(expected_leaf.as_os_str())
            {
                return Err(
                    "The E2E workspace was not an ordinary directory with the exact expected name directly below the canonical OS temporary folder."
                        .into(),
                );
            }
            // Keep the ordinary drive-letter spelling at the shell/API edge.
            // `canonical` above is used only for containment proof; Windows
            // returns a verbatim `\\?\` path there, which shell-open correctly
            // refuses as an externally supplied UNC-looking input.
            Ok(Self {
                root,
                canonical_parent: temp,
                expected_leaf,
                runner_owned,
            })
        }
    }

    impl Drop for TempWorkspace {
        fn drop(&mut self) {
            for attempt in 1..=40 {
                if !self.root.exists() {
                    return;
                }
                if let Err(reason) = self.verify_cleanup_target() {
                    eprintln!(
                        "{TEST_ID} cleanup refused: {reason}; retained {:?} for safe inspection.",
                        self.root
                    );
                    return;
                }
                match fs::remove_dir_all(&self.root) {
                    Ok(()) => return,
                    Err(error) if self.runner_owned => {
                        eprintln!(
                            "{TEST_ID} cleanup deferred until the native process exits: {error}"
                        );
                        return;
                    }
                    Err(_) if attempt < 40 => {
                        thread::sleep(Duration::from_millis(50));
                        if !self.root.exists() {
                            return;
                        }
                        // Every retry re-proves the exact canonical target and
                        // reparse status before issuing another delete.
                        continue;
                    }
                    Err(error) => {
                        eprintln!(
                            "{TEST_ID} cleanup warning: could not remove verified temp workspace after {attempt} attempts: {error}"
                        );
                        return;
                    }
                }
            }
        }
    }

    impl TempWorkspace {
        fn verify_cleanup_target(&self) -> Result<(), String> {
            if self.root.file_name() != Some(self.expected_leaf.as_os_str()) {
                return Err("the workspace leaf name drifted".into());
            }

            let ordinary_parent = self
                .root
                .parent()
                .ok_or_else(|| "the workspace has no parent".to_string())?;
            let current_parent = ordinary_parent
                .canonicalize()
                .map_err(|error| format!("the workspace parent cannot be resolved: {error}"))?;
            if current_parent != self.canonical_parent {
                return Err(
                    "the workspace parent drifted from the canonical OS temp folder".into(),
                );
            }

            let identity = hangar_fs::inspect_path_identity(&self.root);
            if identity.inaccessible
                || identity.is_symlink
                || identity.is_reparse
                || identity.reparse_kind.is_some()
            {
                return Err(format!(
                    "the workspace is inaccessible or became a symlink/reparse point ({:?})",
                    identity.reparse_kind
                ));
            }

            let current_root = self
                .root
                .canonicalize()
                .map_err(|error| format!("the workspace cannot be resolved: {error}"))?;
            let expected_root = self.canonical_parent.join(&self.expected_leaf);
            if current_root != expected_root
                || current_root.parent() != Some(self.canonical_parent.as_path())
                || current_root.file_name() != Some(self.expected_leaf.as_os_str())
            {
                return Err("the canonical workspace target drifted".into());
            }
            Ok(())
        }
    }

    async fn run_blocking<T: Send + 'static>(
        task: impl FnOnce() -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        tauri::async_runtime::spawn_blocking(task)
            .await
            .map_err(|error| format!("Background task failed: {error}"))?
    }

    #[tauri::command]
    fn p1_e2e_context(proof: State<'_, ProofState>) -> HarnessContext {
        eprintln!("{TEST_ID} trace: frontend crossed IPC bootstrap.");
        HarnessContext {
            project_id: proof.project_id,
            node_id: proof.node_id,
            project_name: PROJECT_NAME,
        }
    }

    #[tauri::command]
    fn p1_e2e_probe(message: String) {
        eprintln!("{TEST_ID} WebView probe: {message}");
    }

    #[tauri::command]
    async fn editable_values(
        state: State<'_, AppState>,
        node_id: i64,
    ) -> Result<hangar_core::EditableValueSet, String> {
        eprintln!("{TEST_ID} trace: unlock completed; loading production value editor.");
        let app_state = state.inner().clone();
        run_blocking(move || hangar_api::editable_values(&app_state, node_id)).await
    }

    #[tauri::command]
    async fn preview_value_edit(
        state: State<'_, AppState>,
        node_id: i64,
        request: hangar_core::ValueEditRequest,
    ) -> Result<hangar_core::FileEditPreview, String> {
        let app_state = state.inner().clone();
        run_blocking(move || hangar_api::preview_value_edit(&app_state, node_id, &request)).await
    }

    #[tauri::command]
    async fn apply_value_edit(
        state: State<'_, AppState>,
        node_id: i64,
        request: hangar_core::ValueEditRequest,
        reviewed_after_hash: String,
    ) -> Result<hangar_core::ValueEditResult, String> {
        eprintln!("{TEST_ID} trace: applying reviewed production value edit.");
        let app_state = state.inner().clone();
        run_blocking(move || {
            hangar_api::apply_reviewed_value_edit(
                &app_state,
                node_id,
                &request,
                &reviewed_after_hash,
            )
        })
        .await
    }

    #[tauri::command]
    async fn edit_snapshots_for_node(
        state: State<'_, AppState>,
        node_id: i64,
        limit: usize,
    ) -> Result<Vec<hangar_core::EditSnapshotSummary>, String> {
        let app_state = state.inner().clone();
        run_blocking(move || hangar_api::edit_snapshots_for_node(&app_state, node_id, limit)).await
    }

    #[tauri::command]
    async fn edit_snapshot_compare(
        state: State<'_, AppState>,
        snapshot_id: i64,
    ) -> Result<hangar_core::EditSnapshotComparison, String> {
        eprintln!("{TEST_ID} trace: comparing through production Previous Versions.");
        let app_state = state.inner().clone();
        run_blocking(move || hangar_api::edit_snapshot_compare(&app_state, snapshot_id)).await
    }

    #[tauri::command]
    async fn edit_snapshot_restore(
        state: State<'_, AppState>,
        snapshot_id: i64,
    ) -> Result<hangar_core::EditSnapshotRestoreResult, String> {
        eprintln!("{TEST_ID} trace: restoring reviewed production snapshot.");
        let app_state = state.inner().clone();
        run_blocking(move || hangar_api::edit_snapshot_restore(&app_state, snapshot_id)).await
    }

    #[tauri::command]
    async fn project_checks_detect(
        state: State<'_, AppState>,
        project_id: i64,
    ) -> Result<Vec<hangar_core::ProjectCheckDefinition>, String> {
        let app_state = state.inner().clone();
        run_blocking(move || hangar_api::project_checks_detect(&app_state, project_id)).await
    }

    #[tauri::command]
    async fn p1_e2e_complete(
        app: AppHandle,
        state: State<'_, AppState>,
        proof: State<'_, ProofState>,
        observed: Option<UiObservation>,
        error: Option<String>,
    ) -> Result<(), String> {
        eprintln!("{TEST_ID} trace: frontend reported journey completion.");
        let app_state = state.inner().clone();
        let proof = proof.inner().clone();
        let outcome = Arc::clone(&proof.outcome);
        let result = if let Some(error) = error {
            Err(format!("UI journey failed: {error}"))
        } else if let Some(observed) = observed {
            run_blocking(move || verify_completed_journey(&app_state, &proof, observed)).await
        } else {
            Err("UI journey returned neither observations nor an error.".into())
        };

        let exit_code = if result.is_ok() { 0 } else { 1 };
        let mut slot = outcome
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *slot = Some(result);
        drop(slot);
        app.exit(exit_code);
        Ok(())
    }

    fn verify_completed_journey(
        state: &AppState,
        proof: &ProofState,
        observed: UiObservation,
    ) -> Result<String, String> {
        let expected_phases = EXPECTED_PHASES
            .iter()
            .map(|phase| (*phase).to_string())
            .collect::<Vec<_>>();
        require_equal(&observed.phases, &expected_phases, "UI phase sequence")?;
        require_equal(
            &observed.edit_removed,
            &vec!["  \"enabled\": false,".to_string()],
            "reviewed apply removal",
        )?;
        require_equal(
            &observed.edit_added,
            &vec!["  \"enabled\": true,".to_string()],
            "reviewed apply addition",
        )?;
        require_equal(
            &observed.compare_removed,
            &vec!["  \"enabled\": true,".to_string()],
            "reviewed restore removal",
        )?;
        require_equal(
            &observed.compare_added,
            &vec!["  \"enabled\": false,".to_string()],
            "reviewed restore addition",
        )?;

        let current = fs::read(&proof.source_path)
            .map_err(|error| format!("Could not read the restored file: {error}"))?;
        if current != ORIGINAL {
            return Err(format!(
                "Restored bytes differ: expected {:?}, got {:?}.",
                ORIGINAL, current
            ));
        }
        let db_meta = fs::metadata(&proof.db_path)
            .map_err(|error| format!("The persistent SQLite inventory is missing: {error}"))?;
        if db_meta.len() == 0 {
            return Err("The persistent SQLite inventory is empty.".into());
        }
        if !proof.source_path.starts_with(&proof.temp_root)
            || !proof.db_path.starts_with(&proof.temp_root)
        {
            return Err("The journey escaped its temporary filesystem boundary.".into());
        }

        let original_hash = blake3::hash(ORIGINAL).to_hex().to_string();
        let modified_hash = blake3::hash(MODIFIED).to_hex().to_string();
        let snapshots = hangar_api::edit_snapshots_for_node(state, proof.node_id, 20)?;
        let value_snapshot = snapshots
            .iter()
            .find(|snapshot| snapshot.origin == "value")
            .ok_or_else(|| "No Value edit snapshot was retained.".to_string())?;
        let restore_snapshot = snapshots
            .iter()
            .find(|snapshot| snapshot.origin == "restore")
            .ok_or_else(|| "No Restore safety copy snapshot was retained.".to_string())?;
        require_str(
            &value_snapshot.blake3_before,
            &original_hash,
            "Value snapshot before hash",
        )?;
        require_option_str(
            value_snapshot.blake3_after.as_deref(),
            &modified_hash,
            "Value snapshot after hash",
        )?;
        if value_snapshot.restored_at.is_none() {
            return Err("The restored Value edit snapshot was not marked restored.".into());
        }
        require_str(
            &restore_snapshot.blake3_before,
            &modified_hash,
            "Restore safety snapshot before hash",
        )?;
        require_option_str(
            restore_snapshot.blake3_after.as_deref(),
            &original_hash,
            "Restore safety snapshot after hash",
        )?;

        let ledger = hangar_api::project_review_ledger(state, proof.project_id, Some(20))?;
        let value_entry = ledger
            .iter()
            .find(|entry| entry.origin.as_deref() == Some("value"))
            .ok_or_else(|| "The review ledger has no Value edit entry.".to_string())?;
        let restore_entry = ledger
            .iter()
            .find(|entry| entry.origin.as_deref() == Some("restore"))
            .ok_or_else(|| "The review ledger has no Restore entry.".to_string())?;
        verify_ledger_entry(
            value_entry,
            proof,
            &original_hash,
            &modified_hash,
            "Changed one recognized value",
        )?;
        verify_ledger_entry(
            restore_entry,
            proof,
            &modified_hash,
            &original_hash,
            "Restored a verified previous version",
        )?;
        if value_entry.entry_hash == restore_entry.entry_hash {
            return Err(
                "Value and Restore ledger entries unexpectedly share one entry hash.".into(),
            );
        }

        Ok(format!(
            "{TEST_ID} PASS: real UI + Tauri IPC restored exact bytes; {} snapshots and {} ledger entries verified.",
            snapshots.len(),
            ledger.len()
        ))
    }

    fn verify_ledger_entry(
        entry: &hangar_core::ReviewLedgerEntry,
        proof: &ProofState,
        before_hash: &str,
        after_hash: &str,
        expected_summary: &str,
    ) -> Result<(), String> {
        if entry.node_id != Some(proof.node_id) || entry.project_id != proof.project_id {
            return Err(format!(
                "Ledger entry {} is attached to the wrong project or node.",
                entry.id
            ));
        }
        require_option_str(
            entry.before_hash.as_deref(),
            before_hash,
            "Ledger before hash",
        )?;
        require_option_str(entry.after_hash.as_deref(), after_hash, "Ledger after hash")?;
        if entry.entry_hash.len() != 64 || entry.encoded_bytes == 0 {
            return Err(format!(
                "Ledger entry {} lacks a retained hash or encoded payload size.",
                entry.id
            ));
        }
        if entry.source_kind != "Code Hangar edit history"
            || !entry
                .source_ref
                .starts_with(&format!("codehangar:{}:", proof.node_id))
        {
            return Err(format!(
                "Ledger entry {} does not identify the verified in-app write boundary.",
                entry.id
            ));
        }
        let summary = entry
            .change_set
            .files
            .first()
            .and_then(|file| file.edits.first())
            .map(|edit| edit.summary.as_str());
        if summary != Some(expected_summary) {
            return Err(format!(
                "Ledger entry {} summary mismatch: expected {expected_summary:?}, got {summary:?}.",
                entry.id
            ));
        }
        Ok(())
    }

    fn require_equal<T: std::fmt::Debug + PartialEq>(
        actual: &T,
        expected: &T,
        label: &str,
    ) -> Result<(), String> {
        if actual == expected {
            Ok(())
        } else {
            Err(format!(
                "{label} mismatch: expected {expected:?}, got {actual:?}."
            ))
        }
    }

    fn require_str(actual: &str, expected: &str, label: &str) -> Result<(), String> {
        if actual == expected {
            Ok(())
        } else {
            Err(format!(
                "{label} mismatch: expected {expected}, got {actual}."
            ))
        }
    }

    fn require_option_str(actual: Option<&str>, expected: &str, label: &str) -> Result<(), String> {
        match actual {
            Some(actual) => require_str(actual, expected, label),
            None => Err(format!("{label} is missing.")),
        }
    }

    fn wait_until_ready(state: &AppState) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let status = hangar_api::startup_status(state);
            match status.state.as_str() {
                "ready" => return Ok(()),
                "failed" => return Err(status.message),
                _ if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
                _ => return Err(format!("Inventory startup timed out: {}", status.message)),
            }
        }
    }

    fn wait_for_scan(state: &AppState, job_id: &str) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let status = hangar_api::scan_status(state, job_id.to_string())?;
            match status.state.as_str() {
                "completed" => return Ok(()),
                "failed" | "cancelled" | "partial" => {
                    return Err(format!("Temporary project scan failed: {}", status.message))
                }
                _ if Instant::now() < deadline => thread::sleep(Duration::from_millis(25)),
                _ => {
                    return Err(format!(
                        "Temporary project scan timed out: {}",
                        status.message
                    ))
                }
            }
        }
    }

    fn bootstrap(
        workspace: &TempWorkspace,
        outcome: SharedOutcome,
    ) -> Result<(AppState, ProofState), String> {
        let project_root = workspace.root.join(PROJECT_NAME);
        let app_data = workspace.root.join("app-data");
        fs::create_dir_all(&project_root)
            .map_err(|error| format!("Could not create the temporary project: {error}"))?;
        fs::create_dir_all(&app_data)
            .map_err(|error| format!("Could not create temporary app data: {error}"))?;
        let source_path = project_root.join("settings.json");
        fs::write(&source_path, ORIGINAL)
            .map_err(|error| format!("Could not create the temporary settings file: {error}"))?;
        let db_path = app_data.join("codehangar.sqlite3");
        let state = AppState::open(&db_path)?;
        wait_until_ready(&state)?;

        let prepared = hangar_api::prepare_open_target(
            &state,
            source_path.to_string_lossy().into_owned(),
            "manual".into(),
            Some(project_root.to_string_lossy().into_owned()),
            Some("balanced".into()),
        )?;
        let scan =
            hangar_api::start_open_target_scan(&state, prepared.root_id, Some("balanced".into()))?;
        wait_for_scan(&state, &scan.job_id)?;
        let node_id = hangar_api::resolve_open_target(
            &state,
            prepared.project_id,
            source_path.to_string_lossy().into_owned(),
        )?
        .ok_or_else(|| "The scanner completed without inventorying settings.json.".to_string())?;
        let values = hangar_api::editable_values(&state, node_id)?;
        if !values.values.iter().any(|value| {
            value.path.to_ascii_lowercase().contains("enabled") && value.display_value == "false"
        }) {
            return Err("The production value extractor did not expose enabled=false.".into());
        }

        Ok((
            state,
            ProofState {
                temp_root: workspace.root.clone(),
                db_path,
                source_path,
                project_id: prepared.project_id,
                node_id,
                outcome,
            },
        ))
    }

    pub fn run() -> Result<i32, String> {
        let workspace = TempWorkspace::create()?;
        let outcome: SharedOutcome = Arc::new(Mutex::new(None));
        let (app_state, proof) = bootstrap(&workspace, Arc::clone(&outcome))?;
        let app = tauri::Builder::<tauri::Wry>::default()
            .manage(app_state)
            .manage(proof)
            .invoke_handler(tauri::generate_handler![
                p1_e2e_context,
                p1_e2e_probe,
                editable_values,
                preview_value_edit,
                apply_value_edit,
                edit_snapshots_for_node,
                edit_snapshot_compare,
                edit_snapshot_restore,
                project_checks_detect,
                p1_e2e_complete
            ])
            .setup(|app| {
                tauri::WebviewWindowBuilder::new(
                    app,
                    "main",
                    WebviewUrl::App("p1-e2e.html".into()),
                )
                .title(TEST_ID)
                // A never-visible/offscreen WebView2 is eligible for lifecycle
                // suspension before its ES module executes. A two-pixel native
                // surface keeps the real renderer alive while remaining
                // non-focusable, undecorated and absent from the taskbar.
                .inner_size(2.0, 2.0)
                .position(0.0, 0.0)
                .decorations(false)
                .resizable(false)
                .skip_taskbar(true)
                .visible(true)
                .focused(false)
                .focusable(false)
                .initialization_script(FRONTEND_DIAGNOSTIC_SCRIPT)
                .on_web_resource_request(|request, response| {
                    eprintln!(
                        "{TEST_ID} resource: {} -> {}.",
                        request.uri(),
                        response.status()
                    );
                })
                .on_page_load(|webview, payload| {
                    eprintln!(
                        "{TEST_ID} trace: WebView page {:?} {}.",
                        payload.event(),
                        payload.url()
                    );
                    if let Err(error) = webview.eval(
                        r#"
(() => {
  const invoke = window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke;
  if (typeof invoke !== 'function') return;
  const root = document.getElementById('root');
  const message = JSON.stringify({
    readyState: document.readyState,
    scripts: Array.from(document.scripts).map((script) => script.src),
    rootChildren: root ? root.childElementCount : null
  });
  invoke('p1_e2e_probe', { message });
})();
"#,
                    ) {
                        eprintln!("{TEST_ID} WebView probe injection failed: {error}");
                    }
                })
                .build()?;
                Ok(())
            })
            .build(tauri::generate_context!("p1-e2e/tauri.conf.json"))
            .map_err(|error| format!("Could not build the native E2E app: {error}"))?;

        let watchdog_app = app.handle().clone();
        let watchdog_outcome = Arc::clone(&outcome);
        let watchdog = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(45);
            loop {
                {
                    let slot = watchdog_outcome
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if slot.is_some() {
                        return;
                    }
                }
                if Instant::now() >= deadline {
                    let mut slot = watchdog_outcome
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if slot.is_none() {
                        *slot = Some(Err(format!(
                            "{TEST_ID} timed out before the real UI journey completed."
                        )));
                        drop(slot);
                        watchdog_app.exit(1);
                    }
                    return;
                }
                thread::sleep(Duration::from_millis(50));
            }
        });

        let exit_code = app.run_return(|_, _| {});
        {
            let mut slot = outcome
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if slot.is_none() {
                *slot = Some(Err(format!("{TEST_ID} exited before reporting a result.")));
            }
        }
        watchdog
            .join()
            .map_err(|_| format!("{TEST_ID} watchdog thread panicked."))?;
        let result = outcome
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .expect("the native exit path always records an outcome");
        match result {
            Ok(message) if exit_code == 0 => {
                println!("{message}");
                Ok(0)
            }
            Ok(message) => Err(format!("{message} Native exit code was {exit_code}.")),
            Err(error) => Err(error),
        }
    }

    #[allow(dead_code)]
    fn _assert_temp_path(path: &Path, root: &Path) -> bool {
        path.starts_with(root)
    }
}

#[cfg(all(windows, feature = "p1_ui_e2e"))]
fn main() {
    match windows_harness::run() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
