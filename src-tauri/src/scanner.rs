use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
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
    #[error("cancelled")]
    Cancelled,
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

/// A spelling absent from a scan must also be absent on the opposite volume.
/// This asks the destination filesystem about its own case rules, including
/// per-directory case sensitivity, without rewriting names or snapshot keys.
#[cfg(test)]
pub fn validate_name_mapping(
    left_root: &Path,
    right_root: &Path,
    left: &[FileEntry],
    right: &[FileEntry],
) -> Result<(), String> {
    validate_name_mapping_cancellable(left_root, right_root, left, right, &NEVER_CANCEL)
}

pub fn validate_name_mapping_cancellable(
    left_root: &Path,
    right_root: &Path,
    left: &[FileEntry],
    right: &[FileEntry],
    cancel: &AtomicBool,
) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("scan cancelled".into());
    }
    #[cfg(windows)]
    {
        validate_prospective_windows_names(left, right_root, cancel)?;
        validate_prospective_windows_names(right, left_root, cancel)?;
    }
    for (entries, other, other_root) in [(left, right, right_root), (right, left, left_root)] {
        for entry in entries {
            if cancel.load(Ordering::Relaxed) {
                return Err("scan cancelled".into());
            }
            if other
                .binary_search_by(|candidate| candidate.relative_path.cmp(&entry.relative_path))
                .is_ok()
            {
                continue;
            }
            let path =
                other_root.join(entry.relative_path.replace('/', std::path::MAIN_SEPARATOR_STR));
            match std::fs::symlink_metadata(&path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
                Ok(_) => return Err(format!("Cannot plan sync: '{}' maps to an existing unscanned or differently spelled entry. Review the names and filters.", entry.relative_path)),
                Err(error) => return Err(format!("Cannot plan sync: cannot safely check name '{}': {error}", entry.relative_path)),
            }
        }
    }
    Ok(())
}

