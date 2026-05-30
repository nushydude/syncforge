use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::persistence::{Database, PersistenceError};
use crate::scheduler::ScheduleService;
use crate::watcher::WatchService;

/// Ignore watch events for this long after a pair's sync completes (self-write feedback).
pub const WATCH_SUPPRESS_AFTER_RUN_MS: u64 = 2000;

pub struct AppState {
    pub db: Arc<Mutex<Database>>,
    /// Per-pair cancel flags while a sync run is active.
    pub active_runs: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// Pair ids whose debounced watch sync could not start while that pair was busy.
    pub pending_watch_syncs: Mutex<HashSet<String>>,
    /// Pair ids whose scheduled sync could not start while that pair was busy.
    pub pending_schedule_syncs: Mutex<HashSet<String>>,
    /// Pair ids currently executing sync (manual, watch, or schedule).
    pub sync_in_progress: Mutex<HashSet<String>>,
    /// Ignore watch events for a pair until this instant (post-run suppression).
    pub watch_suppress_until: Mutex<HashMap<String, Instant>>,
    pub watch_service: Mutex<Option<WatchService>>,
    pub schedule_service: Mutex<Option<ScheduleService>>,
}

impl AppState {
    pub fn new(data_dir: PathBuf) -> Result<Self, PersistenceError> {
        let db_path = data_dir.join("syncforge.db");
        let db = Database::open(&db_path)?;
        Ok(Self {
            db: Arc::new(Mutex::new(db)),
            active_runs: Mutex::new(HashMap::new()),
            pending_watch_syncs: Mutex::new(HashSet::new()),
            pending_schedule_syncs: Mutex::new(HashSet::new()),
            sync_in_progress: Mutex::new(HashSet::new()),
            watch_suppress_until: Mutex::new(HashMap::new()),
            watch_service: Mutex::new(None),
            schedule_service: Mutex::new(None),
        })
    }
}

/// Reserves a run slot for `pair_id`. Different pairs may run concurrently.
pub fn try_acquire_pair_run(state: &AppState, pair_id: &str) -> Result<Arc<AtomicBool>, String> {
    let cancel = Arc::new(AtomicBool::new(false));
    let mut runs = state.active_runs.lock().map_err(|e| e.to_string())?;
    if runs.contains_key(pair_id) {
        return Err(format!("sync run already in progress for pair {pair_id}"));
    }
    let mut in_progress = state.sync_in_progress.lock().map_err(|e| e.to_string())?;
    in_progress.insert(pair_id.to_string());
    drop(in_progress);
    runs.insert(pair_id.to_string(), cancel.clone());
    Ok(cancel)
}

/// Clears the run slot when it matches `slot` and arms post-run watch suppression.
/// Returns whether a pending watch sync and/or scheduled sync were queued for this pair.
pub fn release_pair_run_slot(
    state: &AppState,
    pair_id: &str,
    slot: &Arc<AtomicBool>,
) -> Option<(bool, bool)> {
    let mut runs = state.active_runs.lock().ok()?;
    let active = runs.get(pair_id)?;
    if !Arc::ptr_eq(active, slot) {
        return None;
    }
    runs.remove(pair_id);
    drop(runs);

    if let Ok(mut in_progress) = state.sync_in_progress.lock() {
        in_progress.remove(pair_id);
    }
    if let Ok(mut suppress) = state.watch_suppress_until.lock() {
        suppress.insert(
            pair_id.to_string(),
            Instant::now() + Duration::from_millis(WATCH_SUPPRESS_AFTER_RUN_MS),
        );
    }

    let watch_pending = state
        .pending_watch_syncs
        .lock()
        .ok()
        .is_some_and(|mut g| g.remove(pair_id));
    let schedule_pending = state
        .pending_schedule_syncs
        .lock()
        .ok()
        .is_some_and(|mut g| g.remove(pair_id));

    Some((watch_pending, schedule_pending))
}

pub fn should_ignore_watch_event(state: &AppState, pair_id: &str) -> bool {
    if let Ok(in_progress) = state.sync_in_progress.lock() {
        if in_progress.contains(pair_id) {
            return true;
        }
    }
    if let Ok(suppress) = state.watch_suppress_until.lock() {
        if let Some(until) = suppress.get(pair_id) {
            if Instant::now() < *until {
                return true;
            }
        }
    }
    false
}

