use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use uuid::Uuid;

use crate::diff::{apply_conflict_resolutions, build_sync_plan, DiffOptions};
use crate::hashing;
use crate::models::{
    ConflictResolution, FileEntry, FolderPair, RunItem, RunReport, RunStatus, Snapshot, SyncAction,
    SyncPlan,
};
use crate::path_normalization;
use crate::persistence::Database;
use crate::scanner::{assert_destructive_scan_allowed, scan_directory, ScanIntegrity};

#[cfg(test)]
use std::sync::atomic::AtomicUsize;

#[cfg(test)]
static PROGRESS_EMITS: AtomicUsize = AtomicUsize::new(0);
#[cfg(test)]
static MAX_RUN_ITEM_BUFFER: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
pub fn reset_progress_emit_counter() {
    PROGRESS_EMITS.store(0, Ordering::Relaxed);
    MAX_RUN_ITEM_BUFFER.store(0, Ordering::Relaxed);
}

#[cfg(test)]
pub fn progress_emit_count() -> usize {
    PROGRESS_EMITS.load(Ordering::Relaxed)
}

#[cfg(test)]
pub fn max_run_item_buffer() -> usize {
    MAX_RUN_ITEM_BUFFER.load(Ordering::Relaxed)
}

const TEMP_SUFFIX: &str = ".syncforge.tmp";
/// Bounds run-item memory and SQLite transaction duration during large runs.
pub const RUN_ITEM_BATCH_SIZE: usize = 1_000;

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub verify_hashes: bool,
    pub use_recycle_bin: bool,
    pub conflict_resolutions: HashMap<String, ConflictResolution>,
    /// When true, the first non-conflict action failure ends the apply loop (manual default).
    pub stop_on_error: bool,
    /// Hash file contents when metadata matches to catch same-second edits.
    pub content_hash_compare: bool,
    /// Maximum file size (bytes) eligible for content hashing during planning.
    pub content_hash_max_bytes: u64,
    /// Precomputed plan from preview; skips the initial left/right directory scan when set.
    pub plan: Option<SyncPlan>,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            verify_hashes: false,
            use_recycle_bin: true,
            conflict_resolutions: HashMap::new(),
            stop_on_error: true,
            content_hash_compare: true,
            content_hash_max_bytes: 50 * 1024 * 1024,
            plan: None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncProgress {
    pub run_id: String,
    pub pair_id: String,
    pub phase: String,
    pub current: u32,
    pub total: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<RunReport>,
}

/// Borrowed progress fields used before the bridge sink decides to deliver an event.
pub struct ProgressUpdate<'a> {
    pub run_id: &'a str,
    pub pair_id: &'a str,
    pub phase: &'a str,
    pub current: u32,
    pub total: u32,
    pub path: Option<&'a str>,
    pub message: Option<&'a str>,
}

pub enum ProgressEvent<'a> {
    Update(ProgressUpdate<'a>),
    Owned(SyncProgress),
}

pub fn join_relative(base: &Path, relative: &str) -> PathBuf {
    let rel = relative.replace('/', std::path::MAIN_SEPARATOR_STR);
    base.join(rel)
}

pub fn temp_copy_path(dest: &Path) -> PathBuf {
    let name =
        dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    dest.with_file_name(format!("{name}{TEMP_SUFFIX}"))
}

pub fn action_kind(action: &SyncAction) -> &'static str {
    match action {
        SyncAction::CopyLeftToRight { .. } => "copyLeftToRight",
        SyncAction::CopyRightToLeft { .. } => "copyRightToLeft",
        SyncAction::DeleteLeft { .. } => "deleteLeft",
        SyncAction::DeleteRight { .. } => "deleteRight",
        SyncAction::CreateDirLeft { .. } => "createDirLeft",
        SyncAction::CreateDirRight { .. } => "createDirRight",
        SyncAction::Conflict { .. } => "conflict",
        SyncAction::Skip { .. } => "skip",
    }
}

fn now_millis() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn ensure_parent(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    Ok(())
}

/// Move or copy a verified temp file onto `dest` without deleting `dest` first.
///
/// `fs::rename` is preferred (atomic replace on Unix; fast on same volume). It fails
/// when `dest` already exists on Windows, or when `temp` and `dest` are on different
/// volumes. In those cases we `fs::copy` over `dest` so a failed commit leaves the
/// original file intact.
pub(crate) fn commit_temp_file(temp: &Path, dest: &Path) -> io::Result<()> {
    if fs::rename(temp, dest).is_ok() {
        return Ok(());
    }
    fs::copy(temp, dest)?;
    fs::remove_file(temp)?;
    Ok(())
}

pub fn safe_copy_file(src: &Path, dest: &Path, verify: bool) -> io::Result<u64> {
    ensure_parent(dest)?;
    let temp = temp_copy_path(dest);
    if temp.exists() {
        fs::remove_file(&temp)?;
    }
    let bytes = fs::copy(src, &temp)?;
    if verify {
        let src_hash = hashing::hash_file(src)?;
        let dest_hash = hashing::hash_file(&temp)?;
        if src_hash != dest_hash {
            let _ = fs::remove_file(&temp);
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("hash mismatch after copy: {src:?} -> {dest:?}"),
            ));
        }
    }
    commit_temp_file(&temp, dest)?;
    Ok(bytes)
}