// Windows ordinal comparison reflects the platform's case mapping. A pair of
// distinct incoming spellings may collide even while both targets are absent.
// Until destination directory semantics are known, reject that ambiguity.
#[cfg(windows)]
fn validate_prospective_windows_names(
    entries: &[FileEntry],
    target_root: &Path,
    cancel: &AtomicBool,
) -> Result<(), String> {
    use std::cmp::Ordering;
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn CompareStringOrdinal(
            a: *const u16,
            a_len: i32,
            b: *const u16,
            b_len: i32,
            ignore_case: i32,
        ) -> i32;
    }
    let mut names: Vec<(&str, Vec<u16>)> = entries
        .iter()
        .map(|entry| {
            (
                entry.relative_path.as_str(),
                std::ffi::OsStr::new(&entry.relative_path).encode_wide().collect(),
            )
        })
        .collect();
    let compare = |a: &[u16], b: &[u16]| -> Result<Ordering, String> {
        let a_len = i32::try_from(a.len())
            .map_err(|_| "Cannot safely compare a long file name".to_string())?;
        let b_len = i32::try_from(b.len())
            .map_err(|_| "Cannot safely compare a long file name".to_string())?;
        match unsafe { CompareStringOrdinal(a.as_ptr(), a_len, b.as_ptr(), b_len, 1) } {
            1 => Ok(Ordering::Less),
            2 => Ok(Ordering::Equal),
            3 => Ok(Ordering::Greater),
            _ => Err("Cannot safely compare file names on this Windows filesystem".into()),
        }
    };
    let mut comparison_failed = false;
    names.sort_by(|a, b| {
        compare(&a.1, &b.1).unwrap_or_else(|_| {
            comparison_failed = true;
            Ordering::Equal
        })
    });
    if comparison_failed {
        return Err("Cannot safely compare file names on this Windows filesystem".into());
    }
    for pair in names.windows(2) {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("scan cancelled".into());
        }
        if pair[0].0 != pair[1].0 && compare(&pair[0].1, &pair[1].1)? == Ordering::Equal {
            let mut parent = target_root.to_path_buf();
            for (a, b) in pair[0].0.split('/').zip(pair[1].0.split('/')) {
                if a != b {
                    break;
                }
                parent.push(a);
            }
            if !windows_directory_case_sensitive(&parent)? {
                return Err(format!("Cannot plan sync: names '{}' and '{}' may map to the same destination. Review the names.", pair[0].0, pair[1].0));
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
fn windows_directory_case_sensitive(path: &Path) -> Result<bool, String> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileCaseSensitiveInfo, GetFileInformationByHandleEx, FILE_FLAG_BACKUP_SEMANTICS,
    };
    let dir = std::fs::OpenOptions::new()
        .access_mode(0)
        .share_mode(7)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .map_err(|e| {
            format!("Cannot establish destination case sensitivity for '{}': {e}", path.display())
        })?;
    let mut flags: u32 = 0;
    let ok = unsafe {
        GetFileInformationByHandleEx(
            dir.as_raw_handle() as _,
            FileCaseSensitiveInfo,
            (&mut flags as *mut u32).cast(),
            std::mem::size_of::<u32>() as u32,
        )
    };
    if ok == 0 {
        return Err(format!(
            "Cannot establish destination case sensitivity for '{}': {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(flags & 1 != 0)
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

/// How often the walk checks the cancel flag — cheap enough to stay responsive.
const CANCEL_CHECK_INTERVAL: usize = 512;

/// Cancel flag for callers that have nothing to cancel with.
#[cfg(test)]
static NEVER_CANCEL: AtomicBool = AtomicBool::new(false);

/// Scans `root` in parallel and returns filtered entries keyed by relative path.
#[cfg(test)]
pub fn scan_directory(root: &Path, filters: &Filters) -> ScanResult {
    scan_directory_cancellable(root, filters, &NEVER_CANCEL)
}

/// How often the walk reports progress — often enough to look alive.
const PROGRESS_REPORT_INTERVAL: usize = 128;

/// Scans with cancellation and aborts with [`ScanError::Cancelled`] when
/// `cancel` is set. Long scans on spinning disks must be stoppable.
pub fn scan_directory_cancellable(
    root: &Path,
    filters: &Filters,
    cancel: &AtomicBool,
) -> ScanResult {
    scan_directory_observed(root, filters, cancel, &mut |_, _| {})
}

/// Cancellable scan that also reports `(entries seen so far, latest path)` as it
/// walks, so a long scan can show that it is making progress.
pub fn scan_directory_observed(
    root: &Path,
    filters: &Filters,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(u64, &str),
) -> ScanResult {
    #[cfg(test)]
    scan_test_hooks::record_scan_invocation();

    if cancel.load(Ordering::Relaxed) {
        return Err(ScanError::Cancelled);
    }

    let root = root.canonicalize().map_err(|_| ScanError::NotFound(root.display().to_string()))?;

    if !root.is_dir() {
        return Err(ScanError::NotFound(root.display().to_string()));
    }

    let include_set = build_glob_set(&filters.include)?;
    let exclude_set = build_glob_set(&filters.exclude)?;

    let mut entries = Vec::new();
    let mut skipped_entries = 0u32;
    let mut warnings = Vec::new();

    let mut visited = 0usize;
    for entry in WalkDir::new(&root).follow_links(false).into_iter() {
        visited += 1;
        if visited.is_multiple_of(CANCEL_CHECK_INTERVAL) && cancel.load(Ordering::Relaxed) {
            return Err(ScanError::Cancelled);
        }
        if visited.is_multiple_of(PROGRESS_REPORT_INTERVAL) {
            let latest = entry
                .as_ref()
                .ok()
                .and_then(|e| e.path().strip_prefix(&root).ok().map(to_relative_string))
                .unwrap_or_default();
            on_progress(visited as u64, &latest);
        }
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
            deleted: false,
        });
    }

    append_overflow_summary(&mut warnings, skipped_entries);
    on_progress(visited as u64, "");
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

    #[cfg(windows)]
    #[test]
    fn prospective_case_aliases_follow_destination_parent_semantics() {
        let root = tempfile::TempDir::new().unwrap();
        let seed = root.path().join("seed");
        std::fs::write(&seed, b"a").unwrap();
        let mut entry = scan_directory(root.path(), &Filters::default()).unwrap().entries.remove(0);
        entry.relative_path = "Name".into();
        let mut other = entry.clone();
        other.relative_path = "name".into();
        let sensitive = windows_directory_case_sensitive(root.path()).unwrap();
        assert_eq!(
            validate_prospective_windows_names(
                &[entry.clone(), other.clone()],
                root.path(),
                &NEVER_CANCEL
            )
            .is_ok(),
            sensitive
        );
        assert_eq!(
            validate_prospective_windows_names(&[other, entry], root.path(), &NEVER_CANCEL).is_ok(),
            sensitive
        );
    }

    #[cfg(unix)]
    #[test]
    fn name_mapping_rejects_dangling_symlink_without_modifying_it() {
        use std::os::unix::fs::symlink;
        let root = tempfile::TempDir::new().unwrap();
        let left = root.path().join("left");
        let right = root.path().join("right");
        std::fs::create_dir_all(&left).unwrap();
        std::fs::create_dir_all(&right).unwrap();
        std::fs::write(left.join("name"), b"source").unwrap();
        let link = right.join("name");
        symlink("missing-target", &link).unwrap();
        let entries = scan_directory(&left, &Filters::default()).unwrap().entries;
        assert!(validate_name_mapping(&left, &right, &entries, &[]).is_err());
        assert_eq!(std::fs::read_link(&link).unwrap(), std::path::PathBuf::from("missing-target"));
    }

    #[test]
    fn name_mapping_uses_destination_filesystem_rules() {
        let root = tempfile::TempDir::new().unwrap();
        let left = root.path().join("left");
        let right = root.path().join("right");
        std::fs::create_dir_all(&left).unwrap();
        std::fs::create_dir_all(&right).unwrap();
        std::fs::write(left.join("Name"), b"a").unwrap();
        std::fs::write(right.join("name"), b"b").unwrap();
        let l = scan_directory(&left, &Filters::default()).unwrap();
        let r = scan_directory(&right, &Filters::default()).unwrap();
        let result = validate_name_mapping(&left, &right, &l.entries, &r.entries);
        if right.join("Name").exists() {
            assert!(result.is_err());
        } else {
            assert!(result.is_ok());
        }
    }

    #[test]
    fn empty_include_matches_all_non_excluded() {
        let filters = Filters { include: vec![], exclude: vec!["*.tmp".into()] };
        assert!(matches_filter_rules("any.bin", &filters));
        assert!(!matches_filter_rules("skip.tmp", &filters));
    }

    #[test]
    fn cancelled_scan_stops_before_walking() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        fs::write(root.join("a.txt"), "hello").expect("write");

        let cancel = AtomicBool::new(true);
        let error = scan_directory_cancellable(root, &Filters::default(), &cancel)
            .expect_err("cancelled scan");

        assert!(matches!(error, ScanError::Cancelled));
        assert_eq!(error.to_string(), "cancelled");
    }

    #[test]
    fn uncancelled_scan_still_returns_entries() {
        let dir = TempDir::new().expect("tempdir");
        let root = dir.path();
        fs::write(root.join("a.txt"), "hello").expect("write");

        let cancel = AtomicBool::new(false);
        let output = scan_directory_cancellable(root, &Filters::default(), &cancel).expect("scan");

        assert_eq!(output.entries.len(), 1);
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
    #[cfg(all(unix, not(target_os = "linux")))]
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