pub(crate) fn enqueue_pending_watch_sync(state: &AppState, pair_id: impl Into<String>) {
    if let Ok(mut guard) = state.pending_watch_syncs.lock() {
        guard.insert(pair_id.into());
    }
}

pub(crate) fn enqueue_pending_schedule_sync(state: &AppState, pair_id: impl Into<String>) {
    if let Ok(mut guard) = state.pending_schedule_syncs.lock() {
        guard.insert(pair_id.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use tempfile::TempDir;

    #[test]
    fn different_pairs_can_run_concurrently() {
        let data_dir = TempDir::new().expect("tempdir");
        let state = AppState::new(data_dir.path().to_path_buf()).expect("app state");

        let slot_a = try_acquire_pair_run(&state, "pair-a").expect("acquire a");
        let slot_b = try_acquire_pair_run(&state, "pair-b").expect("acquire b");

        assert!(try_acquire_pair_run(&state, "pair-a").is_err());

        assert!(release_pair_run_slot(&state, "pair-a", &slot_a).is_some());
        let slot_a2 = try_acquire_pair_run(&state, "pair-a").expect("re-acquire a");

        assert!(release_pair_run_slot(&state, "pair-b", &slot_b).is_some());
        assert!(release_pair_run_slot(&state, "pair-a", &slot_a2).is_some());
    }

    #[test]
    fn watch_suppression_blocks_events_until_deadline() {
        let data_dir = TempDir::new().expect("tempdir");
        let state = AppState::new(data_dir.path().to_path_buf()).expect("app state");
        let slot = try_acquire_pair_run(&state, "pair-a").expect("acquire");

        assert!(should_ignore_watch_event(&state, "pair-a"));

        let _ = release_pair_run_slot(&state, "pair-a", &slot);

        assert!(should_ignore_watch_event(&state, "pair-a"));
        {
            let mut suppress = state.watch_suppress_until.lock().expect("lock");
            suppress.insert("pair-a".into(), Instant::now() - Duration::from_millis(1));
        }
        assert!(!should_ignore_watch_event(&state, "pair-a"));
    }

    #[test]
    fn pending_watch_and_schedule_queues_are_separate() {
        let data_dir = TempDir::new().expect("tempdir");
        let state = AppState::new(data_dir.path().to_path_buf()).expect("app state");

        enqueue_pending_watch_sync(&state, "pair-a");
        enqueue_pending_watch_sync(&state, "pair-a");
        enqueue_pending_schedule_sync(&state, "pair-a");
        enqueue_pending_schedule_sync(&state, "pair-b");

        let mut watch = state.pending_watch_syncs.lock().expect("lock").drain().collect::<Vec<_>>();
        watch.sort();
        let mut schedule =
            state.pending_schedule_syncs.lock().expect("lock").drain().collect::<Vec<_>>();
        schedule.sort();

        assert_eq!(watch, vec!["pair-a".to_string()]);
        assert_eq!(schedule, vec!["pair-a".to_string(), "pair-b".to_string()]);
    }

    #[test]
    fn release_slot_drains_only_matching_pair_pending() {
        let data_dir = TempDir::new().expect("tempdir");
        let state = AppState::new(data_dir.path().to_path_buf()).expect("app state");
        enqueue_pending_watch_sync(&state, "pair-a");
        enqueue_pending_watch_sync(&state, "pair-b");
        enqueue_pending_schedule_sync(&state, "pair-a");

        let slot = try_acquire_pair_run(&state, "pair-a").expect("acquire");
        let pending = release_pair_run_slot(&state, "pair-a", &slot).expect("release");

        assert_eq!(pending, (true, true));
        assert!(state.pending_watch_syncs.lock().expect("lock").contains("pair-b"));
        assert!(state.pending_schedule_syncs.lock().expect("lock").is_empty());
    }

    #[test]
    fn cancel_flag_stored_in_active_runs() {
        let data_dir = TempDir::new().expect("tempdir");
        let state = AppState::new(data_dir.path().to_path_buf()).expect("app state");
        let slot = try_acquire_pair_run(&state, "pair-a").expect("acquire");
        state
            .active_runs
            .lock()
            .expect("lock")
            .get("pair-a")
            .expect("flag")
            .store(true, Ordering::Relaxed);
        assert!(slot.load(Ordering::Relaxed));
    }
}
