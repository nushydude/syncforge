use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use tauri::{AppHandle, State};

use crate::models::FolderPair;
use crate::path_normalization;
use crate::run_coordinator::{emit_event, run_watch_sync};
use crate::state::{AppState, RegisteredWatch};

pub const DEBOUNCE_MS: u64 = 2000;
const MAX_DEBOUNCE_MS: u64 = 10_000;
const REGISTRATION_RETRY: Duration = Duration::from_secs(5);
const SUPERVISOR_TICK: Duration = Duration::from_millis(100);

/// Bounded per-pair debounce: continuous changes are serviced by a maximum deadline.
#[derive(Debug)]
pub struct DebounceScheduler {
    delay: Duration,
    max_delay: Duration,
    deadlines: HashMap<String, (Instant, Instant)>,
}
impl DebounceScheduler {
    pub fn new(delay: Duration) -> Self {
        Self { delay, max_delay: Duration::from_millis(MAX_DEBOUNCE_MS), deadlines: HashMap::new() }
    }
    pub fn touch(&mut self, pair_id: &str, now: Instant) {
        let value = self
            .deadlines
            .entry(pair_id.to_string())
            .or_insert((now + self.delay, now + self.max_delay));
        value.0 = (now + self.delay).min(value.1);
    }
    pub fn take_ready(&mut self, now: Instant) -> Vec<String> {
        let ready: Vec<_> = self
            .deadlines
            .iter()
            .filter(|(_, (deadline, _))| now >= *deadline)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ready {
            self.deadlines.remove(id);
        }
        ready
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct RootKey {
    pair_id: String,
    side: &'static str,
    path: PathBuf,
}

fn desired_roots(pairs: &[FolderPair]) -> HashSet<RootKey> {
    pairs
        .iter()
        .filter(|p| p.enabled && p.watch_enabled)
        .flat_map(|pair| {
            [
                RootKey {
                    pair_id: pair.id.clone(),
                    side: "left",
                    path: PathBuf::from(path_normalization::to_long_path(&pair.left_path)),
                },
                RootKey {
                    pair_id: pair.id.clone(),
                    side: "right",
                    path: PathBuf::from(path_normalization::to_long_path(&pair.right_path)),
                },
            ]
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchHealth {
    pub pair_id: String,
    pub status: &'static str,
    pub issues: Vec<String>,
}

struct RootSlot<W> {
    watcher: Option<W>,
    identity: Option<String>,
    error: Option<String>,
    failed_once: bool,
}

/// Reconciles each registration independently; failed roots never drop healthy handles.
fn reconcile_roots<W>(
    slots: &mut HashMap<RootKey, RootSlot<W>>,
    desired: &HashSet<RootKey>,
    mut register: impl FnMut(&RootKey) -> Result<(W, String), String>,
) -> HashSet<String> {
    slots.retain(|key, _| desired.contains(key));
    let mut recovered = HashSet::new();
    for key in desired {
        let slot = slots.entry(key.clone()).or_insert_with(|| RootSlot {
            watcher: None,
            identity: None,
            error: None,
            failed_once: false,
        });
        if slot.watcher.is_some() {
            continue;
        }
        match register(key) {
            Ok((watcher, identity)) => {
                slot.watcher = Some(watcher);
                slot.identity = Some(identity);
                slot.error = None;
                if slot.failed_once {
                    recovered.insert(key.pair_id.clone());
                }
                slot.failed_once = false;
            }
            Err(error) => {
                slot.error = Some(error);
                slot.failed_once = true;
            }
        }
    }
    recovered
}

fn invalidate_changed_roots<W>(
    slots: &mut HashMap<RootKey, RootSlot<W>>,
    mut identify: impl FnMut(&RootKey) -> Result<String, String>,
) {
    for (key, slot) in slots {
        if slot.watcher.is_none() {
            continue;
        }
        let observed = identify(key);
        if observed.as_ref().ok() != slot.identity.as_ref() {
            slot.watcher = None;
            slot.identity = None;
            slot.error = Some(observed.err().unwrap_or_else(|| "folder identity changed".into()));
            slot.failed_once = true;
        }
    }
}

fn watch_configs(pairs: &[FolderPair]) -> HashMap<String, String> {
    pairs
        .iter()
        .filter(|pair| pair.enabled && pair.watch_enabled)
        .map(|pair| (pair.id.clone(), crate::commands::preview::config_fingerprint(pair)))
        .collect()
}

fn healthy_pairs<W>(
    slots: &HashMap<RootKey, RootSlot<W>>,
    configs: &HashMap<String, String>,
) -> HashMap<String, RegisteredWatch> {
    let mut roots: HashMap<String, Vec<(PathBuf, String)>> = HashMap::new();
    let mut unhealthy = HashSet::new();
    for (key, slot) in slots {
        if slot.watcher.is_some() && slot.error.is_none() {
            if let Some(identity) = &slot.identity {
                roots
                    .entry(key.pair_id.clone())
                    .or_default()
                    .push((key.path.clone(), identity.clone()));
                continue;
            }
        }
        unhealthy.insert(key.pair_id.clone());
    }
    roots
        .into_iter()
        .filter_map(|(id, paths)| {
            if paths.len() != 2 || unhealthy.contains(&id) {
                return None;
            }
            configs.get(&id).map(|config_fingerprint| {
                (
                    id,
                    RegisteredWatch {
                        config_fingerprint: config_fingerprint.clone(),
                        roots: paths,
                    },
                )
            })
        })
        .collect()
}

fn snapshot_health<W>(slots: &HashMap<RootKey, RootSlot<W>>) -> Vec<WatchHealth> {
    let mut issues: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for (key, slot) in slots {
        let list = issues.entry(key.pair_id.clone()).or_default();
        if let Some(error) = &slot.error {
            list.push(format!("{} folder ({}): {}", key.side, key.path.display(), error));
        }
    }
    issues
        .into_iter()
        .map(|(pair_id, mut issues)| {
            issues.sort();
            WatchHealth {
                pair_id,
                status: if issues.is_empty() { "watching" } else { "degraded" },
                issues,
            }
        })
        .collect()
}

enum Control {
    Refresh(Vec<FolderPair>, u64),
    Stop,
}
pub struct WatchService {
    control: Sender<Control>,
    health: Arc<Mutex<Vec<WatchHealth>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Drop for WatchService {
    fn drop(&mut self) {
        let _ = self.control.send(Control::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
impl WatchService {
    fn start(app: AppHandle, state: Arc<AppState>, pairs: Vec<FolderPair>, epoch: u64) -> Self {
        let (control, rx) = mpsc::channel();
        let health = Arc::new(Mutex::new(Vec::new()));
        let health_thread = Arc::clone(&health);
        let thread = std::thread::spawn(move || {
            let dirty = Arc::new(Mutex::new(HashSet::<String>::new()));
            let faults = Arc::new(Mutex::new(HashMap::<RootKey, String>::new()));
            let mut desired = desired_roots(&pairs);
            let mut configs = watch_configs(&pairs);
            let mut current_epoch = epoch;
            let mut slots = HashMap::<RootKey, RootSlot<RecommendedWatcher>>::new();
            let mut debounce = DebounceScheduler::new(Duration::from_millis(DEBOUNCE_MS));
            let mut retry_at = Instant::now();
            let mut last_health = Vec::new();
            loop {
                match rx.recv_timeout(SUPERVISOR_TICK) {
                    Ok(Control::Refresh(pairs, epoch)) => {
                        current_epoch = epoch;
                        desired = desired_roots(&pairs);
                        configs = watch_configs(&pairs);
                        let active_pairs: HashSet<_> =
                            desired.iter().map(|root| root.pair_id.as_str()).collect();
                        if let Ok(mut failed) = state.watch_failed_fingerprints.lock() {
                            failed.retain(|pair_id, _| active_pairs.contains(pair_id.as_str()));
                        }
                        retry_at = Instant::now();
                    }
                    Ok(Control::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                    Err(RecvTimeoutError::Timeout) => {}
                }
                let mut probe_now = false;
                // Consume faults under the gate lock, matching the callback's lock order.
                // A fault arriving later leaves recheck set and blocks stale publication.
                if let Ok(mut gate) = state.watch_gate.lock() {
                    if let Ok(mut errors) = faults.lock() {
                        for (key, error) in errors.drain() {
                            if let Some(slot) = slots.get_mut(&key) {
                                slot.watcher = None;
                                slot.identity = None;
                                slot.error = Some(error);
                                slot.failed_once = true;
                                probe_now = true;
                            }
                        }
                    }
                    probe_now |= gate.take_recheck();
                }
                let now = Instant::now();
                if probe_now {
                    retry_at = now;
                }
                if now >= retry_at {
                    // Detect unavailable or replaced roots, even when the same pathname still exists.
                    invalidate_changed_roots(&mut slots, |key| {
                        if !key.path.is_dir() {
                            return Err("folder is unavailable".into());
                        }
                        path_normalization::root_identity(&key.path)
                            .map_err(|e| format!("cannot verify folder identity: {e}"))
                    });
                    let recovered = reconcile_roots(&mut slots, &desired, |key| {
                        let key_callback = key.clone();
                        let dirty_callback = Arc::clone(&dirty);
                        let faults_callback = Arc::clone(&faults);
                        let state_callback = Arc::clone(&state);
                        if !key.path.is_dir() {
                            return Err("folder is unavailable".into());
                        }
                        let identity = path_normalization::root_identity(&key.path)
                            .map_err(|e| format!("cannot verify folder identity: {e}"))?;
                        let mut watcher = RecommendedWatcher::new(
                            move |result: Result<notify::Event, notify::Error>| match result {
                                Ok(event)
                                    if matches!(
                                        event.kind,
                                        EventKind::Create(_)
                                            | EventKind::Modify(_)
                                            | EventKind::Remove(_)
                                    ) =>
                                {
                                    if let Ok(mut dirty) = dirty_callback.lock() {
                                        dirty.insert(key_callback.pair_id.clone());
                                    }
                                }
                                Err(error) => {
                                    if let Ok(mut gate) = state_callback.watch_gate.lock() {
                                        gate.mark_unhealthy(&key_callback.pair_id);
                                        if let Ok(mut faults) = faults_callback.lock() {
                                            faults.insert(key_callback.clone(), error.to_string());
                                        }
                                    }
                                }
                                _ => {}
                            },
                            Config::default(),
                        )
                        .map_err(|e| e.to_string())?;
                        watcher
                            .watch(&key.path, RecursiveMode::Recursive)
                            .map_err(|e| e.to_string())?;
                        let after = path_normalization::root_identity(&key.path)
                            .map_err(|e| format!("cannot verify folder identity: {e}"))?;
                        if identity != after {
                            return Err("folder identity changed during registration".into());
                        }
                        Ok((watcher, identity))
                    });
                    if let Ok(mut dirty) = dirty.lock() {
                        dirty.extend(recovered);
                    }
                    retry_at = now + REGISTRATION_RETRY;
                }
                let configured: HashSet<String> =
                    desired.iter().map(|root| root.pair_id.clone()).collect();
                let recovered_pending = state
                    .watch_gate
                    .lock()
                    .map(|mut gate| {
                        gate.publish(current_epoch, healthy_pairs(&slots, &configs), &configured)
                    })
                    .unwrap_or_default();
                if let Ok(mut dirty) = dirty.lock() {
                    dirty.extend(recovered_pending);
                }
                let current = snapshot_health(&slots);
                if current != last_health {
                    if let Ok(mut shared) = health_thread.lock() {
                        *shared = current.clone();
                    }
                    let _ = emit_event(&app, "watch://health", &current);
                    last_health = current;
                }
                if let Ok(mut dirty) = dirty.lock() {
                    for id in dirty.drain() {
                        debounce.touch(&id, now);
                    }
                }
                // This runs on every iteration, even under sustained control or event traffic.
                for pair_id in debounce.take_ready(now) {
                    run_watch_sync(app.clone(), Arc::clone(&state), pair_id);
                }
            }
        });
        Self { control, health, thread: Some(thread) }
    }
}

#[tauri::command]
pub fn get_watch_health(state: State<'_, Arc<AppState>>) -> Result<Vec<WatchHealth>, String> {
    let service = state.watch_service.lock().map_err(|e| e.to_string())?;
    match service.as_ref() {
        Some(service) => {
            service.health.lock().map(|value| value.clone()).map_err(|e| e.to_string())
        }
        None => Ok(Vec::new()),
    }
}

pub fn refresh_watch_service(app: &AppHandle, state: &Arc<AppState>) -> Result<(), String> {
    // Serialize refreshes so an older configuration cannot replace a newer one.
    let mut guard = state.watch_service.lock().map_err(|e| e.to_string())?;
    let pairs = state.db.list_pairs().map_err(|e| e.to_string())?;
    let epoch = state.watch_gate.lock().map_err(|e| e.to_string())?.invalidate();
    if let Some(service) = guard.as_ref() {
        service.control.send(Control::Refresh(pairs, epoch)).map_err(|e| e.to_string())?;
    } else {
        *guard = Some(WatchService::start(app.clone(), Arc::clone(state), pairs, epoch));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deadlines_run_under_continuous_unrelated_events_and_bound_active_pair() {
        let start = Instant::now();
        let mut scheduler = DebounceScheduler::new(Duration::from_millis(100));
        scheduler.max_delay = Duration::from_millis(300);
        scheduler.touch("quiet", start);
        for millis in (0..=350).step_by(10) {
            let now = start + Duration::from_millis(millis);
            scheduler.touch("busy", now);
            let ready = scheduler.take_ready(now);
            if millis == 100 {
                assert!(ready.contains(&"quiet".into()));
            }
            if millis == 300 {
                assert!(ready.contains(&"busy".into()));
            }
        }
    }
    #[test]
    fn degraded_ready_tick_retains_one_catch_up_until_both_roots_recover() {
        let temp = tempfile::TempDir::new().unwrap();
        let left = temp.path().join("left");
        let right = temp.path().join("right");
        std::fs::create_dir(&left).unwrap();
        std::fs::create_dir(&right).unwrap();
        let key = |side, path: PathBuf| RootKey { pair_id: "pair".into(), side, path };
        let desired = HashSet::from([key("left", left.clone()), key("right", right)]);
        let mut slots = HashMap::new();
        reconcile_roots(&mut slots, &desired, |root| {
            if root.side == "right" {
                Err("offline".into())
            } else {
                Ok(((), "left-id".into()))
            }
        });
        let configured = HashSet::from(["pair".to_string()]);
        let configs = HashMap::from([("pair".to_string(), "config".to_string())]);
        let mut gate = crate::state::WatchGate::default();
        let epoch = gate.invalidate();
        assert!(gate.publish(epoch, healthy_pairs(&slots, &configs), &configured).is_empty());
        let mut debounce = DebounceScheduler::new(Duration::from_millis(1));
        let now = Instant::now();
        debounce.touch("pair", now);
        for pair in debounce.take_ready(now + Duration::from_millis(1)) {
            assert!(!gate.request_with(&pair, "config", |_| Some("left-id".into())));
        }
        // Release and capacity retry routes may ask again while degraded.
        assert!(!gate.request_with("pair", "config", |_| Some("left-id".into())));
        assert!(!gate.request_with("pair", "config", |_| Some("left-id".into())));
        reconcile_roots(&mut slots, &desired, |_| Ok(((), "right-id".into())));
        assert_eq!(gate.publish(epoch, healthy_pairs(&slots, &configs), &configured), vec!["pair"]);
        assert!(gate.publish(epoch, healthy_pairs(&slots, &configs), &configured).is_empty());
        assert!(gate.request_with("pair", "config", |path| Some(
            if path == left { "left-id" } else { "right-id" }.into()
        )));
        gate.mark_unhealthy("pair");
        assert!(gate.publish(epoch, healthy_pairs(&slots, &configs), &configured).is_empty());
        assert!(!gate.request_with("pair", "config", |_| Some("left-id".into())));
        assert!(gate.take_recheck());
        assert_eq!(gate.publish(epoch, healthy_pairs(&slots, &configs), &configured), vec!["pair"]);
        let new_epoch = gate.invalidate();
        assert!(!gate.request_with("pair", "config", |_| Some("left-id".into())));
        assert!(gate.publish(epoch, healthy_pairs(&slots, &configs), &configured).is_empty());
        assert!(!gate.request_with("pair", "config", |_| Some("left-id".into())));
        assert_eq!(
            gate.publish(new_epoch, healthy_pairs(&slots, &configs), &configured),
            vec!["pair"]
        );
    }

    #[test]
    fn dispatch_detects_replaced_root_before_periodic_probe() {
        let temp = tempfile::TempDir::new().unwrap();
        let left = temp.path().join("left");
        let right = temp.path().join("right");
        std::fs::create_dir(&left).unwrap();
        std::fs::create_dir(&right).unwrap();
        let roots = vec![
            (left.clone(), path_normalization::root_identity(&left).unwrap()),
            (right, path_normalization::root_identity(&temp.path().join("right")).unwrap()),
        ];
        let mut gate = crate::state::WatchGate::default();
        let epoch = gate.invalidate();
        let configured = HashSet::from(["pair".to_string()]);
        gate.publish(
            epoch,
            HashMap::from([(
                "pair".into(),
                RegisteredWatch { config_fingerprint: "config".into(), roots },
            )]),
            &configured,
        );
        assert!(gate
            .request_with("pair", "config", |path| path_normalization::root_identity(path).ok()));
        std::fs::rename(&left, temp.path().join("old-left")).unwrap();
        std::fs::create_dir(&left).unwrap();
        assert!(!gate
            .request_with("pair", "config", |path| path_normalization::root_identity(path).ok()));
        assert!(gate.take_recheck());
        assert!(!gate.take_recheck());
    }

    #[test]
    fn changed_saved_pair_cannot_use_old_registration_and_gets_one_catch_up() {
        use crate::models::{ConflictPolicy, Filters, SyncMode};
        let temp = tempfile::TempDir::new().unwrap();
        let left = temp.path().join("left");
        let right = temp.path().join("right");
        let new_right = temp.path().join("new-right");
        for path in [&left, &right, &new_right] {
            std::fs::create_dir(path).unwrap();
        }
        let original = FolderPair {
            id: "pair".into(),
            name: "test".into(),
            left_path: left.to_string_lossy().into_owned(),
            right_path: right.to_string_lossy().into_owned(),
            mode: SyncMode::Echo,
            filters: Filters::default(),
            conflict_policy: ConflictPolicy::NewerWins,
            enabled: true,
            continue_on_error: false,
            watch_enabled: true,
            schedule_enabled: false,
            schedule_cron: None,
            created_at: 1,
            updated_at: 1,
        };
        let registered = |pair: &FolderPair| RegisteredWatch {
            config_fingerprint: crate::commands::preview::config_fingerprint(pair),
            roots: [&pair.left_path, &pair.right_path]
                .iter()
                .map(|path| {
                    let path = PathBuf::from(path);
                    let identity = path_normalization::root_identity(&path).unwrap();
                    (path, identity)
                })
                .collect(),
        };
        let configured = HashSet::from([original.id.clone()]);
        for change in 0..3 {
            let mut gate = crate::state::WatchGate::default();
            let epoch = gate.invalidate();
            gate.publish(
                epoch,
                HashMap::from([(original.id.clone(), registered(&original))]),
                &configured,
            );
            assert!(gate.request(&original));
            let mut changed = original.clone();
            match change {
                0 => changed.right_path = new_right.to_string_lossy().into_owned(),
                1 => changed.mode = SyncMode::Synchronize,
                _ => changed.filters.exclude.push("*.tmp".into()),
            }
            // The database is now newer, but the supervisor has not processed refresh.
            assert!(!gate.request(&changed));
            assert!(!gate.request(&changed));
            let refreshed = gate.invalidate();
            // An old supervisor pass cannot restore authorization.
            assert!(gate
                .publish(
                    epoch,
                    HashMap::from([(original.id.clone(), registered(&original))]),
                    &configured
                )
                .is_empty());
            assert!(gate.take_recheck());
            assert_eq!(
                gate.publish(
                    refreshed,
                    HashMap::from([(changed.id.clone(), registered(&changed))]),
                    &configured
                ),
                vec!["pair"]
            );
            assert!(gate.request(&changed));
            assert!(gate
                .publish(
                    refreshed,
                    HashMap::from([(changed.id.clone(), registered(&changed))]),
                    &configured
                )
                .is_empty());
        }
    }

    #[test]
    fn replacement_directory_forces_registration_refresh_and_catch_up() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("watched");
        std::fs::create_dir(&root).unwrap();
        let key = RootKey { pair_id: "pair".into(), side: "left", path: root.clone() };
        let desired = HashSet::from([key.clone()]);
        let mut slots = HashMap::new();
        reconcile_roots(&mut slots, &desired, |root| {
            Ok(((), path_normalization::root_identity(&root.path).unwrap()))
        });
        std::fs::rename(&root, temp.path().join("old")).unwrap();
        std::fs::create_dir(&root).unwrap();
        invalidate_changed_roots(&mut slots, |root| {
            path_normalization::root_identity(&root.path).map_err(|e| e.to_string())
        });
        assert!(slots[&key].watcher.is_none());
        assert_eq!(snapshot_health(&slots)[0].status, "degraded");
        let recovered = reconcile_roots(&mut slots, &desired, |root| {
            Ok(((), path_normalization::root_identity(&root.path).unwrap()))
        });
        assert_eq!(recovered, HashSet::from(["pair".to_string()]));
        assert_eq!(snapshot_health(&slots)[0].status, "watching");
    }

    #[test]
    fn registration_failure_is_isolated_and_recovers_without_replacing_healthy_handle() {
        let key = |id: &str| RootKey { pair_id: id.into(), side: "left", path: PathBuf::from(id) };
        let desired = HashSet::from([key("healthy"), key("broken")]);
        let mut slots = HashMap::new();
        let mut calls = HashMap::<String, usize>::new();
        let first = reconcile_roots(&mut slots, &desired, |root| {
            *calls.entry(root.pair_id.clone()).or_default() += 1;
            if root.pair_id == "broken" {
                Err("offline".into())
            } else {
                Ok((42, "healthy-id".into()))
            }
        });
        assert!(first.is_empty());
        assert_eq!(
            snapshot_health(&slots).iter().find(|s| s.pair_id == "healthy").unwrap().status,
            "watching"
        );
        assert_eq!(
            snapshot_health(&slots).iter().find(|s| s.pair_id == "broken").unwrap().status,
            "degraded"
        );
        let recovered = reconcile_roots(&mut slots, &desired, |root| {
            *calls.entry(root.pair_id.clone()).or_default() += 1;
            Ok((99, "recovered-id".into()))
        });
        assert_eq!(recovered, HashSet::from(["broken".into()]));
        assert_eq!(calls["healthy"], 1);
        assert_eq!(slots[&key("healthy")].watcher, Some(42));
        assert!(snapshot_health(&slots).iter().all(|s| s.status == "watching"));
    }
}
