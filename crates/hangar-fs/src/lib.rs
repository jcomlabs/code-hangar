use hangar_core::{
    display_name_for_path, display_path_for_path, normalize_path, FileIdentity, GitRepoSummary,
    ScanOutcome, ScannedFile,
};
use hangar_protect::{
    collapse_default_for_path, is_context_path, is_markdown_path, is_sensitive_path,
    protected_level_for_path, should_index_body,
};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
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
}

impl ScanLimits {
    pub fn root_scan() -> Self {
        Self {
            batch_size: SCAN_BATCH_SIZE,
            max_items_per_job: Some(MAX_SCAN_ITEMS_PER_JOB),
            max_items_per_directory: Some(MAX_SCAN_ITEMS_PER_DIRECTORY),
            worker_count: 1,
        }
    }

    pub fn resume_subtree() -> Self {
        Self {
            batch_size: SCAN_BATCH_SIZE,
            max_items_per_job: Some(MAX_SCAN_ITEMS_PER_JOB),
            max_items_per_directory: None,
            worker_count: 1,
        }
    }
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
    let root = root
        .canonicalize()
        .map_err(|err| format!("Cannot open scan root {}: {err}", root.display()))?;
    let start_path = match start_relative {
        Some(relative) if !relative.is_empty() => root.join(relative),
        _ => root.clone(),
    };
    let mut estimate = InventoryEstimate::default();
    let mut walk = WalkDir::new(&start_path).follow_links(false).into_iter();
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
            && (inspect_path_identity(&absolute_path).is_reparse
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
    is_cancelled: C,
    mut on_progress: P,
    mut on_batch: B,
) -> Result<ScanStreamSummary, String>
where
    C: Fn() -> bool,
    P: FnMut(u64, u64, &str),
    B: FnMut(Vec<ScannedFile>) -> Result<(), String>,
{
    let root = root
        .canonicalize()
        .map_err(|err| format!("Cannot open scan root {}: {err}", root.display()))?;
    let start_path = match start_relative {
        Some(relative) if !relative.is_empty() => root.join(relative),
        _ => root.clone(),
    };

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
    let mut walk = WalkDir::new(&start_path).follow_links(false).into_iter();

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
            Err(_) => {
                inaccessible_items += 1;
                continue;
            }
        };

