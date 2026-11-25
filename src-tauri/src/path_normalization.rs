//! Cross-platform path normalization for sync comparisons.
//!
//! On Windows: long-path (`\\?\`) prefixes, UNC handling, and case-insensitive equality.

#[cfg(windows)]
const MAX_PATH: usize = 260;

/// Returns true when `path` is a UNC path (`\\server\share\...`).
#[allow(dead_code)]
pub fn is_unc(path: &str) -> bool {
    let path = path.trim();
    if path.starts_with(r"\\?\UNC\") {
        return true;
    }
    path.starts_with(r"\\")
        && !path.starts_with(r"\\?\")
        && path.len() >= 3
        && path.as_bytes().get(2) != Some(&b'\\')
}

/// Normalizes a path for stable comparison and filesystem access on Windows.
pub fn normalize_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    #[cfg(windows)]
    {
        normalize_windows_path(trimmed)
    }

    #[cfg(not(windows))]
    {
        trimmed.replace('\\', "/")
    }
}

/// Returns true when `path` is the same as or nested under `root`.
///
/// Both arguments are normalized the same way as sync paths (`to_long_path`), including
/// `\\?\` vs conventional Windows paths and UNC extended prefixes.
pub fn path_is_within_root(path: &str, root: &str) -> bool {
    if paths_equal(path, root) {
        return true;
    }

    let path_norm = to_long_path(path);
    let root_norm = to_long_path(root);

    #[cfg(windows)]
    {
        let path_lower = path_norm.to_ascii_lowercase();
        let mut root_lower = root_norm.to_ascii_lowercase();
        if !root_lower.ends_with('\\') {
            root_lower.push('\\');
        }
        path_lower.starts_with(&root_lower)
    }

    #[cfg(not(windows))]
    {
        let root_prefix =
            if root_norm.ends_with('/') { root_norm } else { format!("{root_norm}/") };
        path_norm.starts_with(&root_prefix)
    }
}

/// Returns true when one folder root is the same as or nested inside the other.
pub fn pair_roots_nested(left: &str, right: &str) -> bool {
    if paths_equal(left, right) {
        return true;
    }
    path_is_within_root(left, right) || path_is_within_root(right, left)
}

/// Compares two paths after normalization (case-insensitive on Windows).
pub fn paths_equal(a: &str, b: &str) -> bool {
    let na = normalize_path(a);
    let nb = normalize_path(b);

    #[cfg(windows)]
    {
        na.eq_ignore_ascii_case(&nb)
    }

    #[cfg(not(windows))]
    {
        na == nb
    }
}

/// Resolve both execution roots before scanning or writing. Canonicalization rejects
/// unresolved paths and resolves ordinary aliases and dot components.
pub fn validate_pair_roots(left: &std::path::Path, right: &std::path::Path) -> Result<(), String> {
    let left = std::fs::canonicalize(left)
        .map_err(|e| format!("Invalid folder pair: cannot resolve left folder: {e}"))?;
    let right = std::fs::canonicalize(right)
        .map_err(|e| format!("Invalid folder pair: cannot resolve right folder: {e}"))?;
    if pair_roots_nested(&left.to_string_lossy(), &right.to_string_lossy()) {
        return Err("Invalid folder pair: choose separate folders; the sync roots must not be the same or nested.".into());
    }
    Ok(())
}

/// Ordered canonical root identities used to bind planning and execution.
pub fn pair_root_keys(
    left: &std::path::Path,
    right: &std::path::Path,
) -> Result<[String; 2], String> {
    validate_pair_roots(left, right)?;
    let key = |path: &std::path::Path| -> Result<String, String> {
        let value =
            std::fs::canonicalize(path).map_err(|e| e.to_string())?.to_string_lossy().into_owned();
        #[cfg(windows)]
        let value = value.to_lowercase();
        let identity =
            root_identity(path).map_err(|e| format!("Cannot identify sync root: {e}"))?;
        Ok(format!("{value}|{identity}"))
    };
    Ok([key(left)?, key(right)?])
}

#[cfg(unix)]
fn root_identity(path: &std::path::Path) -> std::io::Result<String> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path)?;
    Ok(format!("{}:{}", meta.dev(), meta.ino()))
}

#[cfg(windows)]
fn root_identity(path: &std::path::Path) -> std::io::Result<String> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };
    let file = std::fs::OpenOptions::new()
        .access_mode(0x80)
        .share_mode(7)
        .custom_flags(0x0200_0000)
        .open(path)?;
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(format!("{}:{}:{}", info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow))
}

