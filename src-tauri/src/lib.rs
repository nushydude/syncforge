mod commands;
mod models;
mod path_normalization;
mod persistence;
mod state;

use state::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            let app_state = AppState::new(data_dir)
                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            app.manage(app_state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::greet,
            commands::pairs::list_pairs,
            commands::pairs::save_pair,
            commands::pairs::delete_pair,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
