use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::duplicates::{
    self, DuplicateJobSnapshot, DuplicateMatchMode, DuplicateScanJob, DuplicateScanPhase,
    DuplicateScanResult, DuplicateScanStatus,
};
use crate::run_coordinator::retry_pending_syncs;
use crate::state::{
    canonical_job_roots, AppState, HeavyJobKind, HeavyJobPermit, WorkCoordinator, WorkRequest,
};

const PROGRESS_EVENT: &str = "syncforge://duplicates-progress";
const PROGRESS_INTERVAL: Duration = Duration::from_millis(150);

const PERSISTENCE_ERROR_EVENT: &str = "syncforge://duplicates-persistence-error";
const PERSISTENCE_ERROR_MESSAGE: &str =
    "Scan stopped before its results could be saved. Scan again.";
static PENDING_FAILURE: OnceLock<Mutex<Option<(String, u64)>>> = OnceLock::new();
fn pending_failure() -> &'static Mutex<Option<(String, u64)>> {
    PENDING_FAILURE.get_or_init(|| Mutex::new(None))
}
fn worker_key(id: &str, attempt: u64) -> String {
    format!("{id}:{attempt}")
}
fn recover_pending_failure(state: &Arc<AppState>) -> Result<(), String> {
    let mut pending = pending_failure().lock().map_err(|e| e.to_string())?;
    if let Some((id, attempt)) = pending.as_ref() {
        state
            .db
            .interrupt_abandoned_duplicate(id, *attempt)
            .map_err(|_| PERSISTENCE_ERROR_MESSAGE.to_string())?;
        *pending = None;
    }
    Ok(())
}

pub(crate) fn admit_duplicate(
    coordinator: &Arc<WorkCoordinator>,
    roots: Vec<PathBuf>,
) -> Result<HeavyJobPermit, String> {
    coordinator.acquire(WorkRequest::new(roots, false, HeavyJobKind::Duplicate))
}

fn current_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn save_job(state: &Arc<AppState>, job: &mut DuplicateScanJob) -> Result<(), String> {
    state.db.save_duplicate_scan(job).map_err(|error| error.to_string())?;
    if job.result.is_some() {
        state.db.ensure_duplicate_index(&job.id).map_err(|e| e.to_string())?;
    }
    *job = state
        .db
        .get_duplicate_scan(&job.id)
        .map_err(|error| error.to_string())?
        .ok_or("Saved scan disappeared")?;
    Ok(())
}

fn publish_job(app: &AppHandle, job: &DuplicateScanJob) {
    let _ = app.emit(PROGRESS_EVENT, DuplicateJobSnapshot::from_job(job.clone()));
}

fn save_and_publish(
    app: &AppHandle,
    state: &Arc<AppState>,
    job: &mut DuplicateScanJob,
    last_progress: &mut Instant,
    force: bool,
) -> Result<(), String> {
    if !force && last_progress.elapsed() < PROGRESS_INTERVAL {
        return Ok(());
    }
    job.updated_at = current_millis();
    *last_progress = Instant::now();
    save_job(state, job)?;
    publish_job(app, job);
    Ok(())
}

fn canonical_root(root: &str) -> Result<String, String> {
    let path = Path::new(root)
        .canonicalize()
        .map_err(|error| format!("could not open folder: {error}"))?;
    if !path.is_dir() {
        return Err(format!("not a folder: {}", path.display()));
    }
    Ok(path.display().to_string())
}

fn new_job(root: String, mode: DuplicateMatchMode) -> DuplicateScanJob {
    let now = current_millis();
    DuplicateScanJob {
        attempt: 1,
        sequence: 0,
        id: Uuid::new_v4().to_string(),
        root,
        mode,
        status: DuplicateScanStatus::Running,
        phase: Some(DuplicateScanPhase::Collecting),
        files_found: 0,
        total_files: None,
        hashed_files: 0,
        hash_total: None,
        bytes_processed: 0,
        bytes_total: None,
        current_path: None,
        cancel_requested: false,
        result: None,
        error: None,
        started_at: now,
        updated_at: now,
    }
}

