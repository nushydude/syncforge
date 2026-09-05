# Folder Sniffer: storage explorer specification

Status: proposed implementation specification. No application changes are implemented by this document.

Date: 2026-09-05. Baseline: repository version 0.8.0.

## 1. Objective and release scope

Turn Folder Sniffer into a scan-once storage explorer that helps users answer:

1. Where is space being used beneath this folder?
2. Which files are largest, even several folders down?
3. Can I explore the results without waiting for repeated scans?
4. How complete and current are these measurements?
5. Can I inspect or clean up a selected item without accidentally affecting another item?

The first release includes cancellable scan jobs, a retained scan index, indexed navigation, a sortable table, subtree-wide largest files, basic search/filtering, bounded treemaps, error inspection, and safer existing file actions. Safety work is a release prerequisite, not an optional later enhancement.

Subsequent releases add richer storage insights, allocated-space accounting, duplicate-finder integration, and saved-scan comparisons. Sections below distinguish these extensions from first-release requirements.

Non-goals for the first release:

- Automatic deletion, scheduled cleanup, or claims that old files are safe to delete.
- Content indexing, file-content reads, or duplicate detection implemented inside Sniffer.
- A replacement file manager, background filesystem watcher, or automatic scan at app startup.
- Following links, resuming interrupted traversal after restart, or multi-root scans.
- A point-in-time filesystem snapshot or exact prediction of recoverable disk space.

## 2. Verified baseline

| Area       | Current behavior                                                                 | Required change                                                                     |
| ---------- | -------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------- |
| Traversal  | `measure` walks iteratively and skips links/name-surrogate reparse points.       | Preserve these safeguards; add cancellation, structured issues, and indexed output. |
| Results    | Full traversal produces only immediate-child entries and aggregate counts.       | Retain descendants and parent relationships for later queries.                      |
| Navigation | Each uncached child is rescanned.                                                | Query the existing scan generation.                                                 |
| Progress   | One update after each top-level child; percentage based on child count.          | Throttled nested progress with honest, non-percentage traversal status.             |
| Retention  | React caches eight scans of at most 10,000 immediate entries each.               | Backend owns a disk-backed index and explicit retention limits.                     |
| Rendering  | Expanded file lists page at 100 rows; map initially shows 40 entries plus Other. | Keep every map bounded; Other must never render every entry.                        |
| Size       | Sum of `metadata.len()` for files.                                               | Label logical size; distinguish later allocated-size support.                       |
| Actions    | Native open/reveal/properties, clipboard, rename, permanent deletion.            | Preserve inspection; strengthen mutation validation and prefer recycling.           |
| Errors     | One skipped count; action errors share the scan-error heading.                   | Structured issue reporting and operation-specific errors.                           |

Source references:

- [Backend commands and traversal](../src-tauri/src/commands/sniffer.rs)
- [React view](../src/components/sniffer/FolderSnifferView.tsx)
- [Frontend API](../src/api/sniffer.ts) and [types](../src/types/sniffer.ts)
- [Existing UI regression test](../src/test/folderSnifferView.test.tsx)
- [Work coordinator](../src-tauri/src/state.rs)
- [Duplicate job lifecycle](../src-tauri/src/commands/duplicates.rs)
- [Recent audit](code-audit-2026-09-05.md)

Do not reimplement the already-completed iterative traversal or file-list pagination fixes as new work. Existing duplicate-job and progress patterns are references, not contracts to copy without reviewing ordering, cancellation, and cleanup behavior.

## 3. User experience

### 3.1 Starting and monitoring a scan