pub fn delete_path(path: &Path, use_recycle_bin: bool) -> io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_dir() {
        if use_recycle_bin {
            trash::delete(path).map_err(|e| io::Error::other(e.to_string()))?;
        } else {
            fs::remove_dir_all(path)?;
        }
    } else if use_recycle_bin {
        trash::delete(path).map_err(|e| io::Error::other(e.to_string()))?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

pub fn create_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)
}

fn actions_were_applied(report: &RunReport) -> bool {
    report.files_copied > 0 || report.files_deleted > 0
}

/// Persist a snapshot from fresh scans so the DB matches disk after partial runs.
///
/// Snapshot policy: whenever one or more file operations were applied (including
/// cancel or stop-on-error), we save a post-run snapshot. Runs that never applied
/// anything keep the previous snapshot. Run status may still be `Failed` or
/// `Cancelled` when a snapshot is saved.
fn save_post_run_snapshot(
    db: &Mutex<Database>,
    pair: &FolderPair,
    left_root: &Path,
    right_root: &Path,
) -> Result<(), String> {
    let left_after = scan_directory(left_root, &pair.filters)
        .map_err(|e| format!("post-run scan left failed: {e}"))?;
    let right_after = scan_directory(right_root, &pair.filters)
        .map_err(|e| format!("post-run scan right failed: {e}"))?;
    let snapshot = Snapshot {
        id: Uuid::new_v4().to_string(),
        pair_id: pair.id.clone(),
        captured_at: now_millis(),
        entries: build_snapshot_entries(&left_after.entries, &right_after.entries),
    };
    with_db(db, |db| db.save_snapshot(&snapshot).map_err(|e| e.to_string()))
}

fn build_snapshot_entries(left: &[FileEntry], right: &[FileEntry]) -> Vec<FileEntry> {
    use std::collections::BTreeMap;
    let mut map: BTreeMap<String, FileEntry> = BTreeMap::new();
    for entry in right {
        map.insert(entry.relative_path.clone(), entry.clone());
    }
    for entry in left {
        map.insert(entry.relative_path.clone(), entry.clone());
    }
    map.into_values().collect()
}

fn with_db<T, F>(db: &Mutex<Database>, f: F) -> Result<T, String>
where
    F: FnOnce(&Database) -> Result<T, String>,
{
    let guard = db.lock().map_err(|e| e.to_string())?;
    f(&guard)
}

fn flush_run_items(db: &Mutex<Database>, items: &mut Vec<RunItem>) -> Result<(), String> {
    if items.is_empty() {
        return Ok(());
    }
    with_db(db, |db| {
        db.insert_run_items(items).map_err(|e| {
            format!("history persistence failed while flushing {} run items: {e}", items.len())
        })
    })?;
    items.clear();
    Ok(())
}

pub fn run_pair_impl<F>(
    db: &Mutex<Database>,
    pair: &FolderPair,
    options: RunOptions,
    cancel: &AtomicBool,
    mut emit: F,
) -> Result<RunReport, String>
where
    F: FnMut(ProgressEvent<'_>),
{
    if pair.id.is_empty() {
        return Err("pair id required for run".into());
    }

    let run_id = Uuid::new_v4().to_string();
    let started_at = now_millis();
    let left_root = PathBuf::from(path_normalization::to_long_path(&pair.left_path));
    let right_root = PathBuf::from(path_normalization::to_long_path(&pair.right_path));

    let mut report = RunReport {
        run_id: run_id.clone(),
        pair_id: pair.id.clone(),
        started_at,
        finished_at: None,
        status: RunStatus::Running,
        files_copied: 0,
        files_deleted: 0,
        bytes_transferred: 0,
        errors: vec![],
    };

    with_db(db, |db| db.save_run(&report).map_err(|e| e.to_string()))?;

    let mut counted_emit = |progress: ProgressEvent<'_>| {
        #[cfg(test)]
        PROGRESS_EMITS.fetch_add(1, Ordering::Relaxed);
        emit(progress);
    };

    let result = run_pair_impl_inner(
        db,
        pair,
        options,
        cancel,
        &mut report,
        &mut counted_emit,
        &run_id,
        &left_root,
        &right_root,
    );

    if let Err(ref error) = result {
        if report.finished_at.is_none() {
            finish_failed(db, &mut report, &mut counted_emit, error)?;
        }
    }

    result
}

