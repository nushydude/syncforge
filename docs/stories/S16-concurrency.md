# S16 — Per-pair concurrency and watch suppression

## Context (audit P1)

Global single-run lock blocks all pairs; watch retriggers sync after own writes.

## Goal

Independent pairs and no feedback loops from self-generated FS events.

## Acceptance criteria

- [ ] `AppState` holds per-pair run slot (e.g. `HashMap<pair_id, Arc<AtomicBool>>` cancel flags) instead of one global flag
- [ ] `run_pair` allows concurrent runs on different pairs; rejects only if same pair already running
- [ ] `cancel_run` accepts `pair_id` parameter (or cancels all — document behavior)
- [ ] Watch path: set `sync_in_progress` for pair during run; ignore/debounce events under pair roots for N ms after run completes
- [ ] Or: skip watch-triggered run when preview plan has zero actions
- [ ] Separate pending queues per pair for watch vs schedule (no arbitrary global drain)
- [ ] Tests for two pairs not blocking each other; watch does not immediately re-queue empty plan
- [ ] `cargo test` where feasible; manual test notes otherwise

## Files likely touched

- `src-tauri/src/state.rs`
- `src-tauri/src/commands/run.rs`
- `src-tauri/src/watcher.rs`
- `src-tauri/src/scheduler.rs`
- `src/api/run.ts`, `src/store/runStore.ts` (cancel API if pair_id added)

## Depends on

S10, S14