/// Converts a path to an extended-length Windows path when beneficial.
pub fn to_long_path(path: &str) -> String {
    let normalized = normalize_path(path);
    #[cfg(windows)]
    {
        if normalized.starts_with(r"\\?\") {
            return normalized;
        }
        if is_unc(&normalized) {
            let rest = normalized.trim_start_matches('\\');
            return format!(r"\\?\UNC\{rest}");
        }
        if normalized.len() >= MAX_PATH || normalized.contains("..") {
            return format!(r"\\?\{}", normalized);
        }
        normalized
    }

    #[cfg(not(windows))]
    {
        normalized
    }
}

#[cfg(windows)]
fn normalize_windows_path(path: &str) -> String {
    let path = path.replace('/', "\\");

    if path.starts_with(r"\\?\") {
        return path;
    }

    if path.starts_with(r"\\") {
        if path.starts_with(r"\\?\UNC\") {
            return path;
        }
        // UNC: \\server\share\...
        let without_prefix = path.trim_start_matches('\\');
        if without_prefix.contains('\\') {
            return format!(r"\\?\UNC\{without_prefix}");
        }
        return path;
    }

    if path.len() >= MAX_PATH {
        return format!(r"\\?\{}", path);
    }

    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_roots_reject_equal_and_nested_but_accept_siblings() {
        let root = tempfile::TempDir::new().unwrap();
        let left = root.path().join("left");
        let nested = left.join("nested");
        let right = root.path().join("right");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(&right).unwrap();
        assert!(validate_pair_roots(&left, &left).is_err());
        assert!(validate_pair_roots(&left, &nested).is_err());
        assert!(validate_pair_roots(&nested, &left).is_err());
        assert!(validate_pair_roots(&left, &right).is_ok());
    }

    #[test]
    fn empty_path_normalizes_to_empty() {
        assert_eq!(normalize_path("   "), "");
    }

    #[cfg(windows)]
    #[test]
    fn unc_path_detected() {
        assert!(is_unc(r"\\server\share\folder"));
        assert!(!is_unc(r"C:\folder"));
        assert!(is_unc(r"\\?\UNC\server\share"));
    }

    #[cfg(windows)]
    #[test]
    fn unc_normalizes_to_extended_unc() {
        let normalized = normalize_path(r"\\server\share\file.txt");
        assert!(normalized.starts_with(r"\\?\UNC\server\share"));
    }

    #[cfg(windows)]
    #[test]
    fn long_path_gets_prefix() {
        let long = format!(r"C:\{}", "a".repeat(300));
        let normalized = normalize_path(&long);
        assert!(normalized.starts_with(r"\\?\"));
    }

    #[cfg(windows)]
    #[test]
    fn paths_equal_is_case_insensitive() {
        assert!(paths_equal(r"C:\Users\Docs\file.txt", r"c:\users\docs\file.txt"));
    }

    #[cfg(not(windows))]
    #[test]
    fn paths_equal_is_case_sensitive() {
        assert!(paths_equal("/tmp/a", "/tmp/a"));
        assert!(!paths_equal("/tmp/A", "/tmp/a"));
    }

    #[cfg(windows)]
    #[test]
    fn to_long_path_preserves_already_extended() {
        let extended = r"\\?\C:\already";
        assert_eq!(to_long_path(extended), extended);
    }

    #[cfg(windows)]
    #[test]
    fn path_is_within_root_matches_conventional_under_extended_unc() {
        let root = r"\\?\UNC\server\share\sync";
        let event = r"\\server\share\sync\file.txt";
        assert!(path_is_within_root(event, root));
        assert!(!path_is_within_root(r"\\server\share\other\file.txt", root));
    }

    #[cfg(windows)]
    #[test]
    fn path_is_within_root_is_case_insensitive() {
        assert!(path_is_within_root(r"C:\Users\Docs\file.txt", r"c:\users\docs"));
        assert!(!path_is_within_root(r"C:\Users\Documents\file.txt", r"c:\users\docs"));
    }

    #[cfg(not(windows))]
    #[test]
    fn path_is_within_root_respects_directory_boundary() {
        assert!(path_is_within_root("/tmp/a/b", "/tmp/a"));
        assert!(!path_is_within_root("/tmp/ab", "/tmp/a"));
    }

    #[test]
    fn pair_roots_nested_detects_subfolder() {
        assert!(pair_roots_nested(r"C:\Users\Docs\Projects", r"C:\Users\Docs"));
        assert!(pair_roots_nested(r"C:\Users\Docs", r"C:\Users\Docs\Projects"));
    }

    #[test]
    fn pair_roots_nested_rejects_sibling_paths() {
        assert!(!pair_roots_nested(r"C:\Users\Docs", r"C:\Users\Photos"));
    }

    #[cfg(not(windows))]
    #[test]
    fn pair_roots_nested_respects_directory_boundary() {
        assert!(!pair_roots_nested("/tmp/ab", "/tmp/a"));
        assert!(pair_roots_nested("/tmp/a/b", "/tmp/a"));
    }
}
