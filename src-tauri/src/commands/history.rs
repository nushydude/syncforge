use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use crate::models::{RunItem, RunReport};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunDetail {
    pub report: RunReport,
    pub items: Vec<RunItem>,
}

#[tauri::command]
pub fn get_history(
    pair_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<RunReport>, String> {
    state
        .db
        .lock()
        .map_err(|e| e.to_string())?
        .list_runs(pair_id.as_deref())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_run_detail(
    run_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<Option<RunDetail>, String> {
    let db = state.db.lock().map_err(|e| e.to_string())?;
    let Some(report) = db.get_run(&run_id).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let items = db.list_run_items(&run_id).map_err(|e| e.to_string())?;
    Ok(Some(RunDetail { report, items }))
}