#[allow(clippy::too_many_arguments)]
fn run_pair_impl_inner<F>(
    db: &Mutex<Database>,
    pair: &FolderPair,
    options: RunOptions,
    cancel: &AtomicBool,
    report: &mut RunReport,
    emit: &mut F,
    run_id: &str,
    left_root: &Path,
    right_root: &Path,
) -> Result<RunReport, String>
where
    F: FnMut(ProgressEvent<'_>),
{
    let RunOptions {
        verify_hashes,
        use_recycle_bin,
        conflict_resolutions,
        stop_on_error,
        content_hash_compare,
        content_hash_max_bytes,
        plan: provided_plan,
    } = options;

    let mut progress =
        |phase: &str, current: u32, total: u32, path: Option<&str>, message: Option<&str>| {
            emit(ProgressEvent::Update(ProgressUpdate {
                run_id,
                pair_id: &pair.id,
                phase,
                current,
                total,
                path,
                message,
            }));
        };

    progress("scanning", 0, 0, None, Some("Scanning folders"));

    let mut run_items: Vec<RunItem> = Vec::new();

    if cancel.load(Ordering::Relaxed) {
        flush_run_items(db, &mut run_items)?;
        return finish_cancelled(db, pair, left_root, right_root, report, emit, 0, 0, None);
    }

    let mut plan = if let Some(plan) = provided_plan {
        if plan.pair_id != pair.id {
            return Err("plan pair id does not match run pair".into());
        }
        plan
    } else {
        let left_scan = scan_directory(left_root, &pair.filters)
            .map_err(|e| format!("scan left failed: {e}"))?;
        let right_scan = scan_directory(right_root, &pair.filters)
            .map_err(|e| format!("scan right failed: {e}"))?;

        assert_destructive_scan_allowed(pair.mode, &left_scan, &right_scan)?;

        let snapshot = with_db(db, |db| db.latest_snapshot(&pair.id).map_err(|e| e.to_string()))?;
        let snapshot_entries = snapshot.as_ref().map(|s| s.entries.as_slice());

        let scan = ScanIntegrity::from_sides(&left_scan, &right_scan);

        build_sync_plan(
            &pair.id,
            pair.mode,
            pair.conflict_policy,
            &left_scan.entries,
            &right_scan.entries,
            snapshot_entries,
            scan,
            DiffOptions {
                left_root: Some(left_root.to_path_buf()),
                right_root: Some(right_root.to_path_buf()),
                content_hash_compare,
                content_hash_max_bytes,
            },
        )
    };

    if plan.requires_attention
        && matches!(pair.mode, crate::models::SyncMode::Echo | crate::models::SyncMode::Synchronize)
    {
        return Err("Cannot run sync: content verification requires attention. Preview again after fixing file access.".into());
    }

    if !conflict_resolutions.is_empty() {
        apply_conflict_resolutions(&mut plan.actions, &conflict_resolutions);
    }

    let total =
        plan.actions.iter().filter(|a| !matches!(a, SyncAction::Skip { .. })).count() as u32;

    progress("running", 0, total, None, Some("Applying sync actions"));

    let mut stopped_on_error = false;
    let mut last_executed_path = None;
    let mut last_current = 0;
    for (index, action) in
        plan.actions.iter().filter(|a| !matches!(a, SyncAction::Skip { .. })).enumerate()
    {
        if cancel.load(Ordering::Relaxed) {
            flush_run_items(db, &mut run_items)?;
            return finish_cancelled(
                db,
                pair,
                left_root,
                right_root,
                report,
                emit,
                index as u32,
                total,
                last_executed_path.map(str::to_owned),
            );
        }

        let current = index as u32 + 1;
        last_current = current;
        let path = action_path(action);
        progress("running", current, total, Some(path), None);

        let item_id = Uuid::new_v4().to_string();
        let kind = action_kind(action);
        last_executed_path = Some(path);
        let result = execute_action(action, left_root, right_root, verify_hashes, use_recycle_bin);

        match result {
            Ok(stats) => {
                report.files_copied += stats.copied;
                report.files_deleted += stats.deleted;
                report.bytes_transferred += stats.bytes;
                let run_item = RunItem {
                    id: item_id,
                    run_id: run_id.to_string(),
                    path: path.to_string(),
                    action: kind.to_string(),
                    status: "completed".into(),
                    message: None,
                    bytes: if stats.bytes > 0 { Some(stats.bytes) } else { None },
                };
                run_items.push(run_item);
                #[cfg(test)]
                MAX_RUN_ITEM_BUFFER.fetch_max(run_items.len(), Ordering::Relaxed);
                if run_items.len() >= RUN_ITEM_BATCH_SIZE {
                    flush_run_items(db, &mut run_items)?;
                }
            }
            Err(e) => {
                let msg = e.to_string();
                report.errors.push(format!("{path}: {msg}"));
                let is_conflict = matches!(action, SyncAction::Conflict { .. });
                let run_item = RunItem {
                    id: item_id,
                    run_id: run_id.to_string(),
                    path: path.to_string(),
                    action: kind.to_string(),
                    status: if is_conflict { "skipped".into() } else { "failed".into() },
                    message: Some(msg),
                    bytes: None,
                };
                run_items.push(run_item);
                #[cfg(test)]
                MAX_RUN_ITEM_BUFFER.fetch_max(run_items.len(), Ordering::Relaxed);
                if run_items.len() >= RUN_ITEM_BATCH_SIZE {
                    flush_run_items(db, &mut run_items)?;
                }
                if !is_conflict && stop_on_error {
                    stopped_on_error = true;
                    break;
                }
            }
        }
    }

    if cancel.load(Ordering::Relaxed) {
        flush_run_items(db, &mut run_items)?;
        return finish_cancelled(
            db,
            pair,
            left_root,
            right_root,
            report,
            emit,
            last_current,
            total,
            last_executed_path.map(str::to_owned),
        );
    }

    flush_run_items(db, &mut run_items)?;

    progress("scanning", total, total, None, Some("Capturing snapshot"));

    save_post_run_snapshot(db, pair, left_root, right_root)?;

    report.status = if report.errors.is_empty() && !stopped_on_error {
        RunStatus::Completed
    } else {
        RunStatus::Failed
    };
    report.finished_at = Some(now_millis());
    with_db(db, |db| db.save_run(report).map_err(|e| e.to_string()))?;
    let final_path = last_executed_path.map(str::to_owned);

    emit(ProgressEvent::Owned(SyncProgress {
        run_id: run_id.to_string(),
        pair_id: pair.id.clone(),
        phase: if report.status == RunStatus::Completed {
            "completed".into()
        } else {
            "failed".into()
        },
        current: last_current,
        total,
        path: final_path,
        message: None,
        report: Some(report.clone()),
    }));

    Ok(report.clone())
}

struct ActionStats {
    copied: u32,
    deleted: u32,
    bytes: u64,
}

fn execute_action(
    action: &SyncAction,
    left_root: &Path,
    right_root: &Path,
    verify_hashes: bool,
    use_recycle_bin: bool,
) -> Result<ActionStats, String> {
    match action {
        SyncAction::CreateDirLeft { path } => {
            create_directory(&join_relative(left_root, path)).map_err(|e| e.to_string())?;
            Ok(ActionStats { copied: 0, deleted: 0, bytes: 0 })
        }
        SyncAction::CreateDirRight { path } => {
            create_directory(&join_relative(right_root, path)).map_err(|e| e.to_string())?;
            Ok(ActionStats { copied: 0, deleted: 0, bytes: 0 })
        }
        SyncAction::CopyLeftToRight { path } => {
            let src = join_relative(left_root, path);
            let dest = join_relative(right_root, path);
            let bytes = safe_copy_file(&src, &dest, verify_hashes).map_err(|e| e.to_string())?;
            Ok(ActionStats { copied: 1, deleted: 0, bytes })
        }
        SyncAction::CopyRightToLeft { path } => {
            let src = join_relative(right_root, path);
            let dest = join_relative(left_root, path);
            let bytes = safe_copy_file(&src, &dest, verify_hashes).map_err(|e| e.to_string())?;
            Ok(ActionStats { copied: 1, deleted: 0, bytes })
        }
        SyncAction::DeleteLeft { path } => {
            delete_path(&join_relative(left_root, path), use_recycle_bin)
                .map_err(|e| e.to_string())?;
            Ok(ActionStats { copied: 0, deleted: 1, bytes: 0 })
        }
        SyncAction::DeleteRight { path } => {
            delete_path(&join_relative(right_root, path), use_recycle_bin)
                .map_err(|e| e.to_string())?;
            Ok(ActionStats { copied: 0, deleted: 1, bytes: 0 })
        }
        SyncAction::Conflict { path, .. } => {
            Err(format!("unresolved conflict at {path} (resolve in a future story)"))
        }
        SyncAction::Skip { path, reason } => Err(format!("skipped {path}: {reason}")),
    }
}

fn action_path(action: &SyncAction) -> &str {
    match action {
        SyncAction::CopyLeftToRight { path }
        | SyncAction::CopyRightToLeft { path }
        | SyncAction::DeleteLeft { path }
        | SyncAction::DeleteRight { path }
        | SyncAction::CreateDirLeft { path }
        | SyncAction::CreateDirRight { path }
        | SyncAction::Conflict { path, .. }
        | SyncAction::Skip { path, .. } => path,
    }
}

fn finish_failed<F>(
    db: &Mutex<Database>,
    report: &mut RunReport,
    emit: &mut F,
    error: &str,
) -> Result<(), String>
where
    F: FnMut(ProgressEvent<'_>),
{
    if !report.errors.iter().any(|e| e == error) {
        report.errors.push(error.to_string());
    }
    report.status = RunStatus::Failed;
    report.finished_at = Some(now_millis());
    with_db(db, |db| db.save_run(report).map_err(|e| e.to_string()))?;
    emit(ProgressEvent::Owned(SyncProgress {
        run_id: report.run_id.clone(),
        pair_id: report.pair_id.clone(),
        phase: "failed".into(),
        current: 0,
        total: 0,
        path: None,
        message: Some(error.to_string()),
        report: Some(report.clone()),
    }));
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn finish_cancelled<F>(
    db: &Mutex<Database>,
    pair: &FolderPair,
    left_root: &Path,
    right_root: &Path,
    report: &mut RunReport,
    emit: &mut F,
    current: u32,
    total: u32,
    path: Option<String>,
) -> Result<RunReport, String>
where
    F: FnMut(ProgressEvent<'_>),
{
    if actions_were_applied(report) {
        save_post_run_snapshot(db, pair, left_root, right_root)?;
    }
    report.status = RunStatus::Cancelled;
    report.finished_at = Some(now_millis());
    with_db(db, |db| db.save_run(report).map_err(|e| e.to_string()))?;
    emit(ProgressEvent::Owned(SyncProgress {
        run_id: report.run_id.clone(),
        pair_id: report.pair_id.clone(),
        phase: "cancelled".into(),
        current,
        total,
        path,
        message: Some("Run cancelled".into()),
        report: Some(report.clone()),
    }));
    Ok(report.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn temp_copy_path_appends_suffix() {
        let dest = PathBuf::from(r"C:\data\file.txt");
        assert_eq!(temp_copy_path(&dest), PathBuf::from(r"C:\data\file.txt.syncforge.tmp"));
    }

    #[test]
    fn join_relative_normalizes_slashes() {
        let base = PathBuf::from("/root");
        let joined = join_relative(&base, "sub/file.txt");
        assert!(joined.to_string_lossy().contains("sub"));
        assert!(joined.extension().is_some_and(|e| e == "txt"));
    }

    #[test]
    fn safe_copy_writes_dest_and_cleans_temp() {
        let dir = TempDir::new().expect("tempdir");
        let src = dir.path().join("src.txt");
        let dest = dir.path().join("dest.txt");
        fs::write(&src, "payload").expect("write");
        let bytes = safe_copy_file(&src, &dest, true).expect("copy");
        assert_eq!(bytes, 7);
        assert!(dest.exists());
        assert!(!temp_copy_path(&dest).exists());
        assert_eq!(fs::read_to_string(&dest).expect("read"), "payload");
    }

    #[test]
    fn safe_copy_without_verify_overwrites_dest() {
        let dir = TempDir::new().expect("tempdir");
        let src = dir.path().join("src.txt");
        let dest = dir.path().join("dest.txt");
        fs::write(&src, "new").expect("write");
        fs::write(&dest, "old").expect("write dest");
        safe_copy_file(&src, &dest, false).expect("copy");
        assert_eq!(fs::read_to_string(&dest).expect("read"), "new");
    }

    /// On Windows, `rename(temp, dest)` fails when `dest` exists or paths are on
    /// different volumes (e.g. `C:\` → `D:\`); we `fs::copy` over `dest` without
    /// unlinking it first so a failed copy leaves the original bytes intact.
    #[test]
    #[cfg(windows)]
    fn commit_temp_file_replaces_existing_via_copy_fallback() {
        let dir = TempDir::new().expect("tempdir");
        let dest = dir.path().join("dest.txt");
        let temp = temp_copy_path(&dest);
        fs::write(&dest, "old").expect("write dest");
        fs::write(&temp, "new").expect("write temp");
        commit_temp_file(&temp, &dest).expect("commit");
        assert_eq!(fs::read_to_string(&dest).expect("read"), "new");
        assert!(!temp.exists());
    }

    #[test]
    #[cfg(all(unix, not(target_os = "linux")))]
    fn commit_temp_file_preserves_dest_when_replace_fails() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().expect("tempdir");
        let dest = dir.path().join("dest.txt");
        let temp = temp_copy_path(&dest);
        fs::write(&dest, "original").expect("write dest");
        fs::write(&temp, "replacement").expect("write temp");
        fs::set_permissions(&dest, fs::Permissions::from_mode(0o000)).expect("chmod dest");

        commit_temp_file(&temp, &dest).expect_err("commit should fail");

        fs::set_permissions(&dest, fs::Permissions::from_mode(0o644)).expect("restore dest");
        assert_eq!(fs::read_to_string(&dest).expect("read dest"), "original");
        assert!(temp.exists());
    }

    #[test]
    #[cfg(windows)]
    fn delete_path_recycle_bin_removes_file() {
        let dir = TempDir::new().expect("tempdir");
        let file = dir.path().join("to-recycle.txt");
        fs::write(&file, "recycle-me").expect("write");
        delete_path(&file, true).expect("recycle delete");
        assert!(!file.exists());
    }

    #[test]
    fn build_snapshot_entries_prefers_left_on_overlap() {
        let left = vec![FileEntry {
            relative_path: "a.txt".into(),
            size: 2,
            modified_secs: 2,
            modified_nanos: 0,
            is_dir: false,
            hash: None,
        }];
        let right = vec![FileEntry {
            relative_path: "a.txt".into(),
            size: 1,
            modified_secs: 1,
            modified_nanos: 0,
            is_dir: false,
            hash: None,
        }];
        let merged = build_snapshot_entries(&left, &right);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].size, 2);
    }

    #[test]
    fn run_pair_impl_skips_initial_scan_when_plan_provided() {
        use crate::commands::preview::preview_pair_impl;
        use crate::scanner::with_scan_counting;

        let data_dir = TempDir::new().expect("tempdir");
        let db = Mutex::new(
            crate::persistence::Database::open(&data_dir.path().join("test.db")).expect("db"),
        );

        let left_dir = data_dir.path().join("left");
        let right_dir = data_dir.path().join("right");
        fs::create_dir_all(&left_dir).expect("mkdir");
        fs::create_dir_all(&right_dir).expect("mkdir");
        fs::write(left_dir.join("only.txt"), "data").expect("write");

        let pair = crate::models::FolderPair {
            id: crate::persistence::new_pair_id(),
            name: "PlanReuse".into(),
            left_path: left_dir.to_string_lossy().into_owned(),
            right_path: right_dir.to_string_lossy().into_owned(),
            mode: crate::models::SyncMode::Echo,
            filters: crate::models::Filters::default(),
            conflict_policy: crate::models::ConflictPolicy::NewerWins,
            enabled: true,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 2,
        };
        db.lock().expect("lock").save_pair(&pair).expect("save pair");

        let cancel = AtomicBool::new(false);

        let (plan, preview_scans) =
            with_scan_counting(|| preview_pair_impl(&pair, None).expect("preview"));
        assert_eq!(preview_scans, 2, "preview should scan left and right once each");

        let (_, run_scans) = with_scan_counting(|| {
            run_pair_impl(
                &db,
                &pair,
                RunOptions { plan: Some(plan), use_recycle_bin: false, ..Default::default() },
                &cancel,
                |_| {},
            )
            .expect("run with plan")
        });
        assert_eq!(run_scans, 2, "run with provided plan should only post-run scan left and right");

        let (_, full_run_scans) = with_scan_counting(|| {
            run_pair_impl(
                &db,
                &pair,
                RunOptions { use_recycle_bin: false, ..Default::default() },
                &cancel,
                |_| {},
            )
            .expect("run without plan")
        });
        assert_eq!(
            full_run_scans, 4,
            "manual run without plan should pre-scan and post-scan both sides"
        );
    }

    #[test]
    fn run_pair_impl_echo_integration() {
        let data_dir = TempDir::new().expect("tempdir");
        let db = Mutex::new(
            crate::persistence::Database::open(&data_dir.path().join("test.db")).expect("db"),
        );

        let left_dir = data_dir.path().join("left");
        let right_dir = data_dir.path().join("right");
        fs::create_dir_all(&left_dir).expect("mkdir");
        fs::create_dir_all(&right_dir).expect("mkdir");
        fs::write(left_dir.join("sync.txt"), "from-left").expect("write");
        fs::write(right_dir.join("orphan.txt"), "remove-me").expect("write");

        let pair = crate::models::FolderPair {
            id: crate::persistence::new_pair_id(),
            name: "Echo".into(),
            left_path: left_dir.to_string_lossy().into_owned(),
            right_path: right_dir.to_string_lossy().into_owned(),
            mode: crate::models::SyncMode::Echo,
            filters: crate::models::Filters::default(),
            conflict_policy: crate::models::ConflictPolicy::NewerWins,
            enabled: true,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 2,
        };
        db.lock().expect("lock").save_pair(&pair).expect("save pair");

        let cancel = AtomicBool::new(false);
        let mut events = Vec::new();
        let report = run_pair_impl(
            &db,
            &pair,
            RunOptions { verify_hashes: true, use_recycle_bin: false, ..Default::default() },
            &cancel,
            |p| match p {
                ProgressEvent::Owned(p) => events.push(p.phase),
                ProgressEvent::Update(p) => events.push(p.phase.to_owned()),
            },
        )
        .expect("run");

        assert_eq!(report.status, RunStatus::Completed);
        assert!(right_dir.join("sync.txt").exists());
        assert!(!right_dir.join("orphan.txt").exists());
        assert!(db.lock().expect("lock").latest_snapshot(&pair.id).expect("snapshot").is_some());
        assert!(events.contains(&"completed".to_string()));
    }

    #[test]
    fn run_pair_impl_finalizes_run_on_scan_failure() {
        let data_dir = TempDir::new().expect("tempdir");
        let db = Mutex::new(
            crate::persistence::Database::open(&data_dir.path().join("test.db")).expect("db"),
        );

        let right_dir = data_dir.path().join("right");
        fs::create_dir_all(&right_dir).expect("mkdir");

        let pair = crate::models::FolderPair {
            id: crate::persistence::new_pair_id(),
            name: "Broken".into(),
            left_path: data_dir.path().join("missing-left").to_string_lossy().into_owned(),
            right_path: right_dir.to_string_lossy().into_owned(),
            mode: crate::models::SyncMode::Echo,
            filters: crate::models::Filters::default(),
            conflict_policy: crate::models::ConflictPolicy::NewerWins,
            enabled: true,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 2,
        };
        db.lock().expect("lock").save_pair(&pair).expect("save pair");

        let cancel = AtomicBool::new(false);
        let mut failed_report: Option<RunReport> = None;
        let err = run_pair_impl(
            &db,
            &pair,
            RunOptions { use_recycle_bin: false, ..Default::default() },
            &cancel,
            |p| {
                if let ProgressEvent::Owned(p) = p {
                    if p.phase == "failed" {
                        failed_report = p.report.clone();
                    }
                }
            },
        )
        .expect_err("scan should fail");

        assert!(err.contains("scan left failed"));
        let report = failed_report.expect("failed progress event");
        assert_eq!(report.status, RunStatus::Failed);
        assert!(report.finished_at.is_some());
    }

    #[test]
    fn run_pair_impl_finalizes_run_on_cancel() {
        let data_dir = TempDir::new().expect("tempdir");
        let db = Mutex::new(
            crate::persistence::Database::open(&data_dir.path().join("test.db")).expect("db"),
        );

        let left_dir = data_dir.path().join("left");
        let right_dir = data_dir.path().join("right");
        fs::create_dir_all(&left_dir).expect("mkdir");
        fs::create_dir_all(&right_dir).expect("mkdir");

        let pair = crate::models::FolderPair {
            id: crate::persistence::new_pair_id(),
            name: "Cancel".into(),
            left_path: left_dir.to_string_lossy().into_owned(),
            right_path: right_dir.to_string_lossy().into_owned(),
            mode: crate::models::SyncMode::Echo,
            filters: crate::models::Filters::default(),
            conflict_policy: crate::models::ConflictPolicy::NewerWins,
            enabled: true,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 2,
        };
        db.lock().expect("lock").save_pair(&pair).expect("save pair");

        let cancel = AtomicBool::new(true);
        let report = run_pair_impl(
            &db,
            &pair,
            RunOptions { use_recycle_bin: false, ..Default::default() },
            &cancel,
            |_| {},
        )
        .expect("cancelled run returns report");

        assert_eq!(report.status, RunStatus::Cancelled);
        assert!(report.finished_at.is_some());
    }

    #[test]
    fn cancel_mid_run_saves_snapshot_matching_disk() {
        let data_dir = TempDir::new().expect("tempdir");
        let db = Mutex::new(
            crate::persistence::Database::open(&data_dir.path().join("test.db")).expect("db"),
        );

        let left_dir = data_dir.path().join("left");
        let right_dir = data_dir.path().join("right");
        fs::create_dir_all(&left_dir).expect("mkdir");
        fs::create_dir_all(&right_dir).expect("mkdir");
        fs::write(left_dir.join("first.txt"), "one").expect("write");
        fs::write(left_dir.join("second.txt"), "two").expect("write");

        let pair = crate::models::FolderPair {
            id: crate::persistence::new_pair_id(),
            name: "CancelMid".into(),
            left_path: left_dir.to_string_lossy().into_owned(),
            right_path: right_dir.to_string_lossy().into_owned(),
            mode: crate::models::SyncMode::Echo,
            filters: crate::models::Filters::default(),
            conflict_policy: crate::models::ConflictPolicy::NewerWins,
            enabled: true,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 2,
        };
        db.lock().expect("lock").save_pair(&pair).expect("save pair");

        let cancel = AtomicBool::new(false);
        let report = run_pair_impl(&db, &pair, RunOptions::default(), &cancel, |p| {
            if let ProgressEvent::Update(p) = p {
                if p.phase == "running" && p.current >= 1 {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
        })
        .expect("cancelled run returns report");

        assert_eq!(report.status, RunStatus::Cancelled);
        assert!(actions_were_applied(&report));

        let snapshot = db
            .lock()
            .expect("lock")
            .latest_snapshot(&pair.id)
            .expect("snapshot")
            .expect("snapshot saved after partial cancel");

        let snapshot_paths: std::collections::HashSet<_> =
            snapshot.entries.iter().map(|e| e.relative_path.as_str()).collect();

        for path in ["first.txt", "second.txt"] {
            let on_left = left_dir.join(path).exists();
            let on_right = right_dir.join(path).exists();
            let in_snapshot = snapshot_paths.contains(path);
            assert_eq!(
                in_snapshot,
                on_left || on_right,
                "snapshot entry for {path} must match disk"
            );
        }
    }

    #[test]
    fn stop_on_error_does_not_apply_remaining_actions() {
        let data_dir = TempDir::new().expect("tempdir");
        let db = Mutex::new(
            crate::persistence::Database::open(&data_dir.path().join("test.db")).expect("db"),
        );

        let left_dir = data_dir.path().join("left");
        let right_dir = data_dir.path().join("right");
        fs::create_dir_all(&left_dir).expect("mkdir");
        fs::create_dir_all(&right_dir).expect("mkdir");
        fs::write(left_dir.join("blocked.txt"), "blocked").expect("write");
        fs::write(left_dir.join("ok.txt"), "ok").expect("write");
        fs::create_dir(right_dir.join("blocked.txt")).expect("block copy target");

        let pair = crate::models::FolderPair {
            id: crate::persistence::new_pair_id(),
            name: "StopOnError".into(),
            left_path: left_dir.to_string_lossy().into_owned(),
            right_path: right_dir.to_string_lossy().into_owned(),
            mode: crate::models::SyncMode::Echo,
            filters: crate::models::Filters::default(),
            conflict_policy: crate::models::ConflictPolicy::NewerWins,
            enabled: true,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 2,
        };
        db.lock().expect("lock").save_pair(&pair).expect("save pair");

        let cancel = AtomicBool::new(false);
        let report = run_pair_impl(
            &db,
            &pair,
            RunOptions { stop_on_error: true, ..Default::default() },
            &cancel,
            |_| {},
        )
        .expect("run returns report");

        assert_eq!(report.status, RunStatus::Failed);
        assert!(!report.errors.is_empty());
        assert!(
            !right_dir.join("ok.txt").exists(),
            "second copy must not run after first non-conflict failure"
        );

        let items = db.lock().expect("lock").list_run_items(&report.run_id).expect("items");
        assert_eq!(items.len(), 1, "only the failing action should be recorded");
    }

    #[test]
    fn large_run_flushes_run_items_in_bounded_batches() {
        let data_dir = TempDir::new().expect("tempdir");
        let db = Mutex::new(
            crate::persistence::Database::open(&data_dir.path().join("test.db")).expect("db"),
        );
        let left_dir = data_dir.path().join("left");
        let right_dir = data_dir.path().join("right");
        fs::create_dir_all(&left_dir).expect("mkdir");
        fs::create_dir_all(&right_dir).expect("mkdir");
        let pair = crate::models::FolderPair {
            id: crate::persistence::new_pair_id(),
            name: "Batching".into(),
            left_path: left_dir.to_string_lossy().into_owned(),
            right_path: right_dir.to_string_lossy().into_owned(),
            mode: crate::models::SyncMode::Echo,
            filters: crate::models::Filters::default(),
            conflict_policy: crate::models::ConflictPolicy::NewerWins,
            enabled: true,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 2,
        };
        db.lock().expect("lock").save_pair(&pair).expect("save pair");
        let action_count = RUN_ITEM_BATCH_SIZE * 2 + RUN_ITEM_BATCH_SIZE / 2 + 1;
        let plan = SyncPlan {
            pair_id: pair.id.clone(),
            actions: (0..action_count)
                .map(|index| SyncAction::CreateDirRight { path: format!("dir-{index}") })
                .collect(),
            scanned_left: 0,
            scanned_right: 0,
            scan_skipped_left: 0,
            scan_skipped_right: 0,
            scan_warnings: vec![],
            requires_attention: false,
        };
        crate::persistence::reset_run_item_batch_counter();
        reset_progress_emit_counter();
        let report = run_pair_impl(
            &db,
            &pair,
            RunOptions { plan: Some(plan), use_recycle_bin: false, ..Default::default() },
            &AtomicBool::new(false),
            |_| {},
        )
        .expect("run");
        let items = db.lock().expect("lock").list_run_items(&report.run_id).expect("items");
        assert_eq!(items.len(), action_count);
        assert_eq!(crate::persistence::run_item_batch_insert_count(), 3);
        assert!(max_run_item_buffer() <= RUN_ITEM_BATCH_SIZE);

        let failing_plan = SyncPlan {
            pair_id: pair.id.clone(),
            actions: vec![SyncAction::CreateDirRight { path: "failure-dir".into() }],
            scanned_left: 0,
            scanned_right: 0,
            scan_skipped_left: 0,
            scan_skipped_right: 0,
            scan_warnings: vec![],
            requires_attention: false,
        };
        crate::persistence::fail_next_run_item_batch();
        let error = run_pair_impl(
            &db,
            &pair,
            RunOptions { plan: Some(failing_plan), use_recycle_bin: false, ..Default::default() },
            &AtomicBool::new(false),
            |_| {},
        )
        .expect_err("injected history batch failure");
        assert!(error.contains("history persistence failed"));
    }

    #[test]
    #[cfg(all(unix, not(target_os = "linux")))]
    fn run_echo_refuses_when_scan_skips_paths() {
        use std::os::unix::fs::PermissionsExt;

        let data_dir = TempDir::new().expect("tempdir");
        let db = Mutex::new(
            crate::persistence::Database::open(&data_dir.path().join("test.db")).expect("db"),
        );

        let left_dir = data_dir.path().join("left");
        let right_dir = data_dir.path().join("right");
        fs::create_dir_all(&left_dir).expect("mkdir");
        fs::create_dir_all(&right_dir).expect("mkdir");
        fs::write(left_dir.join("visible.txt"), "ok").expect("write");
        let secret = left_dir.join("secret.txt");
        fs::write(&secret, "hidden").expect("write");
        fs::set_permissions(&secret, fs::Permissions::from_mode(0o000)).expect("chmod");

        let pair = crate::models::FolderPair {
            id: crate::persistence::new_pair_id(),
            name: "Echo".into(),
            left_path: left_dir.to_string_lossy().into_owned(),
            right_path: right_dir.to_string_lossy().into_owned(),
            mode: crate::models::SyncMode::Echo,
            filters: crate::models::Filters::default(),
            conflict_policy: crate::models::ConflictPolicy::NewerWins,
            enabled: true,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 2,
        };
        db.lock().expect("lock").save_pair(&pair).expect("save pair");

        let cancel = AtomicBool::new(false);
        let err = run_pair_impl(
            &db,
            &pair,
            RunOptions { use_recycle_bin: false, ..Default::default() },
            &cancel,
            |_| {},
        )
        .expect_err("run must fail");

        assert!(err.contains("Cannot run Echo sync"));
        assert!(err.contains("skipped"));

        fs::set_permissions(&secret, fs::Permissions::from_mode(0o644)).expect("restore");
    }
}
