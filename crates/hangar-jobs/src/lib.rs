use hangar_core::ScanStatus;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Default)]
pub struct JobStore {
    next_id: Arc<AtomicU64>,
    jobs: Arc<Mutex<HashMap<String, ScanStatus>>>,
    cancels: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
}

impl JobStore {
    pub fn create_running(&self, message: impl Into<String>) -> String {
        self.create_running_for_roots(message, Vec::new(), Vec::new())
    }

    pub fn create_running_for_roots(
        &self,
        message: impl Into<String>,
        root_ids: Vec<i64>,
        root_paths: Vec<String>,
    ) -> String {
        self.create_running_for_roots_with_estimate(message, root_ids, root_paths, None)
    }

    pub fn create_running_for_roots_with_estimate(
        &self,
        message: impl Into<String>,
        root_ids: Vec<i64>,
        root_paths: Vec<String>,
        estimated_total_files: Option<u64>,
    ) -> String {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let job_id = format!("scan-{id}");
        let timestamp_ms = now_ms();
        let status = ScanStatus {
            job_id: job_id.clone(),
            state: "running".to_string(),
            scan_phase: "estimating".to_string(),
            scanned_files: 0,
            indexed_documents: 0,
            started_at_ms: timestamp_ms,
            phase_started_at_ms: timestamp_ms,
            last_progress_at_ms: timestamp_ms,
            updated_at_ms: timestamp_ms,
            estimated_total_files,
            estimated_total_bytes: None,
            worker_count: None,
            estimate_ms: None,
            scan_ms: None,
            body_read_ms: None,
            persist_ms: None,
            finalize_ms: None,
            accounting_select_ms: None,
            accounting_compute_ms: None,
            accounting_update_ms: None,
            partial: false,
            root_ids,
            root_paths,
            current_path: None,
            error: None,
            message: message.into(),
        };
        {
            let mut jobs = self.jobs.lock().expect("job mutex poisoned");
            // Bound the map: completed/cancelled/failed jobs are otherwise never removed, so a
            // long session that runs many scans/investigations grows it without limit. Prune the
            // oldest terminal entries on each new job (running jobs are always kept).
            prune_terminal_jobs(&mut jobs);
            jobs.insert(job_id.clone(), status);
        }
        self.cancels
            .lock()
            .expect("job cancel mutex poisoned")
            .insert(job_id.clone(), Arc::new(AtomicBool::new(false)));
        job_id
    }

    pub fn update_progress(
        &self,
        job_id: &str,
        scanned_files: u64,
        indexed_documents: u64,
        current_path: Option<String>,
        message: impl Into<String>,
    ) {
        let mut jobs = self.jobs.lock().expect("job mutex poisoned");
        let status = jobs
            .entry(job_id.to_string())
            .or_insert_with(|| unknown_status(job_id));
        if matches!(
            status.state.as_str(),
            "cancelling" | "cancelled" | "failed" | "completed" | "partial"
        ) {
            return;
        }
        let timestamp_ms = now_ms();
        status.state = "running".to_string();
        if status.scan_phase != "scanning" {
            status.scan_phase = "scanning".to_string();
            status.phase_started_at_ms = timestamp_ms;
        }
        status.scanned_files = scanned_files;
        status.indexed_documents = indexed_documents;
        status.updated_at_ms = timestamp_ms;
        status.last_progress_at_ms = timestamp_ms;
        status.current_path = current_path;
        status.message = message.into();
    }

    pub fn set_worker_count(&self, job_id: &str, worker_count: u64) {
        let mut jobs = self.jobs.lock().expect("job mutex poisoned");
        let status = jobs
            .entry(job_id.to_string())
            .or_insert_with(|| unknown_status(job_id));
        status.worker_count = Some(worker_count);
        status.updated_at_ms = now_ms();
    }

