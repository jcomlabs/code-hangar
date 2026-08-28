use hangar_core::{
    display_name_for_path, display_path_for_path, normalize_path, FileIdentity, GitRepoSummary,
    ScanOutcome, ScannedFile,
};
use hangar_protect::{
    collapse_default_for_path, is_context_path, is_heavy_or_protected_container_path,
    is_markdown_path, is_regenerable_cleanup_forbidden_path, is_sensitive_path,
    protected_level_for_path, regenerable_container_kind, should_index_body,
};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Read};
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

const MAX_INDEX_BYTES: u64 = 1024 * 1024;
pub const SCAN_BATCH_SIZE: usize = 5_000;
pub const MAX_SCAN_ITEMS_PER_JOB: u64 = 500_000;
pub const MAX_SCAN_ITEMS_PER_DIRECTORY: u64 = 50_000;

#[derive(Debug, Clone)]
pub struct ScanLimits {
    pub batch_size: usize,
    pub max_items_per_job: Option<u64>,
    pub max_items_per_directory: Option<u64>,
    pub worker_count: usize,
    /// Whether ignored build/dependency containers should receive a recursive
    /// metadata-only size measurement. Resident scans disable this so volatile
    /// caches never become background I/O hotspots.
    pub measure_build_dir_sizes: bool,
    /// Optional duty-cycle pause for the otherwise unbounded metadata-only walk
    /// used to account for ignored build/dependency trees. Normal interactive
    /// scans leave it disabled; the resident profile yields every small chunk.
    pub build_dir_batch_pause: Option<Duration>,
}

impl ScanLimits {
    pub fn root_scan() -> Self {
        Self {
            batch_size: SCAN_BATCH_SIZE,
            max_items_per_job: Some(MAX_SCAN_ITEMS_PER_JOB),
            max_items_per_directory: Some(MAX_SCAN_ITEMS_PER_DIRECTORY),
            worker_count: 1,
            measure_build_dir_sizes: true,
            build_dir_batch_pause: None,
        }
    }

    pub fn resume_subtree() -> Self {
        Self {
            batch_size: SCAN_BATCH_SIZE,
            max_items_per_job: Some(MAX_SCAN_ITEMS_PER_JOB),
            max_items_per_directory: None,
            worker_count: 1,
            measure_build_dir_sizes: true,
            build_dir_batch_pause: None,
        }
    }

    /// Fixed safety profile for the one explicit operation that expands an
    /// allowlisted regenerable container. It remains metadata-only, single
    /// worker and bounded independently of UI performance settings.
    pub fn regenerable_expansion() -> Self {
        Self {
            batch_size: SCAN_BATCH_SIZE,
            max_items_per_job: Some(MAX_SCAN_ITEMS_PER_JOB),
            max_items_per_directory: None,
            worker_count: 1,
            measure_build_dir_sizes: false,
            build_dir_batch_pause: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InventoryTraversalPolicy {
    NormalOpaque,
    ExplicitRegenerable,
}

struct InventoryScanRequest<'a> {
    root: &'a Path,
    start_relative: Option<&'a str>,
    limits: ScanLimits,
    cached_body_fingerprints: Option<&'a HashMap<String, BodyFingerprint>>,
    traversal_policy: InventoryTraversalPolicy,
}

#[derive(Debug, Clone)]
pub struct ScanStreamSummary {
    pub scanned_files: u64,
    pub indexed_documents: u64,
    pub inaccessible_items: u64,
    pub cancelled: bool,
    pub partial: bool,
    pub partial_error: Option<String>,
    pub git: Option<GitRepoSummary>,
}

/// Last completed identity and content fingerprint used by a one-worker
/// incremental scan. Eligible Markdown/context text is reopened through the
/// no-recall gate and hashed, but equal bytes avoid FTS/relationship DB writes;
/// a changed identity or hash refreshes the text index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyFingerprint {
    pub modified_at: Option<String>,
    pub size_apparent: Option<u64>,
    pub volume_id: Option<String>,
    pub inode_key: Option<String>,
    pub content_hash: Option<String>,
    pub body_indexed: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InventoryEstimate {
    pub item_count: u64,
    pub apparent_bytes: u64,
    pub inaccessible_items: u64,
    pub cancelled: bool,
}

pub fn scan_markdown_context_root(root: &Path) -> Result<ScanOutcome, String> {
    scan_markdown_context_root_with_progress(root, || false, |_, _, _| {})
}

pub fn estimate_inventory<C, P>(
    root: &Path,
    start_relative: Option<&str>,
    is_cancelled: C,
    mut on_progress: P,
) -> Result<InventoryEstimate, String>
where
    C: Fn() -> bool,
    P: FnMut(u64, u64, &str),
{
    let root = validate_local_scan_root(root)
        .map_err(|err| format!("Cannot safely open scan root {}: {err}", root.display()))?;
    let start_path = validated_scan_start(&root, start_relative)
        .map_err(|err| format!("Cannot safely open scan subtree: {err}"))?;
    let mut estimate = InventoryEstimate::default();
    let mut walk = WalkDir::new(&start_path)
        .follow_links(false)
        .follow_root_links(false)
        .into_iter();
    while let Some(entry) = walk.next() {
        if is_cancelled() {
            estimate.cancelled = true;
            return Ok(estimate);
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                estimate.inaccessible_items += 1;
                continue;
            }
        };
        let absolute_path = entry.path().to_path_buf();
        let relative_path = relative_to(&root, &absolute_path);
        let normalized_relative = normalize_path(&relative_path);
        if normalized_relative.is_empty() {
            continue;
        }

        estimate.item_count = estimate.item_count.saturating_add(1);
        if entry.file_type().is_file() {
            if let Ok(metadata) = entry.metadata() {
                estimate.apparent_bytes = estimate.apparent_bytes.saturating_add(metadata.len());
            } else {
                estimate.inaccessible_items += 1;
            }
        } else if entry.file_type().is_dir()
            && (identity_blocks_directory_traversal(&inspect_path_identity(&absolute_path))
                || is_build_dependency_dir_name(entry.file_name().to_str().unwrap_or("")))
        {
            // Don't descend into a directory junction / mount point (reparse dir) — it can point
            // outside the scanned root or self-reference — nor into a build/dependency folder whose
            // children the real scan does not index. scan_inventory_stream skips both; mirror it so
            // the estimate matches the tree the scan will actually walk.
            walk.skip_current_dir();
        }
        if estimate.item_count % 1_000 == 0 {
            on_progress(
                estimate.item_count,
                estimate.apparent_bytes,
                &normalized_relative,
            );
        }
    }

    Ok(estimate)
}

pub fn scan_inventory_stream<C, P, B>(
    root: &Path,
    start_relative: Option<&str>,
    limits: ScanLimits,
    cached_body_fingerprints: Option<&HashMap<String, BodyFingerprint>>,
    is_cancelled: C,
    on_progress: P,
    on_batch: B,
) -> Result<ScanStreamSummary, String>
where
    C: Fn() -> bool,
    P: FnMut(u64, u64, &str),
    B: FnMut(Vec<ScannedFile>) -> Result<(), String>,
{
    scan_inventory_stream_with_policy(
        InventoryScanRequest {
            root,
            start_relative,
            limits,
            cached_body_fingerprints,
            traversal_policy: InventoryTraversalPolicy::NormalOpaque,
        },
        is_cancelled,
        on_progress,
        on_batch,
    )
}

/// Expand exactly one narrow, allowlisted regenerable container into concrete
/// metadata inventory. Normal scans remain opaque. The target must be relative
/// to the validated project root; protected/shared/global-cache components are
/// rejected before the walk. The walk never reads file bodies and is clamped to
/// 500,000 items even if a caller supplies a looser profile.
pub fn scan_regenerable_inventory_stream<C, P, B>(
    root: &Path,
    start_relative: &str,
    mut limits: ScanLimits,
    is_cancelled: C,
    on_progress: P,
    on_batch: B,
) -> Result<ScanStreamSummary, String>
where
    C: Fn() -> bool,
    P: FnMut(u64, u64, &str),
    B: FnMut(Vec<ScannedFile>) -> Result<(), String>,
{
    if regenerable_container_kind(start_relative).is_none() {
        return Err(
            "The requested subtree is not an allowlisted project-local regenerable container."
                .to_string(),
        );
    }
    limits.max_items_per_job = Some(
        limits
            .max_items_per_job
            .unwrap_or(MAX_SCAN_ITEMS_PER_JOB)
            .min(MAX_SCAN_ITEMS_PER_JOB),
    );
    limits.max_items_per_directory = None;
    limits.worker_count = 1;
    limits.measure_build_dir_sizes = false;
    limits.build_dir_batch_pause = None;
    scan_inventory_stream_with_policy(
        InventoryScanRequest {
            root,
            start_relative: Some(start_relative),
            limits,
            cached_body_fingerprints: None,
            traversal_policy: InventoryTraversalPolicy::ExplicitRegenerable,
        },
        is_cancelled,
        on_progress,
        on_batch,
    )
}

fn scan_inventory_stream_with_policy<C, P, B>(
    request: InventoryScanRequest<'_>,
    is_cancelled: C,
    mut on_progress: P,
    mut on_batch: B,
) -> Result<ScanStreamSummary, String>
where
    C: Fn() -> bool,
    P: FnMut(u64, u64, &str),
    B: FnMut(Vec<ScannedFile>) -> Result<(), String>,
{
    let InventoryScanRequest {
        root,
        start_relative,
        limits,
        cached_body_fingerprints,
        traversal_policy,
    } = request;
    let root = validate_local_scan_root(root)
        .map_err(|err| format!("Cannot safely open scan root {}: {err}", root.display()))?;
    let start_path = validated_scan_start(&root, start_relative)
        .map_err(|err| format!("Cannot safely open scan subtree: {err}"))?;

    let mut batch = Vec::with_capacity(limits.batch_size.max(1));
    let mut scanned_files = 0;
    let mut indexed_documents = 0;
    let mut inaccessible_items = 0;
    let mut child_counts: HashMap<String, u64> = HashMap::new();
    let mut capped_dirs: HashSet<String> = HashSet::new();
    let mut partial_dirs: HashSet<String> = HashSet::new();
    let worker_count = limits.worker_count.max(1);
    let pending_flush_size = pending_flush_size_for(worker_count, limits.batch_size);
    let mut file_workers =
        (worker_count > 1).then(|| FileWorkerPool::new(&root, worker_count, pending_flush_size));
    let mut partial = false;
    let mut partial_error = None;
    let mut walk = WalkDir::new(&start_path)
        .follow_links(false)
        .follow_root_links(false)
        .into_iter();

    while let Some(entry) = walk.next() {
        if is_cancelled() {
            if let Some(pool) = file_workers.take() {
                let drain = drain_file_worker_results(
                    pool.cancel(),
                    &mut batch,
                    limits.batch_size,
                    &mut on_batch,
                )?;
                indexed_documents += drain.indexed_documents;
            }
            mark_batch_partial(&mut batch, "Cancelled");
            flush_batch(&mut batch, 1, &mut on_batch)?;
            return Ok(ScanStreamSummary {
                scanned_files,
                indexed_documents,
                inaccessible_items,
                cancelled: true,
                partial: true,
                partial_error: Some("Cancelled".to_string()),
                git: read_git_metadata(&root),
            });
        }

        if let Some(max_items) = limits.max_items_per_job {
            if scanned_files >= max_items {
                if let Some(pool) = file_workers.take() {
                    let drain = drain_file_worker_results(
                        pool.finish(),
                        &mut batch,
                        limits.batch_size,
                        &mut on_batch,
                    )?;
                    indexed_documents += drain.indexed_documents;
                }
                partial = true;
                partial_error = Some("Scan item limit reached".to_string());
                mark_batch_partial(&mut batch, "Scan item limit reached");
                flush_batch(&mut batch, 1, &mut on_batch)?;
                return Ok(ScanStreamSummary {
                    scanned_files,
                    indexed_documents,
                    inaccessible_items,
                    cancelled: false,
                    partial,
                    partial_error,
                    git: read_git_metadata(&root),
                });
            }
        }

        if let Some(pool) = file_workers.as_mut() {
            if scanned_files > 0 && scanned_files % pending_flush_size.max(1) as u64 == 0 {
                let drain = drain_available_file_worker_results(
                    pool,
                    &mut batch,
                    limits.batch_size,
                    &mut on_batch,
                )?;
                indexed_documents += drain.indexed_documents;
                if let Some(current_path) = drain.current_path {
                    on_progress(scanned_files, indexed_documents, &current_path);
                }
            }
        }

        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                inaccessible_items += 1;
                if traversal_policy == InventoryTraversalPolicy::ExplicitRegenerable {
                    partial = true;
                    partial_error.get_or_insert_with(|| {
                        format!(
                            "Regenerable inventory is incomplete because an entry was inaccessible: {error}"
                        )
                    });
                }
                continue;
            }
        };

