use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tauri::{AppHandle, Emitter};

use crate::commands::preview::preview_pair_impl;
use crate::engine::{run_pair_impl, RunOptions};
use crate::models::{ConflictPolicy, RunStatus, SyncAction};
use crate::notifications::{notify_sync_error, notify_sync_report};
use crate::progress::ProgressCoalescer;
use crate::state::{
    canonical_job_roots, dequeue_pending_schedule_sync, dequeue_pending_watch_sync,
    enqueue_pending_schedule_sync, enqueue_pending_watch_sync, release_pair_run_slot,
    try_acquire_pair_run, AppState, HeavyJobKind, HeavyJobPermit, WorkCoordinator, WorkRequest,
};
use serde::Serialize;

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(test)]
static TAURI_EVENT_EMITS: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
pub fn reset_tauri_event_counter() {
    TAURI_EVENT_EMITS.store(0, Ordering::Relaxed);
}

#[cfg(test)]
pub fn tauri_event_count() -> usize {
    TAURI_EVENT_EMITS.load(Ordering::Relaxed)
}

fn emit_event_with<T, F>(event: &str, payload: &T, emit: F) -> Result<(), tauri::Error>
where
    T: Serialize,
    F: FnOnce(&str, &T) -> Result<(), tauri::Error>,
{
    #[cfg(test)]
    TAURI_EVENT_EMITS.fetch_add(1, Ordering::Relaxed);
    emit(event, payload)
}

pub(crate) fn emit_event<T: Serialize>(
    app: &AppHandle,
    event: &str,
    payload: &T,
) -> Result<(), tauri::Error> {
    emit_event_with(event, payload, |event, payload| app.emit(event, payload))
}

#[cfg(test)]
pub fn emit_test_event<T: Serialize>(event: &str, payload: &T) {
    let _ = emit_event_with(event, payload, |_, _| Ok(()));
}

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

pub(crate) fn admit_watch(
    coordinator: &Arc<WorkCoordinator>,
    roots: Vec<PathBuf>,
) -> Result<Option<HeavyJobPermit>, String> {
    coordinator.try_acquire(WorkRequest::new(roots, true, HeavyJobKind::Watch))
}

pub(crate) fn admit_scheduled(
    coordinator: &Arc<WorkCoordinator>,
    roots: Vec<PathBuf>,
) -> Result<Option<HeavyJobPermit>, String> {
    coordinator.try_acquire(WorkRequest::new(roots, true, HeavyJobKind::Scheduled))
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
    let Some((watch_pending, schedule_pending)) = release_pair_run_slot(state, pair_id, slot)
    else {
        return;
    };
    if watch_pending {
        run_watch_sync(app.clone(), Arc::clone(state), pair_id.to_string());
    }
    if schedule_pending {
        run_scheduled_sync(app, Arc::clone(state), pair_id.to_string());
    }
}

