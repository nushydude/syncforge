use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::duplicates::{
    self, DuplicateCleanupResult, DuplicateMatchMode, DuplicateScanJob, DuplicateScanPhase,
    DuplicateScanResult, DuplicateScanStatus,
};
use crate::state::AppState;

const PROGRESS_EVENT: &str = "syncforge://duplicates-progress";
const PROGRESS_INTERVAL: Duration = Duration::from_millis(150);

fn current_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn save_job(state: &Arc<AppState>, job: &DuplicateScanJob) -> Result<(), String> {
    state
        .db
        .lock()
        .map_err(|error| error.to_string())?
        .save_duplicate_scan(job)
        .map_err(|error| error.to_string())
}

fn publish_job(app: &AppHandle, job: &DuplicateScanJob) {
    let _ = app.emit(PROGRESS_EVENT, job);
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
    save_job(state, job)?;
    publish_job(app, job);
    *last_progress = Instant::now();
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
    job: DuplicateScanJob,
) -> Result<DuplicateScanJob, String> {
    let cancel = Arc::new(AtomicBool::new(false));
    state
        .duplicate_scan_cancels
        .lock()
        .map_err(|error| error.to_string())?
        .insert(job.id.clone(), cancel.clone());
    save_job(&state, &job)?;
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

fn run_scan(app: AppHandle, state: Arc<AppState>, job: DuplicateScanJob, cancel: Arc<AtomicBool>) {
    let progress = Arc::new(Mutex::new(ProgressState {
        job,
        last_progress: Instant::now() - PROGRESS_INTERVAL,
        hash_bytes_before_file: 0,
    }));

    {
        let mut current = match progress.lock() {
            Ok(current) => current,
            Err(_) => {
                return;
            }
        };
        if persist_progress(&app, &state, &mut current, true).is_err() {
            remove_cancel_flag(&state, &current.job.id);
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

    match result {
        Ok(result) => {
            finish_job(&app, &state, &mut job, DuplicateScanStatus::Completed, Some(result), None)
        }
        Err(error) if error == "cancelled" || cancel.load(Ordering::Relaxed) => finish_job(
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

    remove_cancel_flag(&state, &job.id);
}

fn finish_job(
    app: &AppHandle,
    state: &Arc<AppState>,
    job: &mut DuplicateScanJob,
    status: DuplicateScanStatus,
    result: Option<DuplicateScanResult>,
    error: Option<String>,
) {
    job.status = status;
    job.phase = None;
    job.cancel_requested = false;
    job.result = result;
    job.error = error;
    job.current_path = None;
    job.updated_at = current_millis();
    let _ = save_job(state, job);
    publish_job(app, job);
}

fn remove_cancel_flag(state: &Arc<AppState>, id: &str) {
    if let Ok(mut flags) = state.duplicate_scan_cancels.lock() {
        flags.remove(id);
    }
}

#[tauri::command]
pub async fn start_duplicate_scan(
    root: String,
    mode: DuplicateMatchMode,
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<DuplicateScanJob, String> {
    let state = state.inner().clone();
    let root = canonical_root(&root)?;
    let current = state
        .db
        .lock()
        .map_err(|error| error.to_string())?
        .latest_duplicate_scan()
        .map_err(|error| error.to_string())?;
    if current.is_some_and(|job| job.status == DuplicateScanStatus::Running) {
        return Err("A duplicate scan is already running.".into());
    }
    launch_scan(app, state, new_job(root, mode))
}

#[tauri::command]
pub fn get_duplicate_scan(
    state: State<'_, Arc<AppState>>,
) -> Result<Option<DuplicateScanJob>, String> {
    state
        .db
        .lock()
        .map_err(|error| error.to_string())?
        .latest_duplicate_scan()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn resume_duplicate_scan(
    id: String,
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<DuplicateScanJob, String> {
    let state = state.inner().clone();
    let mut job = state
        .db
        .lock()
        .map_err(|error| error.to_string())?
        .get_duplicate_scan(&id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "Duplicate scan was not found.".to_string())?;
    if job.status == DuplicateScanStatus::Running {
        return Err("That duplicate scan is already running.".into());
    }
    let root = canonical_root(&job.root)?;
    let now = current_millis();
    job.root = root;
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
    launch_scan(app, state, job)
}

#[tauri::command]
pub fn cancel_duplicate_scan(
    id: String,
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<Option<DuplicateScanJob>, String> {
    let state = state.inner().clone();
    if let Ok(flags) = state.duplicate_scan_cancels.lock() {
        if let Some(flag) = flags.get(&id) {
            flag.store(true, Ordering::Relaxed);
        }
    }
    let mut job = state
        .db
        .lock()
        .map_err(|error| error.to_string())?
        .get_duplicate_scan(&id)
        .map_err(|error| error.to_string())?;
    if let Some(job) = job.as_mut() {
        if job.status == DuplicateScanStatus::Running {
            job.cancel_requested = true;
            job.updated_at = current_millis();
            state
                .db
                .lock()
                .map_err(|error| error.to_string())?
                .save_duplicate_scan(job)
                .map_err(|error| error.to_string())?;
            publish_job(&app, job);
        }
    }
    Ok(job)
}

#[tauri::command]
pub async fn find_duplicates(
    root: String,
    mode: DuplicateMatchMode,
    _state: State<'_, Arc<AppState>>,
) -> Result<DuplicateScanResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        duplicates::find_duplicates(Path::new(&root), mode)
    })
    .await
    .map_err(|error| format!("duplicate scan task failed: {error}"))?
}

#[tauri::command]
pub async fn remove_duplicates(
    root: String,
    paths: Vec<String>,
    _state: State<'_, Arc<AppState>>,
) -> Result<DuplicateCleanupResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        duplicates::remove_duplicates(Path::new(&root), &paths)
    })
    .await
    .map_err(|error| format!("duplicate cleanup task failed: {error}"))?
}
