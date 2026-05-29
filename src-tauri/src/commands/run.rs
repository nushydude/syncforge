use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Deserialize;
use tauri::{AppHandle, Emitter, State};

use crate::engine::{run_pair_impl, RunOptions};
use crate::models::{ConflictResolution, FolderPair, RunReport, RunStatus};
use crate::notifications::notify_sync_report;
use crate::state::AppState;
use crate::watcher::release_sync_slot;

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
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut guard = state.cancel_flag.lock().map_err(|e| e.to_string())?;
        if guard.is_some() {
            return Err("a sync run is already in progress".into());
        }
        *guard = Some(cancel.clone());
    }

    let run_options = RunOptions {
        verify_hashes: options.verify_hashes,
        use_recycle_bin: options.use_recycle_bin,
        conflict_resolutions: options.conflict_resolutions,
        stop_on_error: options.stop_on_error,
    };

    let pair_name = pair.name.clone();
    let db = Arc::clone(&state.db);
    let app_emit = app.clone();
    let cancel_for_run = Arc::clone(&cancel);

    let result = tauri::async_runtime::spawn_blocking(move || {
        run_pair_impl(
            db.as_ref(),
            &pair,
            run_options,
            &cancel_for_run,
            |progress| {
                let _ = app_emit.emit("sync://progress", &progress);
            },
        )
    })
    .await
    .map_err(|e| format!("sync run task failed: {e}"))?;

    release_sync_slot(app.clone(), &state, &cancel);

    if let Ok(ref report) = result {
        if report.status == RunStatus::Completed || report.status == RunStatus::Failed {
            notify_sync_report(&app, &pair_name, report);
        }
    }

    result
}

#[tauri::command]
pub fn cancel_run(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let guard = state.cancel_flag.lock().map_err(|e| e.to_string())?;
    if let Some(flag) = guard.as_ref() {
        flag.store(true, Ordering::Relaxed);
        Ok(())
    } else {
        Err("no sync run in progress".into())
    }
}
