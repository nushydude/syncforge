use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::commands::preview::preview_pair_impl;
use crate::engine::{run_pair_impl, RunOptions};
use crate::models::{ConflictPolicy, FolderPair, SyncAction};
use crate::path_normalization;
use crate::state::AppState;

/// Debounce window for filesystem events (bulk drops coalesce into one sync).
pub const DEBOUNCE_MS: u64 = 2000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchSkippedPayload {
    pub pair_id: String,
    pub reason: String,
}

/// Tracks per-pair debounce deadlines; resets the timer on each new event.
#[derive(Debug)]
pub struct DebounceScheduler {
    delay: Duration,
    deadlines: HashMap<String, Instant>,
}

impl DebounceScheduler {
    pub fn new(delay: Duration) -> Self {
        Self {
            delay,
            deadlines: HashMap::new(),
        }
    }

    pub fn touch(&mut self, pair_id: &str, now: Instant) {
        self.deadlines
            .insert(pair_id.to_string(), now + self.delay);
    }

    /// Returns pair ids whose debounce period has elapsed and removes them.
    pub fn take_ready(&mut self, now: Instant) -> Vec<String> {
        let ready: Vec<String> = self
            .deadlines
            .iter()
            .filter(|(_, deadline)| now >= **deadline)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ready {
            self.deadlines.remove(id);
        }
        ready
    }
}

#[derive(Clone)]
struct WatchedRoots {
    pair_id: String,
    left: PathBuf,
    right: PathBuf,
}

fn pairs_for_path(roots: &[WatchedRoots], path: &Path) -> Vec<String> {
    let path_str = path.to_string_lossy();
    let mut ids = HashSet::new();
    for root in roots {
        let left = root.left.to_string_lossy();
        let right = root.right.to_string_lossy();
        if path_normalization::path_is_within_root(&path_str, &left)
            || path_normalization::path_is_within_root(&path_str, &right)
        {
            ids.insert(root.pair_id.clone());
        }
    }
    ids.into_iter().collect()
}

fn build_roots(pairs: &[FolderPair]) -> Vec<WatchedRoots> {
    pairs
        .iter()
        .filter(|p| p.enabled && p.watch_enabled)
        .filter_map(|pair| {
            let left = PathBuf::from(path_normalization::to_long_path(&pair.left_path));
            let right = PathBuf::from(path_normalization::to_long_path(&pair.right_path));
            if left.exists() && right.exists() {
                Some(WatchedRoots {
                    pair_id: pair.id.clone(),
                    left,
                    right,
                })
            } else {
                None
            }
        })
        .collect()
}

fn plan_has_conflicts(actions: &[SyncAction]) -> bool {
    actions
        .iter()
        .any(|a| matches!(a, SyncAction::Conflict { .. }))
}

pub(crate) fn enqueue_pending_watch_sync(state: &AppState, pair_id: impl Into<String>) {
    if let Ok(mut guard) = state.pending_watch_syncs.lock() {
        guard.insert(pair_id.into());
    }
}

/// Clears the active sync slot when it matches `slot`, then starts any queued watch syncs.
pub(crate) fn release_sync_slot(
    app: AppHandle,
    state: &Arc<AppState>,
    slot: &Arc<AtomicBool>,
) {
    let pending: Vec<String> = {
        let mut flag_guard = match state.cancel_flag.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if !flag_guard.as_ref().is_some_and(|f| Arc::ptr_eq(f, slot)) {
            return;
        }
        *flag_guard = None;
        let mut pending_guard = match state.pending_watch_syncs.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        pending_guard.drain().collect()
    };
    for pair_id in pending {
        run_watch_sync(app.clone(), Arc::clone(state), pair_id);
    }
}