    pub fn add_timing(&self, job_id: &str, timing_key: &str, elapsed_ms: u64) {
        let mut jobs = self.jobs.lock().expect("job mutex poisoned");
        let status = jobs
            .entry(job_id.to_string())
            .or_insert_with(|| unknown_status(job_id));
        match timing_key {
            "estimate" => add_ms(&mut status.estimate_ms, elapsed_ms),
            "scan" => add_ms(&mut status.scan_ms, elapsed_ms),
            "body_read" => add_ms(&mut status.body_read_ms, elapsed_ms),
            "persist" => add_ms(&mut status.persist_ms, elapsed_ms),
            "finalize" => add_ms(&mut status.finalize_ms, elapsed_ms),
            "accounting_select" => add_ms(&mut status.accounting_select_ms, elapsed_ms),
            "accounting_compute" => add_ms(&mut status.accounting_compute_ms, elapsed_ms),
            "accounting_update" => add_ms(&mut status.accounting_update_ms, elapsed_ms),
            _ => {}
        }
        status.updated_at_ms = now_ms();
    }

    pub fn update_estimation(
        &self,
        job_id: &str,
        current_path: Option<String>,
        message: impl Into<String>,
    ) {
        let mut jobs = self.jobs.lock().expect("job mutex poisoned");
        let status = jobs
            .entry(job_id.to_string())
            .or_insert_with(|| unknown_status(job_id));
        if matches!(
            status.state.as_str(),
            "cancelling" | "cancelled" | "failed" | "completed" | "partial"
        ) {
            return;
        }
        let timestamp_ms = now_ms();
        status.state = "running".to_string();
        if status.scan_phase != "estimating" {
            status.scan_phase = "estimating".to_string();
            status.phase_started_at_ms = timestamp_ms;
        }
        status.updated_at_ms = timestamp_ms;
        status.current_path = current_path;
        status.message = message.into();
    }

    pub fn update_phase(
        &self,
        job_id: &str,
        scan_phase: impl Into<String>,
        current_path: Option<String>,
        message: impl Into<String>,
    ) {
        let mut jobs = self.jobs.lock().expect("job mutex poisoned");
        let status = jobs
            .entry(job_id.to_string())
            .or_insert_with(|| unknown_status(job_id));
        if matches!(
            status.state.as_str(),
            "cancelling" | "cancelled" | "failed" | "completed" | "partial"
        ) {
            return;
        }
        let timestamp_ms = now_ms();
        status.state = "running".to_string();
        let next_phase = scan_phase.into();
        if status.scan_phase != next_phase {
            status.scan_phase = next_phase;
            status.phase_started_at_ms = timestamp_ms;
        }
        status.updated_at_ms = timestamp_ms;
        status.current_path = current_path;
        status.message = message.into();
    }

    pub fn set_estimate(
        &self,
        job_id: &str,
        estimated_total_files: u64,
        estimated_total_bytes: u64,
        message: impl Into<String>,
    ) {
        let mut jobs = self.jobs.lock().expect("job mutex poisoned");
        let status = jobs
            .entry(job_id.to_string())
            .or_insert_with(|| unknown_status(job_id));
        if matches!(
            status.state.as_str(),
            "cancelling" | "cancelled" | "failed" | "completed" | "partial"
        ) {
            return;
        }
        let timestamp_ms = now_ms();
        status.state = "running".to_string();
        if status.scan_phase != "scanning" {
            status.scan_phase = "scanning".to_string();
            status.phase_started_at_ms = timestamp_ms;
        }
        status.updated_at_ms = timestamp_ms;
        status.estimated_total_files = Some(estimated_total_files);
        status.estimated_total_bytes = Some(estimated_total_bytes);
        status.current_path = None;
        status.message = message.into();
    }

