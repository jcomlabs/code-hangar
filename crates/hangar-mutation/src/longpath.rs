//! Windows extended-length (`\\?\`) path normalization for the mutation engine's
//! own filesystem calls.
//!
//! Windows applies the legacy `MAX_PATH` (260) limit to most Win32 path APIs
//! unless the path is in the *verbatim* / extended-length form (`\\?\C:\…`, or
//! `\\?\UNC\server\share\…` for UNC). Whether the non-verbatim form works on a
//! path over 260 chars depends on the machine's `LongPathsEnabled` registry key;
//! a machine without it fails such calls per-item (fail-closed — no data loss,
//! but the operation cannot complete). To make quarantine / backup / restore /
//! purge of long-path items work regardless of that key, the engine prefixes the
//! ABSOLUTE paths it hands to `std::fs` with the verbatim form here.
//!
//! WHY this must only ever run on an absolute, already-canonical path: the
//! verbatim form **disables** Win32 path normalization, so `.`/`..` and relative
//! segments are taken literally (`\\?\C:\a\..\b` names a real `..` directory
//! rather than resolving upward). This helper therefore refuses (returns the path
//! unchanged) anything that is not a clean absolute drive/UNC path — a no-op is
//! always safe, a wrongly-verbatimized path is not. It is also a no-op on
//! non-Windows and for paths already in verbatim form, so call sites can wrap
//! every path unconditionally.

use std::path::Path;

#[cfg(windows)]
use std::borrow::Cow;
#[cfg(windows)]
use std::ffi::OsString;
#[cfg(windows)]
use std::path::{Component, Prefix};

/// Convert an absolute, canonical Windows path to its extended-length (`\\?\`)
/// form for a filesystem call. Returns the path unchanged when it is already
/// verbatim, is not a plain drive/UNC absolute path, or contains a `.`/`..`
/// segment (the verbatim form would take those literally). A no-op on non-Windows.
///
/// Apply this at the boundary of a `std::fs` operation only — never store the
/// result in the journal, which must keep the ordinary user-visible path so a
/// restore targets the real original location.
#[cfg(windows)]
pub(crate) fn to_extended(path: &Path) -> Cow<'_, Path> {
    // A `.`/`..` segment must never reach the verbatim form (it disables
    // normalization). Scan the RAW text, not `Path::components()`: component
    // iteration silently folds away `.` (CurDir), which would let a literal
    // `\dot\.\seg` slip through and be prefixed. A dotfile *name* such as
    // `.env`/`.git` splits to a segment that is neither "." nor ".." and is
    // correctly left alone.
    if has_dot_segment(path) {
        return Cow::Borrowed(path);
    }

    let mut components = path.components();
    let prefix = match components.next() {
        Some(Component::Prefix(prefix)) => prefix,
        // Not an absolute prefixed path (relative, or bare `\` root) — leave it;
        // verbatim requires a fully-qualified drive/UNC path.
        _ => return Cow::Borrowed(path),
    };

    match prefix.kind() {
        // Already extended-length (or a `\\.\` device namespace path): return the
        // ORIGINAL bytes untouched. (Do not rebuild — that would re-encode and, for a
        // trailing-dot/space name, alter it; and the verbatim form is already correct.)
        Prefix::Verbatim(_)
        | Prefix::VerbatimUNC(_, _)
        | Prefix::VerbatimDisk(_)
        | Prefix::DeviceNS(_) => Cow::Borrowed(path),
        // `C:\…` -> `\\?\C:\…`. The verbatim namespace treats `/` as an ordinary
        // character, not a separator, so any forward slash in the input (paths here are
        // often built by joining a `/`-style relative) MUST be rewritten to `\` first —
        // otherwise Win32 rejects the path (error 123, invalid filename). `.`/`..` were
        // already refused above, so this only reshapes plain segments.
        Prefix::Disk(_) => {
            let backslashed = to_backslashed(path);
            let mut out = OsString::from(r"\\?\");
            out.push(backslashed);
            Cow::Owned(out.into())
        }
        // `\\server\share\…` -> `\\?\UNC\server\share\…` (drop the leading `\\`), with
        // the same forward-slash-to-backslash rewrite for the verbatim namespace.
        Prefix::UNC(_, _) => {
            let backslashed = to_backslashed(path);
            let stripped = backslashed
                .to_string_lossy()
                .strip_prefix(r"\\")
                .map(str::to_string)
                .unwrap_or_else(|| backslashed.to_string_lossy().to_string());
            let mut out = OsString::from(r"\\?\UNC\");
            out.push(stripped);
            Cow::Owned(out.into())
        }
    }
}

/// No-op on non-Windows: no platform has the `MAX_PATH`/verbatim distinction, so
/// the engine's paths are used verbatim already. Borrows so call sites are
/// identical across platforms.
#[cfg(not(windows))]
pub(crate) fn to_extended(path: &Path) -> std::borrow::Cow<'_, Path> {
    std::borrow::Cow::Borrowed(path)
}