fn launch_scan(
    app: AppHandle,
    state: Arc<AppState>,
    mut job: DuplicateScanJob,
) -> Result<DuplicateScanJob, String> {
    let active_root = Path::new(&job.root).to_path_buf();
    {
        let mut jobs = state.active_duplicate_jobs.lock().map_err(|e| e.to_string())?;
        if !jobs.is_empty() {
            return Err("a duplicate operation is already queued".into());
        }
        jobs.insert(active_root);
    }
    let cancel = Arc::new(AtomicBool::new(false));
    match state.duplicate_scan_cancels.lock() {
        Ok(mut flags) => {
            flags.insert(worker_key(&job.id, job.attempt), cancel.clone());
        }
        Err(error) => {
            release_duplicate_job(&state, &job.root);
            return Err(error.to_string());
        }
    }
    if let Err(error) = save_job(&state, &mut job) {
        close_cancellation(&state, &job.id, job.attempt);
        release_duplicate_job(&state, &job.root);
        return Err(error);
    }
    publish_job(&app, &job);

    let worker_job = job.clone();
    tauri::async_runtime::spawn_blocking(move || run_scan(app, state, worker_job, cancel));
    Ok(job)
}

struct ProgressState {
    job: DuplicateScanJob,
    last_progress: Instant,
    hash_bytes_before_file: u64,
}

fn persist_progress(
    app: &AppHandle,
    state: &Arc<AppState>,
    progress: &mut ProgressState,
    force: bool,
) -> Result<(), String> {
    save_and_publish(app, state, &mut progress.job, &mut progress.last_progress, force)
}

struct ScanRegistration {
    app: AppHandle,
    state: Arc<AppState>,
    job: DuplicateScanJob,
    finished: bool,
}
impl Drop for ScanRegistration {
    fn drop(&mut self) {
        if !self.finished {
            finish_job(
                &self.app,
                &self.state,
                &mut self.job,
                DuplicateScanStatus::Failed,
                None,
                Some("The duplicate scan stopped unexpectedly. Scan again.".into()),
            );
        }
        release_duplicate_job(&self.state, &self.job.root);
        retry_pending_syncs(self.app.clone(), &self.state);
    }
}

