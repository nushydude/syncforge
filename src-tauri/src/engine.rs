use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use jwalk::WalkDir;
use uuid::Uuid;

use crate::diff::apply_conflict_resolutions;
use crate::hashing;
use crate::models::{
    ConflictResolution, FileEntry, FolderPair, RunItem, RunReport, RunStatus, Snapshot, SyncAction,
    SyncPlan,
};
use crate::path_normalization;
use crate::persistence::DatabaseHandle;
use crate::scanner::{
    assert_destructive_scan_allowed, scan_directory_cancellable, validate_name_mapping_cancellable,
};

#[cfg(test)]
use crate::scanner::scan_directory;
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

#[cfg(test)]
const TEMP_SUFFIX: &str = ".syncforge.tmp";
/// Bounds run-item memory and SQLite transaction duration during large runs.
pub const RUN_ITEM_BATCH_SIZE: usize = 1_000;

#[derive(Debug, Clone)]
pub struct PlanPreconditions {
    pub left: Vec<FileEntry>,
    pub right: Vec<FileEntry>,
}

#[derive(Debug, Clone)]
pub struct PreparedPlan {
    pub plan: SyncPlan,
    pub preconditions: PlanPreconditions,
    pub root_keys: [String; 2],
    pub baseline_generation: Option<String>,
    pub baseline_revision: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub verify_hashes: bool,
    pub use_recycle_bin: bool,
    pub conflict_resolutions: HashMap<String, ConflictResolution>,
    /// Overrides the folder pair's error handling for this run.
    pub stop_on_error: Option<bool>,
    /// Precomputed plan from preview; skips the initial left/right directory scan when set.
    pub prepared: Option<PreparedPlan>,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            verify_hashes: false,
            use_recycle_bin: true,
            conflict_resolutions: HashMap::new(),
            stop_on_error: None,
            prepared: None,
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

#[cfg(test)]
fn temp_copy_path(dest: &Path) -> PathBuf {
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

/// Commit a verified sibling stage without truncating the current destination.
/// A failed rename leaves the destination in place; Windows uses its replace API.
pub(crate) fn commit_temp_file(temp: &Path, dest: &Path) -> io::Result<()> {
    let renamed = fs::rename(temp, dest);
    #[cfg(not(windows))]
    {
        renamed
    }
    #[cfg(windows)]
    renamed.or_else(|_| {
        use std::os::windows::ffi::OsStrExt;
        let temp_wide: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
        let dest_wide: Vec<u16> = dest.as_os_str().encode_wide().chain(Some(0)).collect();
        let replaced = unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                temp_wide.as_ptr(),
                dest_wide.as_ptr(),
                windows_sys::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING
                    | windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
            )
        };
        if replaced != 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    })
}

#[cfg(test)]
pub fn safe_copy_file(src: &Path, dest: &Path, verify: bool) -> io::Result<u64> {
    safe_copy_file_cancellable(src, dest, verify, &AtomicBool::new(false))
}

fn safe_copy_file_cancellable(
    src: &Path,
    dest: &Path,
    verify: bool,
    cancel: &AtomicBool,
) -> io::Result<u64> {
    ensure_parent(dest)?;
    let stage_dir = loop {
        let parent = dest.parent().unwrap_or_else(|| Path::new("."));
        let candidate = parent.join(format!(".syncforge-{}.stage", Uuid::new_v4()));
        match fs::create_dir(&candidate) {
            Ok(()) => break candidate,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let temp = stage_dir.join("payload");
    let result = (|| {
        let bytes = fs::copy(src, &temp)?;
        if verify {
            let src_hash =
                hashing::hash_file_with_progress(src, |_, _| !cancel.load(Ordering::Relaxed))?;
            let dest_hash =
                hashing::hash_file_with_progress(&temp, |_, _| !cancel.load(Ordering::Relaxed))?;
            if src_hash != dest_hash {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("hash mismatch after copy: {src:?} -> {dest:?}"),
                ));
            }
        }
        commit_temp_file(&temp, dest)?;
        Ok(bytes)
    })();
    if result.is_err() {
        #[cfg(windows)]
        if let Ok(metadata) = fs::metadata(&temp) {
            let mut permissions = metadata.permissions();
            // Windows readonly is an attribute, not Unix permission bits.
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            let _ = fs::set_permissions(&temp, permissions);
        }
        let _ = fs::remove_file(&temp);
    }
    // Never recursively remove a stage: anything unexpected is left untouched.
    let _ = fs::remove_dir(&stage_dir);
    result
}

