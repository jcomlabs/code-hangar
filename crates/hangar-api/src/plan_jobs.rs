use hangar_core::{OperationPlan, PlanPreviewStatus, RiskReport};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Default)]
pub struct PlanJobStore {
    next_id: Arc<AtomicU64>,
    jobs: Arc<Mutex<HashMap<String, PlanPreviewStatus>>>,
    cancels: Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>,
}

impl PlanJobStore {
    pub fn create_running(
        &self,
        target_node_id: i64,
        action_label: String,
    ) -> (String, Arc<AtomicBool>) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let job_id = format!("plan-{id}");
        let cancel = Arc::new(AtomicBool::new(false));
        let status = PlanPreviewStatus {
            job_id: job_id.clone(),
            state: "running".to_string(),
            target_node_id,
            action_label,
            message: "Calculating read-only preview plan.".to_string(),
            error: None,
            plan: None,
            report: None,
        };
        {
            let mut jobs = self.jobs.lock().expect("plan job mutex poisoned");
            // Bound the map: terminal preview jobs (each retaining a full OperationPlan +
            // RiskReport) are otherwise never removed, so a long session grows it without limit.
            prune_terminal_plan_jobs(&mut jobs);
            jobs.insert(job_id.clone(), status);
        }
        self.cancels
            .lock()
            .expect("plan cancel mutex poisoned")
            .insert(job_id.clone(), Arc::clone(&cancel));
        (job_id, cancel)
    }

    pub fn update_message(&self, job_id: &str, message: impl Into<String>) {
        let mut jobs = self.jobs.lock().expect("plan job mutex poisoned");
        if let Some(status) = jobs.get_mut(job_id) {
            if matches!(status.state.as_str(), "running" | "cancelling") {
                status.message = message.into();
            }
        }
    }

    pub fn complete(&self, job_id: &str, plan: OperationPlan, report: RiskReport) {
        let mut jobs = self.jobs.lock().expect("plan job mutex poisoned");
        if let Some(status) = jobs.get_mut(job_id) {
            status.state = "completed".to_string();
            status.message = "Preview plan calculated.".to_string();
            status.error = None;
            status.plan = Some(plan);
            status.report = Some(report);
        }
        self.cancels
            .lock()
            .expect("plan cancel mutex poisoned")
            .remove(job_id);
    }

    pub fn request_cancel(&self, job_id: &str) {
        if let Some(cancel) = self
            .cancels
            .lock()
            .expect("plan cancel mutex poisoned")
            .get(job_id)
        {
            cancel.store(true, Ordering::Relaxed);
        }
        let mut jobs = self.jobs.lock().expect("plan job mutex poisoned");
        if let Some(status) = jobs.get_mut(job_id) {
            if status.state == "running" {
                status.state = "cancelling".to_string();
                status.message =
                    "Stopping preview calculation at the next local checkpoint.".to_string();
            }
        }
    }

    pub fn cancel(&self, job_id: &str) {
        let mut jobs = self.jobs.lock().expect("plan job mutex poisoned");
        if let Some(status) = jobs.get_mut(job_id) {
            status.state = "cancelled".to_string();
            status.message = "Preview calculation stopped.".to_string();
            status.error = None;
            status.plan = None;
            status.report = None;
        }
        self.cancels
            .lock()
            .expect("plan cancel mutex poisoned")
            .remove(job_id);
    }

    pub fn fail(&self, job_id: &str, message: impl Into<String>) {
        let message = message.into();
        let mut jobs = self.jobs.lock().expect("plan job mutex poisoned");
        if let Some(status) = jobs.get_mut(job_id) {
            status.state = "failed".to_string();
            status.error = Some(message.clone());
            status.message = message;
            status.plan = None;
            status.report = None;
        }
        self.cancels
            .lock()
            .expect("plan cancel mutex poisoned")
            .remove(job_id);
    }

    pub fn status(&self, job_id: &str) -> Option<PlanPreviewStatus> {
        self.jobs
            .lock()
            .expect("plan job mutex poisoned")
            .get(job_id)
            .cloned()
    }
}

/// Keep at most this many terminal (completed/cancelled/failed) plan-preview records.
const MAX_TERMINAL_PLAN_JOBS: usize = 20;

/// Drop the oldest terminal plan-preview jobs beyond the cap. Running/cancelling jobs are always
/// kept; job ids are `plan-{n}` with `n` monotonic, so the lowest-numbered terminals are oldest.
fn prune_terminal_plan_jobs(jobs: &mut HashMap<String, PlanPreviewStatus>) {
    let mut terminal: Vec<String> = jobs
        .iter()
        .filter(|(_, status)| !matches!(status.state.as_str(), "running" | "cancelling"))
        .map(|(id, _)| id.clone())
        .collect();
    if terminal.len() <= MAX_TERMINAL_PLAN_JOBS {
        return;
    }
    terminal.sort_by_key(|id| {
        id.strip_prefix("plan-")
            .and_then(|n| n.parse::<u64>().ok())
            .unwrap_or(0)
    });
    let remove_count = terminal.len() - MAX_TERMINAL_PLAN_JOBS;
    for id in terminal.into_iter().take(remove_count) {
        jobs.remove(&id);
    }
}