fn run_watch_sync(app: AppHandle, state: Arc<AppState>, pair_id: String) {
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut guard = match state.cancel_flag.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if guard.is_some() {
            enqueue_pending_watch_sync(&state, pair_id);
            return;
        }
        *guard = Some(cancel.clone());
    }

    let db = Arc::clone(&state.db);
    let app_emit = app.clone();

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

            let plan = {
                let guard = db.lock().map_err(|e| e.to_string())?;
                preview_pair_impl(&guard, &pair)?
            };

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
            };

            run_pair_impl(
                db.as_ref(),
                &pair,
                options,
                &cancel,
                |progress| {
                    let _ = app_emit.emit("sync://progress", &progress);
                },
            )?;
            Ok(())
        })();

        release_sync_slot(app_emit.clone(), &state, &cancel);

        if let Err(e) = run_result {
            let _ = app_emit.emit(
                "sync://watch-skipped",
                &WatchSkippedPayload {
                    pair_id,
                    reason: e,
                },
            );
        }
    });
}

pub struct WatchService {
    _watcher: RecommendedWatcher,
    _debounce_thread: std::thread::JoinHandle<()>,
}

impl WatchService {
    pub fn start(app: AppHandle, state: Arc<AppState>) -> Result<Self, String> {
        let pairs = state
            .db
            .lock()
            .map_err(|e| e.to_string())?
            .list_pairs()
            .map_err(|e| e.to_string())?;
        let roots = Arc::new(Mutex::new(build_roots(&pairs)));
        let (event_tx, event_rx) = mpsc::channel::<String>();

        let watcher = Self::build_watcher(Arc::clone(&roots), event_tx)?;

        let app_debounce = app.clone();
        let state_debounce = Arc::clone(&state);
        let debounce_thread = std::thread::spawn(move || {
            let mut scheduler =
                DebounceScheduler::new(Duration::from_millis(DEBOUNCE_MS));
            loop {
                match event_rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(pair_id) => scheduler.touch(&pair_id, Instant::now()),
                    Err(RecvTimeoutError::Timeout) => {
                        for pair_id in scheduler.take_ready(Instant::now()) {
                            run_watch_sync(
                                app_debounce.clone(),
                                Arc::clone(&state_debounce),
                                pair_id,
                            );
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        });

        Ok(Self {
            _watcher: watcher,
            _debounce_thread: debounce_thread,
        })
    }

    fn build_watcher(
        roots: Arc<Mutex<Vec<WatchedRoots>>>,
        event_tx: mpsc::Sender<String>,
    ) -> Result<RecommendedWatcher, String> {
        let roots_for_callback = Arc::clone(&roots);
        let mut watcher = RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                let Ok(event) = res else {
                    return;
                };
                if !matches!(
                    event.kind,
                    EventKind::Create(_)
                        | EventKind::Modify(_)
                        | EventKind::Remove(_)
                ) {
                    return;
                }
                let current_roots = match roots_for_callback.lock() {
                    Ok(g) => g.clone(),
                    Err(_) => return,
                };
                let mut pair_ids = HashSet::new();
                for path in &event.paths {
                    for id in pairs_for_path(&current_roots, path) {
                        pair_ids.insert(id);
                    }
                }
                for pair_id in pair_ids {
                    let _ = event_tx.send(pair_id);
                }
            },
            Config::default(),
        )
        .map_err(|e| format!("watch init failed: {e}"))?;

        Self::apply_watches(&mut watcher, &roots)?;
        Ok(watcher)
    }

    fn apply_watches(
        watcher: &mut RecommendedWatcher,
        roots: &Arc<Mutex<Vec<WatchedRoots>>>,
    ) -> Result<(), String> {
        let guard = roots.lock().map_err(|e| e.to_string())?;
        let mut seen = HashSet::new();
        for root in guard.iter() {
            for path in [&root.left, &root.right] {
                if seen.insert(path.clone()) {
                    watcher
                        .watch(path, RecursiveMode::Recursive)
                        .map_err(|e| format!("watch {:?} failed: {e}", path))?;
                }
            }
        }
        Ok(())
    }
}

pub fn refresh_watch_service(app: &AppHandle, state: &Arc<AppState>) -> Result<(), String> {
    let mut guard = state.watch_service.lock().map_err(|e| e.to_string())?;
    *guard = None;
    let service = WatchService::start(app.clone(), Arc::clone(state))?;
    *guard = Some(service);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debounce_coalesces_rapid_events_for_same_pair() {
        let mut scheduler = DebounceScheduler::new(Duration::from_millis(100));
        let t0 = Instant::now();
        scheduler.touch("pair-a", t0);
        scheduler.touch("pair-a", t0 + Duration::from_millis(30));
        scheduler.touch("pair-a", t0 + Duration::from_millis(60));

        assert!(scheduler.take_ready(t0 + Duration::from_millis(90)).is_empty());
        let ready = scheduler.take_ready(t0 + Duration::from_millis(161));
        assert_eq!(ready, vec!["pair-a".to_string()]);
        assert!(scheduler.take_ready(t0 + Duration::from_millis(200)).is_empty());
    }

    #[test]
    fn debounce_batches_multiple_pairs_after_quiet_period() {
        let mut scheduler = DebounceScheduler::new(Duration::from_millis(50));
        let t0 = Instant::now();
        scheduler.touch("pair-a", t0);
        scheduler.touch("pair-b", t0 + Duration::from_millis(10));

        let ready = scheduler.take_ready(t0 + Duration::from_millis(70));
        assert_eq!(ready.len(), 2);
        assert!(ready.contains(&"pair-a".to_string()));
        assert!(ready.contains(&"pair-b".to_string()));
    }

    #[test]
    fn debounce_extends_deadline_on_late_events() {
        let mut scheduler = DebounceScheduler::new(Duration::from_millis(100));
        let t0 = Instant::now();
        scheduler.touch("pair-a", t0);
        scheduler.touch("pair-a", t0 + Duration::from_millis(90));

        assert!(scheduler.take_ready(t0 + Duration::from_millis(150)).is_empty());
        let ready = scheduler.take_ready(t0 + Duration::from_millis(191));
        assert_eq!(ready, vec!["pair-a".to_string()]);
    }

    #[cfg(windows)]
    #[test]
    fn pairs_for_path_resolves_conventional_event_under_extended_root() {
        let roots = vec![WatchedRoots {
            pair_id: "pair-1".into(),
            left: PathBuf::from(r"\\?\UNC\server\share\left"),
            right: PathBuf::from(r"C:\Pairs\right"),
        }];
        let ids = pairs_for_path(
            &roots,
            Path::new(r"\\server\share\left\sub\file.txt"),
        );
        assert_eq!(ids, vec!["pair-1".to_string()]);

        let ids = pairs_for_path(&roots, Path::new(r"c:\pairs\right\doc.txt"));
        assert_eq!(ids, vec!["pair-1".to_string()]);
    }

    #[test]
    fn pending_watch_sync_queue_dedupes_pair_ids() {
        use std::sync::Arc;

        use tempfile::TempDir;

        use crate::state::AppState;

        let data_dir = TempDir::new().expect("tempdir");
        let state = Arc::new(AppState::new(data_dir.path().to_path_buf()).expect("app state"));
        enqueue_pending_watch_sync(&state, "pair-a");
        enqueue_pending_watch_sync(&state, "pair-a");
        enqueue_pending_watch_sync(&state, "pair-b");

        let mut pending = state
            .pending_watch_syncs
            .lock()
            .expect("lock")
            .drain()
            .collect::<Vec<_>>();
        pending.sort();
        assert_eq!(pending, vec!["pair-a".to_string(), "pair-b".to_string()]);
    }

    #[cfg(not(windows))]
    #[test]
    fn pairs_for_path_resolves_nested_paths() {
        let roots = vec![WatchedRoots {
            pair_id: "pair-1".into(),
            left: PathBuf::from("/var/left"),
            right: PathBuf::from("/var/right"),
        }];
        let ids = pairs_for_path(&roots, Path::new("/var/left/sub/file.txt"));
        assert_eq!(ids, vec!["pair-1".to_string()]);
    }
}
