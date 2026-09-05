use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Condvar;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

use crate::models::{PreviewActionPage, SyncPlan};
use crate::persistence::{DatabaseManager, PersistenceError};
use crate::scheduler::ScheduleService;
use crate::sniffer::SnifferService;
use crate::watcher::WatchService;

/// Ignore watch events for this long after a pair's sync completes (self-write feedback).
pub const WATCH_SUPPRESS_AFTER_RUN_MS: u64 = 2000;
/// Global heavy-job concurrency cap to protect disks, shares, CPU hashing, and SQLite.
pub const HEAVY_JOB_CAPACITY: usize = 2;
pub const PREVIEW_PAGE_MAX: usize = 200;
const PREVIEW_PLAN_TTL_MS: i64 = 5 * 60 * 1000;
const PREVIEW_PLAN_MAX_COUNT: usize = 8;
const PREVIEW_PLAN_MAX_BYTES: usize = 128 * 1024 * 1024;

struct StoredPreviewPlan {
    id: String,
    pair_id: String,
    config_fingerprint: String,
    created_at: i64,
    plan: SyncPlan,
    left_preconditions: Vec<crate::models::FileEntry>,
    right_preconditions: Vec<crate::models::FileEntry>,
    conflict_indexes: Vec<usize>,
    estimated_bytes: usize,
}

pub struct PreviewPlanStore {
    plans: VecDeque<StoredPreviewPlan>,
    bytes: usize,
}

impl PreviewPlanStore {
    pub fn new() -> Self {
        Self { plans: VecDeque::new(), bytes: 0 }
    }

    pub fn insert(
        &mut self,
        pair_id: String,
        config_fingerprint: String,
        plan: SyncPlan,
        left_preconditions: Vec<crate::models::FileEntry>,
        right_preconditions: Vec<crate::models::FileEntry>,
        now: i64,
    ) -> Result<String, String> {
        self.remove_expired(now);
        let id = uuid::Uuid::new_v4().to_string();
        let estimated_bytes = serde_json::to_vec(&plan).map(|bytes| bytes.len()).unwrap_or(0)
            + serde_json::to_vec(&left_preconditions).map(|bytes| bytes.len()).unwrap_or(0)
            + serde_json::to_vec(&right_preconditions).map(|bytes| bytes.len()).unwrap_or(0);
        if estimated_bytes > PREVIEW_PLAN_MAX_BYTES {
            return Err("preview plan exceeds the bounded storage limit".into());
        }
        self.bytes = self.bytes.saturating_add(estimated_bytes);
        let conflict_indexes = plan
            .actions
            .iter()
            .enumerate()
            .filter_map(|(index, action)| {
                matches!(action, crate::models::SyncAction::Conflict { .. }).then_some(index)
            })
            .collect();
        self.plans.push_back(StoredPreviewPlan {
            id: id.clone(),
            pair_id,
            config_fingerprint,
            created_at: now,
            plan,
            left_preconditions,
            right_preconditions,
            conflict_indexes,
            estimated_bytes,
        });
        self.trim_to_bounds();
        Ok(id)
    }

    pub fn get_page(
        &mut self,
        id: &str,
        pair_id: &str,
        config_fingerprint: &str,
        cursor: usize,
        limit: usize,
        now: i64,
    ) -> Result<PreviewActionPage, String> {
        if limit == 0 {
            return Err("preview page limit must be greater than zero".into());
        }
        self.remove_expired(now);
        let stored = self
            .plans
            .iter()
            .find(|plan| plan.id == id)
            .ok_or_else(|| "preview plan expired or not found".to_string())?;
        if stored.pair_id != pair_id || stored.config_fingerprint != config_fingerprint {
            return Err("preview plan no longer matches this pair configuration".into());
        }
        let cursor = cursor.min(stored.plan.actions.len());
        let end = cursor.saturating_add(limit.min(PREVIEW_PAGE_MAX)).min(stored.plan.actions.len());
        Ok(PreviewActionPage {
            plan_id: stored.id.clone(),
            cursor,
            next_cursor: (end < stored.plan.actions.len()).then_some(end),
            actions: stored.plan.actions[cursor..end].to_vec(),
        })
    }

