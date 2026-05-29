use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Deserialize;
use tauri::{AppHandle, Emitter, State};

use crate::engine::{run_pair_impl, RunOptions};
use crate::models::{FolderPair, RunReport};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunPairOptions {
    #[serde(default)]
    pub verify_hashes: bool,
    #[serde(default = "default_recycle_bin")]
    pub use_recycle_bin: bool,
}

fn default_recycle_bin() -> bool {
    true
}

#[tauri::command]
pub async fn run_pair(
    pair: FolderPair,
    options: RunPairOptions,
    app: AppHandle,
    state: State<'_, AppState>,
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
    };

    let result = {
        let db = state.db.lock().map_err(|e| e.to_string())?;
        run_pair_impl(
            &db,
            &pair,
            run_options,
            &cancel,
            |progress| {
                let _ = app.emit("sync://progress", &progress);
            },
        )
    };

    {
        let mut guard = state.cancel_flag.lock().map_err(|e| e.to_string())?;
        *guard = None;
    }

    result
}

#[tauri::command]
pub fn cancel_run(state: State<'_, AppState>) -> Result<(), String> {
    let guard = state.cancel_flag.lock().map_err(|e| e.to_string())?;
    if let Some(flag) = guard.as_ref() {
        flag.store(true, Ordering::Relaxed);
        Ok(())
    } else {
        Err("no sync run in progress".into())
    }
}