        let absolute_path = entry.path().to_path_buf();
        let relative_path = relative_to(&root, &absolute_path);
        let normalized_relative = normalize_path(&relative_path);
        if normalized_relative.is_empty() {
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

        let (mut item, indexed) = scanned_item_with_body(
            &root,
            &absolute_path,
            if entry_type.is_dir() {
                "directory"
            } else {
                "file"
            },
        );
        if indexed {
            indexed_documents += 1;
        }

        let is_reparse_dir = entry_type.is_dir()
            && item
                .identity
                .as_ref()
                .map(|identity| identity.is_reparse)
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
        if !is_reparse_dir
            && entry_type.is_dir()
            && is_build_dependency_dir_name(entry.file_name().to_str().unwrap_or(""))
        {
            let (recursive_apparent, size_partial) =
                build_dir_recursive_apparent_bytes(&absolute_path, &is_cancelled);
            if let Some(identity) = item.identity.as_mut() {
                let own = identity.size_apparent.unwrap_or(0);
                let total = own.saturating_add(recursive_apparent);
                identity.size_apparent = Some(total);
                // The size-only walk does not open every file handle, so it cannot truthfully
                // report allocated bytes. Leaving this unknown makes physical accounting use its
                // documented apparent-size fallback instead of presenting an approximation as an
                // exact allocation figure.
                identity.size_allocated = None;
            }
            if size_partial {
                item.fully_scanned = false;
                item.scan_error = Some(
                    "Build/dependency folder size is incomplete because the measurement was cancelled or an entry was inaccessible."
                        .to_string(),
                );
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

/// Build/dependency folder names whose CONTENTS the inventory scanner records as a single node
/// instead of descending into. These are regenerable, not user content — `classify_orphan_candidate`
/// already excludes the exact same set from results — and indexing their 10⁴–10⁵ children each
/// (a Rust `target/`, a `node_modules/`, a `.venv/`) is the dominant source of inventory bloat and
/// of the slow project loads / Organize scans that follow.
fn is_build_dependency_dir_name(name: &str) -> bool {
    matches!(
        name,
        "target"
            | "node_modules"
            | ".venv"
            | "venv"
            | "__pycache__"
            | ".git"
            | "dist"
            | "build"
            | ".cache"
            | ".ssh"
    )
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
fn build_dir_recursive_apparent_bytes<C>(root: &Path, is_cancelled: &C) -> (u64, bool)
where
    C: Fn() -> bool,
{
    let mut total: u64 = 0;
    let mut partial = false;
    let mut walk = WalkDir::new(root).follow_links(false).into_iter();
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
    let root = root
        .canonicalize()
        .map_err(|err| format!("Cannot open scan root {}: {err}", root.display()))?;

    let mut files = Vec::new();
    let mut scanned_files = 0;
    let mut indexed_documents = 0;
    let mut inaccessible_items = 0;

    let mut child_counts: HashMap<String, i64> = HashMap::new();
    let mut walk = WalkDir::new(&root)
        .follow_links(false)
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
        let identity = inspect_path_identity(&absolute_path);
        if entry.file_type().is_dir() && identity.is_reparse {
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
            && !file_is_cloud_placeholder(&absolute_path)
            && identity
                .size_apparent
                .map(|size| size <= MAX_INDEX_BYTES)
                .unwrap_or(false)
        {
            match fs::read_to_string(&absolute_path) {
                Ok(value) => {
                    indexed_documents += 1;
                    Some(value)
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
        FileStandardInfo, GetFileInformationByHandle, GetFileInformationByHandleEx,
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_NO_RECALL, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_STANDARD_INFO, OPEN_EXISTING,
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

#[cfg(not(windows))]
fn fill_platform_identity(_path: &Path, _metadata: &fs::Metadata, _identity: &mut FileIdentity) {}

fn system_time_to_unix_string(value: SystemTime) -> Option<String> {
    value
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs().to_string())
}

pub fn read_git_metadata(root: &Path) -> Option<GitRepoSummary> {
    let git_dir = root.join(".git");
    if !git_dir.is_dir() {
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

    match fs::read_to_string(git_dir.join("HEAD")) {
        Ok(head) => {
            let trimmed = head.trim();
            summary.head_ref = Some(trimmed.to_string());
            if let Some(branch) = trimmed.strip_prefix("ref: refs/heads/") {
                summary.current_branch = Some(branch.to_string());
            }
        }
        Err(err) => summary.metadata_error = Some(format!("HEAD read failed: {err}")),
    }

    match fs::read_to_string(git_dir.join("config")) {
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
) -> (ScannedFile, bool) {
    let mut item = scanned_item_for_path(root, absolute_path, None);
    item.item_kind = item_kind.to_string();
    if item_kind == "file"
        && should_index_body(&item.relative_path)
        && !file_is_cloud_placeholder(absolute_path)
        // Never read the body of a symlink/reparse point: fs::read_to_string follows the link
        // and would index a TARGET outside the scanned root (e.g. a symlink named README.md ->
        // ~/.ssh/id_rsa). The legacy markdown scanner gated on file_type().is_file(); the
        // streaming rewrite dropped that, so restore it here.
        && item
            .identity
            .as_ref()
            .map(|identity| !identity.is_symlink && !identity.is_reparse)
            .unwrap_or(false)
        && item
            .identity
            .as_ref()
            .and_then(|identity| identity.size_apparent)
            .map(|size| size <= MAX_INDEX_BYTES)
            .unwrap_or(false)
    {
        if let Ok(value) = fs::read_to_string(absolute_path) {
            item.body = Some(value);
            return (item, true);
        }
    }
    (item, false)
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
                        let result = scanned_item_with_body(&root, &path, "file");
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
fn file_is_cloud_placeholder(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;

    fs::symlink_metadata(path)
        .map(|metadata| cloud_placeholder_attributes(metadata.file_attributes()))
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

#[cfg(not(windows))]
fn file_is_cloud_placeholder(_path: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

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
            },
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

        let mut files: Vec<ScannedFile> = Vec::new();
        scan_inventory_stream(
            dir.path(),
            None,
            ScanLimits {
                batch_size: 64,
                max_items_per_job: None,
                max_items_per_directory: None,
                worker_count: 1,
            },
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
        // …but NONE of their children are indexed (this is the bloat fix).
        assert!(
            !paths.iter().any(|p| {
                p.starts_with("node_modules/")
                    || p.starts_with("target/")
                    || p.starts_with(".venv/")
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
        assert!(files
            .iter()
            .filter(
                |file| ["node_modules", "target", ".venv"].contains(&file.relative_path.as_str())
            )
            .all(|file| file
                .identity
                .as_ref()
                .is_some_and(|identity| identity.size_allocated.is_none())));
    }

    #[test]
    fn cancelled_build_dir_measurement_is_marked_partial() {
        use std::cell::Cell;

        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join("target/debug")).unwrap();
        fs::write(dir.path().join("target/debug/a.bin"), [1_u8; 8]).unwrap();
        fs::write(dir.path().join("target/debug/b.bin"), [2_u8; 8]).unwrap();

        let checks = Cell::new(0_u32);
        let (_bytes, partial) = build_dir_recursive_apparent_bytes(dir.path(), &|| {
            let next = checks.get() + 1;
            checks.set(next);
            next >= 3
        });

        assert!(
            partial,
            "a cancelled size walk must be reported as a lower bound"
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
            },
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
            },
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
            },
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
            },
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
