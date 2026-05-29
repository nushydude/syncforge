# S11 — Precise change detection (mtime + optional hash)

## Context (audit P0/P1)

`entries_differ` uses second-granularity mtime only; same-size edits within one second are missed.

## Goal

Improve `FileEntry` and diff comparison so sync plans are correct on fast saves and tied mtimes.

## Acceptance criteria

- [ ] `FileEntry` stores subsecond modification time (`modified_nanos` or full epoch ns); scanner populates from `metadata.modified()`
- [ ] `entries_differ` compares nanos; when size+mtime match but both are files, optional content compare via BLAKE3 over configurable threshold (e.g. files > 0 and < 50MB, or setting in `RunOptions`)
- [ ] `build_snapshot_entries` persists new mtime field
- [ ] Migration or backward-compatible read for old snapshots (default nanos from secs)
- [ ] `NewerWins` conflict path: explicit tie-breaker when mtimes equal (e.g. prefer newer nanos, then hash, then left)
- [ ] Unit tests in `diff.rs` and `scanner.rs` for: same sec different content with hash on; nanos differ detects change
- [ ] `cargo test` pass

## Files likely touched

- `src-tauri/src/models.rs`
- `src-tauri/src/scanner.rs`
- `src-tauri/src/diff.rs`
- `src-tauri/src/hashing.rs`
- `src-tauri/src/persistence.rs` (snapshot JSON schema)
- `src/types/` mirrors

## Depends on

S10 recommended (engine options pattern) but may proceed in parallel if no conflict.