    pub fn complete(&self, job_id: &str, scanned_files: u64, indexed_documents: u64) {
        let mut jobs = self.jobs.lock().expect("job mutex poisoned");
        let root_ids = previous_root_ids(&jobs, job_id);
        let root_paths = previous_root_paths(&jobs, job_id);
        let started_at_ms = previous_started_at_ms(&jobs, job_id);
        let phase_started_at_ms = now_ms();
        let last_progress_at_ms = previous_last_progress_at_ms(&jobs, job_id);
        let estimated_total_files =
            previous_estimated_total_files(&jobs, job_id).or(Some(scanned_files));
        let estimated_total_bytes = previous_estimated_total_bytes(&jobs, job_id);
        let worker_count = previous_worker_count(&jobs, job_id);
        let timings = previous_timings(&jobs, job_id);
        jobs.insert(
            job_id.to_string(),
            with_timings(
                ScanStatus {
                    job_id: job_id.to_string(),
                    state: "completed".to_string(),
                    scan_phase: "completed".to_string(),
                    scanned_files,
                    indexed_documents,
                    started_at_ms,
                    phase_started_at_ms,
                    last_progress_at_ms,
                    updated_at_ms: phase_started_at_ms,
                    estimated_total_files,
                    estimated_total_bytes,
                    worker_count,
                    estimate_ms: None,
                    scan_ms: None,
                    body_read_ms: None,
                    persist_ms: None,
                    finalize_ms: None,
                    accounting_select_ms: None,
                    accounting_compute_ms: None,
                    accounting_update_ms: None,
                    partial: false,
                    root_ids,
                    root_paths,
                    current_path: None,
                    error: None,
                    message: "Inventory scan complete.".to_string(),
                },
                timings,
            ),
        );
        self.cancels
            .lock()
            .expect("job cancel mutex poisoned")
            .remove(job_id);
    }

    pub fn complete_partial(
        &self,
        job_id: &str,
        scanned_files: u64,
        indexed_documents: u64,
        message: impl Into<String>,
    ) {
        let mut jobs = self.jobs.lock().expect("job mutex poisoned");
        let root_ids = previous_root_ids(&jobs, job_id);
        let root_paths = previous_root_paths(&jobs, job_id);
        let started_at_ms = previous_started_at_ms(&jobs, job_id);
        let phase_started_at_ms = now_ms();
        let last_progress_at_ms = previous_last_progress_at_ms(&jobs, job_id);
        let estimated_total_files = previous_estimated_total_files(&jobs, job_id);
        let estimated_total_bytes = previous_estimated_total_bytes(&jobs, job_id);
        let worker_count = previous_worker_count(&jobs, job_id);
        let timings = previous_timings(&jobs, job_id);
        jobs.insert(
            job_id.to_string(),
            with_timings(
                ScanStatus {
                    job_id: job_id.to_string(),
                    state: "partial".to_string(),
                    scan_phase: "partial".to_string(),
                    scanned_files,
                    indexed_documents,
                    started_at_ms,
                    phase_started_at_ms,
                    last_progress_at_ms,
                    updated_at_ms: phase_started_at_ms,
                    estimated_total_files,
                    estimated_total_bytes,
                    worker_count,
                    estimate_ms: None,
                    scan_ms: None,
                    body_read_ms: None,
                    persist_ms: None,
                    finalize_ms: None,
                    accounting_select_ms: None,
                    accounting_compute_ms: None,
                    accounting_update_ms: None,
                    partial: true,
                    root_ids,
                    root_paths,
                    current_path: None,
                    error: None,
                    message: message.into(),
                },
                timings,
            ),
        );
        self.cancels
            .lock()
            .expect("job cancel mutex poisoned")
            .remove(job_id);
    }

    pub fn request_cancel(&self, job_id: &str) {
        if let Some(cancel) = self
            .cancels
            .lock()
            .expect("job cancel mutex poisoned")
            .get(job_id)
        {
            cancel.store(true, Ordering::Relaxed);
        }
        let mut jobs = self.jobs.lock().expect("job mutex poisoned");
        let status = jobs
            .entry(job_id.to_string())
            .or_insert_with(|| unknown_status(job_id));
        status.state = "cancelling".to_string();
        status.scan_phase = "cancelling".to_string();
        let timestamp_ms = now_ms();
        status.updated_at_ms = timestamp_ms;
        status.phase_started_at_ms = timestamp_ms;
        status.message = "Stop requested. Scanner will stop at the next safe point.".to_string();
    }

