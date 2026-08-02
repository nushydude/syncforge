# Agent instructions for SyncForge

These instructions apply to any agent working in this repository. More specific instructions in a subdirectory may add constraints, but must not weaken these rules.

## Project safety

- SyncForge copies, deletes, and moves real user files. Treat filesystem operations as high risk.
- Do not delete, overwrite, reset, or force-push user work unless the user explicitly authorizes it and the exact target is verified first.
- Preserve unrelated working-tree changes. Before editing, inspect `git status` and avoid overlapping changes when possible.
- Keep changes scoped to the requested task. Do not silently mix feature work, release work, and cleanup work.

## Validation

For code changes, run the narrowest relevant checks while iterating. Before handing off a broad change or opening a PR, run:

```powershell
pnpm run ci:local
```

Report any checks that could not run and why. Do not describe a check as passing without running it or having direct CI evidence.

## Git and PR workflow

- Use a focused `codex/` or `story/` branch; do not work directly on `main` for feature changes.
- Make focused commits with messages that explain the change.
- Do not push or create a PR unless the user asks for publishing or the task explicitly includes it.
- Before publishing, verify the branch, upstream, diff, and working-tree state.
- After pushing, inspect the PR checks and address failures before declaring the work complete.

## Releases

Follow [docs/RELEASE_GUIDE.md](docs/RELEASE_GUIDE.md) exactly. In particular:

1. Inspect the previous two GitHub releases before drafting notes.
2. Compare the release range from the previous tag to the release commit.
3. Keep `package.json`, `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`, and `src-tauri/tauri.conf.json` on the same version.
4. Run the quality gates and production build before tagging.
5. Verify the generated draft release, installer assets, checksums when available, and final notes before publishing.

Never publish a release with a one-line placeholder body when earlier releases use structured notes.

## Agentic orchestration

The scripts under `orchestrator/` are an optional local story workflow. Read `orchestrator/README.md` and the relevant prompt before using them. The orchestrator is local-only by design; it does not replace the PR, CI, or release workflow described above.
