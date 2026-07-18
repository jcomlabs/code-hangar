use hangar_core::{PerformanceModePlan, ProcessResourceUsage, SystemResourceProfile};
use hangar_fs::ScanLimits;

const MIB: u64 = 1024 * 1024;
const GPU_SUMMARY: &str =
    "GPU/VRAM not used by local inventory tasks. Scans, SQLite and text analysis are CPU/I/O bound.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PerformanceMode {
    Balanced,
    Boost,
    Max,
}

impl PerformanceMode {
    pub fn parse(value: Option<&str>) -> Self {
        match value.unwrap_or_default().to_ascii_lowercase().as_str() {
            "priority" | "boost" => Self::Boost,
            "max" | "max_cpu" | "max-cpu" => Self::Max,
            _ => Self::Balanced,
        }
    }

    pub fn is_boost(self) -> bool {
        matches!(self, Self::Boost | Self::Max)
    }

    pub fn is_max(self) -> bool {
        matches!(self, Self::Max)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Balanced => "Balanced",
            Self::Boost => "Priority",
            Self::Max => "Max CPU",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Balanced => "balanced",
            Self::Boost => "priority",
            Self::Max => "max",
        }
    }

    fn process_priority_label(self) -> &'static str {
        match self {
            Self::Balanced => "normal",
            Self::Boost | Self::Max => "above normal only while a heavy task runs on Windows",
        }
    }
}

pub struct PerformanceScope {
    mode: PerformanceMode,
}

impl PerformanceScope {
    pub fn enter(mode: PerformanceMode) -> Self {
        if mode.is_boost() {
            platform::enter_mode(mode);
        }
        Self { mode }
    }
}

impl Drop for PerformanceScope {
    fn drop(&mut self) {
        if self.mode.is_boost() {
            platform::leave_mode(self.mode);
        }
    }
}

pub fn scan_limits(resume: bool, mode: PerformanceMode) -> ScanLimits {
    let mut limits = if resume {
        ScanLimits::resume_subtree()
    } else {
        ScanLimits::root_scan()
    };
    if mode.is_max() {
        limits.batch_size = 6_000;
        limits.worker_count = max_worker_count();
    } else if mode.is_boost() {
        limits.batch_size = 4_000;
        limits.worker_count = boost_worker_count();
    } else {
        limits.batch_size = 2_000;
        limits.worker_count = balanced_worker_count();
    }
    limits
}

pub fn set_global_mode(_mode: PerformanceMode) {
    // The selected mode is sent with each heavy operation. Keeping the whole
    // desktop process elevated while idle wastes scheduling priority and can
    // interfere with the AI tools Code Hangar is meant to accompany.
}

pub fn system_resource_profile() -> SystemResourceProfile {
    let logical_cpu_count = available_worker_count() as u64;
    let memory = platform::memory_status();
    let plans = [
        PerformanceMode::Balanced,
        PerformanceMode::Boost,
        PerformanceMode::Max,
    ]
    .into_iter()
    .map(|mode| plan_for_mode(mode, memory.available_memory_bytes))
    .collect();

    SystemResourceProfile {
        logical_cpu_count,
        total_memory_bytes: memory.total_memory_bytes,
        available_memory_bytes: memory.available_memory_bytes,
        gpu_acceleration: "Not used by current local inventory tasks. Scans, SQLite work and text analysis are CPU/I/O bound; GPU/VRAM remains reserved for future model/media workloads.".to_string(),
        dedicated_vram_bytes: None,
        plans,
    }
}

/// Live snapshot of this process's own CPU and memory use, plus headline system
/// memory. CPU percent is computed against the previous sample, so the first
/// call after start returns 0 until a baseline exists. Local-only; no network.
pub fn process_resource_usage() -> ProcessResourceUsage {
    let logical_cpu_count = available_worker_count() as u64;
    let memory = platform::memory_status();
    let (cpu_percent, working_set, private, sampled) = match platform::process_sample() {
        Some(sample) => (
            sample.cpu_percent,
            Some(sample.working_set_bytes),
            Some(sample.private_bytes),
            true,
        ),
        None => (0.0, None, None, false),
    };
    ProcessResourceUsage {
        cpu_percent,
        logical_cpu_count,
        memory_working_set_bytes: working_set,
        memory_private_bytes: private,
        total_memory_bytes: memory.total_memory_bytes,
        available_memory_bytes: memory.available_memory_bytes,
        gpu_summary: GPU_SUMMARY.to_string(),
        gpu_usage_percent: None,
        sampled,
    }
}

fn boost_worker_count() -> usize {
    let available = available_worker_count();
    ((available * 3).div_ceil(4)).clamp(2, 48)
}

fn max_worker_count() -> usize {
    available_worker_count().clamp(2, 64)
}

