# S05 — Sync run engine

## Goal

Execute a sync plan with progress events, safe copies, Recycle Bin deletes, and optional hash verification.

## Acceptance criteria

- [ ] `hashing.rs`: blake3 hashing
- [ ] `engine.rs`: create dirs, temp+copy+rename, delete via `trash` crate, verify hashes
- [ ] Emits `sync://progress` Tauri events
- [ ] Commands: `run_pair`, `cancel_run`
- [ ] Frontend: `runStore`, `useSyncProgress`, `RunProgress` UI
- [ ] Updates snapshot and run history row on completion
- [ ] Rust integration test or unit tests for engine helpers
