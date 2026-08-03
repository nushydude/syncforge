use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use cron::Schedule;
use tauri::AppHandle;
use tokio::sync::Notify;

use crate::models::FolderPair;
use crate::run_coordinator::run_scheduled_sync;
use crate::state::AppState;

type DeadlineState = std::collections::HashMap<String, (String, DateTime<Utc>)>;
type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

pub fn validate_cron_expression(expr: &str) -> Result<(), String> {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return Err("cron expression is required".into());
    }
    Schedule::from_str(&normalize_cron_expression(trimmed))
        .map_err(|e| format!("invalid cron expression: {e}"))?;
    Ok(())
}

fn normalize_cron_expression(expr: &str) -> String {
    let field_count = expr.split_whitespace().count();
    if field_count == 5 {
        format!("0 {expr}")
    } else {
        expr.to_string()
    }
}

fn advance_deadline(
    schedule: &Schedule,
    previous: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let mut next = schedule.after(&previous).next()?;
    while next <= now {
        next = schedule.after(&next).next()?;
    }
    Some(next)
}

#[derive(Debug, Clone)]
struct ScheduledPair {
    pair_id: String,
    cron: String,
    next_run: DateTime<Utc>,
}

#[derive(Debug, Clone, Default)]
struct SchedulerState {
    deadlines: DeadlineState,
}

impl SchedulerState {
    fn from_deadlines(deadlines: DeadlineState) -> Self {
        Self { deadlines }
    }

    fn refresh(&mut self, scheduled: &[ScheduledPair]) {
        self.deadlines.retain(|pair_id, _| scheduled.iter().any(|entry| &entry.pair_id == pair_id));
        for entry in scheduled {
            self.deadlines
                .entry(entry.pair_id.clone())
                .and_modify(|(cron, deadline)| {
                    if cron != &entry.cron {
                        *cron = entry.cron.clone();
                        *deadline = entry.next_run;
                    }
                })
                .or_insert_with(|| (entry.cron.clone(), entry.next_run));
        }
    }

    fn due(&self, now: DateTime<Utc>) -> Vec<String> {
        self.deadlines
            .iter()
            .filter(|(_, (_, deadline))| *deadline <= now)
            .map(|(pair_id, _)| pair_id.clone())
            .collect()
    }

    fn advance(&mut self, pair_id: &str, now: DateTime<Utc>) {
        if let Some((cron, deadline)) = self.deadlines.get_mut(pair_id) {
            if let Ok(schedule) = Schedule::from_str(cron) {
                *deadline = advance_deadline(&schedule, *deadline, now)
                    .unwrap_or(now + chrono::Duration::days(1));
            }
        }
    }

    fn next_wake(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.deadlines.values().map(|(_, next)| *next).filter(|next| *next > now).min()
    }
}

fn scheduler_tick<F>(
    state: &mut SchedulerState,
    scheduled: &[ScheduledPair],
    now: DateTime<Utc>,
    mut enqueue: F,
) -> Option<DateTime<Utc>>
where
    F: FnMut(&str),
{
    state.refresh(scheduled);
    for pair_id in state.due(now) {
        enqueue(&pair_id);
        state.advance(&pair_id, now);
    }
    state.next_wake(now)
}

fn build_scheduled_pairs(pairs: &[FolderPair], now: DateTime<Utc>) -> Vec<ScheduledPair> {
    let mut scheduled = Vec::new();
    for pair in pairs {
        if !pair.enabled || !pair.schedule_enabled {
            continue;
        }
        let Some(cron) = pair.schedule_cron.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty())
        else {
            continue;
        };
        let Ok(schedule) = Schedule::from_str(&normalize_cron_expression(cron)) else {
            eprintln!("scheduler: skipping pair {} with invalid cron", pair.name);
            continue;
        };
        let Some(next_run) = schedule.after(&now).next() else {
            continue;
        };
        scheduled.push(ScheduledPair {
            pair_id: pair.id.clone(),
            cron: normalize_cron_expression(cron),
            next_run,
        });
    }
    scheduled
}

pub struct ScheduleService {
    cancel: Arc<AtomicBool>,
    wake: Arc<Notify>,
    deadlines: Arc<std::sync::Mutex<DeadlineState>>,
}

