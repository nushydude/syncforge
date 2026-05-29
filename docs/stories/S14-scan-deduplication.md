# S14 — Eliminate redundant directory scans

## Context (audit P1)

Watch/scheduler call `preview_pair_impl` (2 scans) then `run_pair_impl` (2 pre + 2 post) = 6 walks per auto-sync.

## Goal

Reuse a computed `SyncPlan` and avoid duplicate pre-scans when plan is already known.

## Acceptance criteria

- [ ] `RunOptions` includes `plan: Option<SyncPlan>` and/or `skip_initial_scan: bool`
- [ ] `run_pair_impl` uses provided plan when `Some`, skipping initial left/right scan
- [ ] Post-run snapshot scan still runs once at end (unless cancelled per S10)
- [ ] `watcher.rs` and `scheduler.rs` pass plan from preview into run (single preview + run apply + post scan = 4 scans max, target 2+2)
- [ ] Manual `run_pair` unchanged behavior when plan not passed
- [ ] Tests: mock or unit test that when plan provided, scanner call count is reduced (or integration test with counter)
- [ ] `cargo test` pass

## Files likely touched

- `src-tauri/src/engine.rs`
- `src-tauri/src/commands/run.rs`
- `src-tauri/src/watcher.rs`
- `src-tauri/src/scheduler.rs`
- `src-tauri/src/models.rs`

## Depends on

S10, S13
