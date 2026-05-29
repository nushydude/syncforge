use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde::Deserialize;
use tauri::{AppHandle, Emitter, State};

use crate::engine::{run_pair_impl, RunOptions};
use crate::models::{ConflictResolution, FolderPair, RunReport, RunStatus};
use crate::notifications::notify_sync_report;
use crate::run_coordinator::release_sync_slot;
use crate::state::{try_acquire_pair_run, AppState};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunPairOptions {
    #[serde(default)]
    pub verify_hashes: bool,
    #[serde(default = "default_recycle_bin")]
    pub use_recycle_bin: bool,
    #[serde(default)]
    pub conflict_resolutions: HashMap<String, ConflictResolution>,
    #[serde(default = "default_stop_on_error")]
    pub stop_on_error: bool,
}

fn default_stop_on_error() -> bool {
    true
}

fn default_recycle_bin() -> bool {
    true
}

#[tauri::command]
pub async fn run_pair(
    pair: FolderPair,
    options: RunPairOptions,
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<RunReport, String> {
    let pair_id = pair.id.clone();
    if pair_id.is_empty() {
        return Err("pair id required".into());
    }

    let cancel = try_acquire_pair_run(&state, &pair_id)?;

    let run_options = RunOptions {
        verify_hashes: options.verify_hashes,
        use_recycle_bin: options.use_recycle_bin,
        conflict_resolutions: options.conflict_resolutions,
        stop_on_error: options.stop_on_error,
        ..Default::default()
    };

    let pair_name = pair.name.clone();
    let db = Arc::clone(&state.db);
    let app_emit = app.clone();
    let cancel_for_run = Arc::clone(&cancel);
    let state_inner = Arc::clone(&state);

    let result = tauri::async_runtime::spawn_blocking(move || {
        run_pair_impl(db.as_ref(), &pair, run_options, &cancel_for_run, |progress| {
            let _ = app_emit.emit("sync://progress", &progress);
        })
    })
    .await
    .map_err(|e| format!("sync run task failed: {e}"))?;

    release_sync_slot(app.clone(), &state_inner, &pair_id, &cancel);

    if let Ok(ref report) = result {
        if report.status == RunStatus::Completed || report.status == RunStatus::Failed {
            notify_sync_report(&app, &pair_name, report);
        }
    }

    result
}

/// Cancel an in-progress sync. With `pair_id`, cancels only that pair; without, cancels all active runs.
#[tauri::command]
pub fn cancel_run(
    pair_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let guard = state.active_runs.lock().map_err(|e| e.to_string())?;
    match pair_id {
        Some(id) => {
            let Some(flag) = guard.get(&id) else {
                return Err(format!("no sync run in progress for pair {id}"));
            };
            flag.store(true, Ordering::Relaxed);
            Ok(())
        }
        None => {
            if guard.is_empty() {
                return Err("no sync run in progress".into());
            }
            for flag in guard.values() {
                flag.store(true, Ordering::Relaxed);
            }
            Ok(())
        }
    }
}
