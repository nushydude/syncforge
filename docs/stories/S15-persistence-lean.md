# S15 — Lean persistence (snapshot pruning + SQLite WAL)

## Context (audit P1/P2)

Every run inserts a full snapshot row; DB grows unbounded. No WAL/busy_timeout.

## Goal

Keep SQLite small and concurrent-safe for UI + background sync.

## Acceptance criteria

- [ ] On `save_snapshot` for a pair: delete older snapshots for same `pair_id` (keep latest only, or keep last N=3)
- [ ] `Database::open` sets `PRAGMA journal_mode=WAL` and `PRAGMA busy_timeout=5000`
- [ ] Optional: batch `insert_run_item` in one transaction per run (reduce fsync churn)
- [ ] History list query already paginated or add `LIMIT 100` default if unpaginated
- [ ] Tests: two snapshots for same pair leaves one (or three) rows
- [ ] `cargo test` pass

## Files likely touched

- `src-tauri/src/persistence.rs`
- `src-tauri/src/commands/history.rs` (if pagination added)