    pub fn cancel(&self, job_id: &str, scanned_files: u64, indexed_documents: u64) {
        let mut jobs = self.jobs.lock().expect("job mutex poisoned");
        let root_ids = previous_root_ids(&jobs, job_id);
        let root_paths = previous_root_paths(&jobs, job_id);
        let previous_scanned_files = previous_scanned_files(&jobs, job_id);
        let previous_indexed_documents = previous_indexed_documents(&jobs, job_id);
        let started_at_ms = previous_started_at_ms(&jobs, job_id);
        let phase_started_at_ms = now_ms();
        let last_progress_at_ms = previous_last_progress_at_ms(&jobs, job_id);
        let estimated_total_files = previous_estimated_total_files(&jobs, job_id);
        let estimated_total_bytes = previous_estimated_total_bytes(&jobs, job_id);
        let worker_count = previous_worker_count(&jobs, job_id);
        let timings = previous_timings(&jobs, job_id);
        jobs.insert(
            job_id.to_string(),
            with_timings(
                ScanStatus {
                    job_id: job_id.to_string(),
                    state: "cancelled".to_string(),
                    scan_phase: "cancelled".to_string(),
                    scanned_files: scanned_files.max(previous_scanned_files),
                    indexed_documents: indexed_documents.max(previous_indexed_documents),
                    started_at_ms,
                    phase_started_at_ms,
                    last_progress_at_ms,
                    updated_at_ms: phase_started_at_ms,
                    estimated_total_files,
                    estimated_total_bytes,
                    worker_count,
                    estimate_ms: None,
                    scan_ms: None,
                    body_read_ms: None,
                    persist_ms: None,
                    finalize_ms: None,
                    accounting_select_ms: None,
                    accounting_compute_ms: None,
                    accounting_update_ms: None,
                    partial: true,
                    root_ids,
                    root_paths,
                    current_path: None,
                    error: None,
                    message: "Scan cancelled. Partial inventory remains incomplete.".to_string(),
                },
                timings,
            ),
        );
        self.cancels
            .lock()
            .expect("job cancel mutex poisoned")
            .remove(job_id);
    }

    pub fn fail(&self, job_id: &str, message: impl Into<String>) {
        let message = message.into();
        let mut jobs = self.jobs.lock().expect("job mutex poisoned");
        let mut status = jobs
            .get(job_id)
            .cloned()
            .unwrap_or_else(|| unknown_status(job_id));
        status.state = "failed".to_string();
        status.scan_phase = "failed".to_string();
        let timestamp_ms = now_ms();
        status.updated_at_ms = timestamp_ms;
        status.phase_started_at_ms = timestamp_ms;
        status.error = Some(message.clone());
        status.message = message;
        jobs.insert(job_id.to_string(), status);
        self.cancels
            .lock()
            .expect("job cancel mutex poisoned")
            .remove(job_id);
    }

    pub fn is_cancelled(&self, job_id: &str) -> bool {
        self.cancels
            .lock()
            .expect("job cancel mutex poisoned")
            .get(job_id)
            .map(|cancel| cancel.load(Ordering::Relaxed))
            .unwrap_or(false)
    }

    pub fn cancel_token(&self, job_id: &str) -> Arc<AtomicBool> {
        self.cancels
            .lock()
            .expect("job cancel mutex poisoned")
            .get(job_id)
            .cloned()
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)))
    }

    pub fn has_running_job_for_root(&self, root_id: i64) -> bool {
        self.jobs
            .lock()
            .expect("job mutex poisoned")
            .values()
            .any(|status| {
                matches!(status.state.as_str(), "running" | "cancelling")
                    && status.root_ids.contains(&root_id)
            })
    }

    pub fn has_any_running_job(&self) -> bool {
        self.jobs
            .lock()
            .expect("job mutex poisoned")
            .values()
            .any(|status| matches!(status.state.as_str(), "running" | "cancelling"))
    }

    pub fn status(&self, job_id: &str) -> Option<ScanStatus> {
        self.jobs
            .lock()
            .expect("job mutex poisoned")
            .get(job_id)
            .cloned()
    }
}

