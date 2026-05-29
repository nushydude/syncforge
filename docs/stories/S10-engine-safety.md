# S10 — Engine safety: cancel, snapshot truth, stop-on-error

## Context (audit P0)

Real-user data requires that interrupted or partial syncs never leave the DB snapshot out of sync with disk.

## Goal

Harden `engine.rs` and `commands/run.rs` for cancel and failure paths.

## Acceptance criteria

- [ ] `cancel_run` uses `State<'_, Arc<AppState>>` (matches other commands); cancel works from UI without panic
- [ ] On **cancel**: after partial apply, capture post-run snapshot from fresh scans of both sides OR document rollback; snapshot must reflect disk (not pre-run only)
- [ ] `finish_cancelled` saves snapshot when any actions were applied; run status remains `Cancelled`
- [ ] **Stop-on-error** default for manual runs: first non-conflict action failure stops the loop (configurable `RunOptions.stop_on_error`, default `true`)
- [ ] When errors occurred and run stops early: status `Failed` or new `Partial`; snapshot policy documented in code comments
- [ ] Do not mark run `Completed` when `report.errors` is non-empty
- [ ] Rust unit tests: cancel mid-run leaves consistent snapshot; stop-on-error does not apply remaining actions
- [ ] `pnpm test` and `cargo test` pass

## Files likely touched

- `src-tauri/src/engine.rs`
- `src-tauri/src/commands/run.rs`
- `src-tauri/src/models.rs` (if adding `Partial` status or `RunOptions` field)
- `src/types/run.ts` (mirror options if exposed to UI later)

## Out of scope

- Per-pair cancel flags (S16)
- Hash-based change detection (S11)
