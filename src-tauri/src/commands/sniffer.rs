use std::path::Path;
use std::sync::Arc;

use tauri::State;

use crate::sniffer::{
    ActionResult, ActionReview, EntryPage, EntryRow, IssuePage, PrepareActionRequest, QueryRequest,
    ScanSnapshot, SnifferError, Summary,
};
use crate::state::AppState;
#[cfg(test)]
use crate::state::{HeavyJobKind, HeavyJobPermit, WorkCoordinator, WorkRequest};

#[cfg(test)]
pub(crate) fn admit_sniffer(
    coordinator: &Arc<WorkCoordinator>,
    roots: Vec<std::path::PathBuf>,
    writer: bool,
) -> Result<HeavyJobPermit, String> {
    coordinator.acquire_manual(WorkRequest::new(roots, writer, HeavyJobKind::Sniffer))
}

#[tauri::command]
pub fn start_sniffer_scan(
    root: String,
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<ScanSnapshot, SnifferError> {
    state.sniffer.start(&root, app, state.inner().clone())
}
#[tauri::command]
pub fn get_sniffer_scan(
    scan_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<Option<ScanSnapshot>, SnifferError> {
    state.sniffer.get(scan_id.as_deref())
}
#[tauri::command]
pub fn cancel_sniffer_scan(
    scan_id: String,
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<ScanSnapshot, SnifferError> {
    state.sniffer.cancel(&scan_id, &app)
}
#[tauri::command]
pub fn query_sniffer_entries(
    request: QueryRequest,
    state: State<'_, Arc<AppState>>,
) -> Result<EntryPage, SnifferError> {
    state.sniffer.query(request)
}
#[tauri::command]
pub fn get_sniffer_summary(
    scan_id: String,
    generation_id: String,
    directory_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<Summary, SnifferError> {
    state.sniffer.summary(&scan_id, &generation_id, &directory_id)
}
#[tauri::command]
pub fn get_sniffer_node(
    scan_id: String,
    generation_id: String,
    node_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<EntryRow, SnifferError> {
    state.sniffer.node(&scan_id, &generation_id, &node_id)
}
#[tauri::command]
pub fn query_sniffer_issues(
    scan_id: String,
    category: Option<String>,
    cursor: Option<String>,
    limit: u32,
    state: State<'_, Arc<AppState>>,
) -> Result<IssuePage, SnifferError> {
    state.sniffer.issues(&scan_id, category.as_deref(), cursor.as_deref(), limit)
}
#[tauri::command]
pub fn refresh_sniffer_subtree(
    scan_id: String,
    generation_id: String,
    directory_id: String,
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<ScanSnapshot, SnifferError> {
    state.sniffer.node(&scan_id, &generation_id, &directory_id)?;
    let previous = state
        .sniffer
        .get(Some(&scan_id))?
        .ok_or_else(|| SnifferError::new("expired", "refresh", false, "Scan expired."))?;
    state.sniffer.start(&previous.root, app, state.inner().clone())
}
#[tauri::command]
pub fn prepare_sniffer_action(
    request: PrepareActionRequest,
    state: State<'_, Arc<AppState>>,
) -> Result<ActionReview, SnifferError> {
    state.sniffer.prepare_action(request)
}
#[tauri::command]
pub async fn execute_sniffer_action(
    token: String,
    state: State<'_, Arc<AppState>>,
) -> Result<ActionResult, SnifferError> {
    let service = Arc::clone(&state.sniffer);
    let coordinator = Arc::clone(&state.work_coordinator);
    tauri::async_runtime::spawn_blocking(move || service.execute_action(&token, &coordinator))
        .await
        .map_err(|error| SnifferError::new("failed", "action", true, error.to_string()))?
}
#[tauri::command]
pub fn show_sniffer_item_properties(
    scan_id: String,
    node_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<(), SnifferError> {
    let path = state.sniffer.native_node_path(&scan_id, &node_id)?;
    show_properties(&path)
}

#[cfg(target_os = "windows")]
fn show_properties(path: &Path) -> Result<(), SnifferError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{SHObjectProperties, SHOP_FILEPATH};
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let result = unsafe {
        SHObjectProperties(
            std::ptr::null_mut(),
            SHOP_FILEPATH as u32,
            wide.as_ptr(),
            std::ptr::null(),
        )
    };
    if result == 0 {
        return Err(SnifferError::new(
            "failed",
            "properties",
            true,
            format!("Windows could not open Properties: {}", std::io::Error::last_os_error()),
        ));
    }
    Ok(())
}
#[cfg(not(target_os = "windows"))]
fn show_properties(_path: &Path) -> Result<(), SnifferError> {
    Err(SnifferError::new(
        "unsupported",
        "properties",
        false,
        "File properties are only supported on Windows.",
    ))
}