fn balanced_worker_count() -> usize {
    let available = available_worker_count();
    available.div_ceil(4).clamp(1, 8)
}

fn available_worker_count() -> usize {
    std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(4)
}

fn plan_for_mode(
    mode: PerformanceMode,
    available_memory_bytes: Option<u64>,
) -> PerformanceModePlan {
    let limits = scan_limits(false, mode);
    let memory_budget_bytes = available_memory_bytes.map(|available| {
        let divisor = match mode {
            PerformanceMode::Balanced => 16,
            PerformanceMode::Boost => 6,
            PerformanceMode::Max => 2,
        };
        let cap = match mode {
            PerformanceMode::Balanced => 512 * MIB,
            PerformanceMode::Boost => 2 * 1024 * MIB,
            PerformanceMode::Max => 8 * 1024 * MIB,
        };
        (available / divisor).clamp(128 * MIB, cap)
    });
    PerformanceModePlan {
        mode: mode.key().to_string(),
        label: mode.label().to_string(),
        cpu_threads: limits.worker_count.max(1) as u64,
        process_priority: mode.process_priority_label().to_string(),
        scan_batch_size: limits.batch_size as u64,
        memory_budget_bytes,
        notes: notes_for_mode(mode),
    }
}

fn notes_for_mode(mode: PerformanceMode) -> Vec<String> {
    match mode {
        PerformanceMode::Balanced => vec![
            "Uses a small slice of local CPU threads for new scans.".to_string(),
            "Keeps process priority normal so the rest of Windows stays responsive.".to_string(),
        ],
        PerformanceMode::Boost => vec![
            "Uses about three quarters of local CPU threads for metadata workers.".to_string(),
            "Uses above-normal process priority only for the lifetime of the heavy task."
                .to_string(),
        ],
        PerformanceMode::Max => vec![
            "Uses all available logical CPU threads for newly started scans, capped at 64 workers."
                .to_string(),
            "Uses above-normal process priority only while the heavy task runs; idle UI stays normal."
                .to_string(),
        ],
    }
}

struct MemoryStatus {
    total_memory_bytes: Option<u64>,
    available_memory_bytes: Option<u64>,
}

struct ProcessSample {
    cpu_percent: f64,
    working_set_bytes: u64,
    private_bytes: u64,
}

#[cfg(windows)]
mod platform {
    use super::MemoryStatus;
    use super::PerformanceMode;
    use super::ProcessSample;
    use std::sync::Mutex;
    use std::time::Instant;
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetPriorityClass, GetProcessTimes, SetPriorityClass,
        ABOVE_NORMAL_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
    };

    static CPU_SAMPLE: Mutex<Option<(u64, Instant)>> = Mutex::new(None);

    struct PriorityState {
        scoped_boost_depth: usize,
        scoped_max_depth: usize,
        applied: bool,
        previous: u32,
        applied_class: u32,
    }

    static PRIORITY_STATE: Mutex<PriorityState> = Mutex::new(PriorityState {
        scoped_boost_depth: 0,
        scoped_max_depth: 0,
        applied: false,
        previous: NORMAL_PRIORITY_CLASS,
        applied_class: NORMAL_PRIORITY_CLASS,
    });

    pub fn enter_mode(mode: PerformanceMode) {
        let mut state = PRIORITY_STATE
            .lock()
            .expect("performance priority mutex poisoned");
        if mode.is_max() {
            state.scoped_max_depth += 1;
        } else if mode.is_boost() {
            state.scoped_boost_depth += 1;
        }
        apply_priority(&mut state);
    }

    pub fn leave_mode(mode: PerformanceMode) {
        let mut state = PRIORITY_STATE
            .lock()
            .expect("performance priority mutex poisoned");
        if mode.is_max() {
            state.scoped_max_depth = state.scoped_max_depth.saturating_sub(1);
        } else if mode.is_boost() {
            state.scoped_boost_depth = state.scoped_boost_depth.saturating_sub(1);
        }
        apply_priority(&mut state);
    }

    fn apply_priority(state: &mut PriorityState) {
        let target_class = if state.scoped_max_depth > 0 || state.scoped_boost_depth > 0 {
            Some(ABOVE_NORMAL_PRIORITY_CLASS)
        } else {
            None
        };

        if let Some(target_class) = target_class {
            let process = unsafe { GetCurrentProcess() };
            if !state.applied {
                let current = unsafe { GetPriorityClass(process) };
                state.previous = if current == 0 {
                    NORMAL_PRIORITY_CLASS
                } else {
                    current
                };
            }
            if !state.applied || state.applied_class != target_class {
                let _ = unsafe { SetPriorityClass(process, target_class) };
                state.applied_class = target_class;
            }
            state.applied = true;
        } else if state.applied {
            let process = unsafe { GetCurrentProcess() };
            let _ = unsafe { SetPriorityClass(process, state.previous) };
            state.applied = false;
            state.applied_class = NORMAL_PRIORITY_CLASS;
        }
    }

    pub fn memory_status() -> MemoryStatus {
        let mut status = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
        if ok == 0 {
            return MemoryStatus {
                total_memory_bytes: None,
                available_memory_bytes: None,
            };
        }
        MemoryStatus {
            total_memory_bytes: Some(status.ullTotalPhys),
            available_memory_bytes: Some(status.ullAvailPhys),
        }
    }

    pub fn process_sample() -> Option<ProcessSample> {
        let process = unsafe { GetCurrentProcess() };

        let mut creation = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut exit = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut kernel = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let mut user = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let times_ok =
            unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) };
        let cpu_percent = if times_ok != 0 {
            let proc_time = filetime_to_100ns(kernel).saturating_add(filetime_to_100ns(user));
            let now = Instant::now();
            let mut percent = 0.0;
            if let Ok(mut guard) = CPU_SAMPLE.lock() {
                if let Some((prev_time, prev_instant)) = *guard {
                    let wall_100ns = now.duration_since(prev_instant).as_nanos() as f64 / 100.0;
                    if wall_100ns > 0.0 {
                        let cpu_delta = proc_time.saturating_sub(prev_time) as f64;
                        let ncpu = super::available_worker_count().max(1) as f64;
                        percent = ((cpu_delta / wall_100ns) / ncpu * 100.0).clamp(0.0, 100.0);
                    }
                }
                *guard = Some((proc_time, now));
            }
            percent
        } else {
            0.0
        };

        let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
        counters.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        let mem_ok = unsafe { GetProcessMemoryInfo(process, &mut counters, counters.cb) };
        let (working_set_bytes, private_bytes) = if mem_ok != 0 {
            (
                counters.WorkingSetSize as u64,
                counters.PagefileUsage as u64,
            )
        } else {
            (0, 0)
        };

        Some(ProcessSample {
            cpu_percent,
            working_set_bytes,
            private_bytes,
        })
    }

    fn filetime_to_100ns(ft: FILETIME) -> u64 {
        ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
    }
}