- Choose a folder using the existing picker. Show its complete path in an accessible location.
- Starting returns a job immediately; distinguish Queued from Scanning.
- Allow one queued or running Sniffer traversal globally. A second start returns a structured busy error identifying the existing scan; it must not silently replace it.
- Cancel is available while queued or running. Show Cancelling until the worker acknowledges it.
- During traversal display files visited, folders visited, logical bytes discovered, elapsed time, and current directory. Label discovered bytes as provisional.
- Use indeterminate progress while the total work is unknown. Do not translate completed root children into a time/work percentage or invent an ETA.
- Show partial table results as committed index batches become available. Do not continually reorder rows underneath a user's selection; expose a refresh-results affordance for provisional pages.
- Leaving the Sniffer tab does not cancel a scan. Returning restores its state. Hidden views must not intercept keyboard navigation.
- On cancellation retain queryable committed results, visibly marked Partial — cancelled. A new scan starts a new generation rather than appending to cancelled data.

### 3.2 Browsing results

Use a toolbar, summary, breadcrumbs, results table, optional storage map, and selection details panel.

The toolbar provides Choose folder, Refresh, scope selector, search, filters, and scan age/status. The scope selector offers Current folder and Largest files in subtree. Search applies to the selected directory's subtree when that scope is selected.

The table is the complete navigation surface, including zero-byte files, empty directories, and skipped-link entries. Default sorting is logical size descending, then normalized name, then stable node ID. Columns:

| Column       | Semantics                                                                                                    |
| ------------ | ------------------------------------------------------------------------------------------------------------ |
| Name         | Item name and file/folder/link indicator.                                                                    |
| Logical size | File length or sum of observed descendant file lengths.                                                      |
| % of folder  | Share of the unfiltered selected directory total; unknown if denominator is zero; provisional if incomplete. |
| Files        | Observed descendant file count for a directory; blank for files.                                             |
| Modified     | Filesystem-reported timestamp of that item, not the newest descendant timestamp.                             |
| Path         | Relative path in subtree views; full path in details/copy action.                                            |
| Status       | Complete, scanning, unreadable, excluded, or stale where applicable.                                         |

- Sort by name, size, file count, or modified time. Missing values sort last in both directions.
- First-release filters: case-insensitive literal name/path search, extension, minimum/maximum logical size, modified-date interval, and item kind. Regex is out of scope.
- Extension matching is case-insensitive, uses the final suffix, and has an explicit No extension group. Dotfiles without another dot count as No extension.
- Size intervals are inclusive. Date inputs use the user's local date, converted to a half-open UTC interval. Missing timestamps do not match date filters.
- Changing sort, scope, search, or filters resets paging. Show matched count and filtered bytes separately from whole-folder totals.
- Breadcrumbs represent hierarchy; Back/Forward represent navigation history. Up moves to the indexed parent and is disabled at the selected scan root.
- Commit the new location only after its query succeeds. On failure retain the previous location and rows together and show a retryable navigation error.
- Selecting a row updates details and map highlighting. Double-click/Enter on a directory navigates; opening a file externally requires an explicit Open action.
- Preserve selection by node ID while paging/refetching if that node remains available.
- Details show full path, scan timestamp, completeness, logical size, counts, metadata availability, and visible action buttons.

### 3.3 Storage map

- Render at most 40 real positive-size tiles plus one aggregate Other tile.
- Other opens a paged list of omitted entries. It never removes the map cap.
- Map values and table totals use the same scan generation and size metric.
- When filters apply, label the map Filtered results and disclose the displayed total. Table percentages still use the unfiltered directory denominator.
- Zero-size entries remain in the table and contribute to an explicit zero-size count, not invented tile areas.
- Use an explicit tile kind (`entry`, `other`, `looseFiles`), never a filename or synthetic filesystem path to identify UI groups.
- A real item named Other items or Loose files must remain normally navigable and actionable.
- Bound layout inputs before layout; avoid recursive array slicing. Handle all-zero input without division errors or recursion growth.
- Grouped loose files open the corresponding file list. Hover/focus reveals name, size, and share even when labels do not fit.

### 3.4 Accessibility and errors

- Every action is keyboard accessible without requiring right-click. Menus support Escape, focus restoration, and appropriate menu semantics.
- Announce scan phase transitions and errors; avoid announcing every progress tick to screen readers.
- Respect reduced motion and maintain visible focus, readable contrast, and non-color status labels.
- Use distinct headings for scan, query, rename, recycle, and external-open failures.
- Picker cancellation is not an error. A picker failure must not expose a Retry button that scans an empty path.

