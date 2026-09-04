# Code audit — 2026-09-05

This pass inspected preview paging, Folder Sniffer, duplicate scan UI and job delivery, history storage/paging, scan integrity checks, progress coalescing, and work coordination. It is a focused static review with regression tests, not an exhaustive security review or a large-drive benchmark.

## Implemented

| Area                       | Finding                                                                                                                                       | Change                                                                                                                                                                                 |
| -------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Preview reliability and UX | Failed page fetches advanced the page number without replacing rows; late replies could overwrite a newer preview.                            | Commit page number and rows together after success, discard invalidated requests, display an actionable error, and expose busy state.                                                  |
| Sniffer rendering          | Expanding the largest-file list rendered every file and spread all sizes into `Math.max`; zero-byte remainder files had no expansion control. | Render at most 100 file rows per expanded page, use reduction for the maximum, memoize file filtering, and allow zero-byte files to be browsed.                                        |
| Sniffer memory             | Every visited scan remained cached for the lifetime of the mounted view.                                                                      | Retain at most eight recently used scans; results with more than 10,000 immediate entries are displayed but not cached. This bounds entry count, not exact bytes or the active result. |
| Sniffer traversal          | Recursive traversal followed links, including directory junctions, risking cycles, double counting, and call-stack exhaustion.                | Use an iterative traversal and skip filesystem links and Windows name-surrogate reparse points. Read failures and skipped links contribute to the visible incomplete-total warning.    |
| Sniffer errors             | Folder-picker failures escaped the UI error path.                                                                                             | Show the failure in the existing error area.                                                                                                                                           |

The traversal change is read-only. Windows reparse points without the name-surrogate bit, including cloud placeholders, remain visible. Metadata checks are not a security boundary against concurrent filesystem replacement.

## Follow-up findings

1. **Medium: duplicate results are rendered in full.** `src/components/duplicates/DuplicatesView.tsx` maps every group and file into DOM nodes and rescans the complete result to total selected bytes. Add backend result paging or a virtualized grouped list, then measure selection latency and DOM size on large duplicate sets.
2. **Medium: duplicate job delivery can regress displayed state.** The initial `getDuplicateScan()` response and progress events independently replace the job. A delayed initial response can overwrite an event received in the meantime. Order snapshots/events by job identity and revision; add a deferred-response regression test. Listener rejection and cleanup also deserve attention: cleanup currently invokes the unlisten function twice.
3. **Medium: expanded treemaps remain unbounded.** `buildTreemap` recursively slices and sums groups, and “Other items” expansion removes the tile limit. A zero-size or highly skewed group can produce deep recursion. Prefer bounded drill-down and a layout using index ranges/prefix sums; keep zero-size entries available in a list.
4. **Medium: Sniffer progress can be silent inside a large child directory.** The backend reports progress after each top-level child and has no scan-cancellation API. Introduce cancellable traversal with throttled nested progress, using the existing sync progress coalescing pattern as a reference.

## Existing safeguards observed

- Echo/Synchronize reject incomplete directory scans before destructive planning.
- Preview plans have bounded retention and backend paging.
- History summaries are limited to 100 runs and run items have paged retrieval.
- Sync progress is coalesced before crossing the Tauri bridge.

These are useful foundations; this pass does not establish that every path through them is correct.

## Validation

- Targeted frontend regression tests: 5 passed.
- `pnpm run ci:local`: passed version consistency, formatting, ESLint, 115 frontend tests, the frontend production build, Rust formatting, Clippy with warnings denied, and 132 Rust tests.
- Added Rust tests cover nested totals, missing paths, and a directory-link cycle (a Windows junction on this host).
- No production filesystem mutation, release, push, or PR was requested or performed. Tests create their own temporary fixtures.

The pre-existing untracked `docs/performance-scalability-audit-2026-08-19.md` was preserved.
