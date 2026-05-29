use std::sync::Arc;

use tauri::{AppHandle, State};

use crate::models::FolderPair;
use crate::persistence::{new_pair_id, PersistenceError};
use crate::state::AppState;
use crate::scheduler::refresh_schedule_service;
use crate::watcher::refresh_watch_service;

#[tauri::command]
pub fn list_pairs(state: State<'_, Arc<AppState>>) -> Result<Vec<FolderPair>, String> {
    state
        .db
        .lock()
        .map_err(|e| e.to_string())?
        .list_pairs()
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_pair(
    pair: FolderPair,
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<FolderPair, String> {
    let mut pair = pair;
    if pair.id.trim().is_empty() {
        pair.id = new_pair_id();
    }
    let now = current_millis();
    if pair.created_at == 0 {
        pair.created_at = now;
    }
    pair.updated_at = now;

    let saved = state
        .db
        .lock()
        .map_err(|e| e.to_string())?
        .save_pair(&pair)
        .map_err(|e| e.to_string())?;

    refresh_watch_service(&app, &state)?;
    refresh_schedule_service(&app, &state)?;
    Ok(saved)
}

#[tauri::command]
pub fn delete_pair(
    id: String,
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    state
        .db
        .lock()
        .map_err(|e| e.to_string())?
        .delete_pair(&id)
        .map_err(|e| {
            if matches!(e, PersistenceError::PairNotFound(_)) {
                format!("pair not found: {id}")
            } else {
                e.to_string()
            }
        })?;

    refresh_watch_service(&app, &state)?;
    refresh_schedule_service(&app, &state)?;
    Ok(())
}

fn current_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