## 4. Scan engine and consistency

### 4.1 Lifecycle

Allowed state transitions:

| From                           | To                                         |
| ------------------------------ | ------------------------------------------ |
| queued                         | scanning, cancelling, failed               |
| scanning                       | completed, cancelling, failed              |
| cancelling                     | cancelled, failed                          |
| completed / cancelled / failed | terminal; refresh creates a new generation |

Completion and cancellation race through one synchronized terminal transition. If completion commits first, a later cancel returns the completed job; if cancellation is accepted first, the worker cannot publish completed.

- Assign a UUID scan ID, generation ID, and monotonically increasing revision to snapshots/events.
- Progress events include scan ID and revision; clients reject older revisions and unrelated jobs.
- Subscribe before fetching the current snapshot. Merge the response by revision so a late fetch cannot overwrite newer events.
- Emit progress at most once per 150 ms per job, with forced phase/terminal updates. Events contain counters, not full result arrays.
- A scope guard owns active-job registration, cancellation registration, permits, and pending-sync wakeup. Normal completion, admission errors, worker errors, cancellation, and unwinding must release registrations exactly once.
- Unexpected worker termination becomes a failed job with a diagnostic; no root remains permanently registered as busy.

### 4.2 Traversal and work coordination

- Perform filesystem work off the async/UI thread. Keep traversal iterative.
- Check cancellation before admission, during cancellable admission waits, between directory entries, and before committing a batch.
- Do not enumerate all root children into a vector. Use a bounded traversal frontier, spilling pending directories to the index when necessary.
- Continue skipping filesystem links and Windows name-surrogate reparse points, including junction cycles. Record them with a reason; do not follow them for size totals.
- Preserve visibility of ordinary cloud placeholders without opening file contents or deliberately hydrating them. Report metadata failures as issues.
- Reuse fetched metadata when safe rather than measuring an immediate child twice.
- Coordinate Sniffer mutations with existing writer permits. The current coordinator protects overlapping writers, not a filesystem snapshot for readers.
- Scans may observe external changes and concurrent sync writes. Label results as observations over a time interval. Do not add long-lived reader locks blocking sync for an entire drive scan.
- A known in-app write overlapping an indexed subtree marks that subtree stale. This notification must not mutate or refresh the filesystem automatically.
- An OS metadata call on an unresponsive network share may delay cancellation; show Cancelling without claiming the worker has stopped or prematurely releasing its resources.

### 4.3 Totals and completeness

- Logical bytes count each observed file directory entry once, including hard-link aliases. State this in size help text.
- Directory totals include descendants, exclude the directory itself from descendant-folder counts, and exclude directory metadata allocation.
- Track coverage separately from lifecycle: complete means traversal finished under the configured exclusions; unreadable/missing entries yield incomplete coverage even when the job completed.
- Track issue categories separately: permission denied, vanished during scan, unreadable metadata/directory, skipped link, excluded by policy, unsupported type, and resource limit.
- Parent completeness reflects descendants. An unreadable directory is unknown, not a verified empty directory.
- Store issue details with node/path, category, OS error code where available, and message. Query issues with pagination.
- Aggregate issue counts remain exact even if detail retention is capped; disclose omitted-detail counts.
- Sum with checked arithmetic and surface overflow as an explicit issue. Never silently wrap totals.

## 5. Retained index and query contracts

### 5.1 Storage model

Use a Sniffer-owned SQLite index in the application cache area, isolated from sync history writes. Keep implementation behind a repository/service boundary so commands do not embed traversal and SQL logic.

Logical entities:

| Entity              | Required fields                                                                                                                                        |
| ------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Scan                | ID, root identity/path, generation, lifecycle, revision, started/finished times, options, coverage, counters, stale state.                             |
| Node                | ID, scan/generation ID, parent ID, native name/path representation, display path, kind, logical size, modified time, identity fingerprint, scan state. |
| Directory aggregate | Node ID, descendant bytes/files/folders, completeness, issue counts.                                                                                   |
| Issue               | Scan/node IDs, category, path, code/message.                                                                                                           |
| Pending directory   | Scan ID, parent node ID, native path, traversal state.                                                                                                 |

- Preserve native path round-tripping; lossy display strings must not become authoritative mutation targets. Use opaque node IDs in action APIs.
- Index parent lookups, subtree membership, size ranking, normalized extension/name, and supported sorts. Choose a materialized ancestry key or equivalent indexed subtree representation; avoid a quadratic ancestor-descendant closure table on deep trees.
- Batch writes in bounded transactions. Directory finalization propagates totals upward; avoid a full ancestor update for every file in a deep tree.
- Queries return one committed revision. A completed generation is immutable except for explicit stale markers; refresh builds a replacement generation.
- Initial limits: 100 rows per normal page, server-enforced maximum 200; 1,000 entries per write batch; 10,000 issue details per scan; eight retained scan sessions; 2 GiB total index-file budget including temporary generations/WAL.
- These are configurable internal defaults, not user-facing settings initially. Benchmark before release. Hitting disk budget ends the scan with a clear resource-limit outcome and preserves committed partial data.
- Evict least-recently-used inactive sessions first. Never evict active jobs, a displayed session, or a generation referenced by an in-flight query/action. Expired references return a structured expired-result error.
- On startup, mark interrupted jobs failed, clear abandoned transient files safely, and prune expired sessions. Cache retention defaults to 24 hours for inactive sessions. Saved snapshots are separate opt-in data in a later release.

### 5.2 Proposed Tauri API

Names are proposed contracts; final naming may follow repository conventions while preserving semantics.

| Command                   | Input                                                              | Output                                                       |
| ------------------------- | ------------------------------------------------------------------ | ------------------------------------------------------------ |
| `start_sniffer_scan`      | root path, scan options                                            | job snapshot immediately                                     |
| `get_sniffer_scan`        | scan ID, or current active/recent                                  | job snapshot or explicit none/expired                        |
| `cancel_sniffer_scan`     | scan ID                                                            | acknowledged job snapshot                                    |
| `query_sniffer_entries`   | scan/generation, directory ID, scope, sort, filters, cursor, limit | rows, next cursor, match count/bytes, revision, completeness |
| `get_sniffer_summary`     | scan/generation, directory ID                                      | directory totals, map's bounded tiles, status                |
| `get_sniffer_node`        | scan/generation, node ID                                           | details and supported actions                                |
| `query_sniffer_issues`    | scan ID, category, cursor, limit                                   | issue page and category totals                               |
| `refresh_sniffer_subtree` | scan/generation, directory ID                                      | replacement-generation job                                   |
| `prepare_sniffer_action`  | scan/generation, node ID, action, optional new name                | validated review details and expiring action token           |
| `execute_sniffer_action`  | action token                                                       | operation result, invalidated scope                          |

Contract rules:

- Use decimal strings for byte counts and potentially large aggregate counters across IPC, with lossless formatting/comparison helpers. Do not silently exceed JavaScript's safe integer range. Page lengths remain ordinary bounded integers.
- Use UTC timestamps with explicit null for unavailable metadata.
- Cursors bind scan generation, query signature, committed revision, and deterministic sort position. Reject mismatches. When a running generation advances, return a stale-cursor outcome and let the UI deliberately restart the page.
- Apply filtering, sorting, aggregation, and paging in the backend. The frontend must not fetch all rows to sort or compute summaries.
- Use parameterized queries; escape literal wildcard characters for search.
- Errors carry a code, operation, retryable flag, and user-readable message. Codes include busy, cancelled, expired, stale cursor, stale target, not found, permission denied, name collision, unsupported, and resource limit.
- Every frontend request has a local request token in addition to backend revision checks. Late responses cannot overwrite a newer location or filter selection.

### 5.3 Refresh and freshness