    pub fn get_conflicts_page(
        &mut self,
        id: &str,
        pair_id: &str,
        config_fingerprint: &str,
        cursor: usize,
        limit: usize,
        now: i64,
    ) -> Result<PreviewActionPage, String> {
        if limit == 0 {
            return Err("preview conflict page limit must be greater than zero".into());
        }
        self.remove_expired(now);
        let stored = self
            .plans
            .iter()
            .find(|plan| plan.id == id)
            .ok_or_else(|| "preview plan expired or not found".to_string())?;
        if stored.pair_id != pair_id || stored.config_fingerprint != config_fingerprint {
            return Err("preview plan no longer matches this pair configuration".into());
        }
        let page_limit = limit.min(PREVIEW_PAGE_MAX);
        let cursor = cursor.min(stored.conflict_indexes.len());
        let end = cursor.saturating_add(page_limit).min(stored.conflict_indexes.len());
        let actions = stored.conflict_indexes[cursor..end]
            .iter()
            .map(|index| stored.plan.actions[*index].clone())
            .collect();
        Ok(PreviewActionPage {
            plan_id: stored.id.clone(),
            cursor,
            next_cursor: (end < stored.conflict_indexes.len()).then_some(end),
            actions,
        })
    }

    pub fn take(
        &mut self,
        id: &str,
        pair_id: &str,
        config_fingerprint: &str,
        now: i64,
    ) -> Result<(SyncPlan, Vec<crate::models::FileEntry>, Vec<crate::models::FileEntry>), String>
    {
        self.remove_expired(now);
        let index = self
            .plans
            .iter()
            .position(|plan| plan.id == id)
            .ok_or_else(|| "preview plan expired or not found".to_string())?;
        let stored = self.plans.get(index).expect("preview index exists");
        if stored.pair_id != pair_id || stored.config_fingerprint != config_fingerprint {
            return Err("preview plan no longer matches this pair configuration".into());
        }
        let stored = self.plans.remove(index).expect("preview index exists");
        self.bytes = self.bytes.saturating_sub(stored.estimated_bytes);
        Ok((stored.plan, stored.left_preconditions, stored.right_preconditions))
    }

    fn remove_expired(&mut self, now: i64) {
        while self
            .plans
            .front()
            .is_some_and(|plan| now.saturating_sub(plan.created_at) >= PREVIEW_PLAN_TTL_MS)
        {
            if let Some(plan) = self.plans.pop_front() {
                self.bytes = self.bytes.saturating_sub(plan.estimated_bytes);
            }
        }
    }

    fn trim_to_bounds(&mut self) {
        while self.plans.len() > PREVIEW_PLAN_MAX_COUNT || self.bytes > PREVIEW_PLAN_MAX_BYTES {
            if let Some(plan) = self.plans.pop_front() {
                self.bytes = self.bytes.saturating_sub(plan.estimated_bytes);
            } else {
                break;
            }
        }
    }
}

pub struct WorkCoordinator {
    heavy_jobs: Mutex<WorkState>,
    wake: Condvar,
    released: Arc<Notify>,
}

#[derive(Default)]
struct WorkState {
    available: usize,
    writers: Vec<PathBuf>,
    manual_waiters: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeavyJobKind {
    ManualRun,
    Preview,
    Watch,
    Scheduled,
    Duplicate,
    Sniffer,
}

#[derive(Clone)]
pub struct WorkRequest {
    pub roots: Vec<PathBuf>,
    pub writer: bool,
    pub kind: HeavyJobKind,
}

impl WorkRequest {
    pub fn new(roots: Vec<PathBuf>, writer: bool, kind: HeavyJobKind) -> Self {
        Self { roots, writer, kind }
    }
}

pub fn canonical_job_roots(paths: &[&str]) -> Vec<PathBuf> {
    paths
        .iter()
        .map(|path| {
            let candidate = PathBuf::from(path);
            std::fs::canonicalize(&candidate).unwrap_or_else(|_| {
                if candidate.is_absolute() {
                    candidate
                } else {
                    std::env::current_dir().unwrap_or_default().join(candidate)
                }
            })
        })
        .collect()
}

pub struct HeavyJobPermit {
    coordinator: Arc<WorkCoordinator>,
    writer: bool,
    roots: Vec<PathBuf>,
}

impl WorkCoordinator {
    pub fn new() -> Self {
        Self {
            heavy_jobs: Mutex::new(WorkState {
                available: HEAVY_JOB_CAPACITY,
                ..Default::default()
            }),
            wake: Condvar::new(),
            released: Arc::new(Notify::new()),
        }
    }