impl ScheduleService {
    fn start_with_deadlines(
        app: AppHandle,
        state: Arc<AppState>,
        deadline_state: Arc<std::sync::Mutex<DeadlineState>>,
        clock: Clock,
    ) -> Result<Self, String> {
        let cancel = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(Notify::new());
        let cancel_task = Arc::clone(&cancel);
        let wake_task = Arc::clone(&wake);
        let app_task = app.clone();
        let state_task = Arc::clone(&state);
        let deadline_state_task = Arc::clone(&deadline_state);
        let load_clock = Arc::clone(&clock);
        let load_state = Arc::clone(&state_task);
        let enqueue_state = Arc::clone(&state_task);
        let enqueue_app = app_task.clone();

        tauri::async_runtime::spawn(run_scheduler_loop(
            cancel_task,
            wake_task,
            deadline_state_task,
            clock,
            move || match load_state.db.lock() {
                Ok(guard) => match guard.list_pairs() {
                    Ok(pairs) => build_scheduled_pairs(&pairs, (load_clock)()),
                    Err(e) => {
                        eprintln!("scheduler: list pairs failed: {e}");
                        Vec::new()
                    }
                },
                Err(e) => {
                    eprintln!("scheduler: db lock failed: {e}");
                    Vec::new()
                }
            },
            move |pair_id| {
                run_scheduled_sync(enqueue_app.clone(), Arc::clone(&enqueue_state), pair_id);
            },
        ));

        Ok(Self { cancel, wake, deadlines: deadline_state })
    }

    pub fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.wake.notify_one();
    }
}

async fn run_scheduler_loop<L, E>(
    cancel: Arc<AtomicBool>,
    wake: Arc<Notify>,
    deadline_state: Arc<std::sync::Mutex<DeadlineState>>,
    clock: Clock,
    mut load_scheduled: L,
    enqueue: E,
) where
    L: FnMut() -> Vec<ScheduledPair> + Send + 'static,
    E: Fn(String) + Send + 'static,
{
    let initial = deadline_state.lock().map(|state| state.clone()).unwrap_or_default();
    let mut scheduler_state = SchedulerState::from_deadlines(initial);
    loop {
        if cancel.load(Ordering::Relaxed) {
            break;
        }

        let scheduled = load_scheduled();
        let now = (clock)();
        let sleep_until = scheduler_tick(&mut scheduler_state, &scheduled, now, |pair_id| {
            enqueue(pair_id.to_string());
        });
        let sleep_for = sleep_until
            .map(|next| {
                (next - now).to_std().unwrap_or(Duration::from_secs(1)).max(Duration::from_secs(1))
            })
            .unwrap_or(Duration::from_secs(30));

        if let Ok(mut shared) = deadline_state.lock() {
            *shared = scheduler_state.deadlines.clone();
        }

        tokio::select! {
            _ = tokio::time::sleep(sleep_for) => {}
            _ = wake.notified() => {}
        }
    }
}