        let absolute_path = entry.path().to_path_buf();
        let relative_path = relative_to(&root, &absolute_path);
        let normalized_relative = normalize_path(&relative_path);
        if normalized_relative.is_empty() {
            continue;
        }

        if traversal_policy == InventoryTraversalPolicy::ExplicitRegenerable
            && is_regenerable_cleanup_forbidden_path(&normalized_relative)
        {
            let message =
                "Protected, shared or global-cache content was excluded from regenerable inventory."
                    .to_string();
            if entry.file_type().is_dir() {
                walk.skip_current_dir();
            }
            let mut marker = scanned_item_for_path(&root, &absolute_path, Some(message.clone()));
            marker.fully_scanned = false;
            scanned_files = scanned_files.saturating_add(1);
            let progress_path = marker.relative_path.clone();
            batch.push(marker);
            flush_batch(&mut batch, limits.batch_size, &mut on_batch)?;
            partial = true;
            partial_error.get_or_insert(message);
            if scanned_files % 250 == 0 {
                on_progress(scanned_files, indexed_documents, &progress_path);
            }
            continue;
        }

        if is_inside_capped_dir(&normalized_relative, &capped_dirs) {
            if entry.file_type().is_dir() {
                walk.skip_current_dir();
            }
            continue;
        }

        if let Some(parent) = parent_path(&normalized_relative) {
            let count = child_counts.entry(parent.clone()).or_default();
            *count += 1;
            if let Some(limit) = limits.max_items_per_directory {
                if *count > limit {
                    capped_dirs.insert(parent.clone());
                    if partial_dirs.insert(parent.clone()) {
                        let marker_path = root.join(&parent);
                        batch.push(scanned_item_for_path(
                            &root,
                            &marker_path,
                            Some("Directory item limit reached".to_string()),
                        ));
                    }
                    partial = true;
                    partial_error = Some("Directory item limit reached".to_string());
                    if entry.file_type().is_dir() {
                        walk.skip_current_dir();
                    }
                    flush_batch(&mut batch, limits.batch_size, &mut on_batch)?;
                    continue;
                }
            }
        }

        let entry_type = entry.file_type();
        if worker_count > 1 && entry_type.is_file() {
            scanned_files += 1;
            let progress_path = normalized_relative.clone();
            if let Some(pool) = file_workers.as_mut() {
                let submitted = submit_file_to_worker_pool(
                    pool,
                    absolute_path,
                    &mut batch,
                    limits.batch_size,
                    &mut on_batch,
                    &mut indexed_documents,
                    &is_cancelled,
                )?;
                if !submitted {
                    if let Some(pool) = file_workers.take() {
                        let drain = drain_file_worker_results(
                            pool.cancel(),
                            &mut batch,
                            limits.batch_size,
                            &mut on_batch,
                        )?;
                        indexed_documents += drain.indexed_documents;
                    }
                    mark_batch_partial(&mut batch, "Cancelled");
                    flush_batch(&mut batch, 1, &mut on_batch)?;
                    return Ok(ScanStreamSummary {
                        scanned_files,
                        indexed_documents,
                        inaccessible_items,
                        cancelled: true,
                        partial: true,
                        partial_error: Some("Cancelled".to_string()),
                        git: read_git_metadata(&root),
                    });
                }
                if scanned_files % pending_flush_size.max(1) as u64 == 0 {
                    let drain = drain_available_file_worker_results(
                        pool,
                        &mut batch,
                        limits.batch_size,
                        &mut on_batch,
                    )?;
                    indexed_documents += drain.indexed_documents;
                    on_progress(
                        scanned_files,
                        indexed_documents,
                        drain.current_path.as_deref().unwrap_or(&progress_path),
                    );
                }
            }
            if scanned_files % 250 == 0 {
                on_progress(scanned_files, indexed_documents, &progress_path);
            }
            continue;
        }

        let item_kind = if entry_type.is_dir() {
            "directory"
        } else {
            "file"
        };
        let (mut item, indexed) =
            if traversal_policy == InventoryTraversalPolicy::ExplicitRegenerable {
                let mut item = scanned_item_for_path(&root, &absolute_path, None);
                item.item_kind = item_kind.to_string();
                (item, false)
            } else {
                scanned_item_with_body(&root, &absolute_path, item_kind, cached_body_fingerprints)
            };
        if indexed {
            indexed_documents += 1;
        }

        let explicit_identity_block = (traversal_policy
            == InventoryTraversalPolicy::ExplicitRegenerable)
            .then(|| {
                item.identity
                    .as_ref()
                    .and_then(explicit_regenerable_identity_block_reason)
            })
            .flatten();
        if let Some(reason) = explicit_identity_block {
            item.fully_scanned = false;
            item.scan_error = Some(reason.to_string());
            partial = true;
            partial_error.get_or_insert_with(|| reason.to_string());
        }
        let is_reparse_dir = entry_type.is_dir()
            && item
                .identity
                .as_ref()
                .map(|identity| {
                    if traversal_policy == InventoryTraversalPolicy::ExplicitRegenerable {
                        explicit_regenerable_identity_block_reason(identity).is_some()
                    } else {
                        identity_blocks_directory_traversal(identity)
                    }
                })
                .unwrap_or(false);
        if is_reparse_dir {
            walk.skip_current_dir();
        }

        // Build/dependency folder (target, node_modules, .venv, .git, …): the folder node above is
        // recorded so the tree still shows it, but do NOT descend — indexing its many regenerable
        // children is the dominant source of inventory bloat and of the slow reads that follow.
        // We still measure its recursive size with a cheap metadata-only walk (no nodes / DB writes)
        // and store it on this single node, so a project's Space footprint and the Overview "Largest
        // Project Footprints" ranking account for a heavy target/ or node_modules/ instead of
        // undercounting it as ~0 bytes. A reparse build dir is skipped above and left unmeasured so
        // we never walk a junction that escapes the scanned root.
        if traversal_policy == InventoryTraversalPolicy::NormalOpaque
            && !is_reparse_dir
            && entry_type.is_dir()
            && is_build_dependency_dir_name(entry.file_name().to_str().unwrap_or(""))
        {
            if limits.measure_build_dir_sizes {
                let (recursive_apparent, size_partial) = build_dir_recursive_apparent_bytes(
                    &absolute_path,
                    &is_cancelled,
                    limits.build_dir_batch_pause,
                );
                if let Some(identity) = item.identity.as_mut() {
                    let own = identity.size_apparent.unwrap_or(0);
                    let total = own.saturating_add(recursive_apparent);
                    identity.size_apparent = Some(total);
                    identity.size_allocated = None;
                }
                if size_partial {
                    item.fully_scanned = false;
                    item.scan_error = Some(
                        "Build/dependency folder size is incomplete because the measurement was cancelled or an entry was inaccessible."
                            .to_string(),
                    );
                }
            } else if let Some(identity) = item.identity.as_mut() {
                // A deferred opaque container deliberately carries no volatile
                // fingerprint. The delta comparer treats this as a wildcard and
                // preserves the last interactive measurement already in SQLite.
                identity.modified_at = None;
                identity.size_apparent = None;
                identity.size_allocated = None;
            }
            walk.skip_current_dir();
        }

        scanned_files += 1;
        let progress_path = item.relative_path.clone();
        batch.push(item);
        flush_batch(&mut batch, limits.batch_size, &mut on_batch)?;

        if scanned_files % 250 == 0 {
            on_progress(scanned_files, indexed_documents, &progress_path);
        }
    }

    if let Some(pool) = file_workers.take() {
        let drain =
            drain_file_worker_results(pool.finish(), &mut batch, limits.batch_size, &mut on_batch)?;
        indexed_documents += drain.indexed_documents;
    }
    flush_batch(&mut batch, 1, &mut on_batch)?;
    Ok(ScanStreamSummary {
        scanned_files,
        indexed_documents,
        inaccessible_items,
        cancelled: false,
        partial,
        partial_error,
        git: read_git_metadata(&root),
    })
}

fn explicit_regenerable_identity_block_reason(identity: &FileIdentity) -> Option<&'static str> {
    if identity.inaccessible {
        return Some("Regenerable inventory is incomplete because an entry was inaccessible.");
    }
    if identity.is_symlink || identity.is_reparse {
        return Some(
            "Regenerable inventory excluded a link, reparse point or cloud-managed entry.",
        );
    }
    None
}

/// Build/dependency folder names whose CONTENTS the inventory scanner records as a single node
/// instead of descending into. These are regenerable, not user content — `classify_orphan_candidate`
/// already excludes the exact same set from results — and indexing their 10⁴–10⁵ children each
/// (a Rust `target/`, a `node_modules/`, a `.venv/`) is the dominant source of inventory bloat and
/// of the slow project loads / Organize scans that follow.
fn is_build_dependency_dir_name(name: &str) -> bool {
    // Keep the streamer's traversal boundary in lock-step with the shared
    // protection policy. In particular, framework output such as `.next` and
    // `.nuxt` must be represented by one metadata node, not inventoried as tens
    // of thousands of volatile cache files.
    is_heavy_or_protected_container_path(name)
}

/// Recursively sum the apparent byte size of everything under a build/dependency directory whose
/// children the inventory scan deliberately does NOT index (see `is_build_dependency_dir_name`).
///
/// This is a *size-only* pass: it stats entries via walkdir's directory metadata (cached during
/// iteration on Windows, so no extra syscall per file) and never creates a node, reads a body, or
/// touches the database — so it is far cheaper than the descent #141 removed, yet lets the single
/// recorded build-dir node carry a real footprint instead of the ~0-byte directory-entry size.
/// Nested junctions / mount points (reparse-point dirs) are not descended: they can point outside
/// the build dir or self-reference, which would over-count or loop. Inaccessible entries are
/// skipped (they contribute 0), so the result is a lower bound — far better than the ~0 it replaces.
const BUILD_DIR_METADATA_BATCH: u64 = 128;

fn build_dir_recursive_apparent_bytes<C>(
    root: &Path,
    is_cancelled: &C,
    batch_pause: Option<Duration>,
) -> (u64, bool)
where
    C: Fn() -> bool,
{
    let mut total: u64 = 0;
    let mut partial = false;
    let mut visited = 0_u64;
    let mut walk = WalkDir::new(root)
        .follow_links(false)
        .follow_root_links(false)
        .into_iter();
    while let Some(entry) = walk.next() {
        if is_cancelled() {
            return (total, true);
        }
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                partial = true;
                continue;
            }
        };
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(_) => {
                partial = true;
                continue;
            }
        };
        visited = visited.saturating_add(1);
        if visited.is_multiple_of(BUILD_DIR_METADATA_BATCH) {
            if let Some(pause) = batch_pause {
                thread::sleep(pause);
            }
        }
        if metadata.is_dir() {
            // depth 0 is `root` itself (the build dir we were asked to measure) — always enter it;
            // only skip *nested* reparse dirs so the sum stays inside this build dir.
            if entry.depth() > 0 && metadata_is_reparse(&metadata) {
                walk.skip_current_dir();
            }
        } else if metadata.is_file() {
            total = total.saturating_add(metadata.len());
        }
    }
    (total, partial)
}

#[cfg(windows)]
fn metadata_is_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

