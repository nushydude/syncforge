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
pub async fn start_sniffer_scan(
    root: String,
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<ScanSnapshot, SnifferError> {
    let state = state.inner().clone();
    blocking("scan", move || state.sniffer.start(&root, app, state.clone())).await
}
async fn blocking<T: Send + 'static>(
    operation: &'static str,
    task: impl FnOnce() -> Result<T, SnifferError> + Send + 'static,
) -> Result<T, SnifferError> {
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|error| SnifferError::new("failed", operation, true, error.to_string()))?
}
#[tauri::command]
pub async fn pin_sniffer_scan(
    scan_id: Option<String>,
    state: State<'_, Arc<AppState>>,
) -> Result<(), SnifferError> {
    let service = state.sniffer.clone();
    blocking("pin", move || service.pin(scan_id)).await
}
#[tauri::command]
pub async fn get_sniffer_scan(
    scan_id: Option<String>,
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<Option<ScanSnapshot>, SnifferError> {
    let service = state.sniffer.clone();
    blocking("getScan", move || {
        service.set_app(app);
        service.get_published(scan_id.as_deref())
    })
    .await
}
#[tauri::command]
pub async fn cancel_sniffer_scan(
    scan_id: String,
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<ScanSnapshot, SnifferError> {
    let service = state.sniffer.clone();
    blocking("cancel", move || service.cancel(&scan_id, &app)).await
}
#[tauri::command]
pub async fn query_sniffer_entries(
    request: QueryRequest,
    state: State<'_, Arc<AppState>>,
) -> Result<EntryPage, SnifferError> {
    let service = state.sniffer.clone();
    blocking("query", move || service.query(request)).await
}
#[tauri::command]
pub async fn get_sniffer_summary(
    scan_id: String,
    generation_id: String,
    directory_id: String,
    request: Option<QueryRequest>,
    state: State<'_, Arc<AppState>>,
) -> Result<Summary, SnifferError> {
    let service = state.sniffer.clone();
    blocking("summary", move || service.summary(&scan_id, &generation_id, &directory_id, request))
        .await
}
#[tauri::command]
pub async fn get_sniffer_node(
    scan_id: String,
    generation_id: String,
    node_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<EntryRow, SnifferError> {
    let service = state.sniffer.clone();
    blocking("node", move || service.node(&scan_id, &generation_id, &node_id)).await
}
#[tauri::command]
pub async fn query_sniffer_issues(
    scan_id: String,
    category: Option<String>,
    cursor: Option<String>,
    limit: u32,
    state: State<'_, Arc<AppState>>,
) -> Result<IssuePage, SnifferError> {
    let service = state.sniffer.clone();
    blocking("issues", move || {
        service.issues(&scan_id, category.as_deref(), cursor.as_deref(), limit)
    })
    .await
}
#[tauri::command]
pub async fn refresh_sniffer_subtree(
    scan_id: String,
    generation_id: String,
    directory_id: String,
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<ScanSnapshot, SnifferError> {
    let state = state.inner().clone();
    blocking("refresh", move || {
        state.sniffer.refresh(&scan_id, &generation_id, &directory_id, app, state.clone())
    })
    .await
}
#[tauri::command]
pub async fn prepare_sniffer_action(
    request: PrepareActionRequest,
    state: State<'_, Arc<AppState>>,
) -> Result<ActionReview, SnifferError> {
    let service = state.sniffer.clone();
    blocking("action", move || service.prepare_action(request)).await
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
