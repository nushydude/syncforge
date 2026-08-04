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
    pub next_cursor: Option<usize>,
}

#[tauri::command]
pub async fn get_history(
    pair_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<Vec<RunReport>, String> {
    let db = std::sync::Arc::clone(&state.db);
    tauri::async_runtime::spawn_blocking(move || {
        db.list_runs(pair_id.as_deref()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("history task failed: {e}"))?
}

#[tauri::command]
pub async fn get_run_detail(
    run_id: String,
    cursor: Option<usize>,
    limit: Option<usize>,
    state: State<'_, Arc<AppState>>,
) -> Result<Option<RunDetail>, String> {
    let db = std::sync::Arc::clone(&state.db);
    let cursor = cursor.unwrap_or(0);
    let limit = limit.unwrap_or(crate::persistence::RUN_ITEMS_PAGE_MAX);
    if limit == 0 {
        return Err("history page limit must be greater than zero".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let Some(report) = db.get_run(&run_id).map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let (items, has_more) =
            db.list_run_items_page(&run_id, cursor, limit).map_err(|e| e.to_string())?;
        let next_cursor = has_more.then_some(
            cursor.checked_add(items.len()).ok_or_else(|| "history cursor overflow".to_string())?,
        );
        Ok(Some(RunDetail { report, items, next_cursor }))
    })
    .await
    .map_err(|e| format!("history detail task failed: {e}"))?
}
