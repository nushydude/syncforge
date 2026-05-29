use std::path::Path;
use std::time::UNIX_EPOCH;

use globset::{Glob, GlobSet, GlobSetBuilder};
use jwalk::WalkDir;

use crate::models::{FileEntry, Filters, SyncMode};

/// Maximum path-level warnings returned from a single scan side.
pub const MAX_SCAN_WARNINGS: usize = 50;

/// Extracts whole seconds and subsecond nanoseconds from filesystem metadata.
pub fn metadata_modified(metadata: &std::fs::Metadata) -> (i64, u32) {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| (d.as_secs() as i64, d.subsec_nanos()))
        .unwrap_or((0, 0))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanOutput {
    pub entries: Vec<FileEntry>,
    pub skipped_entries: u32,
    /// Paths that failed walk/stat, capped at [`MAX_SCAN_WARNINGS`] plus an overflow summary.
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("path not found: {0}")]
    NotFound(String),
    #[error("invalid glob pattern: {0}")]
    InvalidPattern(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

pub type ScanResult = std::result::Result<ScanOutput, ScanError>;

fn push_scan_warning(warnings: &mut Vec<String>, root: &Path, path: Option<&Path>, reason: &str) {
    if warnings.len() >= MAX_SCAN_WARNINGS {
        return;
    }
    let display = path
        .and_then(|p| p.strip_prefix(root).ok())
        .map(to_relative_string)
        .or_else(|| path.map(|p| p.display().to_string()))
        .unwrap_or_else(|| "<unknown>".into());
    warnings.push(format!("{display}: {reason}"));
}

fn append_overflow_summary(warnings: &mut Vec<String>, skipped_entries: u32) {
    let recorded = warnings.len();
    if skipped_entries as usize > recorded {
        warnings
            .push(format!("... and {} more skipped path(s)", skipped_entries as usize - recorded));
    }
}

/// Echo and Synchronize treat missing entries as deletions; refuse when the scan is incomplete.
pub fn assert_destructive_scan_allowed(
    mode: SyncMode,
    left: &ScanOutput,
    right: &ScanOutput,
) -> Result<(), String> {
    if !matches!(mode, SyncMode::Echo | SyncMode::Synchronize) {
        return Ok(());
    }
    let total_skipped = left.skipped_entries.saturating_add(right.skipped_entries);
    if total_skipped == 0 {
        return Ok(());
    }
    let mode_label = match mode {
        SyncMode::Echo => "Echo",
        SyncMode::Synchronize => "Synchronize",
        SyncMode::Contribute => "Contribute",
    };
    Err(format!(
        "Cannot run {mode_label} sync: scan skipped {total_skipped} path(s) due to permissions or read errors. \
         Incomplete scans can cause unintended deletions. Fix folder access and preview again."
    ))
}

/// Prefixes per-side scan warnings for inclusion in a sync plan.
pub fn plan_scan_warnings(left: &ScanOutput, right: &ScanOutput) -> Vec<String> {
    let mut warnings = Vec::new();
    for warning in &left.warnings {
        warnings.push(format!("left: {warning}"));
    }
    for warning in &right.warnings {
        warnings.push(format!("right: {warning}"));
    }
    warnings
}

/// Scan integrity metadata passed from directory scans into sync planning.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScanIntegrity {
    pub skipped_left: u32,
    pub skipped_right: u32,
    pub warnings: Vec<String>,
    pub requires_attention: bool,
}

impl ScanIntegrity {
    pub fn from_sides(left: &ScanOutput, right: &ScanOutput) -> Self {
        let skipped_left = left.skipped_entries;
        let skipped_right = right.skipped_entries;
        Self {
            skipped_left,
            skipped_right,
            warnings: plan_scan_warnings(left, right),
            requires_attention: skipped_left > 0 || skipped_right > 0,
        }
    }
}

#[cfg(test)]
mod scan_test_hooks {
    use std::cell::Cell;
    use std::sync::atomic::{AtomicUsize, Ordering};

    thread_local! {
        static COUNT_SCANS: Cell<bool> = const { Cell::new(false) };
    }

    static SCAN_INVOCATIONS: AtomicUsize = AtomicUsize::new(0);