#[cfg(not(windows))]
mod platform {
    use super::{MemoryStatus, PerformanceMode, ProcessSample};

    pub fn enter_mode(_mode: PerformanceMode) {}
    pub fn leave_mode(_mode: PerformanceMode) {}
    pub fn memory_status() -> MemoryStatus {
        MemoryStatus {
            total_memory_bytes: None,
            available_memory_bytes: None,
        }
    }
    pub fn process_sample() -> Option<ProcessSample> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unknown_modes_as_balanced() {
        assert_eq!(PerformanceMode::parse(None), PerformanceMode::Balanced);
        assert_eq!(
            PerformanceMode::parse(Some("ordinary")),
            PerformanceMode::Balanced
        );
        assert_eq!(
            PerformanceMode::parse(Some("BOOST")),
            PerformanceMode::Boost
        );
        assert_eq!(
            PerformanceMode::parse(Some("priority")),
            PerformanceMode::Boost
        );
        assert_eq!(
            PerformanceMode::parse(Some("max-cpu")),
            PerformanceMode::Max
        );
    }

    #[test]
    fn boost_uses_larger_scan_batches() {
        let balanced = scan_limits(false, PerformanceMode::Balanced);
        let boosted = scan_limits(false, PerformanceMode::Boost);

        assert!(boosted.batch_size > balanced.batch_size);
        assert!(boosted.worker_count > balanced.worker_count);
    }

    #[test]
    fn max_uses_all_available_workers() {
        let boosted = scan_limits(false, PerformanceMode::Boost);
        let maxed = scan_limits(false, PerformanceMode::Max);

        assert!(maxed.batch_size > boosted.batch_size);
        assert!(maxed.worker_count >= boosted.worker_count);
    }

    #[test]
    fn resource_profile_orders_modes_by_worker_count() {
        let profile = system_resource_profile();
        assert!(profile.logical_cpu_count >= 1);
        assert_eq!(profile.plans.len(), 3);

        let balanced = profile
            .plans
            .iter()
            .find(|plan| plan.mode == "balanced")
            .unwrap();
        let priority = profile
            .plans
            .iter()
            .find(|plan| plan.mode == "priority")
            .unwrap();
        let max = profile
            .plans
            .iter()
            .find(|plan| plan.mode == "max")
            .unwrap();

        assert!(priority.cpu_threads >= balanced.cpu_threads);
        assert!(max.cpu_threads >= priority.cpu_threads);
        assert!(max.scan_batch_size >= priority.scan_batch_size);
        assert!(priority.process_priority.contains("only while"));
        assert!(max.process_priority.contains("only while"));
        assert!(!max.process_priority.contains("high"));
    }
}
