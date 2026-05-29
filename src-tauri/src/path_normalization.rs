//! Cross-platform path normalization for sync comparisons.
//!
//! On Windows: long-path (`\\?\`) prefixes, UNC handling, and case-insensitive equality.

const MAX_PATH: usize = 260;

/// Returns true when `path` is a UNC path (`\\server\share\...`).
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
        return normalize_windows_path(trimmed);
    }

    #[cfg(not(windows))]
    {
        trimmed.replace('\\', "/")
    }
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
        assert!(paths_equal(
            r"C:\Users\Docs\file.txt",
            r"c:\users\docs\file.txt"
        ));
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
}
