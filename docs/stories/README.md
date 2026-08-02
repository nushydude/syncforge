# SyncForge User Stories

Stories are executed in order by the CLI orchestrator (`orchestrator/Run-All.ps1`).
Each story gets its own git branch, implement → review (fresh eyes) → approve loop, then merge to `main` locally.

## Epics

### Foundation (S01–S09) — complete

Initial Tauri app: pairs, scan/diff/preview, run engine, conflicts, watch, schedule, history.

### Performance & stability (S10–S18)

Hardening from the performance/stability audit (May 2026) and LightFrame patterns.

| ID  | Title                                           | Branch                           | Depends on |
| --- | ----------------------------------------------- | -------------------------------- | ---------- |
| S10 | Engine safety (cancel, snapshot, stop-on-error) | `story/S10-engine-safety`        | S09        |
| S11 | Precise change detection                        | `story/S11-change-detection`     | S10        |
| S12 | Scan integrity & destructive guards             | `story/S12-scan-integrity`       | S10        |
| S13 | Non-blocking preview                            | `story/S13-async-preview`        | S12        |
| S14 | Eliminate redundant scans                       | `story/S14-scan-deduplication`   | S10, S13   |
| S15 | Lean persistence (WAL, prune)                   | `story/S15-persistence-lean`     | S10        |
| S16 | Per-pair concurrency & watch suppression        | `story/S16-concurrency`          | S14        |
| S17 | Frontend stability                              | `story/S17-frontend-stability`   | S10        |
| S18 | Frontend performance                            | `story/S18-frontend-performance` | S17        |

## Foundation index (S01–S09)

| ID  | Title                       | Branch                         |
| --- | --------------------------- | ------------------------------ |
| S01 | Project scaffold            | `story/S01-scaffold`           |
| S02 | Domain models + persistence | `story/S02-models-persistence` |
| S03 | Folder pair CRUD UI         | `story/S03-pair-crud`          |
| S04 | Scan, diff, preview         | `story/S04-scan-diff-preview`  |
| S05 | Sync run engine             | `story/S05-run-engine`         |
| S06 | Conflict resolution         | `story/S06-conflicts`          |
| S07 | Real-time watch + tray      | `story/S07-watch-tray`         |
| S08 | Scheduling + notifications  | `story/S08-schedule`           |
| S09 | History + export            | `story/S09-history`            |

## Verdict format (agents must use)

**Reviewer** ends with exactly one line:

```
REVIEW_VERDICT: APPROVED
```

or

```
REVIEW_VERDICT: CHANGES_REQUESTED
```

**Approver** ends with:

```
APPROVE_VERDICT: YES
```

or

```
APPROVE_VERDICT: NO
```