- Display scan time and status for every view, including file-only views.
- Manual Refresh of a directory rescans its subtree into staging, then atomically replaces that subtree and recomputes ancestor aggregates in a new published generation.
- Keep the old generation browsable while refresh runs. A cancelled/failed refresh preserves it and marks freshness appropriately; do not merge partial replacement totals into complete old totals.
- Distinguish stale from incomplete. Stale means known changes since observation; incomplete means missing coverage during observation.
- After an in-app mutation, mark affected ancestors stale and schedule a refresh of the affected parent. A refresh failure must not turn a successful mutation into a reported mutation failure.
- External changes are detected only on refresh or action revalidation in the first release. Do not imply live accuracy.

## 6. File-action safety

Existing permanent deletion must be addressed before shipping the expanded explorer.

- Default delete behavior becomes Move to Recycle Bin using the supported OS facility. If recycling is unsupported or fails, report that condition; never silently fall back to permanent deletion.
- Permanent deletion is outside the first-release Sniffer UI. Existing commands must not remain an unvalidated bypass through registered IPC handlers.
- Before rename/recycle, resolve the node server-side, validate membership in the selected scan root, and inspect current filesystem identity. Never accept arbitrary display paths as sufficient authority.
- Prevent mutation of the selected scan root, filesystem roots, or synthetic groups. For links/reparse points, inspect without following and reject mutations in the first release.
- Review shows exact full path, item kind, new name/destination if applicable, and that recycling a directory includes its current descendants. Counts from scans are estimates, not authoritative current contents.
- Store the review in a short-lived, single-use server token, bound to node identity, operation, and generation. Revalidate under the writer permit immediately before execution. Changed identity requires a new review.
- Metadata identity checks alone are not a complete boundary against concurrent replacement. Use platform handle-based/no-follow operations where supported; document residual races and reject an operation when target guarantees cannot be met.
- Rename must use platform-appropriate no-replace behavior, with explicit collision errors and valid-name checks. An existence check followed by an overwriting rename is insufficient. Cover Windows reserved names, trailing dots/spaces, and case-only renames explicitly.
- Block repeated action submission while an operation is pending. Distinguish success, failure, and indeterminate outcome; an indeterminate result requires inspection before retry.
- Do not promise app-level Undo unless implemented. Explain restoration through the OS Recycle Bin where applicable.
- Keep open/reveal/properties and clipboard actions available where supported; capability responses determine platform-specific controls.

## 7. Later enhancements

### 7.1 Storage insights and duplicate integration

- Aggregate logical bytes and counts by extension/category, modified-age bucket, and directory. Categories must have documented extension mappings and an Unknown category.
- Identify empty directories only from complete scans. Age means last modified, not last accessed or safe to delete.
- Add Find duplicates here, which opens the existing duplicate finder with the selected root prefilled. The user explicitly starts that separate job; Sniffer supplies no inferred duplicate/reclaimable totals.
- Add CSV export of the active indexed query with scan time, root, metric, completeness, and filter metadata. Stream export without loading all rows in React and neutralize spreadsheet formula injection in names/paths.

### 7.2 Allocated space and hard links

- Add a separate allocated-bytes field from platform metadata, nullable where unsupported. Keep logical and allocated measures distinct throughout sorting, maps, summaries, and export.
- Record volume/file identity for hard-link accounting. Never deduplicate based only on path, size, or name.
- Define per-path allocated size separately from unique physical allocation of a selected set. Unique allocation across sibling folders is not necessarily additive when links cross folders; disclose this instead of presenting a misleading treemap.
- Cover compressed/sparse files and cloud placeholders with real platform fixtures. Do not hydrate remote content to obtain measurements.
- Do not label either metric reclaimable space without accounting for remaining links and platform behavior. Keep that estimate out of scope until independently specified and validated.

### 7.3 Saved snapshots and growth comparison