fn run_scan(
    app: AppHandle,
    state: Arc<AppState>,
    mut job: DuplicateScanJob,
    cancel: Arc<AtomicBool>,
) {
    let mut registration = ScanRegistration {
        app: app.clone(),
        state: Arc::clone(&state),
        job: job.clone(),
        finished: false,
    };
    let roots = canonical_job_roots(&[&job.root]);
    let _permit = match admit_duplicate(&state.work_coordinator, roots) {
        Ok(permit) => permit,
        Err(error) => {
            finish_job(&app, &state, &mut job, DuplicateScanStatus::Failed, None, Some(error));
            registration.finished = true;
            return;
        }
    };
    let progress = Arc::new(Mutex::new(ProgressState {
        job,
        last_progress: Instant::now() - PROGRESS_INTERVAL,
        hash_bytes_before_file: 0,
    }));

    {
        let mut current = match progress.lock() {
            Ok(current) => current,
            Err(_) => return,
        };
        if let Err(error) = persist_progress(&app, &state, &mut current, true) {
            drop(_permit);
            finish_job(
                &app,
                &state,
                &mut current.job,
                DuplicateScanStatus::Failed,
                None,
                Some(error),
            );
            registration.finished = true;
            return;
        }
    }

    let scan_root = progress.lock().map(|current| current.job.root.clone());
    let scan_mode = progress.lock().map(|current| current.job.mode);
    let (scan_root, scan_mode) = match (scan_root, scan_mode) {
        (Ok(scan_root), Ok(scan_mode)) => (scan_root, scan_mode),
        _ => return,
    };

    let file_progress = Arc::clone(&progress);
    let app_for_files = app.clone();
    let state_for_files = Arc::clone(&state);
    let mut on_file = move |file: &duplicates::CandidateFile, count: u64| {
        if let Ok(mut current) = file_progress.lock() {
            current.job.phase = Some(DuplicateScanPhase::Collecting);
            current.job.files_found = count;
            current.job.current_path = Some(file.relative_path.clone());
            let _ = persist_progress(&app_for_files, &state_for_files, &mut current, false);
        }
    };

    let phase_progress = Arc::clone(&progress);
    let app_for_phase = app.clone();
    let state_for_phase = Arc::clone(&state);
    let mut on_phase =
        move |phase: DuplicateScanPhase, total_files: Option<u64>, hash_total: Option<u64>| {
            if let Ok(mut current) = phase_progress.lock() {
                current.job.phase = Some(phase);
                current.job.total_files = total_files;
                current.job.hash_total = hash_total;
                current.job.bytes_total = hash_total;
                current.job.bytes_processed = 0;
                current.hash_bytes_before_file = 0;
                let _ = persist_progress(&app_for_phase, &state_for_phase, &mut current, true);
            }
        };

    let hash_progress = Arc::clone(&progress);
    let app_for_hash = app.clone();
    let state_for_hash = Arc::clone(&state);
    let mut on_hash_progress =
        move |file: &duplicates::CandidateFile, processed: u64, total: u64, complete: bool| {
            if let Ok(mut current) = hash_progress.lock() {
                current.job.phase = Some(DuplicateScanPhase::Hashing);
                current.job.current_path = Some(file.relative_path.clone());
                current.job.bytes_processed = current.hash_bytes_before_file + processed;
                if complete {
                    current.hash_bytes_before_file += total;
                    current.job.hashed_files += 1;
                    current.job.bytes_processed = current.hash_bytes_before_file;
                }
                let _ = persist_progress(&app_for_hash, &state_for_hash, &mut current, false);
            }
        };

    let result = duplicates::find_duplicates_with_progress(
        Path::new(&scan_root),
        scan_mode,
        &cancel,
        &mut on_file,
        &mut on_phase,
        &mut on_hash_progress,
    );
    let mut job = match progress.lock() {
        Ok(current) => current.job.clone(),
        Err(_) => return,
    };

    drop(_permit);
    match result {
        Ok(result) => {
            finish_job(&app, &state, &mut job, DuplicateScanStatus::Completed, Some(result), None)
        }
        Err(error) if error == "cancelled" => finish_job(
            &app,
            &state,
            &mut job,
            DuplicateScanStatus::Cancelled,
            None,
            Some("Scan cancelled.".into()),
        ),
        Err(error) => {
            finish_job(&app, &state, &mut job, DuplicateScanStatus::Failed, None, Some(error))
        }
    }

    registration.finished = true;
}