fn previous_root_ids(jobs: &HashMap<String, ScanStatus>, job_id: &str) -> Vec<i64> {
    jobs.get(job_id)
        .map(|status| status.root_ids.clone())
        .unwrap_or_default()
}

fn previous_root_paths(jobs: &HashMap<String, ScanStatus>, job_id: &str) -> Vec<String> {
    jobs.get(job_id)
        .map(|status| status.root_paths.clone())
        .unwrap_or_default()
}

fn previous_scanned_files(jobs: &HashMap<String, ScanStatus>, job_id: &str) -> u64 {
    jobs.get(job_id)
        .map(|status| status.scanned_files)
        .unwrap_or_default()
}

fn previous_indexed_documents(jobs: &HashMap<String, ScanStatus>, job_id: &str) -> u64 {
    jobs.get(job_id)
        .map(|status| status.indexed_documents)
        .unwrap_or_default()
}

fn previous_started_at_ms(jobs: &HashMap<String, ScanStatus>, job_id: &str) -> u64 {
    jobs.get(job_id)
        .map(|status| status.started_at_ms)
        .unwrap_or_else(now_ms)
}

fn previous_last_progress_at_ms(jobs: &HashMap<String, ScanStatus>, job_id: &str) -> u64 {
    jobs.get(job_id)
        .map(|status| status.last_progress_at_ms)
        .unwrap_or_else(now_ms)
}

fn previous_estimated_total_files(jobs: &HashMap<String, ScanStatus>, job_id: &str) -> Option<u64> {
    jobs.get(job_id)
        .and_then(|status| status.estimated_total_files)
}

fn previous_estimated_total_bytes(jobs: &HashMap<String, ScanStatus>, job_id: &str) -> Option<u64> {
    jobs.get(job_id)
        .and_then(|status| status.estimated_total_bytes)
}

fn previous_worker_count(jobs: &HashMap<String, ScanStatus>, job_id: &str) -> Option<u64> {
    jobs.get(job_id).and_then(|status| status.worker_count)
}

#[derive(Debug, Clone, Copy, Default)]
struct ScanTimings {
    estimate_ms: Option<u64>,
    scan_ms: Option<u64>,
    body_read_ms: Option<u64>,
    persist_ms: Option<u64>,
    finalize_ms: Option<u64>,
    accounting_select_ms: Option<u64>,
    accounting_compute_ms: Option<u64>,
    accounting_update_ms: Option<u64>,
}

fn previous_timings(jobs: &HashMap<String, ScanStatus>, job_id: &str) -> ScanTimings {
    jobs.get(job_id)
        .map(|status| ScanTimings {
            estimate_ms: status.estimate_ms,
            scan_ms: status.scan_ms,
            body_read_ms: status.body_read_ms,
            persist_ms: status.persist_ms,
            finalize_ms: status.finalize_ms,
            accounting_select_ms: status.accounting_select_ms,
            accounting_compute_ms: status.accounting_compute_ms,
            accounting_update_ms: status.accounting_update_ms,
        })
        .unwrap_or_default()
}

fn with_timings(mut status: ScanStatus, timings: ScanTimings) -> ScanStatus {
    status.estimate_ms = timings.estimate_ms;
    status.scan_ms = timings.scan_ms;
    status.body_read_ms = timings.body_read_ms;
    status.persist_ms = timings.persist_ms;
    status.finalize_ms = timings.finalize_ms;
    status.accounting_select_ms = timings.accounting_select_ms;
    status.accounting_compute_ms = timings.accounting_compute_ms;
    status.accounting_update_ms = timings.accounting_update_ms;
    status
}

fn add_ms(slot: &mut Option<u64>, elapsed_ms: u64) {
    *slot = Some(slot.unwrap_or(0).saturating_add(elapsed_ms));
}

