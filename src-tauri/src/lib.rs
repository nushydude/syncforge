mod commands;
mod diff;
mod engine;
mod hashing;
mod models;
mod path_normalization;
mod persistence;
mod scanner;
mod state;
mod watcher;

use std::sync::Arc;

use state::AppState;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent,
};
use watcher::refresh_watch_service;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            let app_state = Arc::new(
                AppState::new(data_dir)
                    .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?,
            );
            app.manage(app_state.clone());

            let show_i = MenuItem::with_id(app, "show", "Show SyncForge", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let tray_menu = Menu::with_items(app, &[&show_i, &quit_i])?;

            let _tray = TrayIconBuilder::with_id("main-tray")
                .icon(
                    app.default_window_icon()
                        .ok_or("missing default window icon")?
                        .clone(),
                )
                .menu(&tray_menu)
                .tooltip("SyncForge")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app)?;

            refresh_watch_service(app.handle(), &app_state)?;

            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::greet,
            commands::dialog::pick_folder,
            commands::dialog::path_exists,
            commands::dialog::paths_equal,
            commands::pairs::list_pairs,
            commands::pairs::save_pair,
            commands::pairs::delete_pair,
            commands::preview::preview_pair,
            commands::run::run_pair,
            commands::run::cancel_run,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