pub fn scan_markdown_context_root_with_progress<C, P>(
    root: &Path,
    is_cancelled: C,
    mut on_progress: P,
) -> Result<ScanOutcome, String>
where
    C: Fn() -> bool,
    P: FnMut(u64, u64, &str),
{
    let root = validate_local_scan_root(root)
        .map_err(|err| format!("Cannot safely open scan root {}: {err}", root.display()))?;

    let mut files = Vec::new();
    let mut scanned_files = 0;
    let mut indexed_documents = 0;
    let mut inaccessible_items = 0;

    let mut child_counts: HashMap<String, i64> = HashMap::new();
    let mut walk = WalkDir::new(&root)
        .follow_links(false)
        .follow_root_links(false)
        .min_depth(1)
        .into_iter();

    while let Some(entry) = walk.next() {
        if is_cancelled() {
            return Ok(finish_outcome(
                files,
                scanned_files,
                indexed_documents,
                inaccessible_items,
                true,
                &child_counts,
                read_git_metadata(&root),
            ));
        }

        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                inaccessible_items += 1;
                continue;
            }
        };

        let absolute_path = entry.path().to_path_buf();
        let mut identity = inspect_path_identity(&absolute_path);
        if entry.file_type().is_dir() && identity_blocks_directory_traversal(&identity) {
            walk.skip_current_dir();
        }

        let relative_path = relative_to(&root, &absolute_path);
        let normalized_relative = normalize_path(&relative_path);
        if normalized_relative.is_empty() {
            continue;
        }

        if let Some(parent) = parent_path(&normalized_relative) {
            *child_counts.entry(parent).or_default() += 1;
        }

        let is_markdown = is_markdown_path(&normalized_relative);
        let is_context = is_context_path(&normalized_relative);
        let is_sensitive = is_sensitive_path(&normalized_relative);
        let protected_level = protected_level_for_path(&normalized_relative);
        let item_kind = if entry.file_type().is_dir() {
            "directory"
        } else {
            "file"
        };

        scanned_files += 1;
        let body = if entry.file_type().is_file()
            && should_index_body(&normalized_relative)
            && identity_allows_local_content(&identity)
            && identity
                .size_apparent
                .map(|size| size <= MAX_INDEX_BYTES)
                .unwrap_or(false)
        {
            match read_local_text_no_recall(&absolute_path, MAX_INDEX_BYTES) {
                Ok(value) => {
                    indexed_documents += 1;
                    Some(value)
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    // The provider changed state between metadata inspection and
                    // the no-recall open. Refresh the stored identity so the tree
                    // says Cloud-only instead of leaving a stale local marker.
                    identity = inspect_path_identity(&absolute_path);
                    None
                }
                Err(_) => None,
            }
        } else {
            None
        };

        files.push(ScannedFile {
            absolute_path: absolute_path.to_string_lossy().to_string(),
            relative_path: normalized_relative.clone(),
            display_path: display_path_for_path(&normalized_relative),
            display_name: display_name_for_path(&normalized_relative),
            item_kind: item_kind.to_string(),
            is_markdown,
            is_context,
            is_sensitive,
            protected_level,
            child_count: 0,
            fully_scanned: true,
            collapse_default: collapse_default_for_path(&normalized_relative),
            scan_error: None,
            identity: Some(identity),
            body,
        });

        if scanned_files % 250 == 0 {
            on_progress(scanned_files, indexed_documents, &normalized_relative);
        }
    }

    Ok(finish_outcome(
        files,
        scanned_files,
        indexed_documents,
        inaccessible_items,
        false,
        &child_counts,
        read_git_metadata(&root),
    ))
}

pub fn inspect_path_identity(path: &Path) -> FileIdentity {
    let _ = enable_cloud_placeholder_visibility();
    match fs::symlink_metadata(path) {
        Ok(metadata) => identity_from_metadata(path, &metadata),
        Err(err) => FileIdentity {
            size_apparent: None,
            size_allocated: None,
            modified_at: None,
            readonly: false,
            is_symlink: false,
            is_reparse: false,
            reparse_kind: None,
            volume_id: None,
            inode_key: None,
            link_count: None,
            inaccessible: true,
            error: Some(err.to_string()),
        },
    }
}

/// Ask Windows to expose Cloud Files placeholder attributes to this process.
///
/// Windows otherwise puts many legacy/unmanifested executables in a compatibility
/// mode that disguises OneDrive placeholders as ordinary files. In that mode no
/// scanner can reliably distinguish local bytes from online-only bytes before an
/// open. The setting is process-local, idempotent and does not alter any file,
/// OneDrive pin or user preference.
#[cfg(windows)]
pub fn enable_cloud_placeholder_visibility() -> Result<(), String> {
    static RESULT: OnceLock<Result<(), String>> = OnceLock::new();
    RESULT
        .get_or_init(|| {
            const PHCM_EXPOSE_PLACEHOLDERS: i8 = 2;
            unsafe extern "system" {
                fn RtlQueryProcessPlaceholderCompatibilityMode() -> i8;
                fn RtlSetProcessPlaceholderCompatibilityMode(mode: i8) -> i8;
            }

            let previous = unsafe {
                RtlSetProcessPlaceholderCompatibilityMode(PHCM_EXPOSE_PLACEHOLDERS)
            };
            let current = unsafe { RtlQueryProcessPlaceholderCompatibilityMode() };
            if previous < 0 || current != PHCM_EXPOSE_PLACEHOLDERS {
                Err(format!(
                    "Windows did not enable exposed Cloud Files placeholders (previous={previous}, current={current})."
                ))
            } else {
                Ok(())
            }
        })
        .clone()
}

#[cfg(not(windows))]
pub fn enable_cloud_placeholder_visibility() -> Result<(), String> {
    Ok(())
}

/// Bytes available to the caller on the volume that contains `path`.
/// Returns `None` when the query is unsupported (non-Windows) or fails.
/// `path` should be an existing directory (e.g. the backup destination root).
pub fn available_space_bytes(path: &Path) -> Option<u64> {
    available_space_platform(path)
}

#[cfg(windows)]
fn available_space_platform(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut free_to_caller: u64 = 0;
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free_to_caller,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(free_to_caller)
}

#[cfg(not(windows))]
fn available_space_platform(_path: &Path) -> Option<u64> {
    None
}

fn identity_from_metadata(path: &Path, metadata: &fs::Metadata) -> FileIdentity {
    let is_symlink = metadata.file_type().is_symlink();
    let modified_at = metadata
        .modified()
        .ok()
        .and_then(system_time_to_unix_string);
    let mut identity = FileIdentity {
        size_apparent: Some(metadata.len()),
        size_allocated: None,
        modified_at,
        readonly: metadata.permissions().readonly(),
        is_symlink,
        is_reparse: is_symlink,
        reparse_kind: is_symlink.then(|| "symlink".to_string()),
        volume_id: None,
        inode_key: None,
        link_count: None,
        inaccessible: false,
        error: None,
    };

    fill_platform_identity(path, metadata, &mut identity);
    identity
}

#[cfg(windows)]
fn fill_platform_identity(path: &Path, metadata: &fs::Metadata, identity: &mut FileIdentity) {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        FileAttributeTagInfo, FileStandardInfo, GetFileInformationByHandle,
        GetFileInformationByHandleEx, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_NO_RECALL,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FILE_STANDARD_INFO, OPEN_EXISTING,
    };
    unsafe extern "system" {
        fn CreateFileW(
            lpfilename: *const u16,
            dwdesiredaccess: u32,
            dwsharemode: u32,
            lpsecurityattributes: *const std::ffi::c_void,
            dwcreationdisposition: u32,
            dwflagsandattributes: u32,
            htemplatefile: HANDLE,
        ) -> HANDLE;
    }

    let attributes = metadata.file_attributes();
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        identity.is_reparse = true;
        if identity.reparse_kind.is_none() {
            identity.reparse_kind = Some("reparse_point".to_string());
        }
    }

    if cloud_placeholder_attributes(attributes) {
        identity.reparse_kind = Some("cloud_placeholder".to_string());
        // A dehydrated (online-only) cloud placeholder occupies ~0 bytes on disk, so record that
        // explicitly — otherwise size_allocated stays None (we return before the handle open
        // below) and the physical footprint falls back to the FULL logical size, massively
        // overstating on-disk usage for OneDrive/cloud-backed folders.
        identity.size_allocated = Some(0);
        return;
    }

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_NO_RECALL | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return;
    }

    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    let ok = unsafe { GetFileInformationByHandle(handle, &mut info) };
    if ok == 0 {
        unsafe {
            CloseHandle(handle);
        }
        return;
    }

    let file_index = ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64;
    identity.volume_id = Some(info.dwVolumeSerialNumber.to_string());
    identity.inode_key = Some(file_index.to_string());
    identity.link_count = Some(info.nNumberOfLinks as u64);

    // FILE_ATTRIBUTE_TAG_INFO is the authoritative, fixed-size handle query for
    // both attributes and the reparse tag. In particular, it preserves
    // materialized Cloud Files that can be under-reported by path metadata,
    // without issuing a failing FSCTL_GET_REPARSE_POINT call and allocating a
    // 16 KiB reparse buffer for every ordinary inventory entry.
    let mut attribute_tag_info = FILE_ATTRIBUTE_TAG_INFO::default();
    let attribute_tag_ok = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileAttributeTagInfo,
            &mut attribute_tag_info as *mut FILE_ATTRIBUTE_TAG_INFO as *mut _,
            std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    };
    let handle_attributes = if attribute_tag_ok != 0 {
        attribute_tag_info.FileAttributes
    } else {
        info.dwFileAttributes
    };
    let reparse_tag = if attribute_tag_ok != 0 {
        (attribute_tag_info.ReparseTag != 0).then_some(attribute_tag_info.ReparseTag)
    } else {
        // FileAttributeTagInfo should be the cheap authoritative path. On a
        // filesystem/Windows build that does not support it, preserve the older
        // Cloud Files proof instead of silently trusting under-reported path
        // attributes. The 16 KiB buffer and IOCTL are now a rare fallback only.
        windows_reparse_tag_from_handle(handle)
    };

    if handle_attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 || reparse_tag.is_some() {
        identity.is_reparse = true;
        if identity.reparse_kind.is_none() {
            identity.reparse_kind = Some("reparse_point".to_string());
        }
    }

    if cloud_placeholder_attributes(handle_attributes) {
        identity.is_reparse = true;
        identity.reparse_kind = Some("cloud_placeholder".to_string());
        identity.size_allocated = Some(0);
        unsafe {
            CloseHandle(handle);
        }
        return;
    }

    if let Some(reparse_tag) = reparse_tag {
        identity.is_reparse = true;
        if cloud_reparse_tag(reparse_tag) {
            // OneDrive keeps even fully materialized files and directories as Cloud Files
            // reparse points. They are safe to read only through `open_local_file_no_recall`,
            // which asks Windows never to recall missing bytes. Keep the physical reparse bit
            // for mutation safety, but distinguish this state from an online-only placeholder
            // and from a symlink/junction.
            identity.reparse_kind = Some("cloud_local".to_string());
        } else if identity.reparse_kind.is_none() {
            identity.reparse_kind = Some("reparse_point".to_string());
        }
    }

    let mut standard = FILE_STANDARD_INFO::default();
    let standard_ok = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileStandardInfo,
            &mut standard as *mut FILE_STANDARD_INFO as *mut _,
            std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
        )
    };
    if standard_ok != 0 && standard.AllocationSize >= 0 {
        identity.size_allocated = Some(standard.AllocationSize as u64);
    }
    unsafe {
        CloseHandle(handle);
    }
}

#[cfg(windows)]
fn cloud_reparse_tag(tag: u32) -> bool {
    use windows_sys::Win32::System::SystemServices::{
        IO_REPARSE_TAG_CLOUD, IO_REPARSE_TAG_CLOUD_MASK,
    };

    tag & !IO_REPARSE_TAG_CLOUD_MASK == IO_REPARSE_TAG_CLOUD
}

#[cfg(windows)]
fn windows_reparse_tag_from_handle(handle: windows_sys::Win32::Foundation::HANDLE) -> Option<u32> {
    use windows_sys::Win32::System::Ioctl::FSCTL_GET_REPARSE_POINT;
    use windows_sys::Win32::System::IO::DeviceIoControl;

    // MAXIMUM_REPARSE_DATA_BUFFER_SIZE from winnt.h. The tag is the first u32.
    // This path runs only when the fixed-size FileAttributeTagInfo query failed.
    let mut buffer = vec![0_u8; 16 * 1024];
    let mut returned = 0_u32;
    let ok = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_GET_REPARSE_POINT,
            std::ptr::null(),
            0,
            buffer.as_mut_ptr().cast(),
            buffer.len() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 || returned < 4 {
        return None;
    }
    Some(u32::from_le_bytes(buffer[..4].try_into().ok()?))
}