pub fn refresh_schedule_service(app: &AppHandle, state: &Arc<AppState>) -> Result<(), String> {
    let mut guard = state.schedule_service.lock().map_err(|e| e.to_string())?;
    let deadline_state = if let Some(service) = guard.take() {
        let deadlines = Arc::clone(&service.deadlines);
        service.stop();
        deadlines
    } else {
        Arc::new(std::sync::Mutex::new(DeadlineState::new()))
    };
    let service = ScheduleService::start_with_deadlines(
        app.clone(),
        Arc::clone(state),
        deadline_state,
        Arc::new(Utc::now),
    )?;
    *guard = Some(service);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn validate_cron_accepts_standard_expression() {
        assert!(validate_cron_expression("0 9 * * *").is_ok());
    }

    #[test]
    fn validate_cron_rejects_empty_expression() {
        assert!(validate_cron_expression("").is_err());
    }

    #[test]
    fn validate_cron_rejects_invalid_expression() {
        assert!(validate_cron_expression("not a cron").is_err());
    }

    #[test]
    fn advancing_a_missed_deadline_uses_the_retained_occurrence() {
        let schedule = Schedule::from_str("0/1 * * * * * *").expect("schedule");
        let previous = Utc.with_ymd_and_hms(2026, 8, 3, 0, 0, 0).single().expect("previous");
        let now = Utc.with_ymd_and_hms(2026, 8, 3, 0, 0, 3).single().expect("now");
        let next = advance_deadline(&schedule, previous, now).expect("next deadline");
        assert!(next > now);
        assert!(next - now <= chrono::Duration::seconds(1));
    }

    #[test]
    fn advancing_a_due_deadline_fires_once_then_moves_forward() {
        let schedule = Schedule::from_str("0/1 * * * * * *").expect("schedule");
        let previous = Utc.with_ymd_and_hms(2026, 8, 3, 0, 0, 1).single().expect("previous");
        let now = Utc.with_ymd_and_hms(2026, 8, 3, 0, 0, 1).single().expect("now");
        let next = advance_deadline(&schedule, previous, now).expect("next deadline");
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 8, 3, 0, 0, 2).single().unwrap());
    }

    #[test]
    fn refresh_reuses_retained_deadline_state() {
        let deadline = Utc.with_ymd_and_hms(2026, 8, 3, 0, 5, 0).single().unwrap();
        let shared = Arc::new(std::sync::Mutex::new(DeadlineState::from([(
            "pair-a".to_string(),
            ("0 * * * * * *".to_string(), deadline),
        )])));
        let refreshed = shared.lock().unwrap().clone();
        assert_eq!(refreshed.get("pair-a").map(|(_, value)| *value), Some(deadline));
    }

    #[test]
    fn scheduler_state_refreshes_config_and_fires_due_pair_once() {
        let first = Utc.with_ymd_and_hms(2026, 8, 3, 0, 5, 0).single().unwrap();
        let second = Utc.with_ymd_and_hms(2026, 8, 3, 0, 6, 0).single().unwrap();
        let mut state = SchedulerState::default();
        state.refresh(&[ScheduledPair {
            pair_id: "pair-a".into(),
            cron: "0 * * * * * *".into(),
            next_run: first,
        }]);
        let mut fired = Vec::new();
        scheduler_tick(&mut state, &[], first, |pair_id| fired.push(pair_id.to_string()));
        assert!(fired.is_empty());
        scheduler_tick(
            &mut state,
            &[ScheduledPair {
                pair_id: "pair-a".into(),
                cron: "0 * * * * * *".into(),
                next_run: first,
            }],
            first,
            |pair_id| fired.push(pair_id.to_string()),
        );
        assert_eq!(fired, vec!["pair-a"]);
        scheduler_tick(
            &mut state,
            &[ScheduledPair {
                pair_id: "pair-a".into(),
                cron: "0 * * * * * *".into(),
                next_run: first,
            }],
            first,
            |pair_id| fired.push(pair_id.to_string()),
        );
        assert_eq!(fired, vec!["pair-a"]);

        state.refresh(&[ScheduledPair {
            pair_id: "pair-a".into(),
            cron: "0/2 * * * * * *".into(),
            next_run: second,
        }]);
        assert_eq!(state.deadlines["pair-a"].0, "0/2 * * * * * *");
        assert_eq!(state.deadlines["pair-a"].1, second);
    }

    #[tokio::test]
    async fn scheduler_cancellation_notification_wakes_waiter() {
        let service = ScheduleService {
            cancel: Arc::new(AtomicBool::new(false)),
            wake: Arc::new(Notify::new()),
            deadlines: Arc::new(std::sync::Mutex::new(DeadlineState::new())),
        };
        let waiter_wake = Arc::clone(&service.wake);
        let waiter = tokio::spawn(async move {
            waiter_wake.notified().await;
        });
        tokio::task::yield_now().await;
        service.stop();
        waiter.await.expect("scheduler waiter awakened");
        assert!(service.cancel.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn scheduler_loop_runs_with_fixed_clock_and_stops_on_cancellation() {
        let now = Utc.with_ymd_and_hms(2026, 8, 3, 0, 0, 0).single().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(Notify::new());
        let deadlines = Arc::new(std::sync::Mutex::new(DeadlineState::new()));
        let (sent, mut received) = tokio::sync::mpsc::unbounded_channel();
        let scheduled = vec![ScheduledPair {
            pair_id: "pair-a".into(),
            cron: "0/1 * * * * * *".into(),
            next_run: now,
        }];
        let fixed_clock: Clock = Arc::new(move || now);
        let loop_task = tokio::spawn(run_scheduler_loop(
            Arc::clone(&cancel),
            Arc::clone(&wake),
            Arc::clone(&deadlines),
            fixed_clock,
            move || scheduled.clone(),
            move |pair_id| {
                sent.send(pair_id).expect("scheduler receiver is active");
            },
        ));

        assert_eq!(received.recv().await.as_deref(), Some("pair-a"));
        assert_eq!(received.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty));
        assert_eq!(deadlines.lock().unwrap().len(), 1);

        cancel.store(true, Ordering::Relaxed);
        wake.notify_one();
        loop_task.await.expect("scheduler loop joined");
    }

    #[tokio::test]
    async fn scheduler_loop_refreshes_pairs_after_wake() {
        let now = Utc.with_ymd_and_hms(2026, 8, 3, 0, 0, 0).single().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(Notify::new());
        let deadlines = Arc::new(std::sync::Mutex::new(DeadlineState::new()));
        let phase = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (loaded, mut loaded_rx) = tokio::sync::mpsc::unbounded_channel();
        let (fired, mut fired_rx) = tokio::sync::mpsc::unbounded_channel();
        let first_phase = vec![ScheduledPair {
            pair_id: "pair-a".into(),
            cron: "0/1 * * * * * *".into(),
            next_run: now + chrono::Duration::seconds(30),
        }];
        let second_phase = vec![ScheduledPair {
            pair_id: "pair-b".into(),
            cron: "0/1 * * * * * *".into(),
            next_run: now,
        }];
        let clock: Clock = Arc::new(move || now);
        let load_phase = Arc::clone(&phase);
        let loop_task = tokio::spawn(run_scheduler_loop(
            Arc::clone(&cancel),
            Arc::clone(&wake),
            Arc::clone(&deadlines),
            clock,
            move || {
                let current = load_phase.fetch_add(1, Ordering::SeqCst);
                loaded.send(current).expect("loader observer is active");
                if current == 0 {
                    first_phase.clone()
                } else {
                    second_phase.clone()
                }
            },
            move |pair_id| {
                fired.send(pair_id).expect("firing receiver is active");
            },
        ));

        assert_eq!(loaded_rx.recv().await, Some(0));
        wake.notify_one();
        assert_eq!(loaded_rx.recv().await, Some(1));
        assert_eq!(fired_rx.recv().await.as_deref(), Some("pair-b"));
        assert_eq!(deadlines.lock().unwrap().len(), 1);
        assert!(deadlines.lock().unwrap().contains_key("pair-b"));

        cancel.store(true, Ordering::Relaxed);
        wake.notify_one();
        loop_task.await.expect("scheduler loop joined");
    }

    #[tokio::test]
    async fn scheduler_loop_fires_missed_deadline_once_and_advances_it() {
        let previous = Utc.with_ymd_and_hms(2026, 8, 3, 0, 0, 0).single().unwrap();
        let now = previous + chrono::Duration::seconds(3);
        let cancel = Arc::new(AtomicBool::new(false));
        let wake = Arc::new(Notify::new());
        let deadlines = Arc::new(std::sync::Mutex::new(DeadlineState::from([(
            "pair-a".into(),
            ("0/1 * * * * * *".into(), previous),
        )])));
        let (fired, mut fired_rx) = tokio::sync::mpsc::unbounded_channel();
        let clock: Clock = Arc::new(move || now);
        let loop_task = tokio::spawn(run_scheduler_loop(
            Arc::clone(&cancel),
            Arc::clone(&wake),
            Arc::clone(&deadlines),
            clock,
            move || {
                vec![ScheduledPair {
                    pair_id: "pair-a".into(),
                    cron: "0/1 * * * * * *".into(),
                    next_run: previous,
                }]
            },
            move |pair_id| {
                fired.send(pair_id).expect("firing receiver is active");
            },
        ));

        assert_eq!(fired_rx.recv().await.as_deref(), Some("pair-a"));
        assert!(deadlines.lock().unwrap()["pair-a"].1 > now);
        assert_eq!(fired_rx.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty));
        cancel.store(true, Ordering::Relaxed);
        wake.notify_one();
        loop_task.await.expect("scheduler loop joined");
    }

    #[test]
    fn build_scheduled_pairs_skips_disabled_pairs() {
        let pairs = vec![
            FolderPair {
                id: "1".into(),
                name: "Disabled".into(),
                left_path: "/a".into(),
                right_path: "/b".into(),
                mode: crate::models::SyncMode::Echo,
                filters: Default::default(),
                conflict_policy: crate::models::ConflictPolicy::NewerWins,
                enabled: true,
                watch_enabled: false,
                schedule_enabled: false,
                schedule_cron: Some("0 9 * * *".into()),
                created_at: 0,
                updated_at: 0,
            },
            FolderPair {
                id: "2".into(),
                name: "Scheduled".into(),
                left_path: "/a".into(),
                right_path: "/b".into(),
                mode: crate::models::SyncMode::Echo,
                filters: Default::default(),
                conflict_policy: crate::models::ConflictPolicy::NewerWins,
                enabled: true,
                watch_enabled: false,
                schedule_enabled: true,
                schedule_cron: Some("0 9 * * *".into()),
                created_at: 0,
                updated_at: 0,
            },
        ];

        let scheduled = build_scheduled_pairs(
            &pairs,
            Utc.with_ymd_and_hms(2026, 8, 3, 0, 0, 0).single().unwrap(),
        );
        assert_eq!(scheduled.len(), 1);
        assert_eq!(scheduled[0].pair_id, "2");
    }
}
