# S16 — Per-pair concurrency and watch suppression

## Context (audit P1)

Global single-run lock blocks all pairs; watch retriggers sync after own writes.

## Goal

Independent pairs and no feedback loops from self-generated FS events.

## Acceptance criteria

- [x] `AppState` holds per-pair run slot (e.g. `HashMap<pair_id, Arc<AtomicBool>>` cancel flags) instead of one global flag
- [x] `run_pair` allows concurrent runs on different pairs; rejects only if same pair already running
- [x] `cancel_run` accepts `pair_id` parameter (or cancels all — document behavior)
- [x] Watch path: set `sync_in_progress` for pair during run; ignore/debounce events under pair roots for N ms after run completes
- [x] Or: skip watch-triggered run when preview plan has zero actions
- [x] Separate pending queues per pair for watch vs schedule (no arbitrary global drain)
- [x] Tests for two pairs not blocking each other; watch does not immediately re-queue empty plan
- [x] `cargo test` where feasible; manual test notes otherwise

## Manual test notes

1. **Two pairs concurrently:** Create two enabled pairs with different folders. Start a manual sync on pair A, then start sync on pair B before A finishes — B should run (not blocked by A). Starting a second sync on A while A is still running should be rejected.
2. **Watch after self-sync:** Enable watch on a pair, run a sync that copies files, then wait for debounce. Confirm no immediate watch loop (no rapid repeated runs); filesystem events during the 2s post-run window should be ignored.
3. **Cancel by pair:** With pair A running, invoke cancel with A’s `pair_id` — only A stops; if B were running independently, B continues.

## Files likely touched

- `src-tauri/src/state.rs`
- `src-tauri/src/commands/run.rs`
- `src-tauri/src/watcher.rs`
- `src-tauri/src/scheduler.rs`
- `src/api/run.ts`, `src/store/runStore.ts` (cancel API if pair_id added)

## Depends on

S10, S14
