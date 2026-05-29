# S02 — Domain models and persistence

## Goal

Define core Rust types and SQLite persistence for folder pairs, snapshots, runs, and run items.

## Acceptance criteria

- [ ] `src-tauri/src/models.rs`: `FolderPair`, `SyncMode` (Synchronize, Echo, Contribute), `Filters`, `FileEntry`, `SyncAction`, `SyncPlan`, `RunReport`, `ConflictPolicy`
- [ ] `src-tauri/src/path_normalization.rs`: Windows long paths, case-insensitive compare, UNC
- [ ] `src-tauri/src/persistence.rs`: rusqlite bundled; tables `pairs`, `snapshots`, `runs`, `run_items`; migrations or init schema
- [ ] `src/types/` TS mirrors: `pair.ts`, `plan.ts`, `history.ts`, `settings.ts`
- [ ] Rust unit tests for path normalization and model serde round-trip
- [ ] Tauri commands stubbed: `list_pairs`, `save_pair`, `delete_pair` (wired in `commands.rs`)

## Out of scope

- Full UI, scan/diff, run engine
