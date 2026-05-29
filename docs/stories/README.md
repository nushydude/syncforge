# SyncForge User Stories

Stories are executed in order by the CLI orchestrator (`orchestrator/run-all.ps1`).
Each story gets its own git branch, implement → review (fresh eyes) → approve loop, then a local commit.

## Stack (from lightframe reference)

- Tauri 2 + Rust backend
- React 18 + TypeScript + Vite frontend
- pnpm, Vitest, ESLint, Prettier, rustfmt

## Story index

| ID | Title | Branch |
|----|-------|--------|
| S01 | Project scaffold | `story/S01-scaffold` |
| S02 | Domain models + persistence | `story/S02-models-persistence` |
| S03 | Folder pair CRUD UI | `story/S03-pair-crud` |
| S04 | Scan, diff, preview | `story/S04-scan-diff-preview` |
| S05 | Sync run engine | `story/S05-run-engine` |
| S06 | Conflict resolution | `story/S06-conflicts` |
| S07 | Real-time watch + tray | `story/S07-watch-tray` |
| S08 | Scheduling + notifications | `story/S08-schedule` |
| S09 | History + export | `story/S09-history` |

## Verdict format (agents must use)

**Reviewer** ends with exactly one line:

```
REVIEW_VERDICT: APPROVED
```

or

```
REVIEW_VERDICT: CHANGES_REQUESTED
```

Followed by bullet list of issues if changes requested.

**Approver** ends with:

```
APPROVE_VERDICT: YES
```

or

```
APPROVE_VERDICT: NO
```
