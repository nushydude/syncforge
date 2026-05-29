# S13 — Non-blocking preview (spawn_blocking + short DB lock)

## Context (audit P1)

`preview_pair` runs two full tree walks on the Tauri command thread and holds the DB mutex during scans.

## Goal

Match `run_pair` pattern: never block the async runtime on large folder scans.

## Acceptance criteria

- [ ] `preview_pair` is `async` and uses `tauri::async_runtime::spawn_blocking` for `preview_pair_impl`
- [ ] DB lock held only to load pair + latest snapshot; clone data, drop lock, then scan
- [ ] Frontend `previewPair` invoke unchanged or updated if signature changes
- [ ] Manual test note in story: preview large folder does not freeze window (document in implement log)
- [ ] Existing preview tests still pass
- [ ] `cargo test` pass

## Files likely touched

- `src-tauri/src/commands/preview.rs`
- `src-tauri/src/lib.rs` (command registration)

## Depends on

S12 (if `SyncPlan` shape changes) — if S12 not merged, keep plan struct stable or coordinate.