pub(crate) fn retry_pending_syncs(app: AppHandle, state: &Arc<AppState>) {
    let watch = state.pending_watch_syncs.lock().ok().and_then(|mut pending| {
        let id = pending.iter().next().cloned();
        if let Some(ref id) = id {
            pending.remove(id);
            if let Ok(mut automatic) = state.pending_automatic_syncs.lock() {
                automatic.remove(id);
            }
        }
        id
    });
    if let Some(pair_id) = watch {
        run_watch_sync(app.clone(), Arc::clone(state), pair_id);
    }
    let scheduled = state.pending_schedule_syncs.lock().ok().and_then(|mut pending| {
        let id = pending.iter().next().cloned();
        if let Some(ref id) = id {
            pending.remove(id);
            if let Ok(mut automatic) = state.pending_automatic_syncs.lock() {
                automatic.remove(id);
            }
        }
        id
    });
    if let Some(pair_id) = scheduled {
        run_scheduled_sync(app, Arc::clone(state), pair_id);
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
    dequeue_pending_watch_sync(&state, &pair_id);

    let pair = match state
        .db
        .get_pair(&pair_id)
        .map_err(|e| e.to_string())
        .ok()
        .flatten()
        .filter(|p| p.enabled && p.watch_enabled)
    {
        Some(pair) => pair,
        None => {
            let _ = release_pair_run_slot(&state, &pair_id, &cancel);
            return;
        }
    };
    let roots = canonical_job_roots(&[&pair.left_path, &pair.right_path]);
    let permit = match admit_watch(&state.work_coordinator, roots.clone()) {
        Ok(Some(permit)) => permit,
        Ok(None) => {
            let _ = release_pair_run_slot(&state, &pair_id, &cancel);
            if enqueue_pending_watch_sync(&state, pair_id.clone()) {
                let retry_state = Arc::clone(&state);
                let retry_coordinator = Arc::clone(&state.work_coordinator);
                let retry_id = pair_id.clone();
                let retry_roots = roots.clone();
                tauri::async_runtime::spawn(async move {
                    retry_coordinator
                        .wait_for_release(WorkRequest::new(retry_roots, true, HeavyJobKind::Watch))
                        .await;
                    run_watch_sync(app, retry_state, retry_id);
                });
            }
            return;
        }
        Err(_) => {
            let _ = release_pair_run_slot(&state, &pair_id, &cancel);
            return;
        }
    };

    let db = Arc::clone(&state.db);
    let app_emit = app.clone();
    let pair_id_for_release = pair_id.clone();

    std::mem::drop(tauri::async_runtime::spawn_blocking(move || {
        let run_result = (|| -> Result<(), String> {
            let _permit = permit;

            let snapshot_entries =
                db.latest_snapshot(&pair.id).map_err(|e| e.to_string())?.map(|s| s.entries);
            let plan = preview_pair_impl(&pair, snapshot_entries.as_deref())?;

            if watch_plan_is_empty(&plan.actions) {
                return Ok(());
            }

            if pair.conflict_policy == ConflictPolicy::Ask && plan_has_conflicts(&plan.actions) {
                let _ = emit_event(
                    &app_emit,
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

            let mut progress_sink = ProgressCoalescer::system(|progress| {
                let _ = emit_event(&app_emit, "sync://progress", &progress);
            });
            let result = run_pair_impl(db.as_ref(), &pair, options, &cancel, |progress| {
                progress_sink.push_event(progress);
            });
            progress_sink.flush();
            result?;
            Ok(())
        })();

        release_sync_slot(app_emit.clone(), &state, &pair_id_for_release, &cancel);

        if let Err(e) = run_result {
            let _ = emit_event(
                &app_emit,
                "sync://watch-skipped",
                &WatchSkippedPayload { pair_id: pair_id_for_release, reason: e },
            );
        }
    }));
}

pub(crate) fn run_scheduled_sync(app: AppHandle, state: Arc<AppState>, pair_id: String) {
    let cancel = match try_acquire_pair_run(&state, &pair_id) {
        Ok(c) => c,
        Err(_) => {
            enqueue_pending_schedule_sync(&state, pair_id);
            return;
        }
    };
    dequeue_pending_schedule_sync(&state, &pair_id);

    let pair = match state
        .db
        .get_pair(&pair_id)
        .map_err(|e| e.to_string())
        .ok()
        .flatten()
        .filter(|p| p.enabled && p.schedule_enabled)
    {
        Some(pair) => pair,
        None => {
            let _ = release_pair_run_slot(&state, &pair_id, &cancel);
            return;
        }
    };
    let roots = canonical_job_roots(&[&pair.left_path, &pair.right_path]);
    let permit = match admit_scheduled(&state.work_coordinator, roots.clone()) {
        Ok(Some(permit)) => permit,
        Ok(None) => {
            let _ = release_pair_run_slot(&state, &pair_id, &cancel);
            if enqueue_pending_schedule_sync(&state, pair_id.clone()) {
                let retry_state = Arc::clone(&state);
                let retry_coordinator = Arc::clone(&state.work_coordinator);
                let retry_id = pair_id.clone();
                let retry_roots = roots.clone();
                tauri::async_runtime::spawn(async move {
                    retry_coordinator
                        .wait_for_release(WorkRequest::new(
                            retry_roots,
                            true,
                            HeavyJobKind::Scheduled,
                        ))
                        .await;
                    run_scheduled_sync(app, retry_state, retry_id);
                });
            }
            return;
        }
        Err(_) => {
            let _ = release_pair_run_slot(&state, &pair_id, &cancel);
            return;
        }
    };

    let db = Arc::clone(&state.db);
    let app_emit = app.clone();
    let pair_id_for_release = pair_id.clone();

    std::mem::drop(tauri::async_runtime::spawn_blocking(move || {
        let run_result = (|| -> Result<(), String> {
            let _permit = permit;
            let pair_name = pair.name.clone();

            let snapshot_entries =
                db.latest_snapshot(&pair.id).map_err(|e| e.to_string())?.map(|s| s.entries);
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

            let mut progress_sink = ProgressCoalescer::system(|progress| {
                let _ = emit_event(&app_emit, "sync://progress", &progress);
            });
            let result = run_pair_impl(db.as_ref(), &pair, options, &cancel, |progress| {
                progress_sink.push_event(progress);
            });
            progress_sink.flush();
            let report = result?;

            if report.status == RunStatus::Completed || report.status == RunStatus::Failed {
                notify_sync_report(&app_emit, &pair_name, &report);
            }
            Ok(())
        })();

        release_sync_slot(app_emit.clone(), &state, &pair_id_for_release, &cancel);

        if let Err(e) = run_result {
            let pair_name = db
                .get_pair(&pair_id_for_release)
                .ok()
                .flatten()
                .map(|p| p.name)
                .unwrap_or_else(|| pair_id_for_release.clone());
            notify_sync_error(&app_emit, &pair_name, &e);
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{
        release_pair_run_slot, should_ignore_watch_event, try_acquire_pair_run, AppState,
    };
    use tempfile::TempDir;

    #[test]
    fn empty_watch_plan_is_detected() {
        assert!(watch_plan_is_empty(&[]));
        assert!(!watch_plan_is_empty(&[SyncAction::CopyLeftToRight { path: "a.txt".into() }]));
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
