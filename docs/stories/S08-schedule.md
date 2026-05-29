# S08 — Scheduling and notifications

## Goal

Cron-based scheduled sync per pair with autostart and desktop notifications.

## Acceptance criteria

- [ ] `scheduler.rs`: tokio + `cron` crate, per-pair schedule
- [ ] `tauri-plugin-autostart` for run at login
- [ ] `tauri-plugin-notification` on sync complete/fail
- [ ] `scheduleParsing.ts`: validate cron, human-readable description; tests
- [ ] Commands: `set_schedule`; UI in PairEditor

## Depends on

S07 (or S05 if watch deferred)
