# S13 — Non-blocking preview (spawn_blocking + short DB lock)

## Context (audit P1)

`preview_pair` runs two full tree walks on the Tauri command thread and holds the DB mutex during scans.

## Goal

Match `run_pair` pattern: never block the async runtime on large folder scans.

## Acceptance criteria

- [x] `preview_pair` is `async` and uses `tauri::async_runtime::spawn_blocking` for `preview_pair_impl`
- [x] DB lock held only to load pair + latest snapshot; clone data, drop lock, then scan
- [x] Frontend `previewPair` invoke unchanged or updated if signature changes
- [x] Manual test note in story: preview large folder does not freeze window (document in implement log)
- [x] Existing preview tests still pass
- [x] `cargo test` pass

## Implement log

- `preview_pair_impl` no longer takes `&Database`; scans run without holding the mutex.
- `preview_pair` loads `latest_snapshot` under a short lock, then runs `preview_pair_impl` on the blocking pool.
- Watcher/scheduler load snapshot under a short lock before calling `preview_pair_impl` (pair already loaded separately).
- **Manual test:** Preview on a large folder pair should keep the window responsive (spinner/UI updates) because folder walks run on the blocking thread pool, not the async runtime or DB lock. Verify in the Pair Editor preview tab while scanning tens of thousands of files.

## Files likely touched

- `src-tauri/src/commands/preview.rs`
- `src-tauri/src/lib.rs` (command registration)

## Depends on

S12 (if `SyncPlan` shape changes) — if S12 not merged, keep plan struct stable or coordinate.