- Allow explicit save/delete of completed scan snapshots with timestamps, root identity, options, metric, and coverage. Saving partial data requires an explicit partial label.
- Compare compatible snapshots by relative path and report added, removed, and size-changed files/directories. Use New, Removed, Increased, and Decreased totals.
- Reject or explain incompatible roots/options/metrics. Incomplete scans must not imply that unobserved paths were deleted.
- Treat renames as removed/added unless stable identity matching is implemented and tested. Do not infer renames from equal sizes.
- Store snapshots separately from disposable scan cache; deleting a snapshot deletes only snapshot metadata.
- Storage retention, migrations, export/import, and multi-volume identity support require a separate implementation story before this phase ships.

## 8. Delivery plan

Each work package should be a focused branch/PR when publishing is requested. Do not add these to the local orchestrator automatically.

| Package | Scope                                                                                                                 | Dependencies                                      | Completion evidence                                                              |
| ------- | --------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------- | -------------------------------------------------------------------------------- |
| FS-01   | Extract scan service; job IDs/revisions; cancellable admission/traversal; cleanup guards; structured progress/issues. | None                                              | Lifecycle, queue cancellation, failure cleanup, event-order tests.               |
| FS-02   | SQLite index, bounded frontier/batches, aggregates, query contracts, retention.                                       | FS-01                                             | Exact fixture totals, query paging/filtering, bounded resources, eviction tests. |
| FS-03   | Indexed table/navigation, details, basic search, bounded map, accessibility.                                          | FS-02                                             | UI interaction tests, no-rescan instrumentation, keyboard/manual checks.         |
| FS-04   | Refresh generations, freshness, operation-specific errors, safe rename/recycle actions.                               | FS-02; FS-03 for UI integration                   | Mutation safety fixtures, stale-result tests, refresh atomicity tests.           |
| FS-05   | First-release integration, platform/performance validation, removal of legacy contracts.                              | FS-01–FS-04                                       | All acceptance criteria, local CI/build, measured benchmark report.              |
| FS-06   | Extension/age insights, duplicate handoff, streamed export.                                                           | FS-05                                             | Aggregation, handoff, export tests.                                              |
| FS-07   | Allocated-space and hard-link semantics.                                                                              | FS-05                                             | Platform fixtures and documented metric behavior.                                |
| FS-08   | Saved snapshots and growth comparisons.                                                                               | FS-05; FS-07 if allocated comparisons are offered | Compatibility, incomplete-scan, delta, retention tests.                          |

Suggested code boundaries: a backend `sniffer` service with traversal/index/query/action modules, thin Tauri command adapters, dedicated frontend job/query hooks, and smaller table/map/details components. Keep filesystem identities and action validation in Rust. Reuse existing visual styles and pagination components where suitable.

Maintain the old UI/API until the new path is integrated, then remove `scan_folder_sizes`, legacy path-only mutation handlers, the React scan cache, and obsolete event types together. Update command registration and tests; avoid maintaining two independently active scanners after migration.

## 9. Acceptance criteria and validation

### 9.1 Functional and safety acceptance

1. After a complete root scan, opening any indexed descendant, sorting, or paging performs no filesystem traversal; verify with scanner invocation counters.
2. Largest files in subtree returns files at every depth in deterministic order and never duplicates a node across pages of one revision.
3. Cancellation while queued acquires no traversal permit afterward. Cancellation during traversal produces one terminal outcome and permits the next scan to start.
4. Permission failures and skipped junctions are separately visible, propagate completeness, and cannot appear as verified empty folders.
5. Treemaps never exceed 41 tiles; table pages never exceed 200 rendered data rows. Zero-size-only fixtures remain usable.
6. Late progress/query replies, expired cursors, and failed navigation never produce mismatched breadcrumbs, selections, and rows.
7. Cancelled/failed refresh leaves the previous generation intact; successful refresh updates the subtree and all ancestor totals together.
8. Real names matching UI group labels behave normally. Long, Unicode, UNC, and native non-round-trippable display paths cannot redirect actions.
9. Rename collisions preserve both items. Changed/stale identities, root targets, junctions, and out-of-root requests are rejected before mutation.
10. Recycling failures never trigger permanent deletion. Mutation success followed by refresh failure reports success plus stale results.
11. Hidden Sniffer views consume no navigation shortcuts and returning to the view restores active job state.
12. Cache pruning only touches verified Sniffer-owned cache paths; active/referenced generations survive retention and budget pressure.