fn finish_job(
    app: &AppHandle,
    state: &Arc<AppState>,
    job: &mut DuplicateScanJob,
    status: DuplicateScanStatus,
    result: Option<DuplicateScanResult>,
    error: Option<String>,
) {
    // Stop accepting cancellations and honor any that won the same lock first.
    let (status, result, error) = if close_cancellation(state, &job.id, job.attempt) {
        (DuplicateScanStatus::Cancelled, None, Some("Scan cancelled.".into()))
    } else {
        (status, result, error)
    };
    job.status = status;
    job.phase = None;
    job.cancel_requested = false;
    job.result = result;
    job.error = error;
    job.current_path = None;
    job.updated_at = current_millis();
    for _ in 0..3 {
        if let Ok(saved) = state.db.save_duplicate_terminal(job) {
            let _ = app.emit(PROGRESS_EVENT, saved);
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // Retain only compact recovery metadata, never the full failed result.
    if let Ok(mut pending) = pending_failure().lock() {
        *pending = Some((job.id.clone(), job.attempt));
    }
    let _ = app.emit(PERSISTENCE_ERROR_EVENT, serde_json::json!({"jobId":job.id,"attempt":job.attempt,"message":PERSISTENCE_ERROR_MESSAGE}));
}

fn close_cancellation(state: &AppState, id: &str, attempt: u64) -> bool {
    state
        .duplicate_scan_cancels
        .lock()
        .ok()
        .and_then(|mut flags| flags.remove(&worker_key(id, attempt)))
        .is_some_and(|flag| flag.load(Ordering::Relaxed))
}

fn release_duplicate_job(state: &Arc<AppState>, root: &str) {
    if let Ok(mut jobs) = state.active_duplicate_jobs.lock() {
        jobs.remove(Path::new(root));
    }
}

#[tauri::command]
pub async fn start_duplicate_scan(
    root: String,
    mode: DuplicateMatchMode,
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<DuplicateJobSnapshot, String> {
    let state = state.inner().clone();
    recover_pending_failure(&state)?;
    let root = canonical_root(&root)?;
    let current = state.db.latest_duplicate_snapshot().map_err(|error| error.to_string())?;
    if current.is_some_and(|snapshot| snapshot.job.status == DuplicateScanStatus::Running) {
        return Err("A duplicate scan is already running.".into());
    }
    launch_scan(app, state, new_job(root, mode)).map(DuplicateJobSnapshot::from_job)
}

#[tauri::command]
pub async fn get_duplicate_scan(
    state: State<'_, Arc<AppState>>,
) -> Result<Option<DuplicateJobSnapshot>, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        recover_pending_failure(&state)?;
        state.db.latest_duplicate_snapshot().map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| format!("Could not load duplicate scan: {error}"))?
}

#[tauri::command]
pub async fn resume_duplicate_scan(
    id: String,
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<DuplicateJobSnapshot, String> {
    let state = state.inner().clone();
    recover_pending_failure(&state)?;
    let mut job = state
        .db
        .duplicate_snapshot(&id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Duplicate scan was not found.".to_string())?
        .job;
    if job.status == DuplicateScanStatus::Running {
        return Err("That duplicate scan is already running.".into());
    }
    let root = canonical_root(&job.root)?;
    let now = current_millis();
    job.root = root;
    job.attempt += 1;
    job.status = DuplicateScanStatus::Running;
    job.phase = Some(DuplicateScanPhase::Collecting);
    job.files_found = 0;
    job.total_files = None;
    job.hashed_files = 0;
    job.hash_total = None;
    job.bytes_processed = 0;
    job.bytes_total = None;
    job.current_path = None;
    job.cancel_requested = false;
    job.result = None;
    job.error = None;
    job.started_at = now;
    job.updated_at = now;
    launch_scan(app, state, job).map(DuplicateJobSnapshot::from_job)
}

#[tauri::command]
pub fn cancel_duplicate_scan(
    id: String,
    attempt: u64,
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<Option<DuplicateJobSnapshot>, String> {
    let state = state.inner().clone();
    recover_pending_failure(&state)?;
    let changed = request_active_cancel(&state, &id, attempt)?;
    let job = state.db.duplicate_snapshot(&id).map_err(|error| error.to_string())?;
    if changed {
        if let Some(job) = &job {
            let _ = app.emit(PROGRESS_EVENT, job);
        }
    }
    Ok(job)
}

fn request_active_cancel(state: &AppState, id: &str, attempt: u64) -> Result<bool, String> {
    let flags = state.duplicate_scan_cancels.lock().map_err(|e| e.to_string())?;
    let Some(flag) = flags.get(&worker_key(id, attempt)) else {
        return Ok(false);
    };
    let changed = state.db.request_duplicate_cancel(id, attempt).map_err(|e| e.to_string())?;
    if changed {
        flag.store(true, Ordering::Relaxed);
    }
    Ok(changed)
}

#[tauri::command]
pub fn duplicate_groups_page(
    job_id: String,
    revision: String,
    cursor: usize,
    limit: usize,
    state: State<'_, Arc<AppState>>,
) -> Result<duplicates::DuplicateGroupsPage, String> {
    state.db.duplicate_groups_page(&job_id, &revision, cursor, limit).map_err(|e| e.to_string())
}
#[tauri::command]
pub fn duplicate_members_page(
    job_id: String,
    revision: String,
    group_id: String,
    cursor: usize,
    limit: usize,
    state: State<'_, Arc<AppState>>,
) -> Result<duplicates::DuplicateMembersPage, String> {
    state
        .db
        .duplicate_members_page(&job_id, &revision, &group_id, cursor, limit)
        .map_err(|e| e.to_string())
}

struct HandoffRegistration {
    app: AppHandle,
    state: Arc<AppState>,
    root: PathBuf,
}
impl Drop for HandoffRegistration {
    fn drop(&mut self) {
        if let Ok(mut jobs) = self.state.active_duplicate_jobs.lock() {
            jobs.remove(&self.root);
        }
        retry_pending_syncs(self.app.clone(), &self.state);
    }
}

#[tauri::command]
pub async fn reveal_duplicate(
    job_id: String,
    result_revision: String,
    item_id: String,
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let state = state.inner().clone();
    let snapshot = state
        .db
        .duplicate_snapshot(&job_id)
        .map_err(|e| e.to_string())?
        .ok_or("This scan is no longer available. Scan again.")?;
    let job_root = Path::new(&snapshot.job.root).to_path_buf();
    {
        let mut jobs = state.active_duplicate_jobs.lock().map_err(|e| e.to_string())?;
        if !jobs.is_empty() {
            return Err("a duplicate operation is already queued".into());
        }
        jobs.insert(job_root.clone());
    }
    let roots = canonical_job_roots(&[&snapshot.job.root]);
    let registration =
        HandoffRegistration { app: app.clone(), state: Arc::clone(&state), root: job_root };
    tauri::async_runtime::spawn_blocking(move || {
        let registration = registration;
        let operation = (|| {
            let _permit = state.work_coordinator.acquire_manual(WorkRequest::new(
                roots,
                false,
                HeavyJobKind::Duplicate,
            ))?;
            let snapshot = state
                .db
                .duplicate_snapshot(&job_id)
                .map_err(|e| e.to_string())?
                .ok_or("This scan is no longer available. Scan again.")?;
            let summary =
                snapshot.summary.ok_or("This scan is no longer available. Scan again.")?;
            if snapshot.job.status != DuplicateScanStatus::Completed
                || summary.revision != result_revision
                || !summary.cleanup_available
                || summary.root != snapshot.job.root
                || summary.mode != snapshot.job.mode
            {
                return Err(
                    "These scan results changed. Scan again before opening the file.".into()
                );
            }
            let root_identity =
                summary.root_identity.ok_or("This saved scan is too old. Scan again.")?;
            let (group_id, file) = state
                .db
                .duplicate_member_by_id(&job_id, &result_revision, &item_id)
                .map_err(|e| e.to_string())?
                .ok_or("This item is no longer in the scan. Scan again.")?;
            duplicates::handoff_reviewed_with(
                Path::new(&snapshot.job.root),
                &root_identity,
                snapshot.job.mode,
                &file,
                |cursor| {
                    state
                        .db
                        .duplicate_members_page(
                            &job_id,
                            &result_revision,
                            &group_id,
                            cursor,
                            duplicates::DUPLICATE_PAGE_LIMIT,
                        )
                        .map_err(|e| e.to_string())
                },
                |path| tauri_plugin_opener::reveal_item_in_dir(path).map_err(|e| e.to_string()),
            )
        })();
        drop(registration);
        operation
    })
    .await
    .map_err(|e| format!("Could not send the Explorer request: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_is_rejected_after_worker_closes_registration_before_terminal_write() {
        let dir = tempfile::TempDir::new().unwrap();
        let state = AppState::new(dir.path().to_path_buf()).unwrap();
        for cancel_before_cutoff in [false, true] {
            let job = new_job("fixture".into(), DuplicateMatchMode::Hash);
            state.db.save_duplicate_scan(&job).unwrap();
            let cancel = Arc::new(AtomicBool::new(false));
            state
                .duplicate_scan_cancels
                .lock()
                .unwrap()
                .insert(worker_key(&job.id, job.attempt), Arc::clone(&cancel));

            if cancel_before_cutoff {
                assert!(request_active_cancel(&state, &job.id, job.attempt).unwrap());
            }
            assert_eq!(close_cancellation(&state, &job.id, job.attempt), cancel_before_cutoff);
            assert!(!request_active_cancel(&state, &job.id, job.attempt).unwrap());
            let stored = state.db.get_duplicate_scan(&job.id).unwrap().unwrap();
            assert_eq!(stored.status, DuplicateScanStatus::Running);
            assert_eq!(stored.cancel_requested, cancel_before_cutoff);
            assert_eq!(cancel.load(Ordering::Relaxed), cancel_before_cutoff);
        }
    }
}
