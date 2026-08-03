mod commands;
mod diff;
mod duplicates;
mod engine;
mod hashing;
mod models;
mod notifications;
mod path_normalization;
mod persistence;
mod progress;
mod run_coordinator;
mod scanner;
mod scheduler;
mod state;
mod watcher;

#[cfg(test)]
mod perf_harness;

use std::sync::Arc;

use scheduler::refresh_schedule_service;
use state::AppState;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, Window, WindowEvent,
};
use watcher::refresh_watch_service;

/// When the user minimizes the window, hide it and leave the app in the tray.
fn minimize_to_tray(window: &Window) {
    if window.is_minimized().unwrap_or(false) {
        let _ = window.hide();
        let _ = window.unminimize();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::Builder::new().build())
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            let app_state = Arc::new(
                AppState::new(data_dir).map_err(|e| -> Box<dyn std::error::Error> { e.into() })?,
            );
            app.manage(app_state.clone());

            let show_i = MenuItem::with_id(app, "show", "Show SyncForge", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let tray_menu = Menu::with_items(app, &[&show_i, &quit_i])?;

            let _tray = TrayIconBuilder::with_id("main-tray")
                .icon(app.default_window_icon().ok_or("missing default window icon")?.clone())
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
            refresh_schedule_service(app.handle(), &app_state)?;

            #[cfg(desktop)]
            {
                use tauri_plugin_autostart::ManagerExt;
                let _ = app.handle().autolaunch().enable();
            }

            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                let _ = window.hide();
                api.prevent_close();
            }
            WindowEvent::Focused(false) | WindowEvent::Resized(_) => {
                // Defer so `is_minimized()` is updated (Tao has no dedicated minimize event).
                let window = window.clone();
                let window_for_tray = window.clone();
                let _ = window.run_on_main_thread(move || {
                    minimize_to_tray(&window_for_tray);
                });
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            commands::greet,
            commands::dialog::pick_folder,
            commands::dialog::path_exists,
            commands::dialog::paths_equal,
            commands::duplicates::start_duplicate_scan,
            commands::duplicates::get_duplicate_scan,
            commands::duplicates::resume_duplicate_scan,
            commands::duplicates::cancel_duplicate_scan,
            commands::duplicates::find_duplicates,
            commands::duplicates::remove_duplicates,
            commands::pairs::list_pairs,
            commands::pairs::save_pair,
            commands::pairs::delete_pair,
            commands::preview::preview_pair,
            commands::run::run_pair,
            commands::run::cancel_run,
            commands::history::get_history,
            commands::history::get_run_detail,
            commands::schedule::set_schedule,
            commands::sniffer::scan_folder_sizes,
            commands::sniffer::rename_sniffer_item,
            commands::sniffer::delete_sniffer_item,
            commands::sniffer::show_sniffer_item_properties,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