### 9.2 Test strategy

- Rust unit/integration tests: nested/wide/deep trees, zero-byte files, aggregate arithmetic, missing paths, link cycles, issue categories, query ordering/filter semantics, cancellation races, admission failure, cleanup, index limits, and refresh transactions.
- Action tests use isolated temporary fixtures exclusively. Verify collision preservation and rejected operations without touching production files. Platform recycling tests must be explicitly isolated and restore/clean their own test artifacts.
- Frontend tests: lifecycle display, revision/request ordering, partial/stale labels, paging/filter reset, keyboard actions, map cap, synthetic-name collisions, action outcomes, and navigation rollback.
- Windows validation: junctions, UNC paths, reserved filenames, case-only rename, hard links and cloud/sparse metadata when those later features ship. Record environmental restrictions rather than treating skipped cases as passing.
- Manual desktop validation: long scan cancellation, switching tabs, large-folder browsing, context menus, screen-reader announcements, and error recovery. Browser mocks alone do not establish native-shell action correctness.

### 9.3 Performance targets

These are proposed acceptance budgets, not measured baseline claims. Record hardware, filesystem, dataset shape, cache state, elapsed time, peak memory, index/WAL bytes, and result correctness. Adjust only with recorded evidence and an explicit specification update.

| Scenario                                                   | Target                                                                              |
| ---------------------------------------------------------- | ----------------------------------------------------------------------------------- |
| Healthy local scan start/cancel acknowledgement            | Within 250 ms, excluding OS picker interaction.                                     |
| Worker cancellation on responsive local storage            | Within 1 second after acknowledgement. Blocked OS calls are reported separately.    |
| Warm indexed children/largest-file page, one million nodes | p95 at most 200 ms, measured over at least 50 queries.                              |
| Warm filtered subtree query, one million nodes             | p95 at most 500 ms; benchmark literal substring queries explicitly.                 |
| Frontend interaction with loaded page                      | No task over 100 ms attributable to map/table processing.                           |
| Progress traffic                                           | At most 7 periodic events/second/job, plus phase/terminal events; no result arrays. |
| Incremental Sniffer memory, one million nodes              | Backend at most 256 MiB; renderer at most 100 MiB above idle.                       |
| First partial results on responsive local storage          | Within 1 second after traversal starts when entries are available.                  |

Use synthetic indexed fixtures for one-million-node query tests and separate actual temporary filesystem fixtures for traversal. Include a wide directory, deep tree, skewed size distribution, zero-byte population, and high-latency share when available. Existing performance proxies must not be reported as production traversal benchmarks.

Run the narrowest relevant tests during implementation. Before handing off a broad code change or opening a PR, run `pnpm run ci:local` as required by [AGENTS.md](../AGENTS.md). Record unavailable checks and reasons. Release work remains governed separately by [RELEASE_GUIDE.md](RELEASE_GUIDE.md).

## 10. Decisions to validate during implementation

The defaults above are sufficient to begin FS-01/FS-02. The following require evidence before their dependent features ship:

- Select a maintained OS recycling/no-replace implementation that satisfies native identity/path guarantees; capture supported-platform behavior in FS-04.
- Validate subtree indexing/query plans against the one-million-node fixture, especially literal substring filtering and deep paths. Tune index representation without weakening bounded-query contracts.
- Confirm disk-budget and memory defaults against realistic path lengths and metadata overhead in FS-05.
- Determine cross-platform allocated-size availability in FS-07. Unknown is an acceptable explicit value; fabricated precision is not.
- Define snapshot schema versioning and saved-data retention before FS-08.

Completion of this specification does not authorize publishing, releases, destructive fixture operations outside verified temporary locations, or changes to unrelated working-tree files.
