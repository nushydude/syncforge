use crate::persistence::DatabaseHandle;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tauri::{AppHandle, Emitter};

use crate::commands::preview::build_preview;
use crate::engine::{run_pair_impl, RunOptions};
use crate::models::{ConflictPolicy, RunStatus, SyncAction};
use crate::notifications::{notify_sync_error, notify_sync_report};
use crate::progress::ProgressCoalescer;
use crate::state::{
    acquire_or_enqueue, canonical_job_roots, defer_for_capacity, release_pair_run_slot,
    take_pending_sync, watch_is_ready_or_defer, AppState, AutomaticReason, HeavyJobKind,
    HeavyJobPermit, WorkCoordinator, WorkRequest,
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
    if let Some(pair_id) = take_pending_sync(state, AutomaticReason::Watch) {
        run_watch_sync(app.clone(), Arc::clone(state), pair_id);
    }
    if let Some(pair_id) = take_pending_sync(state, AutomaticReason::Schedule) {
        run_scheduled_sync(app, Arc::clone(state), pair_id);
    }
}

fn schedule_capacity_retry(
    app: AppHandle,
    state: Arc<AppState>,
    pair_id: String,
    roots: Vec<PathBuf>,
    reason: AutomaticReason,
) {
    let key = (pair_id.clone(), reason);
    if !state
        .capacity_waiters
        .lock()
        .map(|mut waiters| waiters.insert(key.clone()))
        .unwrap_or(false)
    {
        return;
    }
    let coordinator = Arc::clone(&state.work_coordinator);
    tauri::async_runtime::spawn(async move {
        let kind = match reason {
            AutomaticReason::Watch => HeavyJobKind::Watch,
            AutomaticReason::Schedule => HeavyJobKind::Scheduled,
        };
        coordinator.wait_for_release(WorkRequest::new(roots, true, kind)).await;
        if let Ok(mut waiters) = state.capacity_waiters.lock() {
            waiters.remove(&key);
        }
        match reason {
            AutomaticReason::Watch => run_watch_sync(app, state, pair_id),
            AutomaticReason::Schedule => run_scheduled_sync(app, state, pair_id),
        }
    });
}

fn watch_plan_fingerprint(
    pair: &crate::models::FolderPair,
    prepared: &crate::engine::PreparedPlan,
) -> String {
    use std::collections::BTreeSet;
    let paths: BTreeSet<&str> = prepared
        .plan
        .actions
        .iter()
        .map(|action| match action {
            SyncAction::CopyLeftToRight { path }
            | SyncAction::CopyRightToLeft { path }
            | SyncAction::DeleteLeft { path }
            | SyncAction::DeleteRight { path }
            | SyncAction::CreateDirLeft { path }
            | SyncAction::CreateDirRight { path }
            | SyncAction::Conflict { path, .. }
            | SyncAction::Skip { path, .. } => path.as_str(),
        })
        .collect();
    let selected = |entries: &[crate::models::FileEntry]| {
        entries
            .iter()
            .filter(|e| paths.contains(e.relative_path.as_str()))
            .cloned()
            .collect::<Vec<_>>()
    };
    let data = serde_json::to_vec(&(
        pair,
        &prepared.plan.actions,
        selected(&prepared.preconditions.left),
        selected(&prepared.preconditions.right),
        &prepared.plan.preview_file_errors,
    ))
    .unwrap_or_default();
    blake3::hash(&data).to_hex().to_string()
}

const MAX_FAILED_WATCH_ACTIONS: usize = 1_024;
const OVERFLOW_FINGERPRINT: &str = "\0whole-plan";

fn watch_action_path(action: &SyncAction) -> &str {
    match action {
        SyncAction::CopyLeftToRight { path }
        | SyncAction::CopyRightToLeft { path }
        | SyncAction::DeleteLeft { path }
        | SyncAction::DeleteRight { path }
        | SyncAction::CreateDirLeft { path }
        | SyncAction::CreateDirRight { path }
        | SyncAction::Conflict { path, .. }
        | SyncAction::Skip { path, .. } => path,
    }
}