/// True if any path segment is exactly `.` or `..` (a dotfile *name* like `.env`
/// is not a match). Splits on both separators so a mixed-separator path is caught.
#[cfg(windows)]
fn has_dot_segment(path: &Path) -> bool {
    path.as_os_str()
        .to_string_lossy()
        .split(['\\', '/'])
        .any(|segment| segment == "." || segment == "..")
}

/// Rewrite forward slashes to backslashes for the verbatim namespace (where `/` is
/// not a path separator). Only reached after `.`/`..` segments are refused, so this
/// never collapses or reinterprets a traversal — it only reshapes plain separators.
#[cfg(windows)]
fn to_backslashed(path: &Path) -> OsString {
    OsString::from(path.as_os_str().to_string_lossy().replace('/', "\\"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn drive_path_gets_verbatim_prefix() {
        assert_eq!(
            to_extended(Path::new(r"C:\Users\me\file.txt")).as_ref(),
            Path::new(r"\\?\C:\Users\me\file.txt")
        );
        // Lower-case drive letter is preserved as written.
        assert_eq!(
            to_extended(Path::new(r"c:\dir\sub")).as_ref(),
            Path::new(r"\\?\c:\dir\sub")
        );
    }

    #[cfg(windows)]
    #[test]
    fn unc_path_gets_verbatim_unc_prefix() {
        assert_eq!(
            to_extended(Path::new(r"\\server\share\dir\file.bin")).as_ref(),
            Path::new(r"\\?\UNC\server\share\dir\file.bin")
        );
    }

    #[cfg(windows)]
    #[test]
    fn already_verbatim_is_unchanged() {
        // Both verbatim-disk and verbatim-UNC inputs pass through byte-for-byte,
        // so wrapping an already-normalized path twice is idempotent.
        for already in [r"\\?\C:\already\verbatim", r"\\?\UNC\server\share\x"] {
            assert_eq!(to_extended(Path::new(already)).as_ref(), Path::new(already));
        }
    }

    #[cfg(windows)]
    #[test]
    fn dot_and_dotdot_segments_are_refused_but_dotfiles_are_not() {
        // A path that still contains `.`/`..` must NOT be verbatimized (verbatim
        // takes those literally); it is returned unchanged so the normal Win32
        // path handling still applies.
        assert_eq!(
            to_extended(Path::new(r"C:\a\..\b")).as_ref(),
            Path::new(r"C:\a\..\b")
        );
        assert_eq!(
            to_extended(Path::new(r"C:\a\.\b")).as_ref(),
            Path::new(r"C:\a\.\b")
        );
        // A dotfile NAME (.env / .git) is a normal segment and IS prefixed — these
        // are exactly the sensitive files the pipeline handles, so they must work.
        assert_eq!(
            to_extended(Path::new(r"C:\project\.env")).as_ref(),
            Path::new(r"\\?\C:\project\.env")
        );
    }

    #[cfg(windows)]
    #[test]
    fn relative_and_device_paths_pass_through() {
        // Relative paths cannot be verbatimized (no drive/UNC anchor).
        assert_eq!(
            to_extended(Path::new(r"relative\path")).as_ref(),
            Path::new(r"relative\path")
        );
        // `\\.\` device-namespace paths are left alone.
        assert_eq!(
            to_extended(Path::new(r"\\.\PhysicalDrive0")).as_ref(),
            Path::new(r"\\.\PhysicalDrive0")
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_is_a_no_op() {
        use std::path::PathBuf;
        // On non-Windows there is no verbatim form; the path is returned as-is.
        let p = PathBuf::from("/absolute/unix/path");
        assert_eq!(to_extended(&p).as_ref(), p.as_path());
        let rel = PathBuf::from("relative/path");
        assert_eq!(to_extended(&rel).as_ref(), rel.as_path());
    }

    #[cfg(windows)]
    #[test]
    fn verbatim_form_actually_opens_a_long_path() {
        // End-to-end proof the produced form works on a >260-char path regardless
        // of LongPathsEnabled: create + read the file only via the verbatim path.
        let base = std::env::temp_dir().join(format!("lp-helper-{}", std::process::id()));
        let seg = "abcdefghijklmnopqrstuvwxyz";
        let mut deep = base.clone();
        for _ in 0..10 {
            deep.push(seg);
        }
        let file = deep.join("payload.bin");
        assert!(file.as_os_str().len() > 260);

        let vdeep = to_extended(&deep);
        std::fs::create_dir_all(vdeep.as_ref()).unwrap();
        let vfile = to_extended(&file);
        std::fs::write(vfile.as_ref(), b"long payload").unwrap();
        assert_eq!(std::fs::read(vfile.as_ref()).unwrap(), b"long payload");

        // Cleanup via the verbatim base (best effort).
        let _ = std::fs::remove_dir_all(to_extended(&base).as_ref());
    }
}