    pub fn acquire(self: &Arc<Self>, request: WorkRequest) -> Result<HeavyJobPermit, String> {
        let mut slots = self.heavy_jobs.lock().map_err(|e| e.to_string())?;
        while !can_admit(&slots, &request) || (slots.manual_waiters > 0 && slots.available <= 1) {
            slots = self.wake.wait(slots).map_err(|e| e.to_string())?;
        }
        self.admit(&mut slots, request)
    }

    pub fn acquire_manual(
        self: &Arc<Self>,
        request: WorkRequest,
    ) -> Result<HeavyJobPermit, String> {
        let mut slots = self.heavy_jobs.lock().map_err(|e| e.to_string())?;
        slots.manual_waiters += 1;
        while !can_admit(&slots, &request) {
            slots = self.wake.wait(slots).map_err(|e| e.to_string())?;
        }
        slots.manual_waiters -= 1;
        self.admit(&mut slots, request)
    }

    pub fn try_acquire(
        self: &Arc<Self>,
        request: WorkRequest,
    ) -> Result<Option<HeavyJobPermit>, String> {
        let mut slots = self.heavy_jobs.lock().map_err(|e| e.to_string())?;
        if !can_admit(&slots, &request) || (slots.manual_waiters > 0 && slots.available <= 1) {
            return Ok(None);
        }
        self.admit(&mut slots, request).map(Some)
    }

    pub async fn wait_for_release(self: &Arc<Self>, request: WorkRequest) {
        loop {
            let notified = self.released.notified();
            let ready = self
                .heavy_jobs
                .lock()
                .map(|slots| {
                    can_admit(&slots, &request)
                        && !(slots.manual_waiters > 0 && slots.available <= 1)
                })
                .unwrap_or(false);
            if ready {
                return;
            }
            notified.await;
        }
    }

    #[cfg(test)]
    fn manual_waiter_count(&self) -> usize {
        self.heavy_jobs.lock().map(|slots| slots.manual_waiters).unwrap_or(0)
    }