#[cfg(not(windows))]
fn fill_platform_identity(_path: &Path, _metadata: &fs::Metadata, _identity: &mut FileIdentity) {}

#[cfg(windows)]
fn windows_path_attributes(path: &Path) -> Option<u32> {
    windows_path_attribute_tag(path)
        .map(|(attributes, _)| attributes)
        .or_else(|| windows_path_attributes_fallback(path))
}

#[cfg(windows)]
fn windows_path_attribute_tag(path: &Path) -> Option<(u32, u32)> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{FindClose, FindFirstFileW, WIN32_FIND_DATAW};

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut data = WIN32_FIND_DATAW::default();
    let handle = unsafe { FindFirstFileW(wide.as_ptr(), &mut data) };
    if handle == INVALID_HANDLE_VALUE {
        return None;
    }
    unsafe {
        FindClose(handle);
    }
    Some((data.dwFileAttributes, data.dwReserved0))
}

#[cfg(windows)]
fn windows_path_attributes_fallback(path: &Path) -> Option<u32> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetFileAttributesW;

    const INVALID_FILE_ATTRIBUTES: u32 = u32::MAX;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let attributes = unsafe { GetFileAttributesW(wide.as_ptr()) };
    (attributes != INVALID_FILE_ATTRIBUTES).then_some(attributes)
}

fn system_time_to_unix_string(value: SystemTime) -> Option<String> {
    value
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs().to_string())
}

pub fn read_git_metadata(root: &Path) -> Option<GitRepoSummary> {
    let git_dir = root.join(".git");
    if !local_metadata_directory_exists(&git_dir) {
        return None;
    }

    let mut summary = GitRepoSummary {
        project_id: 0,
        has_git: true,
        current_branch: None,
        head_ref: None,
        origin_url: None,
        metadata_error: None,
    };

    match read_local_text_no_recall(&git_dir.join("HEAD"), MAX_INDEX_BYTES) {
        Ok(head) => {
            let trimmed = head.trim();
            summary.head_ref = Some(trimmed.to_string());
            if let Some(branch) = trimmed.strip_prefix("ref: refs/heads/") {
                summary.current_branch = Some(branch.to_string());
            }
        }
        Err(err) => summary.metadata_error = Some(format!("HEAD read failed: {err}")),
    }

    match read_local_text_no_recall(&git_dir.join("config"), MAX_INDEX_BYTES) {
        Ok(config) => {
            summary.origin_url = parse_origin_url(&config);
        }
        Err(err) if summary.metadata_error.is_none() => {
            summary.metadata_error = Some(format!("config read failed: {err}"));
        }
        Err(_) => {}
    }

    Some(summary)
}

fn parse_origin_url(config: &str) -> Option<String> {
    let mut in_origin = false;
    for line in config.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_origin = trimmed == "[remote \"origin\"]";
            continue;
        }
        if in_origin {
            if let Some((key, value)) = trimmed.split_once('=') {
                if key.trim() == "url" {
                    return Some(value.trim().to_string());
                }
            }
        }
    }
    None
}

fn relative_to(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

fn scanned_item_for_path(
    root: &Path,
    absolute_path: &Path,
    scan_error: Option<String>,
) -> ScannedFile {
    let identity = inspect_path_identity(absolute_path);
    let relative_path = relative_to(root, absolute_path);
    let normalized_relative = normalize_path(&relative_path);
    let is_markdown = is_markdown_path(&normalized_relative);
    let is_context = is_context_path(&normalized_relative);
    let is_sensitive = is_sensitive_path(&normalized_relative);
    let protected_level = protected_level_for_path(&normalized_relative);
    let item_kind = if absolute_path.is_dir() {
        "directory"
    } else {
        "file"
    };
    ScannedFile {
        absolute_path: absolute_path.to_string_lossy().to_string(),
        relative_path: normalized_relative.clone(),
        display_path: display_path_for_path(&normalized_relative),
        display_name: display_name_for_path(&normalized_relative),
        item_kind: item_kind.to_string(),
        is_markdown,
        is_context,
        is_sensitive,
        protected_level,
        child_count: 0,
        fully_scanned: scan_error.is_none(),
        collapse_default: collapse_default_for_path(&normalized_relative),
        scan_error,
        identity: Some(identity),
        body: None,
    }
}

fn scanned_item_with_body(
    root: &Path,
    absolute_path: &Path,
    item_kind: &str,
    cached_body_fingerprints: Option<&HashMap<String, BodyFingerprint>>,
) -> (ScannedFile, bool) {
    let mut item = scanned_item_for_path(root, absolute_path, None);
    item.item_kind = item_kind.to_string();
    let body_eligible = item_kind == "file"
        && should_index_body(&item.relative_path)
        && item
            .identity
            .as_ref()
            .map(identity_allows_local_content)
            .unwrap_or(false)
        && item
            .identity
            .as_ref()
            .and_then(|identity| identity.size_apparent)
            .map(|size| size <= MAX_INDEX_BYTES)
            .unwrap_or(false);
    let cached_body_fingerprint = cached_body_fingerprints
        .and_then(|fingerprints| fingerprints.get(&scan_path_key(&item.relative_path)));
    let metadata_fingerprint_matches = cached_body_fingerprint.is_some_and(|cached| {
        cached.body_indexed
            && cached.modified_at.as_deref()
                == item
                    .identity
                    .as_ref()
                    .and_then(|identity| identity.modified_at.as_deref())
            && cached.size_apparent
                == item
                    .identity
                    .as_ref()
                    .and_then(|identity| identity.size_apparent)
            && cached.volume_id.as_deref()
                == item
                    .identity
                    .as_ref()
                    .and_then(|identity| identity.volume_id.as_deref())
            && cached.inode_key.as_deref()
                == item
                    .identity
                    .as_ref()
                    .and_then(|identity| identity.inode_key.as_deref())
    });
    if body_eligible {
        match read_local_text_no_recall(absolute_path, MAX_INDEX_BYTES) {
            Ok(value) => {
                // Size + mtime are not a trustworthy content identity: a file can be
                // atomically replaced while preserving both. Even when metadata and
                // FileId match, verify the current bytes against the hash stored with
                // the last completed text index. Equal bodies generate no DB writes;
                // missing/changed hashes deliberately refresh the document and its
                // relationship evidence.
                let current_hash = blake3::hash(value.as_bytes()).to_hex().to_string();
                let content_fingerprint_matches = metadata_fingerprint_matches
                    && cached_body_fingerprint
                        .and_then(|cached| cached.content_hash.as_deref())
                        .is_some_and(|cached_hash| cached_hash == current_hash);
                if !content_fingerprint_matches {
                    item.body = Some(value);
                    return (item, true);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                item.identity = Some(inspect_path_identity(absolute_path));
            }
            Err(error) => {
                // Never preserve a prior text index merely because a verification
                // read failed. Mark this entry incomplete so downstream accounting
                // and mutation planning remain fail-closed until a clean rescan.
                item.fully_scanned = false;
                item.scan_error = Some(format!("Could not verify local text: {error}"));
            }
        }
    }
    (item, false)
}

fn scan_path_key(path: &str) -> String {
    let normalized = normalize_path(path);
    if cfg!(windows) {
        normalized.to_ascii_lowercase()
    } else {
        normalized
    }
}

fn flush_batch<B>(
    batch: &mut Vec<ScannedFile>,
    batch_size: usize,
    on_batch: &mut B,
) -> Result<(), String>
where
    B: FnMut(Vec<ScannedFile>) -> Result<(), String>,
{
    if !batch.is_empty() && batch.len() >= batch_size.max(1) {
        on_batch(mem::take(batch))?;
    }
    Ok(())
}

fn pending_flush_size_for(worker_count: usize, batch_size: usize) -> usize {
    let batch_size = batch_size.max(1);
    if worker_count <= 1 {
        return batch_size.min(128);
    }

    let upper_bound = batch_size.clamp(1, 256);
    worker_count.saturating_mul(16).max(96).min(upper_bound)
}

struct FileWorkerDrain {
    indexed_documents: u64,
    current_path: Option<String>,
}

struct FileWorkerPool {
    sender: Option<mpsc::SyncSender<PathBuf>>,
    receiver: mpsc::Receiver<(ScannedFile, bool)>,
    handles: Vec<thread::JoinHandle<()>>,
    cancelled: Arc<AtomicBool>,
}

impl FileWorkerPool {
    fn new(root: &Path, worker_count: usize, queue_capacity: usize) -> Self {
        let (sender, job_receiver) = mpsc::sync_channel::<PathBuf>(queue_capacity.max(1));
        let (result_sender, receiver) = mpsc::channel::<(ScannedFile, bool)>();
        let job_receiver = Arc::new(Mutex::new(job_receiver));
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut handles = Vec::with_capacity(worker_count);

        for _ in 0..worker_count {
            let root = root.to_path_buf();
            let job_receiver = Arc::clone(&job_receiver);
            let result_sender = result_sender.clone();
            let cancelled = Arc::clone(&cancelled);
            handles.push(thread::spawn(move || loop {
                let path = match job_receiver.lock() {
                    Ok(receiver) => receiver.recv(),
                    Err(_) => return,
                };
                match path {
                    Ok(path) => {
                        if cancelled.load(Ordering::Relaxed) {
                            return;
                        }
                        let result = scanned_item_with_body(&root, &path, "file", None);
                        if result_sender.send(result).is_err() {
                            return;
                        }
                    }
                    Err(_) => return,
                }
            }));
        }
        drop(result_sender);

        Self {
            sender: Some(sender),
            receiver,
            handles,
            cancelled,
        }
    }

    fn try_submit(&self, path: PathBuf) -> Result<(), mpsc::TrySendError<PathBuf>> {
        let Some(sender) = &self.sender else {
            return Err(mpsc::TrySendError::Disconnected(path));
        };
        sender.try_send(path)
    }

    fn drain_available(&self) -> Vec<(ScannedFile, bool)> {
        let mut results = Vec::new();
        while let Ok(result) = self.receiver.try_recv() {
            results.push(result);
        }
        results
    }

    fn finish(mut self) -> Vec<(ScannedFile, bool)> {
        self.sender.take();
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
        self.drain_remaining()
    }

    fn cancel(mut self) -> Vec<(ScannedFile, bool)> {
        self.cancelled.store(true, Ordering::Relaxed);
        self.sender.take();
        // Do not join here: cloud-backed files can block worker IO, and Stop must
        // return control to the UI while those OS calls unwind in detached workers.
        self.handles.clear();
        self.drain_remaining()
    }

    fn drain_remaining(&self) -> Vec<(ScannedFile, bool)> {
        let mut results = Vec::new();
        while let Ok(result) = self.receiver.try_recv() {
            results.push(result);
        }
        results
    }
}

fn submit_file_to_worker_pool<C, B>(
    pool: &FileWorkerPool,
    mut path: PathBuf,
    batch: &mut Vec<ScannedFile>,
    batch_size: usize,
    on_batch: &mut B,
    indexed_documents: &mut u64,
    is_cancelled: &C,
) -> Result<bool, String>
where
    C: Fn() -> bool,
    B: FnMut(Vec<ScannedFile>) -> Result<(), String>,
{
    loop {
        if is_cancelled() {
            return Ok(false);
        }
        match pool.try_submit(path) {
            Ok(()) => return Ok(true),
            Err(mpsc::TrySendError::Full(returned_path)) => {
                path = returned_path;
                let drain = drain_available_file_worker_results(pool, batch, batch_size, on_batch)?;
                *indexed_documents = (*indexed_documents).saturating_add(drain.indexed_documents);
                if drain.current_path.is_none() {
                    thread::sleep(Duration::from_millis(2));
                }
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                return Err("File scan worker pool stopped.".to_string());
            }
        }
    }
}

fn drain_available_file_worker_results<B>(
    pool: &FileWorkerPool,
    batch: &mut Vec<ScannedFile>,
    batch_size: usize,
    on_batch: &mut B,
) -> Result<FileWorkerDrain, String>
where
    B: FnMut(Vec<ScannedFile>) -> Result<(), String>,
{
    drain_file_worker_results(pool.drain_available(), batch, batch_size, on_batch)
}

fn drain_file_worker_results<B>(
    results: Vec<(ScannedFile, bool)>,
    batch: &mut Vec<ScannedFile>,
    batch_size: usize,
    on_batch: &mut B,
) -> Result<FileWorkerDrain, String>
where
    B: FnMut(Vec<ScannedFile>) -> Result<(), String>,
{
    let mut drain = FileWorkerDrain {
        indexed_documents: 0,
        current_path: None,
    };
    for (item, indexed) in results {
        if indexed {
            drain.indexed_documents = drain.indexed_documents.saturating_add(1);
        }
        drain.current_path = Some(item.relative_path.clone());
        batch.push(item);
        flush_batch(batch, batch_size, on_batch)?;
    }
    Ok(drain)
}

fn mark_batch_partial(batch: &mut [ScannedFile], message: &str) {
    for file in batch {
        file.fully_scanned = false;
        file.scan_error = Some(message.to_string());
    }
}

fn is_inside_capped_dir(path: &str, capped_dirs: &HashSet<String>) -> bool {
    capped_dirs
        .iter()
        .any(|dir| path != dir && path.starts_with(&format!("{dir}/")))
}

fn finish_outcome(
    mut files: Vec<ScannedFile>,
    scanned_files: u64,
    indexed_documents: u64,
    inaccessible_items: u64,
    cancelled: bool,
    child_counts: &HashMap<String, i64>,
    git: Option<GitRepoSummary>,
) -> ScanOutcome {
    for file in &mut files {
        file.child_count = child_counts
            .get(&file.relative_path)
            .copied()
            .unwrap_or_default();
        if cancelled {
            file.fully_scanned = false;
            file.scan_error = Some("Cancelled".to_string());
        }
    }

    ScanOutcome {
        scanned_files,
        indexed_documents,
        inaccessible_items,
        cancelled,
        files,
        git,
    }
}

fn parent_path(path: &str) -> Option<String> {
    normalize_path(path)
        .rsplit_once('/')
        .map(|(parent, _)| parent.to_string())
        .filter(|parent| !parent.is_empty())
}

#[cfg(windows)]
pub fn is_cloud_placeholder(path: &Path) -> bool {
    let _ = enable_cloud_placeholder_visibility();
    windows_path_attributes(path)
        .map(cloud_placeholder_attributes)
        .unwrap_or(false)
}

#[cfg(windows)]
fn cloud_placeholder_attributes(attributes: u32) -> bool {
    const FILE_ATTRIBUTE_OFFLINE: u32 = 0x0000_1000;
    const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0004_0000;
    const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;

    attributes
        & (FILE_ATTRIBUTE_OFFLINE
            | FILE_ATTRIBUTE_RECALL_ON_OPEN
            | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS)
        != 0
}

/// Whether metadata inspection proved that opening the file body is local-only.
///
/// Ordinary files and already-materialized Cloud Files are readable. Symlinks,
/// junctions, unknown reparse points and online-only placeholders are not. The
/// caller must still open through [`open_local_file_no_recall`] so a state change
/// between this check and the actual open cannot silently recall cloud bytes.
pub fn identity_allows_local_content(identity: &FileIdentity) -> bool {
    !identity.inaccessible
        && !identity.is_symlink
        && identity.reparse_kind.as_deref() != Some("cloud_placeholder")
        && (!identity.is_reparse || identity.reparse_kind.as_deref() == Some("cloud_local"))
}

const MAX_LOCAL_CONTENT_PATH_ANCESTORS: usize = 256;

/// Purely lexical UNC detection used before any filesystem metadata call.
/// `display_path_for_path` normalizes verbatim Windows spellings, so
/// `\\?\UNC\server\share` is rejected just like `\\server\share` without
/// contacting either host.
pub fn path_uses_unc_syntax(path: &Path) -> bool {
    display_path_for_path(&path.to_string_lossy())
        .replace('/', "\\")
        .starts_with(r"\\")
}

#[cfg(windows)]
fn windows_drive_letter(path: &Path) -> Option<u8> {
    let normalized = display_path_for_path(&path.to_string_lossy()).replace('/', "\\");
    let bytes = normalized.as_bytes();
    (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\')
        .then(|| bytes[0].to_ascii_uppercase())
}

/// Whether a path can receive a local metadata probe without implicitly
/// contacting a filesystem server. UNC paths are rejected lexically. On
/// Windows, drive-letter paths are classified with `GetDriveTypeW` and mapped
/// network drives (`DRIVE_REMOTE`) are rejected before `stat`, `read_dir` or a
/// content open. `GetDriveTypeW` queries only the OS drive mapping for the root;
/// it does not open the requested path.
///
/// Explicit WSL support is deliberately not encoded here: callers with a user
/// opt-in must recognize that UNC namespace before invoking this local-only
/// helper, while ordinary discovery and shell ingress stay fail-closed.
pub fn local_metadata_probe_allowed(path: &Path) -> bool {
    if !path.is_absolute() || path_uses_unc_syntax(path) {
        return false;
    }

    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
        use windows_sys::Win32::System::WindowsProgramming::DRIVE_REMOTE;

        let Some(drive) = windows_drive_letter(path) else {
            return false;
        };
        let root = format!("{}:\\", drive as char)
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        // SAFETY: `root` is a valid NUL-terminated UTF-16 drive-root string and
        // GetDriveTypeW reads no caller-provided output buffer.
        (unsafe { GetDriveTypeW(root.as_ptr()) }) != DRIVE_REMOTE
    }

    #[cfg(not(windows))]
    {
        true
    }
}

fn local_content_identity_error(identity: &FileIdentity, subject: &str) -> io::Error {
    let kind = if identity.reparse_kind.as_deref() == Some("cloud_placeholder") {
        io::ErrorKind::WouldBlock
    } else {
        io::ErrorKind::PermissionDenied
    };
    io::Error::new(
        kind,
        format!("{subject} is not a verified local, non-link path"),
    )
}

/// Validate every parent component without following the component being
/// inspected. A regular final file is not sufficient proof: an intermediate
/// directory can itself be a junction or symlink that redirects the eventual
/// content open outside the path the policy classified.
pub fn validate_local_content_ancestors(path: &Path) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "local-only content reads require an absolute path",
        ));
    }
    if path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "local-only content paths cannot contain parent-directory components",
        ));
    }

    let mut ancestors = path
        .ancestors()
        .skip(1)
        .filter(|ancestor| !ancestor.as_os_str().is_empty())
        .collect::<Vec<_>>();
    if ancestors.len() > MAX_LOCAL_CONTENT_PATH_ANCESTORS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "local-only content path exceeds the bounded ancestor depth",
        ));
    }

    // Root-to-leaf makes the proof easier to reason about and ensures an outer
    // redirect is rejected before inspecting anything beneath it.
    ancestors.reverse();
    for ancestor in ancestors {
        let identity = inspect_path_identity(ancestor);
        if !identity_allows_local_content(&identity) {
            return Err(local_content_identity_error(
                &identity,
                "a parent directory",
            ));
        }
    }
    Ok(())
}

