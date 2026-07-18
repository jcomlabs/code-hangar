use hangar_fs::{scan_inventory_stream, ScanLimits};
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

const PROJECT_FILE_COUNT: usize = 3_000;
const REGISTRY_FILE_COUNT: usize = 1_000;
const COLLAPSED_CACHE_FILE_COUNT: usize = 1_000;

#[test]
#[ignore = "large generated fixture; run with scripts/acceptance-v011.ps1 -Lane DataStress"]
fn large_adversarial_inventory_stays_bounded_and_cancellable() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("project");
    fs::create_dir_all(&root).unwrap();

    for shard in 0..100 {
        let shard_dir = root.join("src").join(format!("shard-{shard:03}"));
        fs::create_dir_all(&shard_dir).unwrap();
        for file in 0..30 {
            fs::write(
                shard_dir.join(format!("file-{file:03}.txt")),
                b"project data",
            )
            .unwrap();
        }
    }

    let registry_root = root.join(".local/cargo/registry/src");
    for package in 0..40 {
        let package_dir = registry_root.join(format!("vendor-{package:03}"));
        fs::create_dir_all(&package_dir).unwrap();
        for file in 0..25 {
            fs::write(
                package_dir.join(format!("source-{file:03}.rs")),
                b"fn vendored() {}",
            )
            .unwrap();
        }
    }

    let cache_root = root.join("node_modules");
    for package in 0..40 {
        let package_dir = cache_root.join(format!("package-{package:03}"));
        fs::create_dir_all(&package_dir).unwrap();
        for file in 0..25 {
            fs::write(package_dir.join(format!("asset-{file:03}.js")), [b'x'; 64]).unwrap();
        }
    }

    fs::write(root.join("README.md"), b"# Stress project\n").unwrap();
    fs::write(root.join(".env"), b"TOKEN=must-not-be-indexed\n").unwrap();
    fs::write(root.join("CORRUPT.md"), [0xff, 0xfe, 0xfd, 0x00]).unwrap();

    let unicode_root = root.join("unicode");
    fs::create_dir_all(&unicode_root).unwrap();
    for name in [
        "caf\u{e9}.md",
        "cafe\u{301}.md",
        "\u{65e5}\u{672c}\u{8a9e}.md",
        "\u{1f680}-context.md",
    ] {
        fs::write(unicode_root.join(name), b"# Unicode\n").unwrap();
    }

    let mut long_dir = root.clone();
    for segment in 0..5 {
        long_dir.push(format!("long-segment-{segment}-abcdefghijklmnopqrstuvwxyz"));
    }
    fs::create_dir_all(&long_dir).unwrap();
    let long_file = long_dir.join("long-context.md");
    fs::write(&long_file, b"# Long path\n").unwrap();
    let long_relative = long_file
        .strip_prefix(&root)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    assert!(long_relative.len() > 180, "fixture path is not long enough");

    let outside = fixture.path().join("outside-secret.md");
    fs::write(&outside, b"OUTSIDE_SECRET_MUST_NOT_BE_READ").unwrap();
    let linked_path = root.join("linked-context.md");
    let link_created = create_file_symlink(&outside, &linked_path).is_ok();
    let outside_dir = fixture.path().join("outside-directory");
    fs::create_dir_all(&outside_dir).unwrap();
    fs::write(
        outside_dir.join("README.md"),
        b"OUTSIDE_DIRECTORY_MUST_NOT_BE_SCANNED",
    )
    .unwrap();
    let linked_dir = root.join("linked-directory");
    create_directory_link(&outside_dir, &linked_dir)
        .expect("the reparse fixture must be creatable on this platform");
    let denied_dir = root.join("denied-directory");
    fs::create_dir_all(&denied_dir).unwrap();
    fs::write(denied_dir.join("hidden.txt"), b"inaccessible fixture").unwrap();
    let denied_guard = DeniedDirectoryGuard::apply(&denied_dir)
        .expect("the access-denied fixture must be creatable on this platform");
    let denied_enforced = denied_guard.enforced();

    let started = Instant::now();
    let mut items = Vec::new();
    let mut batch_sizes = Vec::new();
    let summary = scan_inventory_stream(
        &root,
        None,
        ScanLimits {
            batch_size: 127,
            max_items_per_job: None,
            max_items_per_directory: None,
            worker_count: 4,
        },
        || false,
        |_, _, _| {},
        |batch| {
            batch_sizes.push(batch.len());
            items.extend(batch);
            Ok(())
        },
    )
    .unwrap();
    let elapsed = started.elapsed();
    denied_guard.restore().unwrap();

    assert!(!summary.cancelled);
    assert!(!summary.partial, "unexpected partial scan: {summary:?}");
    if denied_enforced {
        assert!(summary.inaccessible_items >= 1);
        assert!(!items
            .iter()
            .any(|item| item.relative_path == "denied-directory/hidden.txt"));
    } else {
        println!("access-denied probe skipped: this account bypasses the deny rule");
    }
    assert_eq!(summary.scanned_files, items.len() as u64);
    assert!(summary.scanned_files > (PROJECT_FILE_COUNT + REGISTRY_FILE_COUNT) as u64);
    assert!(batch_sizes.len() > 10);
    assert!(batch_sizes.iter().all(|size| *size <= 127));

    let paths = items
        .iter()
        .map(|item| item.relative_path.as_str())
        .collect::<Vec<_>>();
    assert!(paths.contains(&"src/shard-099/file-029.txt"));
    assert!(paths.contains(&long_relative.as_str()));
    assert_eq!(
        paths
            .iter()
            .filter(|path| path.starts_with(".local/cargo/registry/src/vendor-"))
            .count(),
        REGISTRY_FILE_COUNT + 40
    );

    let sensitive = items
        .iter()
        .find(|item| item.relative_path == ".env")
        .unwrap();
    assert!(sensitive.is_sensitive);
    assert!(sensitive.body.is_none());
    assert!(items
        .iter()
        .find(|item| item.relative_path == "CORRUPT.md")
        .is_some_and(|item| item.body.is_none()));

    let cache = items
        .iter()
        .find(|item| item.relative_path == "node_modules")
        .unwrap();
    assert!(cache.collapse_default);
    assert!(cache
        .identity
        .as_ref()
        .and_then(|identity| identity.size_apparent)
        .is_some_and(|bytes| bytes >= (COLLAPSED_CACHE_FILE_COUNT * 64) as u64));
    assert!(!paths.iter().any(|path| path.starts_with("node_modules/")));

    if link_created {
        let linked = items
            .iter()
            .find(|item| item.relative_path == "linked-context.md")
            .unwrap();
        assert!(linked
            .identity
            .as_ref()
            .is_some_and(|identity| identity.is_reparse));
        assert!(linked.body.is_none());
    } else {
        println!("symlink probe skipped: this Windows account cannot create symlinks");
    }
    let linked_directory = items
        .iter()
        .find(|item| item.relative_path == "linked-directory")
        .unwrap();
    assert!(linked_directory
        .identity
        .as_ref()
        .is_some_and(|identity| identity.is_reparse));
    assert!(!paths
        .iter()
        .any(|path| path.starts_with("linked-directory/")));

    assert!(
        elapsed.as_secs() < 120,
        "generated inventory exceeded the generous 120 s guardrail: {elapsed:?}"
    );
    println!(
        "inventory stress: {} persisted items, {} indexed documents, {} batches in {:?}",
        items.len(),
        summary.indexed_documents,
        batch_sizes.len(),
        elapsed
    );

    let cancellation_checks = AtomicU64::new(0);
    let mut partial_items = 0usize;
    let cancelled = scan_inventory_stream(
        &root,
        None,
        ScanLimits {
            batch_size: 127,
            max_items_per_job: None,
            max_items_per_directory: None,
            worker_count: 4,
        },
        || cancellation_checks.fetch_add(1, Ordering::SeqCst) >= 700,
        |_, _, _| {},
        |batch| {
            partial_items += batch.len();
            Ok(())
        },
    )
    .unwrap();
    assert!(cancelled.cancelled);
    assert!(cancelled.partial);
    assert!(cancelled.scanned_files > 0);
    assert!(cancelled.scanned_files < summary.scanned_files);
    assert!(partial_items <= cancelled.scanned_files as usize);

    fs::remove_dir(&linked_dir).unwrap();
}

