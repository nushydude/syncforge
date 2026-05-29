use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tauri::{AppHandle, Emitter};

use crate::commands::preview::preview_pair_impl;
use crate::engine::{run_pair_impl, RunOptions};
use crate::models::{ConflictPolicy, RunStatus, SyncAction};
use crate::notifications::{notify_sync_error, notify_sync_report};
use crate::state::{
    enqueue_pending_schedule_sync, enqueue_pending_watch_sync, release_pair_run_slot,
    try_acquire_pair_run, AppState,
};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchSkippedPayload {
    pub pair_id: String,
    pub reason: String,
}

/// Returns true when the preview plan has nothing to apply (avoids watch feedback loops).
pub fn watch_plan_is_empty(actions: &[SyncAction]) -> bool {
    actions.is_empty()
}

fn plan_has_conflicts(actions: &[SyncAction]) -> bool {
    actions.iter().any(|a| matches!(a, SyncAction::Conflict { .. }))
}

/// Clears the pair's run slot and starts any pending watch or scheduled sync for that pair only.
pub(crate) fn release_sync_slot(
    app: AppHandle,
    state: &Arc<AppState>,
    pair_id: &str,
    slot: &Arc<AtomicBool>,
) {
    let Some((watch_pending, schedule_pending)) = release_pair_run_slot(state, pair_id, slot) else {
        return;
    };
    if watch_pending {
        run_watch_sync(app.clone(), Arc::clone(state), pair_id.to_string());
    }
    if schedule_pending {
        run_scheduled_sync(app, Arc::clone(state), pair_id.to_string());
    }
}

pub(crate) fn run_watch_sync(app: AppHandle, state: Arc<AppState>, pair_id: String) {
    let cancel = match try_acquire_pair_run(&state, &pair_id) {
        Ok(c) => c,
        Err(_) => {
            enqueue_pending_watch_sync(&state, pair_id);
            return;
        }
    };

    let db = Arc::clone(&state.db);
    let app_emit = app.clone();
    let pair_id_for_release = pair_id.clone();

    let _ = std::thread::spawn(move || {
        let run_result = (|| -> Result<(), String> {
            let pair = {
                let guard = db.lock().map_err(|e| e.to_string())?;
                guard
                    .get_pair(&pair_id)
                    .map_err(|e| e.to_string())?
                    .filter(|p| p.enabled && p.watch_enabled)
                    .ok_or_else(|| "pair not found or watch disabled".to_string())?
            };

            let snapshot_entries = {
                let guard = db.lock().map_err(|e| e.to_string())?;
                guard
                    .latest_snapshot(&pair.id)
                    .map_err(|e| e.to_string())?
                    .map(|s| s.entries)
            };
            let plan = preview_pair_impl(&pair, snapshot_entries.as_deref())?;

            if watch_plan_is_empty(&plan.actions) {
                return Ok(());
            }

            if pair.conflict_policy == ConflictPolicy::Ask && plan_has_conflicts(&plan.actions) {
                let _ = app_emit.emit(
                    "sync://watch-skipped",
                    &WatchSkippedPayload {
                        pair_id: pair.id.clone(),
                        reason: "conflicts require manual resolution".into(),
                    },
                );
                return Ok(());
            }

            let options = RunOptions {
                verify_hashes: false,
                use_recycle_bin: true,
                conflict_resolutions: Default::default(),
                stop_on_error: true,
                plan: Some(plan),
                ..Default::default()
            };

            run_pair_impl(db.as_ref(), &pair, options, &cancel, |progress| {
                let _ = app_emit.emit("sync://progress", &progress);
            })?;
            Ok(())
        })();

        release_sync_slot(app_emit.clone(), &state, &pair_id_for_release, &cancel);

        if let Err(e) = run_result {
            let _ = app_emit.emit(
                "sync://watch-skipped",
                &WatchSkippedPayload { pair_id: pair_id_for_release, reason: e },
            );
        }
    });
}

pub(crate) fn run_scheduled_sync(app: AppHandle, state: Arc<AppState>, pair_id: String) {
    let cancel = match try_acquire_pair_run(&state, &pair_id) {
        Ok(c) => c,
        Err(_) => {
            enqueue_pending_schedule_sync(&state, pair_id);
            return;
        }
    };

    let db = Arc::clone(&state.db);
    let app_emit = app.clone();
    let pair_id_for_release = pair_id.clone();

    let _ = std::thread::spawn(move || {
        let run_result = (|| -> Result<(), String> {
            let pair = {
                let guard = db.lock().map_err(|e| e.to_string())?;
                guard
                    .get_pair(&pair_id)
                    .map_err(|e| e.to_string())?
                    .filter(|p| p.enabled && p.schedule_enabled)
                    .ok_or_else(|| "pair not found or schedule disabled".to_string())?
            };
            let pair_name = pair.name.clone();

            let snapshot_entries = {
                let guard = db.lock().map_err(|e| e.to_string())?;
                guard
                    .latest_snapshot(&pair.id)
                    .map_err(|e| e.to_string())?
                    .map(|s| s.entries)
            };
            let plan = preview_pair_impl(&pair, snapshot_entries.as_deref())?;

            if pair.conflict_policy == ConflictPolicy::Ask && plan_has_conflicts(&plan.actions) {
                notify_sync_error(
                    &app_emit,
                    &pair_name,
                    "Scheduled sync skipped: conflicts require manual resolution.",
                );
                return Ok(());
            }

            let options = RunOptions {
                verify_hashes: false,
                use_recycle_bin: true,
                conflict_resolutions: Default::default(),
                stop_on_error: true,
                plan: Some(plan),
                ..Default::default()
            };

            let report = run_pair_impl(db.as_ref(), &pair, options, &cancel, |progress| {
                let _ = app_emit.emit("sync://progress", &progress);
            })?;

            if report.status == RunStatus::Completed || report.status == RunStatus::Failed {
                notify_sync_report(&app_emit, &pair_name, &report);
            }
            Ok(())
        })();

        release_sync_slot(app_emit.clone(), &state, &pair_id_for_release, &cancel);

        if let Err(e) = run_result {
            let pair_name = db
                .lock()
                .ok()
                .and_then(|g| g.get_pair(&pair_id_for_release).ok().flatten())
                .map(|p| p.name)
                .unwrap_or_else(|| pair_id_for_release.clone());
            notify_sync_error(&app_emit, &pair_name, &e);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{release_pair_run_slot, should_ignore_watch_event, try_acquire_pair_run, AppState};
    use tempfile::TempDir;

    #[test]
    fn empty_watch_plan_is_detected() {
        assert!(watch_plan_is_empty(&[]));
        assert!(!watch_plan_is_empty(&[SyncAction::CopyLeftToRight {
            path: "a.txt".into(),
        }]));
    }

    /// Simulates `run_watch_sync` acquiring, finding an empty plan, and releasing without
    /// enqueueing a follow-up watch run (no feedback loop).
    #[test]
    fn empty_watch_plan_release_does_not_requeue_watch_sync() {
        let data_dir = TempDir::new().expect("tempdir");
        let state = AppState::new(data_dir.path().to_path_buf()).expect("app state");

        let cancel = try_acquire_pair_run(&state, "pair-a").expect("acquire");
        assert!(watch_plan_is_empty(&[]));

        let pending = release_pair_run_slot(&state, "pair-a", &cancel).expect("release");

        assert_eq!(pending, (false, false));
        assert!(state.pending_watch_syncs.lock().expect("lock").is_empty());
        assert!(should_ignore_watch_event(&state, "pair-a"));
    }
}