pub fn delete_path(path: &Path, use_recycle_bin: bool) -> io::Result<()> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(error),
        Err(error) => return Err(error),
    };
    if metadata.is_dir() {
        // A plan never proves membership of an entire physical subtree. In particular,
        // filters and files added after scanning must not be removed recursively.
        return Err(io::Error::other(
            "directory removal blocked: contents may include protected or unplanned files",
        ));
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

/// Persist a snapshot from fresh scans after a fully successful run.
///
/// Cancelled, failed, partially applied, and explicitly skipped runs retain the
/// previous baseline so the next run can reconcile unresolved paths safely.
#[allow(clippy::too_many_arguments)]
fn save_post_run_snapshot(
    db: &dyn DatabaseHandle,
    pair: &FolderPair,
    left_root: &Path,
    right_root: &Path,
    root_keys: &[String; 2],
    cancel: &AtomicBool,
) -> Result<Option<Vec<(FileEntry, FileEntry)>>, String> {
    let left_after = scan_directory_cancellable(left_root, &pair.filters, cancel)
        .map_err(|e| format!("post-run scan left failed: {e}"))?;
    let right_after = scan_directory_cancellable(right_root, &pair.filters, cancel)
        .map_err(|e| format!("post-run scan right failed: {e}"))?;
    let mut verified_entries = None;
    if pair.mode == crate::models::SyncMode::Synchronize {
        if left_after.skipped_entries != 0 || right_after.skipped_entries != 0 {
            return Err("Post-run verification incomplete; baseline retained".into());
        }
        let mut left = left_after.entries.clone();
        let mut right = right_after.entries.clone();
        left.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
        right.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
        if left.len() != right.len() {
            return Err("Post-run folders did not converge; baseline retained".into());
        }
        let mut entries = Vec::with_capacity(left.len());
        for (mut l, mut r) in left.into_iter().zip(right) {
            if cancel.load(Ordering::Relaxed) {
                return Err("post-run verification cancelled".into());
            }
            if l.relative_path != r.relative_path || l.is_dir != r.is_dir || l.size != r.size {
                return Err("Post-run folders did not converge; baseline retained".into());
            }
            if !l.is_dir {
                let lh = hashing::hash_file_with_progress(
                    &join_relative(left_root, &l.relative_path),
                    |_, _| !cancel.load(Ordering::Relaxed),
                )
                .map_err(|e| format!("Post-run verification failed: {e}"))?;
                let rh = hashing::hash_file_with_progress(
                    &join_relative(right_root, &r.relative_path),
                    |_, _| !cancel.load(Ordering::Relaxed),
                )
                .map_err(|e| format!("Post-run verification failed: {e}"))?;
                if lh != rh {
                    return Err("Post-run content did not converge; baseline retained".into());
                }
                l.hash = Some(lh);
                r.hash = Some(rh);
            }
            if cancel.load(Ordering::Relaxed) {
                return Err("post-run verification cancelled".into());
            }
            // Re-read both hashes after comparison to detect changes during verification.
            let mut verification_budget = ValidationBudget { cancel: Some(cancel) };
            validate_precondition(
                left_root,
                Some(&l),
                &l.relative_path,
                "post-run left",
                &mut verification_budget,
            )
            .map_err(|e| format!("Post-run verification failed: {e}"))?;
            validate_precondition(
                right_root,
                Some(&r),
                &r.relative_path,
                "post-run right",
                &mut verification_budget,
            )
            .map_err(|e| format!("Post-run verification failed: {e}"))?;
            entries.push((l, r));
        }
        if crate::path_normalization::pair_root_keys(left_root, right_root)? != *root_keys {
            return Err("Post-run roots changed; baseline retained".into());
        }
        if cancel.load(Ordering::Relaxed) {
            return Err("post-run verification cancelled".into());
        }
        verified_entries = Some(entries);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err("post-run verification cancelled".into());
    }
    let snapshot = Snapshot {
        id: Uuid::new_v4().to_string(),
        pair_id: pair.id.clone(),
        captured_at: now_millis(),
        entries: build_snapshot_entries(&left_after.entries, &right_after.entries),
    };
    with_db(db, |db| db.save_snapshot(&snapshot).map_err(|e| e.to_string()))?;
    Ok(verified_entries)
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

fn with_db<T, F>(db: &dyn DatabaseHandle, f: F) -> Result<T, String>
where
    F: FnOnce(&dyn DatabaseHandle) -> Result<T, String>,
{
    f(db)
}

fn flush_run_items(db: &dyn DatabaseHandle, items: &mut Vec<RunItem>) -> Result<(), String> {
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
    db: &dyn DatabaseHandle,
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

    let result = path_normalization::validate_pair_roots(&left_root, &right_root).and_then(|()| {
        run_pair_impl_inner(
            db,
            pair,
            options,
            cancel,
            &mut report,
            &mut counted_emit,
            &run_id,
            &left_root,
            &right_root,
        )
    });

    if matches!(&result, Err(error) if error == "scan cancelled") {
        return finish_cancelled(
            db,
            pair,
            &left_root,
            &right_root,
            &mut report,
            &mut counted_emit,
            0,
            0,
            None,
        );
    }
    if let Err(ref error) = result {
        if report.finished_at.is_none() {
            finish_failed(db, &mut report, &mut counted_emit, error)?;
        }
    }

    result
}

#[allow(clippy::too_many_arguments)]
fn run_pair_impl_inner<F>(
    db: &dyn DatabaseHandle,
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
        prepared,
    } = options;
    let stop_on_error = stop_on_error.unwrap_or(!pair.continue_on_error);

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

    let root_keys = crate::path_normalization::pair_root_keys(left_root, right_root)?;
    let baseline = db.load_sync_baseline(pair, &root_keys).map_err(|e| e.to_string())?;
    let prepared = if let Some(prepared) = prepared {
        if prepared.root_keys != root_keys
            || prepared.baseline_generation != baseline.generation
            || prepared.baseline_revision.as_deref()
                != baseline.baseline.as_ref().map(|b| b.revision.as_str())
            || prepared.plan.pair_id != pair.id
        {
            return Err("Preview needs refreshing: roots or baseline changed".into());
        }
        let left = scan_directory_cancellable(left_root, &pair.filters, cancel).map_err(|e| {
            if matches!(e, crate::scanner::ScanError::Cancelled) {
                "scan cancelled".into()
            } else {
                format!("Preview needs refreshing: scan left failed: {e}")
            }
        })?;
        let right = scan_directory_cancellable(right_root, &pair.filters, cancel).map_err(|e| {
            if matches!(e, crate::scanner::ScanError::Cancelled) {
                "scan cancelled".into()
            } else {
                format!("Preview needs refreshing: scan right failed: {e}")
            }
        })?;
        assert_destructive_scan_allowed(pair.mode, &left, &right)
            .map_err(|e| format!("Preview needs refreshing: {e}"))?;
        validate_name_mapping_cancellable(
            left_root,
            right_root,
            &left.entries,
            &right.entries,
            cancel,
        )
        .map_err(|e| {
            if e == "scan cancelled" {
                e
            } else {
                format!("Preview needs refreshing: {e}")
            }
        })?;
        let check_scope = |observed: &[FileEntry], expected: &[FileEntry]| -> Result<(), String> {
            let mut expected_map = HashMap::new();
            for entry in expected {
                if cancel.load(Ordering::Relaxed) {
                    return Err("scan cancelled".into());
                }
                expected_map.insert(entry.relative_path.as_str(), entry);
            }
            let expected = expected_map;
            if observed.len() != expected.len() {
                return Err("Preview needs refreshing: folder contents changed".into());
            }
            for entry in observed {
                if cancel.load(Ordering::Relaxed) {
                    return Err("scan cancelled".into());
                }
                let Some(old) = expected.get(entry.relative_path.as_str()) else {
                    return Err(format!(
                        "Preview needs refreshing: folder contents changed at {}",
                        entry.relative_path
                    ));
                };
                if entry.is_dir != old.is_dir
                    || entry.size != old.size
                    || entry.modified_secs != old.modified_secs
                    || entry.modified_nanos != old.modified_nanos
                {
                    return Err(format!(
                        "Preview needs refreshing: folder contents changed at {}",
                        entry.relative_path
                    ));
                }
            }
            Ok(())
        };
        check_scope(&left.entries, &prepared.preconditions.left)?;
        check_scope(&right.entries, &prepared.preconditions.right)?;
        prepared
    } else {
        if !conflict_resolutions.is_empty() {
            return Err("Preview needs refreshing: conflict choices require a reviewed plan".into());
        }
        let (plan, left, right) =
            crate::commands::preview::build_fresh_plan(pair, baseline.baseline.as_ref(), cancel)
                .map_err(|e| {
                    if matches!(
                        e.as_str(),
                        "scan left failed: cancelled"
                            | "scan right failed: cancelled"
                            | "scan cancelled"
                    ) {
                        "scan cancelled".into()
                    } else {
                        e
                    }
                })?;
        PreparedPlan {
            plan,
            preconditions: PlanPreconditions { left, right },
            root_keys: root_keys.clone(),
            baseline_generation: baseline.generation.clone(),
            baseline_revision: baseline.baseline.as_ref().map(|b| b.revision.clone()),
        }
    };
    let PreparedPlan { mut plan, preconditions, .. } = prepared;
    // Old cached plans may still contain recursive directory deletions.
    for action in &mut plan.actions {
        if cancel.load(Ordering::Relaxed) {
            return finish_cancelled(db, pair, left_root, right_root, report, emit, 0, 0, None);
        }
        let candidate = match action {
            SyncAction::DeleteLeft { path } => Some((left_root, path.clone())),
            SyncAction::DeleteRight { path } => Some((right_root, path.clone())),
            _ => None,
        };
        if let Some((root, path)) = candidate {
            match fs::symlink_metadata(join_relative(root, &path)) {
                Ok(metadata) if metadata.is_dir() || metadata.file_type().is_symlink() => {
                    return Err(format!("Preview needs refreshing: planned file changed to a directory or link at {path}"));
                }
                Err(error) if error.kind() != io::ErrorKind::NotFound => {
                    return Err(format!("Cannot inspect planned deletion: {error}"))
                }
                _ => {}
            }
        }
    }

    if plan.requires_attention
        && matches!(pair.mode, crate::models::SyncMode::Echo | crate::models::SyncMode::Synchronize)
    {
        return Err("Preview needs refreshing: content verification requires attention. Fix file access and preview again.".into());
    }

    let precondition_index = Some(PreconditionIndex::new_cancellable(&preconditions, cancel)?);
    let mut initial_validation_budget = ValidationBudget { cancel: Some(cancel) };
    if let Some(index) = precondition_index.as_ref() {
        for action in &plan.actions {
            if preview_file_error(&plan, action).is_some() {
                continue;
            }
            match validate_action_precondition(
                action,
                left_root,
                right_root,
                index,
                &mut initial_validation_budget,
            ) {
                Ok(()) => {}
                Err(error) => {
                    if matches!(error, PreconditionError::Cancelled) {
                        return finish_cancelled(
                            db, pair, left_root, right_root, report, emit, 0, 0, None,
                        );
                    }
                    return Err(format!("Preview needs refreshing: {error}"));
                }
            }
        }
    }

    if !conflict_resolutions.is_empty() {
        apply_conflict_resolutions(&mut plan.actions, &conflict_resolutions);
    }

    if plan.actions.iter().any(|action| matches!(action, SyncAction::Conflict { .. })) {
        return Err(
            "Cannot run sync: unresolved conflicts remain. Resolve every conflict and try again."
                .into(),
        );
    }

    let total =
        plan.actions.iter().filter(|a| !matches!(a, SyncAction::Skip { .. })).count() as u32;

    progress("running", 0, total, None, Some("Applying sync actions"));

    for action in &plan.actions {
        if let SyncAction::Skip { path, reason } = action {
            run_items.push(RunItem {
                id: Uuid::new_v4().to_string(),
                run_id: run_id.to_string(),
                path: path.clone(),
                action: "skip".into(),
                status: "skipped".into(),
                message: Some(reason.clone()),
                bytes: None,
            });
            if run_items.len() >= RUN_ITEM_BATCH_SIZE {
                flush_run_items(db, &mut run_items)?;
            }
        }
    }

    let mut stopped_on_error = false;
    let mut last_executed_path = None;
    let mut last_current = 0;
    let mut action_validation_budget = ValidationBudget { cancel: Some(cancel) };
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

        if path_normalization::pair_root_keys(left_root, right_root)
            .map_err(|e| format!("Preview needs refreshing: {e}"))?
            != root_keys
        {
            flush_run_items(db, &mut run_items)?;
            return Err("Preview needs refreshing: sync roots changed during execution".into());
        }
        let current = index as u32 + 1;
        last_current = current;
        let path = action_path(action);
        progress("running", current, total, Some(path), None);

        let item_id = Uuid::new_v4().to_string();
        let kind = action_kind(action);
        last_executed_path = Some(path);
        let result = if let Some(error) = preview_file_error(&plan, action) {
            Err(error.to_owned())
        } else {
            let precondition = precondition_index.as_ref().map_or(Ok(()), |index| {
                validate_action_precondition(
                    action,
                    left_root,
                    right_root,
                    index,
                    &mut action_validation_budget,
                )
            });
            match precondition {
                Ok(()) => execute_action(
                    action,
                    left_root,
                    right_root,
                    verify_hashes,
                    use_recycle_bin,
                    cancel,
                ),
                Err(error) => {
                    flush_run_items(db, &mut run_items)?;
                    if matches!(error, PreconditionError::Cancelled) {
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
                    return Err(format!("Preview needs refreshing: {error}"));
                }
            }
        };

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
                if e == "copy verification cancelled" {
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

    let can_advance_snapshot = report.errors.is_empty()
        && !plan.actions.iter().any(|action| matches!(action, SyncAction::Skip { .. }));
    let mut verified_entries = None;
    if can_advance_snapshot {
        progress("scanning", total, total, None, Some("Capturing snapshot"));
        let snapshot_result =
            save_post_run_snapshot(db, pair, left_root, right_root, &root_keys, cancel);
        if cancel.load(Ordering::Relaxed) {
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
        verified_entries = snapshot_result?;
    }

    let mut terminal = report.clone();
    terminal.status = if stopped_on_error {
        RunStatus::Failed
    } else if report.errors.is_empty() {
        RunStatus::Completed
    } else {
        RunStatus::CompletedWithErrors
    };
    terminal.finished_at = Some(now_millis());
    if cancel.load(Ordering::Relaxed) {
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
    if let (Some(entries), Some(generation)) =
        (verified_entries.as_deref(), baseline.generation.as_deref())
    {
        if path_normalization::pair_root_keys(left_root, right_root)? != root_keys {
            return Err("Post-run roots changed; baseline retained".into());
        }
        if !db
            .save_sync_baseline_and_run(
                pair,
                &root_keys,
                generation,
                baseline.baseline.as_ref().map(|b| b.revision.as_str()),
                entries,
                &terminal,
            )
            .map_err(|e| e.to_string())?
        {
            return Err("Post-run baseline changed; baseline retained".into());
        }
    } else {
        with_db(db, |db| db.save_run(&terminal).map_err(|e| e.to_string()))?;
    }
    *report = terminal;
    let final_path = last_executed_path.map(str::to_owned);

    emit(ProgressEvent::Owned(SyncProgress {
        run_id: run_id.to_string(),
        pair_id: pair.id.clone(),
        phase: match report.status {
            RunStatus::Completed => "completed",
            RunStatus::CompletedWithErrors => "completedWithErrors",
            _ => "failed",
        }
        .into(),
        current: last_current,
        total,
        path: final_path,
        message: None,
        report: Some(report.clone()),
    }));

    Ok(report.clone())
}

fn preview_file_error<'a>(plan: &'a SyncPlan, action: &SyncAction) -> Option<&'a str> {
    let errors = &plan.preview_file_errors;
    if errors.is_empty() {
        return None;
    }
    let path = action_path(action);
    if let Some(error) = errors.get(path) {
        return Some(error);
    }
    if matches!(action, SyncAction::DeleteLeft { .. } | SyncAction::DeleteRight { .. }) {
        let prefix = format!("{path}/");
        return errors
            .range(prefix.clone()..)
            .next()
            .and_then(|(path, error)| path.starts_with(&prefix).then_some(error.as_str()));
    }
    None
}

#[cfg(test)]
fn validate_plan_preconditions(
    plan: &SyncPlan,
    left_root: &Path,
    right_root: &Path,
    preconditions: &PlanPreconditions,
) -> Result<(), String> {
    let index = PreconditionIndex::new(preconditions);
    let mut budget = ValidationBudget::default();
    for action in &plan.actions {
        validate_action_precondition(action, left_root, right_root, &index, &mut budget)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

struct PreconditionIndex<'a> {
    left: HashMap<&'a str, &'a FileEntry>,
    right: HashMap<&'a str, &'a FileEntry>,
    left_ordered: Vec<&'a FileEntry>,
    right_ordered: Vec<&'a FileEntry>,
}

#[derive(Default)]
struct ValidationBudget<'a> {
    cancel: Option<&'a AtomicBool>,
}

impl ValidationBudget<'_> {
    fn check_cancel(&self) -> Result<(), PreconditionError> {
        if self.cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Err(PreconditionError::Cancelled);
        }
        Ok(())
    }
}

impl<'a> PreconditionIndex<'a> {
    #[cfg(test)]
    fn new(preconditions: &'a PlanPreconditions) -> Self {
        Self::new_cancellable(preconditions, &AtomicBool::new(false)).unwrap()
    }

    fn new_cancellable(
        preconditions: &'a PlanPreconditions,
        cancel: &AtomicBool,
    ) -> Result<Self, String> {
        let mut left = HashMap::new();
        let mut right = HashMap::new();
        let mut left_ordered = Vec::with_capacity(preconditions.left.len());
        let mut right_ordered = Vec::with_capacity(preconditions.right.len());
        for (entries, map, ordered) in [
            (&preconditions.left, &mut left, &mut left_ordered),
            (&preconditions.right, &mut right, &mut right_ordered),
        ] {
            for entry in entries {
                if cancel.load(Ordering::Relaxed) {
                    return Err("scan cancelled".into());
                }
                map.insert(entry.relative_path.as_str(), entry);
                ordered.push(entry);
            }
        }
        left_ordered.sort_unstable_by_key(|entry| entry.relative_path.as_str());
        if cancel.load(Ordering::Relaxed) {
            return Err("scan cancelled".into());
        }
        right_ordered.sort_unstable_by_key(|entry| entry.relative_path.as_str());
        if cancel.load(Ordering::Relaxed) {
            return Err("scan cancelled".into());
        }
        Ok(Self { left, right, left_ordered, right_ordered })
    }
}

#[derive(Debug, thiserror::Error)]
enum PreconditionError {
    #[error("validation cancelled")]
    Cancelled,
    #[error("{0}")]
    FileAccess(String),
    #[error("{0}")]
    UnsafePlan(String),
}

fn validate_action_precondition(
    action: &SyncAction,
    left_root: &Path,
    right_root: &Path,
    index: &PreconditionIndex<'_>,
    budget: &mut ValidationBudget<'_>,
) -> Result<(), PreconditionError> {
    budget.check_cancel()?;
    let path = action_path(action);
    match action {
        SyncAction::CopyLeftToRight { .. } => {
            validate_precondition(
                left_root,
                index.left.get(path).copied(),
                path,
                "source",
                budget,
            )?;
            validate_precondition(
                right_root,
                index.right.get(path).copied(),
                path,
                "target",
                budget,
            )?;
        }
        SyncAction::CopyRightToLeft { .. } => {
            validate_precondition(
                right_root,
                index.right.get(path).copied(),
                path,
                "source",
                budget,
            )?;
            validate_precondition(
                left_root,
                index.left.get(path).copied(),
                path,
                "target",
                budget,
            )?;
        }
        SyncAction::DeleteLeft { .. } => {
            let expected = index.left.get(path).copied();
            validate_precondition(left_root, expected, path, "delete target", budget)?;
            validate_precondition(
                right_root,
                index.right.get(path).copied(),
                path,
                "opposite side",
                budget,
            )?;
            if expected.is_some_and(|entry| entry.is_dir) {
                validate_subtree(left_root, &index.left_ordered, path, "delete target", budget)?;
            }
        }
        SyncAction::DeleteRight { .. } => {
            let expected = index.right.get(path).copied();
            validate_precondition(right_root, expected, path, "delete target", budget)?;
            validate_precondition(
                left_root,
                index.left.get(path).copied(),
                path,
                "opposite side",
                budget,
            )?;
            if expected.is_some_and(|entry| entry.is_dir) {
                validate_subtree(right_root, &index.right_ordered, path, "delete target", budget)?;
            }
        }
        SyncAction::CreateDirLeft { .. } => {
            validate_precondition(left_root, None, path, "directory target", budget)?;
        }
        SyncAction::CreateDirRight { .. } => {
            validate_precondition(right_root, None, path, "directory target", budget)?;
        }
        SyncAction::Conflict { .. } => {
            validate_precondition(
                left_root,
                index.left.get(path).copied(),
                path,
                "conflict source",
                budget,
            )?;
            validate_precondition(
                right_root,
                index.right.get(path).copied(),
                path,
                "conflict source",
                budget,
            )?;
        }
        SyncAction::Skip { .. } => {}
    }
    Ok(())
}

fn validate_subtree(
    root: &Path,
    entries: &[&FileEntry],
    directory: &str,
    label: &str,
    budget: &mut ValidationBudget<'_>,
) -> Result<(), PreconditionError> {
    let prefix = format!("{directory}/");
    let subtree_root = join_relative(root, directory);
    for entry_result in WalkDir::new(&subtree_root).follow_links(false).into_iter() {
        budget.check_cancel()?;
        let entry = entry_result.map_err(|error| {
            PreconditionError::FileAccess(format!(
                "Preview validation failed while scanning {directory}: {error}"
            ))
        })?;
        let entry_path = entry.path();
        let Ok(relative) = entry_path.strip_prefix(root) else {
            continue;
        };
        let relative_path = relative.to_string_lossy().replace('\\', "/");
        if relative_path != directory
            && entries
                .binary_search_by(|expected| expected.relative_path.as_str().cmp(&relative_path))
                .is_err()
        {
            return Err(PreconditionError::UnsafePlan(format!(
                "Preview is stale: {label} changed at {relative_path}"
            )));
        }
    }
    let start = entries.partition_point(|entry| entry.relative_path.as_str() < prefix.as_str());
    for expected in entries.iter().skip(start) {
        if !expected.relative_path.starts_with(&prefix) {
            break;
        }
        validate_precondition(root, Some(expected), &expected.relative_path, label, budget)?;
    }
    Ok(())
}

fn validate_precondition(
    root: &Path,
    expected: Option<&FileEntry>,
    relative_path: &str,
    label: &str,
    budget: &mut ValidationBudget<'_>,
) -> Result<(), PreconditionError> {
    budget.check_cancel()?;
    let path = join_relative(root, relative_path);
    let actual = match fs::metadata(&path) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(PreconditionError::FileAccess(format!(
                "Preview validation failed at {relative_path}: {error}"
            )));
        }
    };
    match (expected, actual) {
        (None, None) => Ok(()),
        (None, Some(_)) => Err(PreconditionError::UnsafePlan(format!(
            "Preview is stale: {label} changed at {relative_path}"
        ))),
        (Some(_), None) => Err(PreconditionError::UnsafePlan(format!(
            "Preview is stale: {label} disappeared at {relative_path}"
        ))),
        (Some(expected), Some(actual)) => {
            let (modified_secs, modified_nanos) = crate::scanner::metadata_modified(&actual);
            if expected.is_dir != actual.is_dir()
                || expected.size != if actual.is_file() { actual.len() } else { 0 }
                || expected.modified_secs != modified_secs
                || expected.modified_nanos != modified_nanos
            {
                return Err(PreconditionError::UnsafePlan(format!(
                    "Preview is stale: {label} changed at {relative_path}"
                )));
            }
            // Scans intentionally avoid storing hashes for every entry. Rehash
            // regular files here so same-metadata edits cannot bypass a
            // reusable preview's stale-plan guard.
            if !expected.is_dir && expected.size > 0 {
                let actual_hash = crate::hashing::hash_file_with_progress(&path, |_, _| {
                    budget.cancel.is_none_or(|flag| !flag.load(Ordering::Relaxed))
                })
                .map_err(|error| {
                    if error.kind() == io::ErrorKind::Interrupted
                        && budget.cancel.is_some_and(|flag| flag.load(Ordering::Relaxed))
                    {
                        PreconditionError::Cancelled
                    } else if error.kind() == io::ErrorKind::NotFound {
                        PreconditionError::UnsafePlan(format!(
                            "Preview is stale: {label} disappeared at {relative_path}"
                        ))
                    } else {
                        PreconditionError::FileAccess(format!(
                            "Preview validation failed at {relative_path}: {error}"
                        ))
                    }
                })?;
                if expected.hash.as_deref() != Some(actual_hash.as_str()) {
                    return Err(PreconditionError::UnsafePlan(format!(
                        "Preview is stale: {label} content changed at {relative_path}"
                    )));
                }
            }
            Ok(())
        }
    }
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
    cancel: &AtomicBool,
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
            let bytes =
                safe_copy_file_cancellable(&src, &dest, verify_hashes, cancel).map_err(|e| {
                    if e.kind() == io::ErrorKind::Interrupted && cancel.load(Ordering::Relaxed) {
                        "copy verification cancelled".into()
                    } else {
                        e.to_string()
                    }
                })?;
            Ok(ActionStats { copied: 1, deleted: 0, bytes })
        }
        SyncAction::CopyRightToLeft { path } => {
            let src = join_relative(right_root, path);
            let dest = join_relative(left_root, path);
            let bytes =
                safe_copy_file_cancellable(&src, &dest, verify_hashes, cancel).map_err(|e| {
                    if e.kind() == io::ErrorKind::Interrupted && cancel.load(Ordering::Relaxed) {
                        "copy verification cancelled".into()
                    } else {
                        e.to_string()
                    }
                })?;
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
    db: &dyn DatabaseHandle,
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
    db: &dyn DatabaseHandle,
    _pair: &FolderPair,
    _left_root: &Path,
    _right_root: &Path,
    report: &mut RunReport,
    emit: &mut F,
    current: u32,
    total: u32,
    path: Option<String>,
) -> Result<RunReport, String>
where
    F: FnMut(ProgressEvent<'_>),
{
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
    use std::sync::Mutex;
    use tempfile::TempDir;

    #[test]
    fn cancelled_copy_verification_preserves_destination_and_cleans_stage() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("source");
        let destination = dir.path().join("destination");
        fs::write(&source, b"new").unwrap();
        fs::write(&destination, b"original").unwrap();
        let error = safe_copy_file_cancellable(&source, &destination, true, &AtomicBool::new(true))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(fs::read(destination).unwrap(), b"original");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn cancellation_stops_precondition_validation_and_index_building() {
        let flag = AtomicBool::new(true);
        let preconditions = PlanPreconditions { left: vec![], right: vec![] };
        assert!(PreconditionIndex::new_cancellable(&preconditions, &flag).is_err());
        let mut budget = ValidationBudget { cancel: Some(&flag) };
        assert!(validate_precondition(Path::new("unused"), None, "item", "target", &mut budget)
            .is_err());
    }

    #[test]
    fn staging_preserves_existing_sibling_and_cleans_failed_stage() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("source");
        let dest = dir.path().join("destination");
        let sentinel = temp_copy_path(&dest);
        fs::write(&source, b"new").unwrap();
        fs::write(&dest, b"old").unwrap();
        fs::write(&sentinel, b"sentinel").unwrap();
        safe_copy_file(&source, &dest, true).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"new");
        assert_eq!(fs::read(&sentinel).unwrap(), b"sentinel");
        assert!(safe_copy_file(&dir.path().join("missing"), &dest, true).is_err());
        assert_eq!(fs::read(&dest).unwrap(), b"new");
        assert_eq!(fs::read(&sentinel).unwrap(), b"sentinel");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 3);
    }

    #[test]
    fn failed_commit_preserves_existing_destination_and_cleans_stage() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("source");
        let destination = dir.path().join("existing-directory");
        fs::write(&source, b"new").unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("sentinel"), b"old").unwrap();
        assert!(safe_copy_file(&source, &destination, true).is_err());
        assert_eq!(fs::read(destination.join("sentinel")).unwrap(), b"old");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn planned_directory_removal_never_removes_new_contents() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("tree");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("protected"), b"keep").unwrap();
        assert!(delete_path(&target, false).is_err());
        assert_eq!(fs::read(target.join("protected")).unwrap(), b"keep");
    }

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

    #[test]
    fn stale_preview_precondition_rejects_changed_source_before_apply() {
        let dir = TempDir::new().expect("tempdir");
        let left = dir.path().join("left");
        let right = dir.path().join("right");
        fs::create_dir_all(&left).expect("left");
        fs::create_dir_all(&right).expect("right");
        fs::write(left.join("file.txt"), "new content").expect("write");
        let plan = SyncPlan {
            pair_id: "pair-a".into(),
            actions: vec![SyncAction::CopyLeftToRight { path: "file.txt".into() }],
            scanned_left: 1,
            scanned_right: 0,
            scan_skipped_left: 0,
            scan_skipped_right: 0,
            scan_warnings: vec![],
            requires_attention: false,
            preview_file_errors: Default::default(),
        };
        let expected = FileEntry {
            relative_path: "file.txt".into(),
            size: 3,
            modified_secs: 1,
            modified_nanos: 0,
            is_dir: false,
            hash: None,
            deleted: false,
        };
        let result = validate_plan_preconditions(
            &plan,
            &left,
            &right,
            &PlanPreconditions { left: vec![expected], right: vec![] },
        );
        assert!(result.expect_err("changed source must reject").contains("Preview is stale"));
        assert!(!right.join("file.txt").exists());
    }

    /// Windows replacement preserves the old destination until commit succeeds.
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
    fn delete_preconditions_reject_opposite_reappearance_and_missing_hash() {
        let temp = TempDir::new().unwrap();
        let left = temp.path().join("left");
        let right = temp.path().join("right");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&right).unwrap();
        fs::write(right.join("item"), b"right").unwrap();
        let mut entry =
            scan_directory(&right, &crate::models::Filters::default()).unwrap().entries.remove(0);
        entry.hash = Some(hashing::hash_file(&right.join("item")).unwrap());
        let preconditions = PlanPreconditions { left: vec![], right: vec![entry.clone()] };
        let index = PreconditionIndex::new(&preconditions);
        let action = SyncAction::DeleteRight { path: "item".into() };
        assert!(validate_action_precondition(
            &action,
            &left,
            &right,
            &index,
            &mut ValidationBudget::default()
        )
        .is_ok());
        fs::write(left.join("item"), b"appeared").unwrap();
        assert!(validate_action_precondition(
            &action,
            &left,
            &right,
            &index,
            &mut ValidationBudget::default()
        )
        .is_err());
        fs::remove_file(left.join("item")).unwrap();
        entry.hash = None;
        let missing = PlanPreconditions { left: vec![], right: vec![entry] };
        assert!(validate_action_precondition(
            &action,
            &left,
            &right,
            &PreconditionIndex::new(&missing),
            &mut ValidationBudget::default()
        )
        .is_err());
    }

    #[test]
    fn synchronize_bootstraps_and_propagates_deletions_in_both_directions() {
        let temp = TempDir::new().unwrap();
        let left = temp.path().join("left");
        let right = temp.path().join("right");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&right).unwrap();
        let db =
            Mutex::new(crate::persistence::Database::open(&temp.path().join("test.db")).unwrap());
        let pair = FolderPair {
            id: crate::persistence::new_pair_id(),
            name: "test".into(),
            left_path: left.to_string_lossy().into_owned(),
            right_path: right.to_string_lossy().into_owned(),
            mode: crate::models::SyncMode::Synchronize,
            filters: Default::default(),
            conflict_policy: crate::models::ConflictPolicy::NewerWins,
            enabled: true,
            continue_on_error: false,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 1,
        };
        db.lock().unwrap().save_pair(&pair).unwrap();
        let run = || {
            run_pair_impl(
                &db,
                &pair,
                RunOptions { use_recycle_bin: false, ..Default::default() },
                &AtomicBool::new(false),
                |_| {},
            )
            .unwrap()
        };
        fs::write(left.join("a"), b"content").unwrap();
        assert_eq!(run().files_copied, 1);
        let keys = path_normalization::pair_root_keys(&left, &right).unwrap();
        let before = db.load_sync_baseline(&pair, &keys).unwrap().baseline.unwrap().revision;
        let cancelled = AtomicBool::new(false);
        let report = run_pair_impl(&db, &pair, RunOptions::default(), &cancelled, |event| {
            if let ProgressEvent::Update(update) = event {
                if update.message == Some("Capturing snapshot") {
                    cancelled.store(true, Ordering::Relaxed);
                }
            }
        })
        .unwrap();
        assert_eq!(report.status, RunStatus::Cancelled);
        assert_eq!(db.load_sync_baseline(&pair, &keys).unwrap().baseline.unwrap().revision, before);
        fs::remove_file(left.join("a")).unwrap();
        assert_eq!(run().files_deleted, 1);
        assert!(!right.join("a").exists());
        fs::write(right.join("b"), b"content").unwrap();
        assert_eq!(run().files_copied, 1);
        fs::remove_file(right.join("b")).unwrap();
        assert_eq!(run().files_deleted, 1);
        assert!(!left.join("b").exists());
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
            deleted: false,
        }];
        let right = vec![FileEntry {
            relative_path: "a.txt".into(),
            size: 1,
            modified_secs: 1,
            modified_nanos: 0,
            is_dir: false,
            hash: None,
            deleted: false,
        }];
        let merged = build_snapshot_entries(&left, &right);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].size, 2);
    }

    fn prepared_for_test(
        db: &dyn DatabaseHandle,
        pair: &FolderPair,
        plan: SyncPlan,
        left: Vec<FileEntry>,
        right: Vec<FileEntry>,
    ) -> PreparedPlan {
        let root_keys = path_normalization::pair_root_keys(
            Path::new(&pair.left_path),
            Path::new(&pair.right_path),
        )
        .unwrap();
        let baseline = db.load_sync_baseline(pair, &root_keys).unwrap();
        PreparedPlan {
            plan,
            preconditions: PlanPreconditions { left, right },
            root_keys,
            baseline_generation: baseline.generation,
            baseline_revision: baseline.baseline.map(|b| b.revision),
        }
    }

    #[test]
    fn baseline_cas_failure_finalizes_failed_report() {
        let temp = TempDir::new().unwrap();
        let left = temp.path().join("left");
        let right = temp.path().join("right");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&right).unwrap();
        fs::write(left.join("item"), b"data").unwrap();
        let db =
            Mutex::new(crate::persistence::Database::open(&temp.path().join("test.db")).unwrap());
        let pair = FolderPair {
            id: crate::persistence::new_pair_id(),
            name: "test".into(),
            left_path: left.to_string_lossy().into_owned(),
            right_path: right.to_string_lossy().into_owned(),
            mode: crate::models::SyncMode::Synchronize,
            filters: Default::default(),
            conflict_policy: crate::models::ConflictPolicy::NewerWins,
            enabled: true,
            continue_on_error: false,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 1,
        };
        db.lock().unwrap().save_pair(&pair).unwrap();
        let mut failed = None;
        let error = run_pair_impl(
            &db,
            &pair,
            RunOptions { use_recycle_bin: false, ..Default::default() },
            &AtomicBool::new(false),
            |event| match event {
                ProgressEvent::Update(update) if update.message == Some("Capturing snapshot") => {
                    db.load_sync_baseline(&pair, &["other-left".into(), "other-right".into()])
                        .unwrap();
                }
                ProgressEvent::Owned(progress) if progress.phase == "failed" => {
                    failed = progress.report
                }
                _ => {}
            },
        )
        .unwrap_err();
        assert!(error.contains("baseline changed"));
        let report = failed.expect("failed progress");
        assert_eq!(report.status, RunStatus::Failed);
        assert_eq!(report.files_copied, 1);
        assert_eq!(db.lock().unwrap().get_run(&report.run_id).unwrap().unwrap(), report);
    }

    #[test]
    fn reviewed_plan_rejects_new_scope_before_writing() {
        let temp = TempDir::new().unwrap();
        let left = temp.path().join("left");
        let right = temp.path().join("right");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&right).unwrap();
        fs::write(left.join("original"), b"one").unwrap();
        let db =
            Mutex::new(crate::persistence::Database::open(&temp.path().join("test.db")).unwrap());
        let pair = FolderPair {
            id: crate::persistence::new_pair_id(),
            name: "test".into(),
            left_path: left.to_string_lossy().into_owned(),
            right_path: right.to_string_lossy().into_owned(),
            mode: crate::models::SyncMode::Echo,
            filters: Default::default(),
            conflict_policy: crate::models::ConflictPolicy::NewerWins,
            enabled: true,
            continue_on_error: false,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 1,
        };
        db.lock().unwrap().save_pair(&pair).unwrap();
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let (plan, l, r) = crate::commands::preview::build_preview(&pair, None, &cancel).unwrap();
        let prepared = prepared_for_test(&db, &pair, plan, l, r);
        fs::write(left.join("new"), b"two").unwrap();
        let error = run_pair_impl(
            &db,
            &pair,
            RunOptions { prepared: Some(prepared), ..Default::default() },
            &cancel,
            |_| {},
        )
        .unwrap_err();
        assert!(error.starts_with("Preview needs refreshing:"));
        assert!(!right.join("original").exists());
        assert!(!right.join("new").exists());
    }

    #[test]
    fn run_pair_impl_rechecks_names_when_plan_provided() {
        use crate::commands::preview::build_preview;
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
            continue_on_error: true,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 2,
        };
        db.lock().expect("lock").save_pair(&pair).expect("save pair");

        let cancel = std::sync::Arc::new(AtomicBool::new(false));

        let ((plan, left, right), preview_scans) =
            with_scan_counting(|| build_preview(&pair, None, &cancel).expect("preview"));
        let prepared = prepared_for_test(&db, &pair, plan, left, right);
        assert_eq!(preview_scans, 2, "preview should scan left and right once each");

        let (_, run_scans) = with_scan_counting(|| {
            run_pair_impl(
                &db,
                &pair,
                RunOptions {
                    prepared: Some(prepared),
                    use_recycle_bin: false,
                    ..Default::default()
                },
                &cancel,
                |_| {},
            )
            .expect("run with plan")
        });
        assert_eq!(run_scans, 4, "cached runs recheck names and capture the post-run snapshot");

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
            continue_on_error: true,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 2,
        };
        db.lock().expect("lock").save_pair(&pair).expect("save pair");

        let cancel = std::sync::Arc::new(AtomicBool::new(false));
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
            continue_on_error: true,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 2,
        };
        db.lock().expect("lock").save_pair(&pair).expect("save pair");

        let cancel = std::sync::Arc::new(AtomicBool::new(false));
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

        assert!(err.contains("Invalid folder pair: cannot resolve left folder"));
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
            continue_on_error: true,
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
    fn cancel_mid_run_preserves_previous_snapshot_baseline() {
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
            continue_on_error: true,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 2,
        };
        db.lock().expect("lock").save_pair(&pair).expect("save pair");

        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let report = run_pair_impl(&db, &pair, RunOptions::default(), &cancel, |p| {
            if let ProgressEvent::Update(p) = p {
                if p.phase == "running" && p.current >= 2 {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
        })
        .expect("cancelled run returns report");

        assert_eq!(report.status, RunStatus::Cancelled);
        assert!(report.files_copied > 0 || report.files_deleted > 0);

        let snapshot = db.lock().expect("lock").latest_snapshot(&pair.id).expect("snapshot");
        assert!(snapshot.is_none(), "partial cancellation must not advance the baseline");
    }

    #[test]
    fn stop_on_error_does_not_apply_remaining_actions() {
        run_after_file_error(false, RunStatus::Failed);
    }

    #[test]
    fn continue_on_error_applies_remaining_actions() {
        run_after_file_error(true, RunStatus::CompletedWithErrors);
    }

    fn run_after_file_error(continue_on_error: bool, expected_status: RunStatus) {
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
            name: "File error handling".into(),
            left_path: left_dir.to_string_lossy().into_owned(),
            right_path: right_dir.to_string_lossy().into_owned(),
            mode: crate::models::SyncMode::Echo,
            filters: crate::models::Filters::default(),
            conflict_policy: crate::models::ConflictPolicy::NewerWins,
            enabled: true,
            continue_on_error,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 2,
        };
        db.lock().expect("lock").save_pair(&pair).expect("save pair");

        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let report = run_pair_impl(&db, &pair, RunOptions::default(), &cancel, |_| {})
            .expect("run returns report");

        assert_eq!(report.status, expected_status);
        assert_eq!(report.errors.len(), 1);
        assert!(report.errors[0].starts_with("blocked.txt:"));
        assert_eq!(right_dir.join("ok.txt").exists(), continue_on_error);
        assert!(right_dir.join("blocked.txt").is_dir());
        assert!(!temp_copy_path(&right_dir.join("blocked.txt")).exists());

        let db = db.lock().expect("lock");
        let items = db.list_run_items(&report.run_id).expect("items");
        assert_eq!(items.len(), 1 + usize::from(continue_on_error));
        assert_eq!(items[0].path, "blocked.txt");
        assert_eq!(items[0].status, "failed");
        assert_eq!(db.get_run(&report.run_id).expect("history"), Some(report));
        assert!(db.latest_snapshot(&pair.id).expect("snapshot").is_none());
        assert!(db.last_synced_at_by_pair().expect("last synced").is_empty());
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
            continue_on_error: true,
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
            preview_file_errors: Default::default(),
        };
        let prepared = prepared_for_test(&db, &pair, plan, vec![], vec![]);
        crate::persistence::reset_run_item_batch_counter();
        reset_progress_emit_counter();
        let report = run_pair_impl(
            &db,
            &pair,
            RunOptions { prepared: Some(prepared), use_recycle_bin: false, ..Default::default() },
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
            preview_file_errors: Default::default(),
        };
        let current_right = scan_directory(&right_dir, &pair.filters).unwrap().entries;
        let prepared = prepared_for_test(&db, &pair, failing_plan, vec![], current_right);
        crate::persistence::fail_next_run_item_batch();
        let error = run_pair_impl(
            &db,
            &pair,
            RunOptions { prepared: Some(prepared), use_recycle_bin: false, ..Default::default() },
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
            continue_on_error: true,
            watch_enabled: false,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 2,
        };
        db.lock().expect("lock").save_pair(&pair).expect("save pair");

        let cancel = std::sync::Arc::new(AtomicBool::new(false));
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