#[cfg(windows)]
fn create_file_symlink(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

#[cfg(unix)]
fn create_file_symlink(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn create_directory_link(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    use std::process::{Command, Stdio};

    if std::os::windows::fs::symlink_dir(target, link).is_ok() {
        return Ok(());
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
        Err(std::io::Error::other(format!(
            "mklink /J failed with {status}"
        )))
    }
}

#[cfg(unix)]
fn create_directory_link(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
struct DeniedDirectoryGuard {
    path: std::path::PathBuf,
    identity: String,
    active: bool,
}

#[cfg(windows)]
impl DeniedDirectoryGuard {
    fn apply(path: &std::path::Path) -> std::io::Result<Self> {
        use std::process::Command;

        let identity_output = Command::new("whoami.exe").output()?;
        if !identity_output.status.success() {
            return Err(std::io::Error::other("whoami.exe failed"));
        }
        let identity = String::from_utf8_lossy(&identity_output.stdout)
            .trim()
            .to_string();
        if identity.is_empty() {
            return Err(std::io::Error::other("whoami.exe returned no identity"));
        }
        let output = Command::new("icacls.exe")
            .arg(path)
            .arg("/deny")
            .arg(format!("{identity}:(RD)"))
            .output()?;
        if !output.status.success() {
            return Err(std::io::Error::other(format!(
                "icacls /deny failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(Self {
            path: path.to_path_buf(),
            identity,
            active: true,
        })
    }

    fn enforced(&self) -> bool {
        fs::read_dir(&self.path).is_err()
    }

    fn restore(mut self) -> std::io::Result<()> {
        let result = self.remove_deny();
        if result.is_ok() {
            self.active = false;
        }
        result
    }

    fn remove_deny(&self) -> std::io::Result<()> {
        use std::process::Command;

        let output = Command::new("icacls.exe")
            .arg(&self.path)
            .arg("/remove:d")
            .arg(&self.identity)
            .output()?;
        if output.status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!(
                "icacls /remove:d failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )))
        }
    }
}

#[cfg(windows)]
impl Drop for DeniedDirectoryGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = self.remove_deny();
        }
    }
}

#[cfg(unix)]
struct DeniedDirectoryGuard {
    path: std::path::PathBuf,
    original_permissions: fs::Permissions,
    active: bool,
}

#[cfg(unix)]
impl DeniedDirectoryGuard {
    fn apply(path: &std::path::Path) -> std::io::Result<Self> {
        use std::os::unix::fs::PermissionsExt;

        let original_permissions = fs::metadata(path)?.permissions();
        fs::set_permissions(path, fs::Permissions::from_mode(0))?;
        Ok(Self {
            path: path.to_path_buf(),
            original_permissions,
            active: true,
        })
    }

    fn enforced(&self) -> bool {
        fs::read_dir(&self.path).is_err()
    }

    fn restore(mut self) -> std::io::Result<()> {
        let result = fs::set_permissions(&self.path, self.original_permissions.clone());
        if result.is_ok() {
            self.active = false;
        }
        result
    }
}

#[cfg(unix)]
impl Drop for DeniedDirectoryGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = fs::set_permissions(&self.path, self.original_permissions.clone());
        }
    }
}
