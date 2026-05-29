use std::sync::Arc;

use tauri::{AppHandle, State};

use crate::models::FolderPair;
use crate::persistence::PersistenceError;
use crate::scheduler::{refresh_schedule_service, validate_cron_expression};
use crate::state::AppState;

#[tauri::command]
pub fn set_schedule(
    pair_id: String,
    enabled: bool,
    cron: Option<String>,
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<FolderPair, String> {
    let cron = cron.map(|value| value.trim().to_string()).filter(|s| !s.is_empty());

    if enabled {
        let expr = cron
            .as_deref()
            .ok_or_else(|| "cron expression is required when schedule is enabled".to_string())?;
        validate_cron_expression(expr)?;
    }

    let mut pair = state
        .db
        .lock()
        .map_err(|e| e.to_string())?
        .get_pair(&pair_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("pair not found: {pair_id}"))?;

    pair.schedule_enabled = enabled;
    pair.schedule_cron = if enabled { cron } else { None };
    pair.updated_at = current_millis();

    let saved = state.db.lock().map_err(|e| e.to_string())?.save_pair(&pair).map_err(|e| {
        if matches!(e, PersistenceError::PairNotFound(_)) {
            format!("pair not found: {pair_id}")
        } else {
            e.to_string()
        }
    })?;

    refresh_schedule_service(&app, &state)?;
    Ok(saved)
}

fn current_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