fn unknown_status(job_id: &str) -> ScanStatus {
    let timestamp_ms = now_ms();
    ScanStatus {
        job_id: job_id.to_string(),
        state: "unknown".to_string(),
        scan_phase: "unknown".to_string(),
        scanned_files: 0,
        indexed_documents: 0,
        started_at_ms: timestamp_ms,
        phase_started_at_ms: timestamp_ms,
        last_progress_at_ms: timestamp_ms,
        updated_at_ms: timestamp_ms,
        estimated_total_files: None,
        estimated_total_bytes: None,
        worker_count: None,
        estimate_ms: None,
        scan_ms: None,
        body_read_ms: None,
        persist_ms: None,
        finalize_ms: None,
        accounting_select_ms: None,
        accounting_compute_ms: None,
        accounting_update_ms: None,
        partial: false,
        root_ids: Vec::new(),
        root_paths: Vec::new(),
        current_path: None,
        error: Some("Unknown scan job.".to_string()),
        message: "Unknown scan job.".to_string(),
    }
}

/// Keep at most this many terminal (completed/cancelled/failed) job records.
const MAX_TERMINAL_JOBS: usize = 40;

/// Cap the number of retained terminal jobs so the map cannot grow without bound over a long
/// session. Running jobs are always kept; the oldest terminal entries (by `updated_at_ms`)
/// beyond the cap are dropped — the frontend has long since polled their final status.
fn prune_terminal_jobs(jobs: &mut HashMap<String, ScanStatus>) {
    let mut terminal: Vec<(String, u64)> = jobs
        .iter()
        .filter(|(_, status)| status.state != "running")
        .map(|(id, status)| (id.clone(), status.updated_at_ms))
        .collect();
    if terminal.len() <= MAX_TERMINAL_JOBS {
        return;
    }
    terminal.sort_by_key(|(_, ts)| std::cmp::Reverse(*ts));
    for (id, _) in terminal.into_iter().skip(MAX_TERMINAL_JOBS) {
        jobs.remove(&id);
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::JobStore;

    #[test]
    fn cancellation_preserves_latest_progress_counts() {
        let jobs = JobStore::default();
        let job_id = jobs.create_running_for_roots_with_estimate(
            "Scanning.",
            vec![42],
            vec!["C:\\large-root".to_string()],
            Some(100_000),
        );

        jobs.set_worker_count(&job_id, 8);
        jobs.update_progress(&job_id, 54_750, 749, None, "Scanning local metadata.");
        jobs.set_estimate(&job_id, 100_000, 1_073_741_824, "Estimate complete.");
        jobs.request_cancel(&job_id);
        jobs.cancel(&job_id, 0, 0);

        let status = jobs.status(&job_id).unwrap();
        assert_eq!(status.state, "cancelled");
        assert_eq!(status.scanned_files, 54_750);
        assert_eq!(status.indexed_documents, 749);
        assert_eq!(status.root_ids, vec![42]);
        assert_eq!(status.root_paths, vec!["C:\\large-root"]);
        assert_eq!(status.estimated_total_files, Some(100_000));
        assert_eq!(status.estimated_total_bytes, Some(1_073_741_824));
        assert_eq!(status.worker_count, Some(8));
        assert!(status.partial);
        assert!(status.updated_at_ms >= status.started_at_ms);
    }

    #[test]
    fn partial_completion_does_not_report_as_completed() {
        let jobs = JobStore::default();
        let job_id = jobs.create_running_for_roots_with_estimate(
            "Scanning.",
            vec![7],
            vec!["C:\\partial-root".to_string()],
            Some(100_000),
        );

        jobs.set_estimate(&job_id, 100_000, 1_073_741_824, "Estimate complete.");
        jobs.complete_partial(&job_id, 33_500, 120, "Inventory is partial.");

        let status = jobs.status(&job_id).unwrap();
        assert_eq!(status.state, "partial");
        assert_eq!(status.scan_phase, "partial");
        assert_eq!(status.scanned_files, 33_500);
        assert_eq!(status.indexed_documents, 120);
        assert_eq!(status.estimated_total_files, Some(100_000));
        assert!(status.partial);
    }
}