/// Metadata-only existence check that never intentionally follows a symlink,
/// junction or unsafe/cloud-only ancestor. The final entry itself may be a
/// materialized or online-only Cloud Files placeholder because reporting its
/// name/state is safe; ordinary links and unknown reparse points are rejected.
pub fn local_metadata_entry_exists(path: &Path) -> bool {
    if validate_local_content_ancestors(path).is_err() {
        return false;
    }
    let Ok(_metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    let identity = inspect_path_identity(path);
    !identity.inaccessible
        && !identity.is_symlink
        && (!identity.is_reparse
            || matches!(
                identity.reparse_kind.as_deref(),
                Some("cloud_local" | "cloud_placeholder")
            ))
}

/// Directory variant of [`local_metadata_entry_exists`]. It uses no-follow
/// metadata for the final entry, so `.git` or marker directories implemented as
/// links never become a path to another local/remote tree.
pub fn local_metadata_directory_exists(path: &Path) -> bool {
    local_metadata_entry_exists(path)
        && fs::symlink_metadata(path)
            .map(|metadata| metadata.is_dir())
            .unwrap_or(false)
}

/// Prove that a file and all of its ancestors are local-only, accessible and
/// free of ordinary symlinks, junctions or unknown reparse points.
///
/// The ancestor walk is deliberately repeated around final-entry inspection.
/// [`open_local_file_no_recall`] additionally compares the no-follow identity
/// returned here with the identity of the opened handle, closing the remaining
/// normal-file-to-link race before any bytes are read.
pub fn validate_local_content_path(path: &Path) -> io::Result<FileIdentity> {
    validate_local_content_ancestors(path)?;
    let identity = inspect_path_identity(path);
    if !identity_allows_local_content(&identity) {
        return Err(local_content_identity_error(&identity, "the file"));
    }
    validate_local_content_ancestors(path)?;
    Ok(identity)
}

/// Validate and resolve a directory before it becomes an inventory root.
/// Neither registration nor a later worker may canonicalize through a link,
/// junction or online-only placeholder. Re-running this at worker start also
/// catches a registered directory that was replaced after registration.
pub fn validate_local_scan_root(path: &Path) -> io::Result<PathBuf> {
    validate_local_content_ancestors(path)?;
    let identity = inspect_path_identity(path);
    if !identity_allows_local_content(&identity) {
        return Err(local_content_identity_error(&identity, "the scan root"));
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the scan root is not a directory",
        ));
    }
    validate_local_content_ancestors(path)?;
    // Do not canonicalize here. Canonicalization follows the mutable path and
    // could itself touch a just-swapped junction/UNC/cloud target before a
    // later identity comparison rejects it. Callers keep this absolute lexical
    // path and re-run the proof at every path-based operation.
    Ok(path.to_path_buf())
}

fn validated_scan_start(root: &Path, start_relative: Option<&str>) -> io::Result<PathBuf> {
    let Some(relative) = start_relative.filter(|relative| !relative.is_empty()) else {
        return Ok(root.to_path_buf());
    };
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative.components().any(|component| {
            !matches!(
                component,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the requested scan subtree must be a relative child path",
        ));
    }
    let candidate = root.join(relative);
    let start = validate_local_scan_root(&candidate)?;
    if !start.starts_with(root) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the requested scan subtree escapes its validated root",
        ));
    }
    Ok(start)
}

fn identity_blocks_directory_traversal(identity: &FileIdentity) -> bool {
    identity.is_reparse
        && !matches!(
            identity.reparse_kind.as_deref(),
            Some("cloud_local" | "cloud_placeholder")
        )
}

/// Open a file for local-only content reads.
///
/// On Windows, `FILE_FLAG_OPEN_NO_RECALL` is the final circuit breaker: if the
/// bytes are no longer local, the open/read fails instead of asking OneDrive (or
/// another Cloud Files provider) to download them. This function never follows
/// an ordinary symlink, junction or unknown reparse point.
#[cfg(windows)]
pub fn open_local_file_no_recall(path: &Path) -> io::Result<fs::File> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_OPEN_NO_RECALL,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    const GENERIC_READ: u32 = 0x8000_0000;
    let identity = validate_local_content_path(path)?;
    let expected_volume = identity
        .volume_id
        .as_deref()
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "local file identity could not be verified before opening",
            )
        })?;
    let expected_index = identity
        .inode_key
        .as_deref()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "local file identity could not be verified before opening",
            )
        })?;

    unsafe extern "system" {
        fn CreateFileW(
            lpfilename: *const u16,
            dwdesiredaccess: u32,
            dwsharemode: u32,
            lpsecurityattributes: *const std::ffi::c_void,
            dwcreationdisposition: u32,
            dwflagsandattributes: u32,
            htemplatefile: HANDLE,
        ) -> HANDLE;
    }

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_NO_RECALL,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }

    // Re-check both cloud state and physical identity on the opened handle. If
    // an ancestor or the final entry changed to a redirect during the race
    // window, following it produces a different volume/file index and is
    // rejected before a single byte is read.
    let mut reopened_info = BY_HANDLE_FILE_INFORMATION::default();
    let reopened_ok = unsafe { GetFileInformationByHandle(handle, &mut reopened_info) };
    if reopened_ok == 0 {
        let error = io::Error::last_os_error();
        unsafe {
            CloseHandle(handle);
        }
        return Err(error);
    }
    if cloud_placeholder_attributes(reopened_info.dwFileAttributes) {
        unsafe {
            CloseHandle(handle);
        }
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "cloud content is online-only and was not recalled",
        ));
    }
    let reopened_index =
        ((reopened_info.nFileIndexHigh as u64) << 32) | reopened_info.nFileIndexLow as u64;
    if reopened_info.dwVolumeSerialNumber != expected_volume || reopened_index != expected_index {
        unsafe {
            CloseHandle(handle);
        }
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "local file identity changed while opening; possible link or junction race",
        ));
    }

    // SAFETY: `handle` is a unique owned file handle returned by CreateFileW;
    // after conversion the Rust `File` is solely responsible for closing it.
    Ok(unsafe { fs::File::from_raw_handle(handle as _) })
}