fn watch_action_evidence(
    pair: &crate::models::FolderPair,
    prepared: &crate::engine::PreparedPlan,
) -> Vec<(String, String, String)> {
    let left_entries: std::collections::HashMap<_, _> = prepared
        .preconditions
        .left
        .iter()
        .map(|entry| (entry.relative_path.as_str(), entry))
        .collect();
    let right_entries: std::collections::HashMap<_, _> = prepared
        .preconditions
        .right
        .iter()
        .map(|entry| (entry.relative_path.as_str(), entry))
        .collect();
    prepared
        .plan
        .actions
        .iter()
        .map(|action| {
            let path = watch_action_path(action);
            let key = format!("{}\0{path}", crate::engine::action_kind(action));
            let left = left_entries.get(path);
            let right = right_entries.get(path);
            let value = serde_json::to_vec(&(
                pair,
                action,
                left,
                right,
                prepared.plan.preview_file_errors.get(path),
            ))
            .unwrap_or_default();
            (key, blake3::hash(&value).to_hex().to_string(), path.to_owned())
        })
        .collect()
}

/// Retain a failed action as a visible skip until its own evidence changes.
fn suppress_unchanged_failures(
    prepared: &mut crate::engine::PreparedPlan,
    evidence: &[(String, String, String)],
    known: &std::collections::HashMap<String, String>,
) -> std::collections::HashSet<String> {
    let mut suppressed = std::collections::HashSet::new();
    for (action, (key, fingerprint, path)) in prepared.plan.actions.iter_mut().zip(evidence) {
        if known.get(key) == Some(fingerprint) {
            suppressed.insert(key.clone());
            *action = SyncAction::Skip {
                path: path.clone(),
                reason: "previous Watch attempt failed; waiting for this item to change".into(),
            };
        }
    }
    suppressed
}

fn remove_equivalent_watch_copies(prepared: &mut crate::engine::PreparedPlan) {
    let left: std::collections::HashMap<_, _> = prepared
        .preconditions
        .left
        .iter()
        .map(|entry| (entry.relative_path.as_str(), entry))
        .collect();
    let right: std::collections::HashMap<_, _> = prepared
        .preconditions
        .right
        .iter()
        .map(|entry| (entry.relative_path.as_str(), entry))
        .collect();
    prepared.plan.actions.retain(|action| {
        let path = match action {
            SyncAction::CopyLeftToRight { path } | SyncAction::CopyRightToLeft { path } => path,
            _ => return true,
        };
        if prepared.plan.preview_file_errors.contains_key(path) {
            return true;
        }
        match (left.get(path.as_str()), right.get(path.as_str())) {
            (Some(l), Some(r)) if !l.is_dir && !r.is_dir && l.size == r.size => {
                !(l.size == 0 || matches!((&l.hash, &r.hash), (Some(a), Some(b)) if a == b))
            }
            _ => true,
        }
    });
}