    fn admit(
        self: &Arc<Self>,
        slots: &mut WorkState,
        request: WorkRequest,
    ) -> Result<HeavyJobPermit, String> {
        let _job_kind = request.kind;
        slots.available -= 1;
        if request.writer {
            slots.writers.extend(request.roots.iter().cloned());
        }
        Ok(HeavyJobPermit {
            coordinator: Arc::clone(self),
            writer: request.writer,
            roots: request.roots,
        })
    }
}

fn can_admit(slots: &WorkState, request: &WorkRequest) -> bool {
    slots.available > 0
        && (!request.writer
            || !slots
                .writers
                .iter()
                .any(|active| request.roots.iter().any(|root| paths_overlap(active, root))))
}

fn paths_overlap(left: &PathBuf, right: &PathBuf) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

impl Drop for HeavyJobPermit {
    fn drop(&mut self) {
        if let Ok(mut slots) = self.coordinator.heavy_jobs.lock() {
            slots.available += 1;
            if self.writer {
                slots.writers.retain(|active| !self.roots.iter().any(|root| active == root));
            }
            self.coordinator.wake.notify_one();
            self.coordinator.released.notify_waiters();
        }
    }
}

pub struct AppState {
    pub work_coordinator: Arc<WorkCoordinator>,
    pub db: Arc<DatabaseManager>,
    /// Cancellation flags for active duplicate analysis jobs.
    pub duplicate_scan_cancels: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// Roots with an active legacy duplicate operation; rejecting repeats bounds queued work.
    pub active_duplicate_jobs: Mutex<HashSet<PathBuf>>,
    pub sniffer: Arc<SnifferService>,
    /// Per-pair cancel flags while a sync run is active.
    pub active_runs: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// Per-pair cancel flags while a preview scan is active.
    pub preview_cancels: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// Pair ids whose debounced watch sync could not start while that pair was busy.
    pub pending_watch_syncs: Mutex<HashSet<String>>,
    /// Pair ids whose scheduled sync could not start while that pair was busy.
    pub pending_schedule_syncs: Mutex<HashSet<String>>,
    /// Unified automatic queue bound: one pending job per pair regardless of reason.
    pub pending_automatic_syncs: Mutex<HashSet<String>>,
    /// Pair ids currently executing sync (manual, watch, or schedule).
    pub sync_in_progress: Mutex<HashSet<String>>,
    /// Ignore watch events for a pair until this instant (post-run suppression).
    pub watch_suppress_until: Mutex<HashMap<String, Instant>>,
    pub watch_service: Mutex<Option<WatchService>>,
    pub schedule_service: Mutex<Option<ScheduleService>>,
    pub preview_plans: Mutex<PreviewPlanStore>,
}

impl AppState {
    pub fn new(data_dir: PathBuf) -> Result<Self, PersistenceError> {
        let db_path = data_dir.join("syncforge.db");
        let db = DatabaseManager::open(&db_path)?;
        let sniffer = SnifferService::open(&data_dir.join("sniffer-cache"))
            .map_err(PersistenceError::Database)?;
        db.mark_duplicate_scans_interrupted()?;
        db.mark_sync_runs_interrupted()?;
        Ok(Self {
            work_coordinator: Arc::new(WorkCoordinator::new()),
            db: Arc::new(db),
            duplicate_scan_cancels: Mutex::new(HashMap::new()),
            active_duplicate_jobs: Mutex::new(HashSet::new()),
            sniffer: Arc::new(sniffer),
            active_runs: Mutex::new(HashMap::new()),
            preview_cancels: Mutex::new(HashMap::new()),
            pending_watch_syncs: Mutex::new(HashSet::new()),
            pending_schedule_syncs: Mutex::new(HashSet::new()),
            pending_automatic_syncs: Mutex::new(HashSet::new()),
            sync_in_progress: Mutex::new(HashSet::new()),
            watch_suppress_until: Mutex::new(HashMap::new()),
            watch_service: Mutex::new(None),
            schedule_service: Mutex::new(None),
            preview_plans: Mutex::new(PreviewPlanStore::new()),
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

    let watch_pending =
        state.pending_watch_syncs.lock().ok().is_some_and(|mut g| g.remove(pair_id));
    let schedule_pending =
        state.pending_schedule_syncs.lock().ok().is_some_and(|mut g| g.remove(pair_id));
    if watch_pending || schedule_pending {
        if let Ok(mut automatic) = state.pending_automatic_syncs.lock() {
            automatic.remove(pair_id);
        }
    }

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

pub(crate) fn enqueue_pending_watch_sync(state: &AppState, pair_id: impl Into<String>) -> bool {
    let pair_id = pair_id.into();
    if let Ok(mut automatic) = state.pending_automatic_syncs.lock() {
        if !automatic.insert(pair_id.clone()) {
            return false;
        }
    }
    if let Ok(mut guard) = state.pending_watch_syncs.lock() {
        return guard.insert(pair_id);
    }
    false
}

pub(crate) fn enqueue_pending_schedule_sync(state: &AppState, pair_id: impl Into<String>) -> bool {
    let pair_id = pair_id.into();
    if let Ok(mut automatic) = state.pending_automatic_syncs.lock() {
        if !automatic.insert(pair_id.clone()) {
            return false;
        }
    }
    if let Ok(mut guard) = state.pending_schedule_syncs.lock() {
        return guard.insert(pair_id);
    }
    false
}

pub(crate) fn dequeue_pending_watch_sync(state: &AppState, pair_id: &str) {
    if let Ok(mut guard) = state.pending_watch_syncs.lock() {
        guard.remove(pair_id);
    }
    if let Ok(mut automatic) = state.pending_automatic_syncs.lock() {
        automatic.remove(pair_id);
    }
}

pub(crate) fn dequeue_pending_schedule_sync(state: &AppState, pair_id: &str) {
    if let Ok(mut guard) = state.pending_schedule_syncs.lock() {
        guard.remove(pair_id);
    }
    if let Ok(mut automatic) = state.pending_automatic_syncs.lock() {
        automatic.remove(pair_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::sync::Barrier;
    use std::thread;
    use tempfile::TempDir;

    #[test]
    fn heavy_jobs_are_admitted_at_the_global_capacity() {
        let coordinator = Arc::new(WorkCoordinator::new());
        let permit_a = coordinator
            .acquire(WorkRequest::new(Vec::new(), false, HeavyJobKind::Watch))
            .expect("first permit");
        let permit_b = coordinator
            .acquire(WorkRequest::new(Vec::new(), false, HeavyJobKind::Watch))
            .expect("second permit");
        let (sender, receiver) = mpsc::channel();
        let started = Arc::new(Barrier::new(2));
        let worker_started = Arc::clone(&started);
        let worker_coordinator = Arc::clone(&coordinator);
        let worker = thread::spawn(move || {
            worker_started.wait();
            let _permit = worker_coordinator
                .acquire(WorkRequest::new(Vec::new(), false, HeavyJobKind::Watch))
                .expect("queued permit");
            sender.send(()).expect("send admission");
        });

        started.wait();
        assert!(receiver.try_recv().is_err());
        drop(permit_a);
        receiver.recv().expect("queued job admitted");
        drop(permit_b);
        worker.join().expect("worker joined");
    }

    #[tokio::test]
    async fn permit_release_notifies_queued_background_work() {
        let coordinator = Arc::new(WorkCoordinator::new());
        let permit_a = coordinator
            .acquire(WorkRequest::new(Vec::new(), false, HeavyJobKind::Watch))
            .expect("first permit");
        let permit_b = coordinator
            .acquire(WorkRequest::new(Vec::new(), false, HeavyJobKind::Watch))
            .expect("second permit");
        let waiter_coordinator = Arc::clone(&coordinator);
        let waiter = tokio::spawn(async move {
            waiter_coordinator
                .wait_for_release(WorkRequest::new(Vec::new(), false, HeavyJobKind::ManualRun))
                .await;
        });
        tokio::task::yield_now().await;
        drop(permit_a);
        waiter.await.expect("waiter notified");
        drop(permit_b);
    }

    #[test]
    fn overlapping_writer_roots_wait_for_each_other() {
        let coordinator = Arc::new(WorkCoordinator::new());
        let root = PathBuf::from("C:\\syncforge-test-root");
        let child = root.join("nested");
        let started = Arc::new(Barrier::new(2));
        let release_first = Arc::new(Barrier::new(2));
        let release_second = Arc::new(Barrier::new(2));
        let first_coordinator = Arc::clone(&coordinator);
        let first_started = Arc::clone(&started);
        let first_release = Arc::clone(&release_first);
        let active = Arc::new(AtomicUsize::new(0));
        let first_active = Arc::clone(&active);
        let (first_sender, first_receiver) = mpsc::channel();
        let first = thread::spawn(move || {
            let _permit = first_coordinator
                .acquire(WorkRequest::new(vec![root], true, HeavyJobKind::Watch))
                .expect("first writer permit");
            assert_eq!(first_active.fetch_add(1, Ordering::SeqCst), 0);
            first_started.wait();
            first_sender.send(()).expect("first writer started");
            first_release.wait();
            first_active.fetch_sub(1, Ordering::SeqCst);
        });

        let second_coordinator = Arc::clone(&coordinator);
        let second_started = Arc::clone(&started);
        let second_release = Arc::clone(&release_second);
        let second_active = Arc::clone(&active);
        let (second_sender, second_receiver) = mpsc::channel();
        let second = thread::spawn(move || {
            second_started.wait();
            let _permit = second_coordinator
                .acquire(WorkRequest::new(vec![child], true, HeavyJobKind::Scheduled))
                .expect("nested writer permit");
            let active_now = second_active.fetch_add(1, Ordering::SeqCst) + 1;
            second_sender.send(active_now).expect("second writer admitted");
            second_release.wait();
            second_active.fetch_sub(1, Ordering::SeqCst);
        });

        first_receiver.recv().expect("first writer admitted");
        assert!(second_receiver.try_recv().is_err());
        release_first.wait();
        assert_eq!(second_receiver.recv().expect("nested writer admitted"), 1);
        release_second.wait();
        first.join().expect("first writer joined");
        second.join().expect("second writer joined");
    }

    #[test]
    fn root_conflict_is_not_a_capacity_retry_loop() {
        let coordinator = Arc::new(WorkCoordinator::new());
        let root = PathBuf::from("C:\\syncforge-test-root");
        let child = root.join("nested");
        let permit = coordinator
            .acquire(WorkRequest::new(vec![root], true, HeavyJobKind::Watch))
            .expect("writer permit");
        assert!(coordinator
            .try_acquire(WorkRequest::new(vec![child.clone()], true, HeavyJobKind::Scheduled))
            .expect("try admission")
            .is_none());
        drop(permit);
        assert!(coordinator
            .try_acquire(WorkRequest::new(vec![child], true, HeavyJobKind::Scheduled))
            .expect("retry admission")
            .is_some());
    }

    #[test]
    fn automatic_queue_has_one_entry_per_pair_across_reasons() {
        let data_dir = TempDir::new().expect("tempdir");
        let state = AppState::new(data_dir.path().to_path_buf()).expect("app state");
        assert!(enqueue_pending_watch_sync(&state, "pair-a"));
        assert!(!enqueue_pending_schedule_sync(&state, "pair-a"));
        assert!(enqueue_pending_schedule_sync(&state, "pair-b"));
        assert_eq!(state.pending_automatic_syncs.lock().unwrap().len(), 2);
    }

    #[test]
    fn preview_plan_store_pages_bounds_and_validates_handles() {
        let mut store = PreviewPlanStore::new();
        let plan = SyncPlan {
            pair_id: "pair-a".into(),
            actions: (0..=PREVIEW_PAGE_MAX)
                .map(|index| crate::models::SyncAction::Skip {
                    path: format!("file-{index}.txt"),
                    reason: "test".into(),
                })
                .collect(),
            scanned_left: 0,
            scanned_right: 0,
            scan_skipped_left: 0,
            scan_skipped_right: 0,
            scan_warnings: vec![],
            requires_attention: false,
        };
        let id = store
            .insert("pair-a".into(), "fingerprint-a".into(), plan, vec![], vec![], 100)
            .expect("store plan");
        let first = store
            .get_page(&id, "pair-a", "fingerprint-a", 0, PREVIEW_PAGE_MAX + 50, 101)
            .expect("first page");
        assert_eq!(first.actions.len(), PREVIEW_PAGE_MAX);
        assert_eq!(first.next_cursor, Some(PREVIEW_PAGE_MAX));
        assert!(store.get_page(&id, "pair-a", "wrong", 0, 1, 101).is_err());
        assert!(store
            .get_page(&id, "pair-a", "fingerprint-a", 0, 1, 100 + PREVIEW_PLAN_TTL_MS)
            .is_err());
    }

    #[test]
    fn every_heavy_job_command_route_uses_the_same_admission_gate() {
        let coordinator = Arc::new(WorkCoordinator::new());
        let roots = vec![PathBuf::from("C:\\syncforge-route-test")];
        drop(crate::commands::run::admit_manual_run(&coordinator, roots.clone()).expect("manual"));
        drop(
            crate::commands::preview::admit_preview(&coordinator, roots.clone()).expect("preview"),
        );
        drop(
            crate::run_coordinator::admit_watch(&coordinator, roots.clone())
                .expect("watch")
                .expect("watch capacity"),
        );
        drop(
            crate::run_coordinator::admit_scheduled(&coordinator, roots.clone())
                .expect("scheduled")
                .expect("scheduled capacity"),
        );
        drop(
            crate::commands::duplicates::admit_duplicate(&coordinator, roots.clone())
                .expect("duplicate"),
        );
        drop(crate::commands::sniffer::admit_sniffer(&coordinator, roots, true).expect("sniffer"));
    }

    #[test]
    fn manual_reservation_prevents_background_slot_stealing() {
        let coordinator = Arc::new(WorkCoordinator::new());
        let permit_a = coordinator
            .acquire(WorkRequest::new(Vec::new(), false, HeavyJobKind::Watch))
            .expect("first background permit");
        let permit_b = coordinator
            .acquire(WorkRequest::new(Vec::new(), false, HeavyJobKind::Watch))
            .expect("second background permit");
        let manual_coordinator = Arc::clone(&coordinator);
        let (manual_sender, manual_receiver) = mpsc::channel();
        let manual = thread::spawn(move || {
            let permit = manual_coordinator
                .acquire_manual(WorkRequest::new(Vec::new(), true, HeavyJobKind::ManualRun))
                .expect("manual permit");
            manual_sender.send(()).expect("manual admission");
            permit
        });
        for _ in 0..10_000 {
            if coordinator.manual_waiter_count() == 1 {
                break;
            }
            thread::yield_now();
        }
        assert_eq!(coordinator.manual_waiter_count(), 1);
        drop(permit_a);
        assert!(coordinator
            .try_acquire(WorkRequest::new(Vec::new(), false, HeavyJobKind::Watch))
            .expect("background admission")
            .is_none());
        manual_receiver.recv().expect("manual job admitted");
        let manual_permit = manual.join().expect("manual worker joined");
        drop(manual_permit);
        drop(permit_b);
    }

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
        enqueue_pending_schedule_sync(&state, "pair-c");
        enqueue_pending_schedule_sync(&state, "pair-b");

        let mut watch = state.pending_watch_syncs.lock().expect("lock").drain().collect::<Vec<_>>();
        watch.sort();
        let mut schedule =
            state.pending_schedule_syncs.lock().expect("lock").drain().collect::<Vec<_>>();
        schedule.sort();

        assert_eq!(watch, vec!["pair-a".to_string()]);
        assert_eq!(schedule, vec!["pair-b".to_string(), "pair-c".to_string()]);
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

        assert_eq!(pending, (true, false));
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
