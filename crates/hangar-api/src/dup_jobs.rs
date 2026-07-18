use hangar_core::{DuplicateConfirmProgress, DuplicateConfirmStatus, DuplicateConfirmation};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Background jobs for on-demand full-hash duplicate confirmation. Mirrors `PlanJobStore`: each job
/// streams file bytes to hash them, so it runs off the UI thread with live progress + cancel.
#[derive(Debug, Clone, Default)]
pub struct DupJobStore {
    next_id: Arc<AtomicU64>,
    jobs: Arc<Mutex<HashMap<String, DuplicateConfirmStatus>>>,
    cancels: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
}

impl DupJobStore {
    pub fn create_running(&self, target_node_id: i64) -> (String, Arc<AtomicBool>) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let job_id = format!("dup-{id}");
        let cancel = Arc::new(AtomicBool::new(false));
        let status = DuplicateConfirmStatus {
            job_id: job_id.clone(),
            state: "running".to_string(),
            target_node_id,
            message: "Verifying duplicates by full content hash.".to_string(),
            error: None,
            progress: DuplicateConfirmProgress::default(),
            result: None,
        };
        {
            let mut jobs = self.jobs.lock().expect("dup job mutex poisoned");
            // Bound the map: terminal jobs (each retaining a full DuplicateConfirmation) are
            // otherwise never removed, so a long session grows it without limit.
            prune_terminal_dup_jobs(&mut jobs);
            jobs.insert(job_id.clone(), status);
        }
        self.cancels
            .lock()
            .expect("dup cancel mutex poisoned")
            .insert(job_id.clone(), Arc::clone(&cancel));
        (job_id, cancel)
    }

    pub fn update_progress(&self, job_id: &str, progress: DuplicateConfirmProgress) {
        let mut jobs = self.jobs.lock().expect("dup job mutex poisoned");
        if let Some(status) = jobs.get_mut(job_id) {
            if matches!(status.state.as_str(), "running" | "cancelling") {
                status.progress = progress;
            }
        }
    }

    pub fn complete(&self, job_id: &str, result: DuplicateConfirmation) {
        let mut jobs = self.jobs.lock().expect("dup job mutex poisoned");
        if let Some(status) = jobs.get_mut(job_id) {
            status.state = "completed".to_string();
            status.message = "Duplicate verification complete.".to_string();
            status.error = None;
            status.result = Some(result);
        }
        self.cancels
            .lock()
            .expect("dup cancel mutex poisoned")
            .remove(job_id);
    }

    pub fn request_cancel(&self, job_id: &str) {
        if let Some(cancel) = self
            .cancels
            .lock()
            .expect("dup cancel mutex poisoned")
            .get(job_id)
        {
            cancel.store(true, Ordering::Relaxed);
        }
        let mut jobs = self.jobs.lock().expect("dup job mutex poisoned");
        if let Some(status) = jobs.get_mut(job_id) {
            if status.state == "running" {
                status.state = "cancelling".to_string();
                status.message = "Stopping verification at the next file boundary.".to_string();
            }
        }
    }

    pub fn cancel(&self, job_id: &str) {
        let mut jobs = self.jobs.lock().expect("dup job mutex poisoned");
        if let Some(status) = jobs.get_mut(job_id) {
            status.state = "cancelled".to_string();
            status.message = "Duplicate verification stopped.".to_string();
            status.error = None;
            status.result = None;
        }
        self.cancels
            .lock()
            .expect("dup cancel mutex poisoned")
            .remove(job_id);
    }

    pub fn fail(&self, job_id: &str, message: impl Into<String>) {
        let message = message.into();
        let mut jobs = self.jobs.lock().expect("dup job mutex poisoned");
        if let Some(status) = jobs.get_mut(job_id) {
            status.state = "failed".to_string();
            status.error = Some(message.clone());
            status.message = message;
            status.result = None;
        }
        self.cancels
            .lock()
            .expect("dup cancel mutex poisoned")
            .remove(job_id);
    }

    pub fn status(&self, job_id: &str) -> Option<DuplicateConfirmStatus> {
        self.jobs
            .lock()
            .expect("dup job mutex poisoned")
            .get(job_id)
            .cloned()
    }
}

/// Keep at most this many terminal (completed/cancelled/failed) confirmation records.
const MAX_TERMINAL_DUP_JOBS: usize = 20;

/// Drop the oldest terminal jobs beyond the cap. Running/cancelling jobs are always kept; job ids
/// are `dup-{n}` with `n` monotonic, so the lowest-numbered terminals are oldest.
fn prune_terminal_dup_jobs(jobs: &mut HashMap<String, DuplicateConfirmStatus>) {
    let mut terminal: Vec<String> = jobs
        .iter()
        .filter(|(_, status)| !matches!(status.state.as_str(), "running" | "cancelling"))
        .map(|(id, _)| id.clone())
        .collect();
    if terminal.len() <= MAX_TERMINAL_DUP_JOBS {
        return;
    }
    terminal.sort_by_key(|id| {
        id.strip_prefix("dup-")
            .and_then(|n| n.parse::<u64>().ok())
            .unwrap_or(0)
    });
    let remove_count = terminal.len() - MAX_TERMINAL_DUP_JOBS;
    for id in terminal.into_iter().take(remove_count) {
        jobs.remove(&id);
    }
}