pub(crate) fn run_watch_sync(app: AppHandle, state: Arc<AppState>, pair_id: String) {
    let pair = match state
        .db
        .get_pair(&pair_id)
        .ok()
        .flatten()
        .filter(|pair| pair.enabled && pair.watch_enabled)
    {
        Some(pair) => pair,
        None => return,
    };
    if !watch_is_ready_or_defer(&state, &pair) {
        return;
    }
    let cancel = match acquire_or_enqueue(&state, &pair_id, AutomaticReason::Watch) {
        Ok(Some(slot)) => slot,
        Ok(None) | Err(_) => return,
    };
    let roots = canonical_job_roots(&[&pair.left_path, &pair.right_path]);
    let permit = match admit_watch(&state.work_coordinator, roots.clone()) {
        Ok(Some(permit)) => permit,
        Ok(None) => {
            if defer_for_capacity(&state, &pair_id, &cancel, AutomaticReason::Watch) {
                schedule_capacity_retry(
                    app,
                    Arc::clone(&state),
                    pair_id,
                    roots,
                    AutomaticReason::Watch,
                );
            }
            return;
        }
        Err(_) => {
            release_sync_slot(app.clone(), &state, &pair_id, &cancel);
            return;
        }
    };

    let db = Arc::clone(&state.db);
    let app_emit = app.clone();
    let pair_id_for_release = pair_id.clone();

    std::mem::drop(tauri::async_runtime::spawn_blocking(move || {
        let run_result = (|| -> Result<(), String> {
            let _permit = permit;
            let current = db
                .get_pair(&pair.id)
                .map_err(|e| e.to_string())?
                .filter(|p| p.enabled && p.watch_enabled);
            let Some(current) = current else {
                return Ok(());
            };
            if crate::commands::preview::config_fingerprint(&current)
                != crate::commands::preview::config_fingerprint(&pair)
            {
                // Preserve one catch-up for the new configuration; release will retry admission.
                let _ = acquire_or_enqueue(&state, &pair.id, AutomaticReason::Watch);
                return Ok(());
            }
            if !watch_is_ready_or_defer(&state, &current) {
                return Ok(());
            }

            let root_keys = crate::path_normalization::pair_root_keys(
                std::path::Path::new(&pair.left_path),
                std::path::Path::new(&pair.right_path),
            )?;
            let baseline = db.load_sync_baseline(&pair, &root_keys).map_err(|e| e.to_string())?;
            let (plan, left, right) = build_preview(&pair, baseline.baseline.as_ref(), &cancel)?;
            let mut prepared = crate::engine::PreparedPlan {
                plan,
                preconditions: crate::engine::PlanPreconditions { left, right },
                root_keys,
                baseline_generation: baseline.generation,
                baseline_revision: baseline.baseline.map(|b| b.revision),
                bounded: true,
            };

            remove_equivalent_watch_copies(&mut prepared);
            let fingerprint = watch_plan_fingerprint(&pair, &prepared);
            let evidence = watch_action_evidence(&pair, &prepared);
            let known = state
                .watch_failed_fingerprints
                .lock()
                .ok()
                .and_then(|all| all.get(&pair.id).cloned())
                .unwrap_or_default();
            if known.get(OVERFLOW_FINGERPRINT) == Some(&fingerprint) {
                return Ok(());
            }
            let suppressed = suppress_unchanged_failures(&mut prepared, &evidence, &known);
            if !suppressed.is_empty()
                && prepared
                    .plan
                    .actions
                    .iter()
                    .all(|action| matches!(action, SyncAction::Skip { .. }))
            {
                return Ok(());
            }
            if watch_plan_is_empty(&prepared.plan.actions)
                && pair.mode != crate::models::SyncMode::Synchronize
            {
                return Ok(());
            }

            if pair.conflict_policy == ConflictPolicy::Ask
                && plan_has_conflicts(&prepared.plan.actions)
            {
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
                prepared: Some(prepared),
                ..Default::default()
            };

            let mut progress_sink = ProgressCoalescer::system(|progress| {
                let _ = emit_event(&app_emit, "sync://progress", &progress);
            });
            let mut failed_report = None;
            let result = run_pair_impl(db.as_ref(), &pair, options, &cancel, |progress| {
                if let crate::engine::ProgressEvent::Owned(ref event) = progress {
                    if event.phase == "failed" {
                        failed_report = event.report.clone();
                    }
                }
                progress_sink.push_event(progress);
            });
            progress_sink.flush();
            let report = result.as_ref().ok().or(failed_report.as_ref());
            if let Some(report) = report {
                if let Ok(mut all) = state.watch_failed_fingerprints.lock() {
                    let cache = all.entry(pair.id.clone()).or_default();
                    cache.remove(OVERFLOW_FINGERPRINT);
                    let mut overflow = false;
                    let errors: std::collections::BTreeSet<&str> =
                        report.errors.iter().map(String::as_str).collect();
                    for (key, value, path) in &evidence {
                        if suppressed.contains(key) {
                            continue;
                        }
                        let prefix = format!("{path}: ");
                        if errors
                            .range(prefix.as_str()..)
                            .next()
                            .is_some_and(|error| error.starts_with(&prefix))
                        {
                            if cache.len() < MAX_FAILED_WATCH_ACTIONS || cache.contains_key(key) {
                                cache.insert(key.clone(), value.clone());
                            } else {
                                overflow = true;
                            }
                        } else if report.status == RunStatus::Completed {
                            cache.remove(key);
                        }
                    }
                    if overflow {
                        cache.insert(OVERFLOW_FINGERPRINT.into(), fingerprint);
                    }
                }
            }
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
    let cancel = match acquire_or_enqueue(&state, &pair_id, AutomaticReason::Schedule) {
        Ok(Some(slot)) => slot,
        Ok(None) | Err(_) => return,
    };

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
            release_sync_slot(app.clone(), &state, &pair_id, &cancel);
            return;
        }
    };
    let roots = canonical_job_roots(&[&pair.left_path, &pair.right_path]);
    let permit = match admit_scheduled(&state.work_coordinator, roots.clone()) {
        Ok(Some(permit)) => permit,
        Ok(None) => {
            if defer_for_capacity(&state, &pair_id, &cancel, AutomaticReason::Schedule) {
                schedule_capacity_retry(
                    app,
                    Arc::clone(&state),
                    pair_id,
                    roots,
                    AutomaticReason::Schedule,
                );
            }
            return;
        }
        Err(_) => {
            release_sync_slot(app.clone(), &state, &pair_id, &cancel);
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

            let root_keys = crate::path_normalization::pair_root_keys(
                std::path::Path::new(&pair.left_path),
                std::path::Path::new(&pair.right_path),
            )?;
            let baseline = db.load_sync_baseline(&pair, &root_keys).map_err(|e| e.to_string())?;
            let (plan, left, right) = build_preview(&pair, baseline.baseline.as_ref(), &cancel)?;
            let prepared = crate::engine::PreparedPlan {
                plan,
                preconditions: crate::engine::PlanPreconditions { left, right },
                root_keys,
                baseline_generation: baseline.generation,
                baseline_revision: baseline.baseline.map(|b| b.revision),
                bounded: true,
            };

            if pair.conflict_policy == ConflictPolicy::Ask
                && plan_has_conflicts(&prepared.plan.actions)
            {
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
                prepared: Some(prepared),
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

            if matches!(
                report.status,
                RunStatus::Completed | RunStatus::CompletedWithErrors | RunStatus::Failed
            ) {
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
    use crate::state::{release_pair_run_slot, try_acquire_pair_run, AppState};
    use tempfile::TempDir;

    fn prepared_copy() -> (crate::models::FolderPair, crate::engine::PreparedPlan) {
        use crate::models::{FileEntry, FolderPair, SyncMode, SyncPlan};
        let pair = FolderPair {
            id: "p".into(),
            name: "test".into(),
            left_path: "left".into(),
            right_path: "right".into(),
            mode: SyncMode::Echo,
            filters: Default::default(),
            conflict_policy: ConflictPolicy::NewerWins,
            enabled: true,
            continue_on_error: false,
            watch_enabled: true,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 1,
        };
        let file = |hash: &str, modified| FileEntry {
            relative_path: "item".into(),
            size: 4,
            modified_secs: modified,
            modified_nanos: 0,
            is_dir: false,
            hash: Some(hash.into()),
            deleted: false,
        };
        let plan = SyncPlan {
            pair_id: pair.id.clone(),
            actions: vec![SyncAction::CopyLeftToRight { path: "item".into() }],
            scanned_left: 1,
            scanned_right: 1,
            scan_skipped_left: 0,
            scan_skipped_right: 0,
            scan_warnings: vec![],
            requires_attention: false,
            preview_file_errors: Default::default(),
        };
        let prepared = crate::engine::PreparedPlan {
            plan,
            preconditions: crate::engine::PlanPreconditions {
                left: vec![file("same", 10)],
                right: vec![file("same", 20)],
            },
            root_keys: ["l".into(), "r".into()],
            baseline_generation: None,
            baseline_revision: None,
            bounded: true,
        };
        (pair, prepared)
    }

    #[test]
    fn watch_equivalent_content_does_not_repeat_copy_on_different_timestamps() {
        let (_, mut prepared) = prepared_copy();
        remove_equivalent_watch_copies(&mut prepared);
        assert!(prepared.plan.actions.is_empty());
    }

    #[test]
    fn failed_plan_fingerprint_ignores_unrelated_stage_churn_but_tracks_new_versions() {
        let (pair, mut prepared) = prepared_copy();
        let original = watch_plan_fingerprint(&pair, &prepared);
        let mut unrelated = prepared.preconditions.right[0].clone();
        unrelated.relative_path = ".syncforge-owned.stage".into();
        unrelated.modified_secs += 1;
        prepared.preconditions.right.push(unrelated);
        assert_eq!(watch_plan_fingerprint(&pair, &prepared), original);
        prepared.preconditions.left[0].hash = Some("changed".into());
        assert_ne!(watch_plan_fingerprint(&pair, &prepared), original);
    }

    #[test]
    fn unchanged_failed_action_is_skipped_while_new_work_and_versions_continue() {
        let (pair, mut prepared) = prepared_copy();
        prepared.preconditions.right[0].hash = Some("different".into());
        let original = watch_action_evidence(&pair, &prepared);
        let known =
            std::collections::HashMap::from([(original[0].0.clone(), original[0].1.clone())]);
        prepared.plan.actions.push(SyncAction::CopyLeftToRight { path: "new".into() });
        let evidence = watch_action_evidence(&pair, &prepared);
        let suppressed = suppress_unchanged_failures(&mut prepared, &evidence, &known);
        assert_eq!(suppressed.len(), 1);
        assert!(
            matches!(&prepared.plan.actions[0], SyncAction::Skip { path, .. } if path == "item")
        );
        assert!(
            matches!(&prepared.plan.actions[1], SyncAction::CopyLeftToRight { path } if path == "new")
        );
        let (_, mut changed) = prepared_copy();
        changed.preconditions.right[0].hash = Some("new-version".into());
        let evidence = watch_action_evidence(&pair, &changed);
        assert!(suppress_unchanged_failures(&mut changed, &evidence, &known).is_empty());
    }

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
    }
}
