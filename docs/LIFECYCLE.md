# Lifecycle and release policy

## Status

SyncForge is an active public product. `C:\Users\CDAND\Projects\syncforge` is the only retained local checkout and should track `main`; release and feature variants belong in Git branches, tags, or worktrees rather than sibling folders.

## Development lifecycle

- Read `AGENTS.md` before changes.
- Treat filesystem operations as high risk and use fixtures for tests.
- Work on focused branches; do not push or publish without explicit authorization.
- Preserve unrelated work and verify the exact branch, diff, and upstream before delivery.

## Release lifecycle

Follow `docs/RELEASE_GUIDE.md` when it exists on the current branch. A release should:

1. Inspect the previous releases and compare the full commit range.
2. Keep JavaScript, Rust, lockfile, and Tauri versions aligned.
3. Run `pnpm run ci:local` and the production build.
4. Verify generated installer assets, checksums, release notes, and final artifact names.
5. Publish only after the draft release and checks have been reviewed.

## Data boundaries

SyncForge operates on real user files. Never use personal folders as test fixtures, and never delete, overwrite, reset, or force-push without explicit authorization and exact target verification.
