use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use cron::Schedule;
use tauri::AppHandle;

use crate::models::FolderPair;
use crate::run_coordinator::run_scheduled_sync;
use crate::state::AppState;

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

#[derive(Debug, Clone)]
struct ScheduledPair {
    pair_id: String,
    next_run: DateTime<Utc>,
}

fn build_scheduled_pairs(pairs: &[FolderPair]) -> Vec<ScheduledPair> {
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
        let Some(next_run) = schedule.upcoming(Utc).next() else {
            continue;
        };
        scheduled.push(ScheduledPair { pair_id: pair.id.clone(), next_run });
    }
    scheduled
}

pub struct ScheduleService {
    cancel: Arc<AtomicBool>,
}

impl ScheduleService {
    pub fn start(app: AppHandle, state: Arc<AppState>) -> Result<Self, String> {
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_task = Arc::clone(&cancel);
        let app_task = app.clone();
        let state_task = Arc::clone(&state);

        tauri::async_runtime::spawn(async move {
            loop {
                if cancel_task.load(Ordering::Relaxed) {
                    break;
                }

                let scheduled = match state_task.db.lock() {
                    Ok(guard) => match guard.list_pairs() {
                        Ok(pairs) => build_scheduled_pairs(&pairs),
                        Err(e) => {
                            eprintln!("scheduler: list pairs failed: {e}");
                            Vec::new()
                        }
                    },
                    Err(e) => {
                        eprintln!("scheduler: db lock failed: {e}");
                        Vec::new()
                    }
                };

                let now = Utc::now();
                let due: Vec<String> = scheduled
                    .iter()
                    .filter(|entry| entry.next_run <= now)
                    .map(|entry| entry.pair_id.clone())
                    .collect();

                for pair_id in due {
                    run_scheduled_sync(
                        app_task.clone(),
                        Arc::clone(&state_task),
                        pair_id,
                    );
                }

                let sleep_until =
                    scheduled.iter().map(|entry| entry.next_run).filter(|next| *next > now).min();

                let sleep_for = sleep_until
                    .map(|next| {
                        (next - now)
                            .to_std()
                            .unwrap_or(Duration::from_secs(1))
                            .max(Duration::from_secs(1))
                    })
                    .unwrap_or(Duration::from_secs(30));

                tokio::select! {
                    _ = tokio::time::sleep(sleep_for) => {}
                    _ = async {
                        while !cancel_task.load(Ordering::Relaxed) {
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    } => {
                        break;
                    }
                }
            }
        });

        Ok(Self { cancel })
    }

    pub fn stop(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub fn refresh_schedule_service(app: &AppHandle, state: &Arc<AppState>) -> Result<(), String> {
    let mut guard = state.schedule_service.lock().map_err(|e| e.to_string())?;
    if let Some(service) = guard.take() {
        service.stop();
    }
    let service = ScheduleService::start(app.clone(), Arc::clone(state))?;
    *guard = Some(service);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let scheduled = build_scheduled_pairs(&pairs);
        assert_eq!(scheduled.len(), 1);
        assert_eq!(scheduled[0].pair_id, "2");
    }
}