    pub fn record_scan_invocation() {
        if COUNT_SCANS.with(|c| c.get()) {
            SCAN_INVOCATIONS.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn scan_invocation_count() -> usize {
        SCAN_INVOCATIONS.load(Ordering::Relaxed)
    }

    pub fn reset_scan_invocations() {
        SCAN_INVOCATIONS.store(0, Ordering::Relaxed);
    }

    /// Runs `f` on the current thread while counting [`super::scan_directory`] calls.
    pub fn with_scan_counting<F, R>(f: F) -> (R, usize)
    where
        F: FnOnce() -> R,
    {
        reset_scan_invocations();
        COUNT_SCANS.with(|c| c.set(true));
        let result = f();
        COUNT_SCANS.with(|c| c.set(false));
        (result, scan_invocation_count())
    }
}

#[cfg(test)]
pub use scan_test_hooks::with_scan_counting;

/// Scans `root` in parallel and returns filtered entries keyed by relative path.
pub fn scan_directory(root: &Path, filters: &Filters) -> ScanResult {
    #[cfg(test)]
    scan_test_hooks::record_scan_invocation();

    let root = root.canonicalize().map_err(|_| ScanError::NotFound(root.display().to_string()))?;

    if !root.is_dir() {
        return Err(ScanError::NotFound(root.display().to_string()));
    }

    let include_set = build_glob_set(&filters.include)?;
    let exclude_set = build_glob_set(&filters.exclude)?;

    let mut entries = Vec::new();
    let mut skipped_entries = 0u32;
    let mut warnings = Vec::new();

    for entry in WalkDir::new(&root).follow_links(false).into_iter() {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                skipped_entries += 1;
                push_scan_warning(&mut warnings, &root, err.path(), "walk failed");
                continue;
            }
        };

        let path = entry.path();
        if path == root {
            continue;
        }

        let Ok(relative) = path.strip_prefix(&root) else {
            continue;
        };
        let relative_path = to_relative_string(relative);
        if relative_path.is_empty() {
            continue;
        }

        if !matches_filters(&relative_path, include_set.as_ref(), exclude_set.as_ref()) {
            continue;
        }

        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(_) => {
                skipped_entries += 1;
                push_scan_warning(
                    &mut warnings,
                    &root,
                    Some(path.as_path()),
                    "metadata unavailable",
                );
                continue;
            }
        };

        let (modified_secs, modified_nanos) = metadata_modified(&metadata);

        entries.push(FileEntry {
            relative_path,
            size: if metadata.is_file() { metadata.len() } else { 0 },
            modified_secs,
            modified_nanos,
            is_dir: metadata.is_dir(),
            hash: None,
        });
    }

    append_overflow_summary(&mut warnings, skipped_entries);
    entries.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok(ScanOutput { entries, skipped_entries, warnings })
}

/// Returns true when `relative_path` passes include/exclude glob rules.
pub fn matches_filters(
    relative_path: &str,
    include: Option<&GlobSet>,
    exclude: Option<&GlobSet>,
) -> bool {
    let path = relative_path.replace('\\', "/");

    if let Some(include) = include {
        if !include.is_match(&path) {
            return false;
        }
    }

    if let Some(exclude) = exclude {
        if exclude.is_match(&path) {
            return false;
        }
    }

    true
}

fn build_glob_set(patterns: &[String]) -> std::result::Result<Option<globset::GlobSet>, ScanError> {
    if patterns.is_empty() {
        return Ok(None);
    }

    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let normalized = normalize_pattern(pattern);
        let glob = Glob::new(&normalized)
            .map_err(|e| ScanError::InvalidPattern(format!("{pattern}: {e}")))?;
        builder.add(glob);
    }
    Ok(Some(builder.build().map_err(|e| ScanError::InvalidPattern(e.to_string()))?))
}

fn normalize_pattern(pattern: &str) -> String {
    let trimmed = pattern.trim();
    if trimmed.contains('/') || trimmed.contains('\\') {
        trimmed.replace('\\', "/")
    } else {
        format!("**/{}", trimmed)
    }
}

