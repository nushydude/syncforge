# S12 — Scan integrity and destructive-mode guards

## Context (audit P0/P1)

Skipped paths during walk are silent; Echo/Synchronize can treat unreadable files as deleted.

## Goal

Make scan failures visible and block dangerous plans when the tree is incomplete.

## Acceptance criteria

- [ ] `ScanResult` includes `warnings: Vec<String>` (capped, e.g. 50) for paths that failed stat/walk
- [ ] `preview_pair` / plan includes warning count; UI can show later (minimal: return in `SyncPlan` struct)
- [ ] When `skipped_entries > 0` and mode is **Echo** or **Synchronize**: preview returns error OR plan flagged `requires_attention` that blocks run until user acknowledges (prefer blocking run command with clear message)
- [ ] Pair validation: reject left/right where one root is inside the other (`path_normalization` helper)
- [ ] Tests: unreadable file in subtree causes warning; Echo preview/run refuses when skips > 0
- [ ] `cargo test` pass

## Files likely touched

- `src-tauri/src/scanner.rs`
- `src-tauri/src/commands/preview.rs`
- `src-tauri/src/commands/pairs.rs`
- `src-tauri/src/path_normalization.rs`
- `src-tauri/src/models.rs`
- `src/types/plan.ts`