#[cfg(not(windows))]
pub fn open_local_file_no_recall(path: &Path) -> io::Result<fs::File> {
    let _identity = validate_local_content_path(path)?;
    fs::File::open(path)
}

pub fn read_local_text_no_recall(path: &Path, max_bytes: u64) -> io::Result<String> {
    let mut file = open_local_file_no_recall(path)?;
    if file.metadata()?.len() > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file exceeds the local read limit",
        ));
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "file exceeds the local read limit",
        ));
    }
    String::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(not(windows))]
pub fn is_cloud_placeholder(_path: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn local_metadata_probe_rejects_unc_spellings_lexically() {
        for path in [
            Path::new(r"\\server\share\project"),
            Path::new("//server/share/project"),
            Path::new(r"\\?\UNC\server\share\project"),
        ] {
            assert!(path_uses_unc_syntax(path), "{}", path.display());
            assert!(!local_metadata_probe_allowed(path), "{}", path.display());
        }
        assert!(!path_uses_unc_syntax(Path::new(
            r"C:\server\share-lookalike"
        )));
    }

    #[test]
    fn local_metadata_probe_accepts_the_real_local_temp_root() {
        let dir = tempdir().unwrap();
        assert!(local_metadata_probe_allowed(dir.path()));
    }

    #[cfg(windows)]
    #[test]
    fn windows_drive_letter_classification_is_lexical() {
        assert_eq!(windows_drive_letter(Path::new(r"C:\project")), Some(b'C'));
        assert_eq!(windows_drive_letter(Path::new("c:/project")), Some(b'C'));
        assert_eq!(
            windows_drive_letter(Path::new(r"\\?\C:\project")),
            Some(b'C')
        );
        assert_eq!(
            windows_drive_letter(Path::new(r"\\server\share\project")),
            None
        );
        assert_eq!(windows_drive_letter(Path::new(r"C:relative")), None);
    }

    #[cfg(windows)]
    #[test]
    fn any_currently_mapped_windows_drive_is_rejected_without_path_io() {
        use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
        use windows_sys::Win32::System::WindowsProgramming::DRIVE_REMOTE;

        for drive in b'A'..=b'Z' {
            let root = format!("{}:\\", drive as char)
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>();
            // SAFETY: `root` is a NUL-terminated UTF-16 drive-root string.
            if unsafe { GetDriveTypeW(root.as_ptr()) } != DRIVE_REMOTE {
                continue;
            }
            let unprobed_child = PathBuf::from(format!(
                "{}:\\CodeHangar-mapped-drive-classification-only",
                drive as char
            ));
            assert!(!local_metadata_probe_allowed(&unprobed_child));
        }
    }

    #[test]
    fn scanner_reads_markdown_without_modifying_files() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("README.md");
        fs::write(&file, "# Hello").unwrap();
        let before = fs::metadata(&file).unwrap().modified().unwrap();

        let outcome = scan_markdown_context_root(dir.path()).unwrap();

        let after = fs::metadata(&file).unwrap().modified().unwrap();
        assert_eq!(outcome.scanned_files, 1);
        assert_eq!(outcome.indexed_documents, 1);
        assert_eq!(outcome.inaccessible_items, 0);
        assert_eq!(before, after);
    }

    #[test]
    fn incremental_stream_skips_unchanged_markdown_body_and_reads_changed_body() {
        let dir = tempdir().unwrap();
        let readme = dir.path().join("README.md");
        fs::write(&readme, "# Incremental").unwrap();
        let identity = inspect_path_identity(&readme);
        let mut fingerprints = HashMap::from([(
            scan_path_key("README.md"),
            BodyFingerprint {
                modified_at: identity.modified_at.clone(),
                size_apparent: identity.size_apparent,
                volume_id: identity.volume_id.clone(),
                inode_key: identity.inode_key.clone(),
                content_hash: Some(blake3::hash(b"# Incremental").to_hex().to_string()),
                body_indexed: true,
            },
        )]);

        let mut unchanged = Vec::new();
        let unchanged_summary = scan_inventory_stream(
            dir.path(),
            None,
            ScanLimits::root_scan(),
            Some(&fingerprints),
            || false,
            |_, _, _| {},
            |batch| {
                unchanged.extend(batch);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(unchanged_summary.indexed_documents, 0);
        assert!(unchanged.iter().all(|file| file.body.is_none()));

        fingerprints
            .get_mut(&scan_path_key("README.md"))
            .unwrap()
            .content_hash = Some(
            blake3::hash(b"same metadata, different bytes")
                .to_hex()
                .to_string(),
        );
        let mut same_metadata_changed_body = Vec::new();
        let changed_hash_summary = scan_inventory_stream(
            dir.path(),
            None,
            ScanLimits::root_scan(),
            Some(&fingerprints),
            || false,
            |_, _, _| {},
            |batch| {
                same_metadata_changed_body.extend(batch);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(changed_hash_summary.indexed_documents, 1);
        assert!(same_metadata_changed_body
            .iter()
            .any(|file| file.body.as_deref() == Some("# Incremental")));

        fingerprints
            .get_mut(&scan_path_key("README.md"))
            .unwrap()
            .size_apparent = identity.size_apparent.map(|size| size + 1);
        let mut changed = Vec::new();
        let changed_summary = scan_inventory_stream(
            dir.path(),
            None,
            ScanLimits::root_scan(),
            Some(&fingerprints),
            || false,
            |_, _, _| {},
            |batch| {
                changed.extend(batch);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(changed_summary.indexed_documents, 1);
        assert!(changed.iter().any(|file| file.body.is_some()));
    }

    #[test]
    fn identity_reports_apparent_size() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("data.bin");
        fs::write(&file, [1_u8; 17]).unwrap();

        let identity = inspect_path_identity(&file);

        assert_eq!(identity.size_apparent, Some(17));
        assert!(!identity.inaccessible);
    }

    #[cfg(windows)]
    #[test]
    fn identity_reports_allocated_size_from_file_standard_info() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("data.bin");
        fs::write(&file, [1_u8; 17]).unwrap();

        let identity = inspect_path_identity(&file);

        assert!(identity.size_allocated.is_some());
    }

    #[cfg(windows)]
    fn create_test_directory_link(target: &Path, link: &Path) -> io::Result<()> {
        use std::process::{Command, Stdio};

        if std::os::windows::fs::symlink_dir(target, link).is_ok() {
            return Ok(());
        }
        // Some Windows configurations create the reparse entry but still
        // report a privilege-related error. Ensure the junction fallback is
        // never asked to overwrite that partial test-only entry.
        if fs::symlink_metadata(link).is_ok() {
            fs::remove_dir(link)?;
        }
        let status = Command::new("cmd.exe")
            .arg("/d")
            .arg("/c")
            .arg("mklink")
            .arg("/J")
            .arg(link)
            .arg(target)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!("mklink /J failed with {status}")))
        }
    }

    #[cfg(windows)]
    #[test]
    fn local_content_open_rejects_a_regular_file_below_a_linked_ancestor() {
        let dir = tempdir().unwrap();
        let protected = dir.path().join(".ssh");
        let safe = dir.path().join("safe");
        let linked = safe.join("docs");
        fs::create_dir_all(&protected).unwrap();
        fs::create_dir_all(&safe).unwrap();
        let secret = protected.join("innocent.md");
        fs::write(&secret, "must never cross the ancestor junction").unwrap();
        create_test_directory_link(&protected, &linked).unwrap();

        let aliased_file = linked.join("innocent.md");
        // The final entry alone looks regular; the content gate must therefore
        // prove every parent rather than trusting only this identity.
        assert!(identity_allows_local_content(&inspect_path_identity(
            &aliased_file
        )));
        let error = open_local_file_no_recall(&aliased_file).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);

        // Remove only the link and prove its target was not altered or removed.
        fs::remove_dir(&linked).unwrap();
        assert_eq!(
            fs::read_to_string(secret).unwrap(),
            "must never cross the ancestor junction"
        );
    }

    #[cfg(windows)]
    #[test]
    fn scan_root_validation_rejects_a_junction_before_canonicalizing_it() {
        let dir = tempdir().unwrap();
        let target = dir.path().join(".ssh");
        let link = dir.path().join("project-link");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("secret.md"), "# must not be scanned").unwrap();
        create_test_directory_link(&target, &link).unwrap();

        let error = validate_local_scan_root(&link).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        let scan_error = scan_markdown_context_root(&link).unwrap_err();
        assert!(scan_error.contains("Cannot safely open scan root"));

        fs::remove_dir(&link).unwrap();
        assert!(target.join("secret.md").is_file());
    }

    #[cfg(windows)]
    #[test]
    fn subtree_validation_rejects_a_linked_subtree_root() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("project");
        let outside = dir.path().join("outside");
        let linked = root.join("docs");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.md"), "# outside subtree").unwrap();
        create_test_directory_link(&outside, &linked).unwrap();
        let canonical_root = validate_local_scan_root(&root).unwrap();

        let error = validated_scan_start(&canonical_root, Some("docs")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        let scan_error = scan_inventory_stream(
            &root,
            Some("docs"),
            ScanLimits::resume_subtree(),
            None,
            || false,
            |_, _, _| {},
            |_| Ok(()),
        )
        .unwrap_err();
        assert!(scan_error.contains("Cannot safely open scan subtree"));

        fs::remove_dir(&linked).unwrap();
        assert!(outside.join("secret.md").is_file());
    }

    #[test]
    fn local_paths_and_scan_subtrees_reject_parent_directory_components() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("project");
        fs::create_dir_all(root.join("docs")).unwrap();

        let content_error =
            validate_local_content_ancestors(&root.join("docs").join("..").join("note.md"))
                .unwrap_err();
        assert_eq!(content_error.kind(), io::ErrorKind::InvalidInput);

        let subtree_error = validated_scan_start(&root, Some("../outside")).unwrap_err();
        assert_eq!(subtree_error.kind(), io::ErrorKind::InvalidInput);
    }

    #[cfg(windows)]
    #[test]
    fn cloud_placeholder_attributes_are_detected_without_opening_file() {
        const FILE_ATTRIBUTE_OFFLINE: u32 = 0x0000_1000;
        const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0004_0000;
        const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;

        assert!(cloud_placeholder_attributes(FILE_ATTRIBUTE_OFFLINE));
        assert!(cloud_placeholder_attributes(FILE_ATTRIBUTE_RECALL_ON_OPEN));
        assert!(cloud_placeholder_attributes(
            FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
        ));
        assert!(!cloud_placeholder_attributes(0));
    }

    #[cfg(windows)]
    #[test]
    fn cloud_reparse_tags_distinguish_materialized_cloud_files_from_links() {
        assert!(cloud_reparse_tag(0x9000_001a));
        assert!(cloud_reparse_tag(0x9000_601a));
        assert!(cloud_reparse_tag(0x9000_f01a));
        assert!(!cloud_reparse_tag(0xa000_000c)); // symbolic link
        assert!(!cloud_reparse_tag(0xa000_0003)); // mount point / junction
    }

    #[test]
    fn local_content_policy_allows_materialized_cloud_files_only() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("README.md");
        fs::write(&file, "# local").unwrap();
        let local = inspect_path_identity(&file);
        assert!(identity_allows_local_content(&local));

        let mut materialized_cloud = local.clone();
        materialized_cloud.is_reparse = true;
        materialized_cloud.reparse_kind = Some("cloud_local".to_string());
        assert!(identity_allows_local_content(&materialized_cloud));

        let mut online_only = materialized_cloud.clone();
        online_only.reparse_kind = Some("cloud_placeholder".to_string());
        assert!(!identity_allows_local_content(&online_only));

        let mut symlink = local;
        symlink.is_symlink = true;
        symlink.is_reparse = true;
        symlink.reparse_kind = Some("symlink".to_string());
        assert!(!identity_allows_local_content(&symlink));
    }

    #[cfg(windows)]
    #[test]
    fn offline_attribute_is_a_hard_no_recall_content_gate() {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::SetFileAttributesW;

        const FILE_ATTRIBUTE_OFFLINE: u32 = 0x0000_1000;
        let dir = tempdir().unwrap();
        let path = dir.path().join("online-only.md");
        fs::write(&path, "bytes remain readable to the test harness").unwrap();
        let original = windows_path_attributes(&path).unwrap();
        let wide = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        assert_ne!(
            unsafe { SetFileAttributesW(wide.as_ptr(), original | FILE_ATTRIBUTE_OFFLINE) },
            0
        );

        let identity = inspect_path_identity(&path);
        assert_eq!(identity.reparse_kind.as_deref(), Some("cloud_placeholder"));
        assert!(!identity_allows_local_content(&identity));
        let error = open_local_file_no_recall(&path).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);

        assert_ne!(unsafe { SetFileAttributesW(wide.as_ptr(), original) }, 0);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "set CODEHANGAR_TEST_CLOUD_LOCAL_PATH to a materialized OneDrive file"]
    fn configured_materialized_cloud_file_opens_without_recall() {
        let path = std::env::var_os("CODEHANGAR_TEST_CLOUD_LOCAL_PATH")
            .map(PathBuf::from)
            .expect("CODEHANGAR_TEST_CLOUD_LOCAL_PATH is required");
        let identity = inspect_path_identity(&path);
        assert_eq!(identity.reparse_kind.as_deref(), Some("cloud_local"));
        assert!(identity_allows_local_content(&identity));
        let file = open_local_file_no_recall(&path).expect("local cloud bytes should open");
        assert_eq!(
            file.metadata().unwrap().len(),
            identity.size_apparent.unwrap()
        );
    }

    #[test]
    fn scanner_blocks_sensitive_body() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join(".env"), "SECRET=value").unwrap();
        let outcome = scan_markdown_context_root(dir.path()).unwrap();
        assert_eq!(outcome.scanned_files, 1);
        assert!(outcome.files[0].body.is_none());
        assert!(outcome.files[0].is_sensitive);
    }

    #[test]
    fn scanner_inventories_all_metadata_not_only_context() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("node_modules/pkg")).unwrap();
        fs::create_dir_all(dir.path().join(".git")).unwrap();
        fs::create_dir_all(dir.path().join("dist")).unwrap();
        fs::write(dir.path().join("notes.txt"), "plain text").unwrap();
        fs::write(dir.path().join("deep-notes.md"), "# not priority context").unwrap();
        fs::write(dir.path().join("image.png"), [0, 1, 2, 3]).unwrap();
        fs::write(dir.path().join("node_modules/pkg/index.js"), "module").unwrap();
        fs::write(dir.path().join("node_modules/pkg/README.md"), "# Package").unwrap();
        fs::write(dir.path().join(".git/config"), "[remote]\n").unwrap();
        fs::write(dir.path().join("dist/bundle.js"), "bundle").unwrap();

        let outcome = scan_markdown_context_root(dir.path()).unwrap();
        let paths = outcome
            .files
            .iter()
            .map(|file| file.relative_path.as_str())
            .collect::<Vec<_>>();

        assert!(paths.contains(&"notes.txt"));
        assert!(paths.contains(&"deep-notes.md"));
        assert!(paths.contains(&"image.png"));
        assert!(paths.contains(&"node_modules"));
        assert!(paths.contains(&"node_modules/pkg/index.js"));
        assert!(paths.contains(&"node_modules/pkg/README.md"));
        assert!(paths.contains(&".git/config"));
        assert!(outcome
            .files
            .iter()
            .any(|file| file.relative_path == "node_modules" && file.collapse_default));
        let vendored_readme = outcome
            .files
            .iter()
            .find(|file| file.relative_path == "node_modules/pkg/README.md")
            .unwrap();
        assert!(!vendored_readme.is_context);
        assert!(vendored_readme.body.is_none());
        // A top-level Markdown file IS project context (root .md are deliberate docs — the real
        // corpus shows they are genuine), so it is inventoried AND its body is read.
        let root_markdown = outcome
            .files
            .iter()
            .find(|file| file.relative_path == "deep-notes.md")
            .unwrap();
        assert!(root_markdown.is_markdown);
        assert!(root_markdown.is_context);
        assert!(root_markdown.body.is_some());
        assert!(outcome
            .files
            .iter()
            .filter(|file| file.body.is_some())
            .all(|file| file.is_context));
    }

    #[test]
    fn cancelled_scan_marks_partial_inventory() {
        use std::cell::Cell;

        let dir = tempdir().unwrap();
        fs::write(dir.path().join("README.md"), "# Hello").unwrap();
        fs::write(dir.path().join("notes.txt"), "plain text").unwrap();
        let checks = Cell::new(0);

        let outcome = scan_markdown_context_root_with_progress(
            dir.path(),
            || {
                let next = checks.get() + 1;
                checks.set(next);
                next > 1
            },
            |_, _, _| {},
        )
        .unwrap();

        assert!(outcome.cancelled);
        assert!(outcome.scanned_files > 0);
        assert!(outcome
            .files
            .iter()
            .all(|file| !file.fully_scanned && file.scan_error.as_deref() == Some("Cancelled")));
    }

    #[test]
    fn streaming_scan_flushes_multiple_batches() {
        let dir = tempdir().unwrap();
        for index in 0..5 {
            fs::write(dir.path().join(format!("file-{index}.txt")), "plain").unwrap();
        }
        let mut batch_sizes = Vec::new();
        let summary = scan_inventory_stream(
            dir.path(),
            None,
            ScanLimits {
                batch_size: 2,
                max_items_per_job: None,
                max_items_per_directory: None,
                worker_count: 1,
                measure_build_dir_sizes: true,
                build_dir_batch_pause: None,
            },
            None,
            || false,
            |_, _, _| {},
            |batch| {
                batch_sizes.push(batch.len());
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(summary.scanned_files, 5);
        assert!(batch_sizes.len() > 1);
        assert!(batch_sizes.iter().all(|size| *size <= 2));
    }

    #[test]
    fn build_dependency_dirs_are_recorded_but_not_descended() {
        let dir = tempdir().unwrap();
        // Normal project content — must be fully indexed.
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(dir.path().join("README.md"), "# Root").unwrap();
        // Build / dependency folders — the folder itself is recorded, its children must NOT be.
        fs::create_dir_all(dir.path().join("node_modules/pkg")).unwrap();
        fs::write(dir.path().join("node_modules/pkg/index.js"), "x").unwrap();
        fs::create_dir_all(dir.path().join("target/debug")).unwrap();
        fs::write(dir.path().join("target/debug/app"), "bin").unwrap();
        fs::create_dir_all(dir.path().join(".venv/lib")).unwrap();
        fs::write(dir.path().join(".venv/lib/site.py"), "py").unwrap();
        fs::create_dir_all(dir.path().join(".next/diagnostics")).unwrap();
        fs::write(dir.path().join(".next/diagnostics/build.log"), "volatile").unwrap();

        let mut files: Vec<ScannedFile> = Vec::new();
        scan_inventory_stream(
            dir.path(),
            None,
            ScanLimits {
                batch_size: 64,
                max_items_per_job: None,
                max_items_per_directory: None,
                worker_count: 1,
                measure_build_dir_sizes: true,
                build_dir_batch_pause: None,
            },
            None,
            || false,
            |_, _, _| {},
            |batch| {
                files.extend(batch);
                Ok(())
            },
        )
        .unwrap();

        let paths: Vec<String> = files
            .iter()
            .map(|file| file.relative_path.clone())
            .collect();

        // Normal content is fully indexed.
        assert!(paths.iter().any(|p| p == "src/main.rs"), "{paths:?}");
        assert!(paths.iter().any(|p| p == "README.md"), "{paths:?}");
        // The build/dependency folders themselves are still recorded (the tree shows them)…
        assert!(paths.iter().any(|p| p == "node_modules"), "{paths:?}");
        assert!(paths.iter().any(|p| p == "target"), "{paths:?}");
        assert!(paths.iter().any(|p| p == ".venv"), "{paths:?}");
        assert!(paths.iter().any(|p| p == ".next"), "{paths:?}");
        // …but NONE of their children are indexed (this is the bloat fix).
        assert!(
            !paths.iter().any(|p| {
                p.starts_with("node_modules/")
                    || p.starts_with("target/")
                    || p.starts_with(".venv/")
                    || p.starts_with(".next/")
            }),
            "build-dependency children must not be indexed: {paths:?}"
        );

        // …yet each recorded build/dependency node still carries the recursive byte size of the
        // children we skipped, so the project's Space footprint is not undercounted (#141 side
        // effect). The folder's own directory-entry size is ~0, so a non-zero size here can only
        // come from summing the skipped contents.
        let build_dir_size = |name: &str| -> u64 {
            files
                .iter()
                .find(|file| file.relative_path == name)
                .and_then(|file| file.identity.as_ref())
                .and_then(|identity| identity.size_apparent)
                .unwrap_or(0)
        };
        assert!(build_dir_size("node_modules") > 0, "{paths:?}");
        assert!(build_dir_size("target") > 0, "{paths:?}");
        assert!(build_dir_size(".venv") > 0, "{paths:?}");
        assert!(build_dir_size(".next") > 0, "{paths:?}");
        assert!(files
            .iter()
            .filter(|file| ["node_modules", "target", ".venv", ".next"]
                .contains(&file.relative_path.as_str()))
            .all(|file| file
                .identity
                .as_ref()
                .is_some_and(|identity| identity.size_allocated.is_none())));
    }

    #[test]
    fn normal_scan_is_opaque_but_explicit_regenerable_scan_expands_nested_content() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("node_modules/pkg/node_modules/child")).unwrap();
        fs::write(
            dir.path()
                .join("node_modules/pkg/node_modules/child/index.js"),
            "derived",
        )
        .unwrap();

        let mut normal = Vec::new();
        scan_inventory_stream(
            dir.path(),
            None,
            ScanLimits::root_scan(),
            None,
            || false,
            |_, _, _| {},
            |batch| {
                normal.extend(batch);
                Ok(())
            },
        )
        .unwrap();
        assert!(normal
            .iter()
            .any(|file| file.relative_path == "node_modules"));
        assert!(!normal
            .iter()
            .any(|file| file.relative_path.starts_with("node_modules/")));

        let mut expanded = Vec::new();
        let summary = scan_regenerable_inventory_stream(
            dir.path(),
            "node_modules",
            ScanLimits::regenerable_expansion(),
            || false,
            |_, _, _| {},
            |batch| {
                expanded.extend(batch);
                Ok(())
            },
        )
        .unwrap();
        assert!(!summary.partial);
        assert!(!summary.cancelled);
        assert!(expanded
            .iter()
            .any(|file| { file.relative_path == "node_modules/pkg/node_modules/child/index.js" }));
        assert!(expanded.iter().all(|file| file.body.is_none()));
    }

    #[test]
    fn explicit_regenerable_scan_excludes_forbidden_subtrees_and_reports_partial() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("node_modules/pkg/vendor/private")).unwrap();
        fs::write(
            dir.path().join("node_modules/pkg/vendor/private/source.rs"),
            "not derived evidence",
        )
        .unwrap();
        fs::write(dir.path().join("node_modules/pkg/index.js"), "derived").unwrap();

        let mut expanded = Vec::new();
        let summary = scan_regenerable_inventory_stream(
            dir.path(),
            "node_modules",
            ScanLimits::regenerable_expansion(),
            || false,
            |_, _, _| {},
            |batch| {
                expanded.extend(batch);
                Ok(())
            },
        )
        .unwrap();

        assert!(summary.partial);
        assert!(summary.partial_error.is_some());
        assert!(expanded
            .iter()
            .any(|file| file.relative_path == "node_modules/pkg/vendor" && !file.fully_scanned));
        assert!(!expanded
            .iter()
            .any(|file| file.relative_path.ends_with("private/source.rs")));
    }

    #[test]
    fn explicit_regenerable_scan_rejects_forged_or_ambiguous_targets() {
        let dir = tempdir().unwrap();
        for target in [".git", "vendor", "shared/node_modules", "build", "dist"] {
            let error = scan_regenerable_inventory_stream(
                dir.path(),
                target,
                ScanLimits::regenerable_expansion(),
                || false,
                |_, _, _| {},
                |_| Ok(()),
            )
            .unwrap_err();
            assert!(error.contains("not an allowlisted"), "{target}: {error}");
        }
    }

    #[test]
    fn explicit_regenerable_scan_honors_a_stricter_bound_and_marks_partial() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("target/debug")).unwrap();
        for index in 0..4 {
            fs::write(
                dir.path().join(format!("target/debug/{index}.bin")),
                [index as u8],
            )
            .unwrap();
        }
        let mut limits = ScanLimits::regenerable_expansion();
        limits.max_items_per_job = Some(2);
        let summary = scan_regenerable_inventory_stream(
            dir.path(),
            "target",
            limits,
            || false,
            |_, _, _| {},
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(summary.scanned_files, 2);
        assert!(summary.partial);
        assert_eq!(
            summary.partial_error.as_deref(),
            Some("Scan item limit reached")
        );
    }

    #[test]
    fn explicit_regenerable_scan_cancellation_keeps_partial_truthful() {
        use std::cell::Cell;

        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("node_modules/pkg")).unwrap();
        for index in 0..16 {
            fs::write(
                dir.path().join(format!("node_modules/pkg/{index:02}.bin")),
                [index as u8],
            )
            .unwrap();
        }
        let checks = Cell::new(0_u64);
        let mut persisted = Vec::new();
        let summary = scan_regenerable_inventory_stream(
            dir.path(),
            "node_modules",
            ScanLimits::regenerable_expansion(),
            || {
                let next = checks.get().saturating_add(1);
                checks.set(next);
                next > 4
            },
            |_, _, _| {},
            |batch| {
                persisted.extend(batch);
                Ok(())
            },
        )
        .unwrap();
        assert!(summary.cancelled);
        assert!(summary.partial);
        assert!(summary.scanned_files > 0);
        assert!(persisted
            .iter()
            .all(|item| !item.fully_scanned && item.scan_error.as_deref() == Some("Cancelled")));
    }

    #[test]
    fn explicit_regenerable_identity_policy_blocks_cloud_reparse_and_inaccessible() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("entry.bin");
        fs::write(&file, [1_u8]).unwrap();
        let local = inspect_path_identity(&file);
        assert!(explicit_regenerable_identity_block_reason(&local).is_none());

        let mut inaccessible = local.clone();
        inaccessible.inaccessible = true;
        assert!(explicit_regenerable_identity_block_reason(&inaccessible)
            .unwrap()
            .contains("inaccessible"));

        let mut cloud = local;
        cloud.is_reparse = true;
        cloud.reparse_kind = Some("cloud_placeholder".to_string());
        assert!(explicit_regenerable_identity_block_reason(&cloud)
            .unwrap()
            .contains("cloud-managed"));
    }

    #[cfg(windows)]
    #[test]
    fn explicit_regenerable_scan_does_not_follow_reparse_child() {
        let dir = tempdir().unwrap();
        let outside = dir.path().join(".ssh");
        let project = dir.path().join("project");
        let target = project.join("node_modules");
        fs::create_dir_all(&outside).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(outside.join("secret.bin"), [9_u8]).unwrap();
        let staged_link = project.join("staged-link");
        create_test_directory_link(&outside, &staged_link).unwrap();
        fs::rename(&staged_link, target.join("linked")).unwrap();

        let mut expanded = Vec::new();
        let summary = scan_regenerable_inventory_stream(
            &project,
            "node_modules",
            ScanLimits::regenerable_expansion(),
            || false,
            |_, _, _| {},
            |batch| {
                expanded.extend(batch);
                Ok(())
            },
        )
        .unwrap();
        assert!(summary.partial);
        assert!(expanded
            .iter()
            .any(|file| file.relative_path == "node_modules/linked" && !file.fully_scanned));
        assert!(!expanded
            .iter()
            .any(|file| file.relative_path.ends_with("secret.bin")));

        fs::remove_dir(target.join("linked")).unwrap();
        assert!(outside.join("secret.bin").exists());
    }

    #[test]
    fn cancelled_build_dir_measurement_is_marked_partial() {
        use std::cell::Cell;

        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("target/debug")).unwrap();
        fs::write(dir.path().join("target/debug/a.bin"), [1_u8; 8]).unwrap();
        fs::write(dir.path().join("target/debug/b.bin"), [2_u8; 8]).unwrap();

        let checks = Cell::new(0_u32);
        let (_bytes, partial) = build_dir_recursive_apparent_bytes(
            dir.path(),
            &|| {
                let next = checks.get() + 1;
                checks.set(next);
                next >= 3
            },
            None,
        );

        assert!(
            partial,
            "a cancelled size walk must be reported as a lower bound"
        );
    }

    #[test]
    fn build_dir_measurement_honors_resident_duty_cycle() {
        let dir = tempdir().unwrap();
        for index in 0..130 {
            fs::write(dir.path().join(format!("entry-{index}.bin")), [1_u8]).unwrap();
        }

        let pause = Duration::from_millis(20);
        let started = std::time::Instant::now();
        let (_bytes, partial) =
            build_dir_recursive_apparent_bytes(dir.path(), &|| false, Some(pause));

        assert!(!partial);
        assert!(
            started.elapsed() >= pause,
            "the background metadata walk must yield after each bounded chunk"
        );
    }

    #[test]
    fn streaming_scan_uses_parallel_file_workers_without_dropping_items() {
        let dir = tempdir().unwrap();
        for index in 0..32 {
            fs::write(dir.path().join(format!("file-{index}.txt")), "plain").unwrap();
        }
        fs::write(dir.path().join("README.md"), "# Root").unwrap();

        let mut persisted = Vec::new();
        let summary = scan_inventory_stream(
            dir.path(),
            None,
            ScanLimits {
                batch_size: 7,
                max_items_per_job: None,
                max_items_per_directory: None,
                worker_count: 4,
                measure_build_dir_sizes: true,
                build_dir_batch_pause: None,
            },
            None,
            || false,
            |_, _, _| {},
            |batch| {
                persisted.extend(batch);
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(summary.scanned_files, 33);
        assert_eq!(persisted.len(), 33);
        assert!(persisted
            .iter()
            .any(|file| file.relative_path == "README.md"));
        assert_eq!(summary.indexed_documents, 1);
    }

    #[test]
    fn parallel_streaming_cancel_returns_partial_inventory() {
        use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

        let dir = tempdir().unwrap();
        for index in 0..128 {
            fs::write(dir.path().join(format!("file-{index}.txt")), "plain").unwrap();
        }
        let cancel_checks = AtomicU64::new(0);
        let mut persisted = Vec::new();
        let summary = scan_inventory_stream(
            dir.path(),
            None,
            ScanLimits {
                batch_size: 64,
                max_items_per_job: None,
                max_items_per_directory: None,
                worker_count: 4,
                measure_build_dir_sizes: true,
                build_dir_batch_pause: None,
            },
            None,
            || cancel_checks.fetch_add(1, AtomicOrdering::SeqCst) > 4,
            |_, _, _| {},
            |batch| {
                persisted.extend(batch);
                Ok(())
            },
        )
        .unwrap();

        assert!(summary.cancelled);
        assert!(summary.partial);
        assert!(summary.scanned_files > 0);
        assert!(persisted
            .iter()
            .all(|file| !file.fully_scanned && file.scan_error.as_deref() == Some("Cancelled")));
    }

    #[test]
    fn pending_flush_size_is_capped_for_high_core_scans() {
        assert_eq!(pending_flush_size_for(1, 5_000), 128);
        assert_eq!(pending_flush_size_for(8, 5_000), 128);
        assert_eq!(pending_flush_size_for(128, 12_000), 256);
        assert_eq!(pending_flush_size_for(8, 64), 64);
    }

    #[test]
    fn estimate_inventory_counts_items_and_apparent_size_before_scan() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("docs")).unwrap();
        fs::write(dir.path().join("README.md"), "hello").unwrap();
        fs::write(dir.path().join("docs").join("notes.md"), "larger text").unwrap();
        let mut progress_seen = false;

        let estimate = estimate_inventory(
            dir.path(),
            None,
            || false,
            |_, _, _| {
                progress_seen = true;
            },
        )
        .unwrap();

        assert_eq!(estimate.item_count, 3);
        assert_eq!(estimate.apparent_bytes, 5 + 11);
        assert!(!estimate.cancelled);
        assert_eq!(estimate.inaccessible_items, 0);
        assert!(!progress_seen);
    }

    #[test]
    fn streaming_cancel_flushes_partial_batch() {
        use std::cell::Cell;

        let dir = tempdir().unwrap();
        for index in 0..5 {
            fs::write(dir.path().join(format!("file-{index}.txt")), "plain").unwrap();
        }
        let cancel_checks = Cell::new(0_u64);
        let mut persisted = Vec::new();
        let summary = scan_inventory_stream(
            dir.path(),
            None,
            ScanLimits {
                batch_size: 10,
                max_items_per_job: None,
                max_items_per_directory: None,
                worker_count: 1,
                measure_build_dir_sizes: true,
                build_dir_batch_pause: None,
            },
            None,
            || {
                let next = cancel_checks.get() + 1;
                cancel_checks.set(next);
                next > 3
            },
            |_, _, _| {},
            |batch| {
                persisted.extend(batch);
                Ok(())
            },
        )
        .unwrap();

        assert!(summary.cancelled);
        assert_eq!(summary.scanned_files, persisted.len() as u64);
        assert!(persisted
            .iter()
            .all(|file| !file.fully_scanned && file.scan_error.as_deref() == Some("Cancelled")));
    }

    #[test]
    fn resume_subtree_limits_bypass_directory_cap() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("heavy")).unwrap();
        fs::write(dir.path().join("heavy/a.txt"), "a").unwrap();
        fs::write(dir.path().join("heavy/b.txt"), "b").unwrap();

        let mut capped_paths = Vec::new();
        let capped = scan_inventory_stream(
            dir.path(),
            None,
            ScanLimits {
                batch_size: 10,
                max_items_per_job: None,
                max_items_per_directory: Some(1),
                worker_count: 1,
                measure_build_dir_sizes: true,
                build_dir_batch_pause: None,
            },
            None,
            || false,
            |_, _, _| {},
            |batch| {
                capped_paths.extend(
                    batch
                        .into_iter()
                        .map(|file| (file.relative_path, file.fully_scanned)),
                );
                Ok(())
            },
        )
        .unwrap();
        assert!(capped.partial);
        assert!(capped_paths
            .iter()
            .any(|(path, fully_scanned)| path == "heavy" && !fully_scanned));

        let mut resumed_paths = Vec::new();
        let resumed = scan_inventory_stream(
            dir.path(),
            Some("heavy"),
            ScanLimits::resume_subtree(),
            None,
            || false,
            |_, _, _| {},
            |batch| {
                resumed_paths.extend(batch.into_iter().map(|file| file.relative_path));
                Ok(())
            },
        )
        .unwrap();
        assert!(!resumed.partial);
        assert!(resumed_paths.contains(&"heavy/a.txt".to_string()));
        assert!(resumed_paths.contains(&"heavy/b.txt".to_string()));
    }

    #[test]
    fn reads_git_metadata_without_git_commands() {
        let dir = tempdir().unwrap();
        let git = dir.path().join(".git");
        fs::create_dir_all(&git).unwrap();
        fs::write(git.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(
            git.join("config"),
            "[remote \"origin\"]\n  url = https://example.invalid/passive.git\n",
        )
        .unwrap();

        let metadata = read_git_metadata(dir.path()).unwrap();

        assert_eq!(metadata.current_branch.as_deref(), Some("main"));
        assert_eq!(
            metadata.origin_url.as_deref(),
            Some("https://example.invalid/passive.git")
        );
    }
}