fn to_relative_string(relative: &Path) -> String {
    relative.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn matches_filter_rules(relative_path: &str, filters: &Filters) -> bool {
        let include = build_glob_set(&filters.include).ok().flatten();
        let exclude = build_glob_set(&filters.exclude).ok().flatten();
        matches_filters(relative_path, include.as_ref(), exclude.as_ref())
    }

    #[test]
    fn matches_include_and_exclude() {
        let filters = Filters { include: vec!["*.txt".into()], exclude: vec!["*.tmp".into()] };
        assert!(matches_filter_rules("notes.txt", &filters));
        assert!(matches_filter_rules("sub/notes.txt", &filters));
        assert!(!matches_filter_rules("notes.tmp", &filters));
        assert!(!matches_filter_rules("image.png", &filters));
    }

    #[test]
    fn empty_include_matches_all_non_excluded() {
        let filters = Filters { include: vec![], exclude: vec!["*.tmp".into()] };
        assert!(matches_filter_rules("any.bin", &filters));
        assert!(!matches_filter_rules("skip.tmp", &filters));
    }

    #[test]
    fn scan_directory_collects_files_and_dirs() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        fs::create_dir_all(root.join("sub")).expect("mkdir");
        fs::write(root.join("a.txt"), "hello").expect("write");
        fs::write(root.join("sub/b.txt"), "world").expect("write");
        fs::write(root.join("skip.tmp"), "x").expect("write");

        let filters = Filters { include: vec![], exclude: vec!["*.tmp".into()] };
        let output = scan_directory(root, &filters).expect("scan");
        let paths: Vec<_> = output.entries.iter().map(|e| e.relative_path.as_str()).collect();
        assert!(paths.contains(&"a.txt"));
        assert!(paths.contains(&"sub"));
        assert!(paths.contains(&"sub/b.txt"));
        assert!(!paths.contains(&"skip.tmp"));
        assert_eq!(output.skipped_entries, 0);
        assert!(output.warnings.is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn scan_directory_records_warning_for_unreadable_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        fs::write(root.join("readable.txt"), "ok").expect("write");
        let secret = root.join("secret.txt");
        fs::write(&secret, "hidden").expect("write");
        fs::set_permissions(&secret, fs::Permissions::from_mode(0o000)).expect("chmod");

        let output = scan_directory(root, &Filters::default()).expect("scan");
        assert!(output.skipped_entries >= 1);
        assert!(output.warnings.iter().any(|w| w.contains("secret.txt")));

        fs::set_permissions(&secret, fs::Permissions::from_mode(0o644)).expect("restore");
    }

    #[test]
    fn scan_warning_list_is_capped() {
        let mut warnings = Vec::new();
        let root = Path::new("/tmp/root");
        for i in 0..60 {
            let path = format!("/tmp/root/file{i}.txt");
            push_scan_warning(&mut warnings, root, Some(Path::new(&path)), "walk failed");
        }
        append_overflow_summary(&mut warnings, 60);
        assert_eq!(warnings.len(), MAX_SCAN_WARNINGS + 1);
        assert!(warnings.last().unwrap().contains("10 more skipped"));
    }

    #[test]
    fn destructive_scan_guard_blocks_echo_when_skips_present() {
        let left = ScanOutput {
            entries: vec![],
            skipped_entries: 1,
            warnings: vec!["secret.txt: metadata unavailable".into()],
        };
        let right = ScanOutput { entries: vec![], skipped_entries: 0, warnings: vec![] };
        assert!(assert_destructive_scan_allowed(SyncMode::Echo, &left, &right).is_err());
        assert!(assert_destructive_scan_allowed(SyncMode::Contribute, &left, &right).is_ok());
    }

    #[test]
    fn scan_populates_modified_nanos_from_metadata() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        fs::write(root.join("a.txt"), "hello").expect("write");

        let output = scan_directory(root, &Filters::default()).expect("scan");
        let entry = output.entries.iter().find(|e| e.relative_path == "a.txt").expect("entry");

        let meta = fs::metadata(root.join("a.txt")).expect("metadata");
        let (secs, nanos) = metadata_modified(&meta);
        assert_eq!(entry.modified_secs, secs);
        assert_eq!(entry.modified_nanos, nanos);
    }
}
