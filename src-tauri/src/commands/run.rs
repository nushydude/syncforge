use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde::Deserialize;
use tauri::{AppHandle, State};

use crate::commands::preview::config_fingerprint;
use crate::engine::{run_pair_impl, PlanPreconditions, RunOptions};
use crate::models::{ConflictResolution, FolderPair, RunReport, RunStatus};
use crate::notifications::notify_sync_report;
use crate::progress::ProgressCoalescer;
use crate::run_coordinator::{emit_event, release_sync_slot};
use crate::state::{
    canonical_job_roots, try_acquire_pair_run, AppState, HeavyJobKind, HeavyJobPermit,
    WorkCoordinator, WorkRequest,
};

pub(crate) fn admit_manual_run(
    coordinator: &Arc<WorkCoordinator>,
    roots: Vec<PathBuf>,
) -> Result<HeavyJobPermit, String> {
    coordinator.acquire_manual(WorkRequest::new(roots, true, HeavyJobKind::ManualRun))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunPairOptions {
    #[serde(default)]
    pub verify_hashes: bool,
    #[serde(default = "default_recycle_bin")]
    pub use_recycle_bin: bool,
    #[serde(default)]
    pub conflict_resolutions: HashMap<String, ConflictResolution>,
    #[serde(default = "default_stop_on_error")]
    pub stop_on_error: bool,
    #[serde(default)]
    pub plan_id: Option<String>,
}

fn default_stop_on_error() -> bool {
    true
}

fn default_recycle_bin() -> bool {
    true
}

fn current_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[tauri::command]
pub async fn run_pair(
    pair: FolderPair,
    options: RunPairOptions,
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
) -> Result<RunReport, String> {
    let pair_id = pair.id.clone();
    if pair_id.is_empty() {
        return Err("pair id required".into());
    }

    // Reserve the pair's run slot first: taking the plan consumes it, so doing
    // that before a failed acquisition would destroy a plan for a run that
    // never starts.
    let cancel = try_acquire_pair_run(&state, &pair_id)?;

    let take_plan = || -> Result<(Option<_>, Option<PlanPreconditions>), String> {
        let Some(plan_id) = options.plan_id.as_deref() else {
            return Ok((None, None));
        };
        let fingerprint = config_fingerprint(&pair);
        let (plan, left_preconditions, right_preconditions) = state
            .preview_plans
            .lock()
            .map_err(|e| e.to_string())?
            .take(plan_id, &pair.id, &fingerprint, current_millis())?;
        Ok((
            Some(plan),
            Some(PlanPreconditions { left: left_preconditions, right: right_preconditions }),
        ))
    };
    let (plan, plan_preconditions) = match take_plan() {
        Ok(taken) => taken,
        Err(e) => {
            release_sync_slot(app.clone(), &state, &pair_id, &cancel);
            return Err(e);
        }
    };

    let run_options = RunOptions {
        verify_hashes: options.verify_hashes,
        use_recycle_bin: options.use_recycle_bin,
        conflict_resolutions: options.conflict_resolutions,
        stop_on_error: options.stop_on_error,
        plan,
        plan_preconditions,
        ..Default::default()
    };

    let pair_name = pair.name.clone();
    let db = Arc::clone(&state.db);
    let app_emit = app.clone();
    let cancel_for_run = Arc::clone(&cancel);
    let state_inner = Arc::clone(&state);
    let work_coordinator = Arc::clone(&state.work_coordinator);
    let roots = canonical_job_roots(&[&pair.left_path, &pair.right_path]);

    let task_result = tauri::async_runtime::spawn_blocking(move || {
        let _permit = admit_manual_run(&work_coordinator, roots)?;
        let mut progress_sink = ProgressCoalescer::system(|progress| {
            let _ = emit_event(&app_emit, "sync://progress", &progress);
        });
        let result = run_pair_impl(db.as_ref(), &pair, run_options, &cancel_for_run, |progress| {
            progress_sink.push_event(progress);
        });
        progress_sink.flush();
        result
    })
    .await
    .map_err(|e| format!("sync run task failed: {e}"));

    release_sync_slot(app.clone(), &state_inner, &pair_id, &cancel);

    let result = task_result?;

    if let Ok(ref report) = result {
        if report.status == RunStatus::Completed || report.status == RunStatus::Failed {
            notify_sync_report(&app, &pair_name, report);
        }
    }

    result
}

/// Cancel an in-progress sync. With `pair_id`, cancels only that pair; without, cancels all active runs.
#[tauri::command]
pub fn cancel_run(pair_id: Option<String>, state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let guard = state.active_runs.lock().map_err(|e| e.to_string())?;
    match pair_id {
        Some(id) => {
            let Some(flag) = guard.get(&id) else {
                return Err(format!("no sync run in progress for pair {id}"));
            };
            flag.store(true, Ordering::Relaxed);
            Ok(())
        }
        None => {
            if guard.is_empty() {
                return Err("no sync run in progress".into());
            }
            for flag in guard.values() {
                flag.store(true, Ordering::Relaxed);
            }
            Ok(())
        }
    }
}
