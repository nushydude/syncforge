use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tauri::AppHandle;

use crate::models::FolderPair;
use crate::path_normalization;
use crate::run_coordinator::run_watch_sync;
use crate::state::{should_ignore_watch_event, AppState};

/// Debounce window for filesystem events (bulk drops coalesce into one sync).
pub const DEBOUNCE_MS: u64 = 2000;

/// Tracks per-pair debounce deadlines; resets the timer on each new event.
#[derive(Debug)]
pub struct DebounceScheduler {
    delay: Duration,
    deadlines: HashMap<String, Instant>,
}

impl DebounceScheduler {
    pub fn new(delay: Duration) -> Self {
        Self { delay, deadlines: HashMap::new() }
    }

    pub fn touch(&mut self, pair_id: &str, now: Instant) {
        self.deadlines.insert(pair_id.to_string(), now + self.delay);
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
                Some(WatchedRoots { pair_id: pair.id.clone(), left, right })
            } else {
                None
            }
        })
        .collect()
}

pub struct WatchService {
    _watcher: RecommendedWatcher,
    _debounce_thread: std::thread::JoinHandle<()>,
}

impl WatchService {
    pub fn start(app: AppHandle, state: Arc<AppState>) -> Result<Self, String> {
        let pairs =
            state.db.lock().map_err(|e| e.to_string())?.list_pairs().map_err(|e| e.to_string())?;
        let roots = Arc::new(Mutex::new(build_roots(&pairs)));
        let (event_tx, event_rx) = mpsc::channel::<String>();

        let watcher = Self::build_watcher(Arc::clone(&state), Arc::clone(&roots), event_tx)?;

        let app_debounce = app.clone();
        let state_debounce = Arc::clone(&state);
        let debounce_thread = std::thread::spawn(move || {
            let mut scheduler = DebounceScheduler::new(Duration::from_millis(DEBOUNCE_MS));
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

        Ok(Self { _watcher: watcher, _debounce_thread: debounce_thread })
    }

    fn build_watcher(
        state: Arc<AppState>,
        roots: Arc<Mutex<Vec<WatchedRoots>>>,
        event_tx: mpsc::Sender<String>,
    ) -> Result<RecommendedWatcher, String> {
        let roots_for_callback = Arc::clone(&roots);
        let state_for_callback = Arc::clone(&state);
        let mut watcher = RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                let Ok(event) = res else {
                    return;
                };
                if !matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
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
                    if should_ignore_watch_event(&state_for_callback, &pair_id) {
                        continue;
                    }
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
    use crate::state::{enqueue_pending_watch_sync, AppState};
    use std::sync::Arc;
    use tempfile::TempDir;

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
        let ids = pairs_for_path(&roots, Path::new(r"\\server\share\left\sub\file.txt"));
        assert_eq!(ids, vec!["pair-1".to_string()]);

        let ids = pairs_for_path(&roots, Path::new(r"c:\pairs\right\doc.txt"));
        assert_eq!(ids, vec!["pair-1".to_string()]);
    }

    #[test]
    fn pending_watch_sync_queue_dedupes_pair_ids() {
        let data_dir = TempDir::new().expect("tempdir");
        let state = Arc::new(AppState::new(data_dir.path().to_path_buf()).expect("app state"));
        enqueue_pending_watch_sync(&state, "pair-a");
        enqueue_pending_watch_sync(&state, "pair-a");
        enqueue_pending_watch_sync(&state, "pair-b");

        let mut pending =
            state.pending_watch_syncs.lock().expect("lock").drain().collect::<Vec<_>>();
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
