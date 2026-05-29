use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;

use crate::path_normalization;

#[tauri::command]
pub async fn pick_folder(app: AppHandle) -> Result<Option<String>, String> {
    let folder = app.dialog().file().blocking_pick_folder();
    Ok(folder.map(|p| path_normalization::normalize_path(&p.to_string())))
}

#[tauri::command]
pub fn path_exists(path: String) -> bool {
    let path = path.trim();
    if path.is_empty() {
        return false;
    }
    let check = path_normalization::to_long_path(path);
    std::path::Path::new(&check).is_dir()
}

#[tauri::command]
pub fn paths_equal(a: String, b: String) -> bool {
    path_normalization::paths_equal(&a, &b)
}
