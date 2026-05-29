use tauri::AppHandle;
use tauri_plugin_notification::NotificationExt;

use crate::models::{RunReport, RunStatus};

pub fn notify_sync_report(app: &AppHandle, pair_name: &str, report: &RunReport) {
    let (title, body) = match report.status {
        RunStatus::Completed => (
            format!("Sync complete: {pair_name}"),
            format!(
                "Copied {} file(s), deleted {} file(s).",
                report.files_copied, report.files_deleted
            ),
        ),
        RunStatus::Failed => {
            let detail = report
                .errors
                .first()
                .cloned()
                .unwrap_or_else(|| "See SyncForge for details.".into());
            (format!("Sync failed: {pair_name}"), detail)
        }
        RunStatus::Cancelled => (
            format!("Sync cancelled: {pair_name}"),
            "The sync run was cancelled.".into(),
        ),
        RunStatus::Running => return,
    };

    let _ = app
        .notification()
        .builder()
        .title(title)
        .body(body)
        .show();
}

pub fn notify_sync_error(app: &AppHandle, pair_name: &str, message: &str) {
    let _ = app
        .notification()
        .builder()
        .title(format!("Sync failed: {pair_name}"))
        .body(message)
        .show();
}
