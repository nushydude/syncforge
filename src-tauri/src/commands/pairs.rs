use std::sync::Arc;

use tauri::{AppHandle, State};

use crate::models::FolderPair;
use crate::path_normalization;
use crate::persistence::{new_pair_id, PersistenceError};
use crate::scheduler::refresh_schedule_service;
use crate::state::AppState;
use crate::watcher::refresh_watch_service;

fn validate_pair_paths(left: &str, right: &str) -> Result<(), String> {
    let left = left.trim();
    let right = right.trim();
    if left.is_empty() || right.is_empty() {
        return Ok(());
    }
    if path_normalization::paths_equal(left, right) {
        return Err("left and right folders must be different".into());
    }
    if path_normalization::pair_roots_nested(left, right) {
        return Err(
            "folder pair roots cannot be nested: one path must not be inside the other".into()
        );
    }
    Ok(())
}

#[tauri::command]
pub fn list_pairs(state: State<'_, Arc<AppState>>) -> Result<Vec<FolderPair>, String> {
    state.db.list_pairs().map_err(|e| e.to_string())
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

    validate_pair_paths(&pair.left_path, &pair.right_path)?;

    let saved = state.db.save_pair(&pair).map_err(|e| e.to_string())?;

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
    state.db.delete_pair(&id).map_err(|e| {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_pair_paths_rejects_nested_roots() {
        let err = validate_pair_paths(r"C:\Data\Projects", r"C:\Data").expect_err("nested");
        assert!(err.contains("nested"));
    }

    #[test]
    fn validate_pair_paths_rejects_identical_paths() {
        let err = validate_pair_paths(r"C:\Data", r"C:\Data").expect_err("same");
        assert!(err.contains("different"));
    }

    #[test]
    fn validate_pair_paths_accepts_siblings() {
        validate_pair_paths(r"C:\Data\Left", r"C:\Data\Right").expect("siblings ok");
    }
}
