# SyncForge performance and scalability audit

**Audit date:** 2026-08-02  
**Audited version:** 0.5.0  
**Scope:** Tauri/Rust sync engine, scanning/diffing, SQLite persistence, watch and schedule services, duplicate finder, folder sniffer, Tauri IPC, React state/rendering, production bundle, and automated tests.  
**Change policy for this audit:** application code was not changed. This report is the only new artifact.

## Executive summary

SyncForge is responsive at ordinary desktop sizes and already contains several good safeguards: filesystem work is moved off the async runtime, scans use `jwalk`, preview/history rows are bounded in the DOM, snapshots are pruned to three per pair, SQLite uses WAL and a busy timeout, and automatic runs can reuse a preview plan. The build and existing tests are green.

The app is not yet safe to call scalable for hundreds of thousands to millions of files. The main limits are not React rendering. They are unbounded in-memory collections, full-tree/full-result JSON boundaries, per-file progress events, a single globally locked SQLite connection, and unbounded concurrent filesystem jobs. Large file replacement on Windows also performs a second full copy when the destination already exists.

The first release should complete **PERF-001 through PERF-004**. Those tasks prevent event storms, bound memory/transaction duration, cap I/O concurrency, and repair a scheduler deadline bug found while auditing load orchestration. The next release should complete **PERF-005 through PERF-012** to make preview, history, duplicates, snapshots, and watch mode scale without transferring or rebuilding whole datasets unnecessarily.

## Current baseline and evidence

### Commands run

| Check | Result |
|---|---|
| `pnpm run build` | Pass; JS 267.41 kB (80.94 kB gzip), CSS 37.45 kB (6.94 kB gzip) |
| `pnpm run test:run` | Pass; 19 files, 90 tests |
| `cargo test --manifest-path src-tauri/Cargo.toml` | Pass; 99 tests |

### Targeted payload measurements

The measurements below use representative objects matching the current camel-case JSON shapes. They isolate JavaScript construction/stringify/parse cost; Tauri bridge overhead and Rust allocations would be additional.

| Payload | JSON size | Stringify | Parse | Observed Node heap |
|---|---:|---:|---:|---:|
| 100,000 preview actions | 4.77 MiB | 33.4 ms | 30.1 ms | 38.6 MiB |
| 1,000,000 preview actions | 47.68 MiB | 229.1 ms | 368.7 ms | 290.3 MiB |
| 100,000 snapshot entries | 11.06 MiB | 45.1 ms | 30.7 ms | 46.4 MiB |
| 1,000,000 snapshot entries | 110.63 MiB | 346.1 ms | 378.6 ms | 427.8 MiB |

These numbers are directional, not product benchmarks. They demonstrate that DOM virtualization alone cannot solve full-plan and full-snapshot transport.

### Confirmed strengths; do not redo these as new work

- `preview_pair` uses `spawn_blocking` and loads the snapshot under a short lock before scanning.
- Watch and scheduled runs pass their preview plan into `run_pair_impl`; this avoids their duplicate pre-run scan.
- SQLite opens with WAL and a 5-second busy timeout.
- Snapshots are pruned to the newest three per pair.
- History lists are capped at 100 runs.
- Preview rows are virtualized above 100 rows; run detail renders 200 rows at a time.
- Store hooks support selectors and shallow comparison.
- Different pairs have independent run slots and watch feedback is suppressed after a run.

The older S14 and S15 story checkboxes are stale: their central changes are present in current code even though the story documents remain unchecked.

## Severity and target operating envelope

- **P0:** Must fix before claiming large-tree support. Risk is runaway memory, UI/DB starvation, uncontrolled I/O, or a broken load-orchestration path.
- **P1:** Required for reliable 100k+ entry use and for keeping startup/history/preview bounded.
- **P2:** Important optimization or hardening after boundedness is established.

Use this initial test envelope until real telemetry supplies better thresholds:

- Small: 10,000 entries per side, 5,000 actions.
- Medium: 100,000 entries per side, 50,000 actions.
- Large: 1,000,000 entries per side, 500,000 actions.
- Progress IPC: at most 10 non-terminal events per second per visible run.
- Run-item memory buffer: at most 1,000 items per run.
- UI list page: at most 200 records returned/rendered at once unless virtualized from a bounded client page.
- Concurrent heavy jobs: default two globally, maximum one writer for overlapping roots.
- No test may require committing a generated fixture tree to Git.

## Dependency order

```text
PERF-001 benchmark harness
  |-- PERF-002 progress coalescing
  |-- PERF-003 bounded run-item persistence
  |-- PERF-004 workload admission/backpressure
  |-- PERF-006 hash cache and comparison cleanup
  |-- PERF-007 linear scan/diff data flow
  |-- PERF-013 Windows file replacement
  `-- PERF-016 sniffer hardening

PERF-003 --> PERF-008 database concurrency --> PERF-009 history paging/retention
PERF-006 --> PERF-007 --> PERF-005 paged reusable preview plans
PERF-005 + PERF-008 --> PERF-012 persistent index/incremental watch
PERF-004 --> PERF-012 and PERF-015 bounded parallel execution
PERF-008 --> PERF-010 duplicate result paging/retention
PERF-010 --> PERF-011 lazy heavy workspaces
PERF-004 --> PERF-014 watcher routing/service refresh
```

PERF-001 should start first, but it must not become an excuse to delay the independent correctness fix in PERF-004 or PERF-005's API design.

---

## Task backlog

### PERF-001 — Add a repeatable performance and scalability harness

**Priority:** P0  
**Dependencies:** None  
**Blocks:** Every optimization task's final acceptance

**Problem and evidence**

There are comprehensive correctness tests but no checked-in workload generator, benchmark, peak-memory check, IPC-rate assertion, or regression budget. Optimizations therefore cannot be compared reliably.

**Files to create or change**

- `src-tauri/benches/` or ignored integration tests under `src-tauri/tests/`
- `scripts/perf/` for deterministic fixture generation and process sampling
- `package.json` for read-only benchmark commands
- `docs/performance-baseline.md` for results; do not put generated trees in Git

**Implementation recipe**

1. Add a deterministic fixture generator accepting entry count, nesting depth, mean file size, changed percentage, and random seed.
2. Generate fixtures in the OS temp directory and print the exact absolute path before use.
3. Add separate measurements for scan, plan build, no-op run, changed-file run, history detail query, duplicate scan, and sniffer scan.
4. Count calls to `scan_directory`, `hash_file`, progress callback, DB item flush, and Tauri emit wrapper. Use test-only counters; do not add logging inside per-file production loops.
5. Capture wall time and peak resident memory. On Windows, sample `WorkingSet64` from the child process at 100–250 ms intervals.
6. Run small and medium cases in CI. Keep the large case manual/nightly because one million real files is too expensive for every PR.
7. Save machine specification, build profile, warm/cold cache status, fixture seed, and results with every baseline.

**Acceptance criteria**

- One documented command reproduces small and medium cases from a clean checkout.
- Counters can prove scan/hash/event/flush counts without parsing logs.
- A benchmark failure exits non-zero when progress exceeds 10 events/second or a run-item buffer exceeds 1,000 after the corresponding fixes land.
- CI compares against generous initial ceilings and prints results even on failure.
- The harness cleans only the exact temp directory it created.

**Validation**

- Run the same seed twice and confirm equal file/action counts.
- Run a deliberately slowed implementation and confirm the budget check fails.
- Run `pnpm run ci:local` after adding the harness.

**Do not do**

- Do not use elapsed time alone as a cross-machine hard threshold.
- Do not add a million-file fixture to the repository.

---

### PERF-002 — Coalesce sync progress before crossing the Tauri bridge

**Priority:** P0  
**Dependencies:** PERF-001 counters  
**Can run in parallel with:** PERF-003 and PERF-004

**Problem and evidence**

`run_pair_impl_inner` creates a progress payload for every executable action, and the manual/watch/schedule callers emit each payload over `sync://progress`. The React listener updates the store and notifies subscribers for every accepted event. A 500,000-action run can therefore create 500,000 IPC serializations and UI store updates.

**Files to change**

- `src-tauri/src/engine.rs`
- `src-tauri/src/commands/run.rs`
- `src-tauri/src/run_coordinator.rs`
- `src/store/runStore.ts` only if a final defensive animation-frame coalescer is still needed
- Rust and frontend run-progress tests

**Implementation recipe**

1. Introduce a progress sink that stores the latest non-terminal update and emits at most once every 100 ms.
2. Always emit phase changes, the first running event, the final running count, and completed/failed/cancelled terminal events immediately.
3. Keep cancellation checks per action. Only event delivery is throttled.
4. Avoid allocating `run_id`, `pair_id`, phase, path, and message strings until an event will actually be delivered.
5. Use an injectable clock in unit tests; do not use sleeps.
6. Apply the same sink to manual, watch, and scheduled runs so behavior does not diverge.
7. Optionally coalesce in the frontend with one `requestAnimationFrame` as a defensive layer, but do not rely on the frontend to fix backend serialization volume.

**Acceptance criteria**

- A simulated 100,000-action run emits no more than `duration_seconds * 10 + 5` events.
- The UI receives the latest path/current count and one terminal report.
- Cancellation latency remains bounded by one action, not 100 ms plus a batch.
- Existing run ownership filtering by pair ID and run ID still works.
- No progress is emitted after the terminal event.

**Validation**

- Unit-test a fake clock and 10,000 instantaneous actions.
- Run the medium benchmark and record emit count and UI commit count.
- Run all Rust and frontend tests.

---

### PERF-003 — Bound run memory and SQLite transaction duration

**Priority:** P0  
**Dependencies:** PERF-001 counters  
**Blocks:** PERF-008 and PERF-009

**Problem and evidence**

The engine pushes every completed/failed `RunItem` into one `Vec` and normally calls `flush_run_items` only after the whole apply loop. `insert_run_items` then writes the complete vector in one transaction while holding the global DB mutex. Memory and DB lock time therefore grow linearly with action count. The engine also builds a second `Vec<&SyncAction>` containing every non-skip action.

**Files to change**

- `src-tauri/src/engine.rs`
- `src-tauri/src/persistence.rs`
- relevant persistence/engine tests

**Implementation recipe**

1. Set `RUN_ITEM_BATCH_SIZE` to 500 or 1,000.
2. Flush the item buffer whenever it reaches the limit, then clear it while retaining capacity.
3. Flush remaining items on completion, cancellation, and stop-on-error. Preserve the current guarantee that applied actions have history records.
4. Count executable actions in one iterator pass, then iterate `plan.actions.iter().filter(...)` directly. Remove `Vec<&SyncAction>`.
5. Keep one transaction per bounded batch, not one transaction per row.
6. If a batch insert fails, mark the run failed and do not silently discard already applied file operations. Include a report error explaining history persistence failed.
7. Add a test-only maximum-buffer counter.

**Acceptance criteria**

- Peak buffered `RunItem` count is at most the configured batch size for 100,000 actions.
- `list_run_items` returns every expected row after completed, cancelled, and stop-on-error runs.
- No transaction contains more than the batch size.
- Different-pair history reads can occur between batches after PERF-008 lands.
- Memory growth from history records is effectively constant with action count.

**Validation**

- Test 2.5 batches plus one item.
- Inject an insert failure at a batch boundary and verify the final report.
- Benchmark 100,000 zero-byte create/delete test actions without the Recycle Bin.

---

### PERF-004 — Add global workload admission and fix scheduler deadline state

**Priority:** P0  
**Dependencies:** None for the scheduler fix; PERF-001 for load validation  
**Blocks:** PERF-012, PERF-014, and PERF-015

**Problem and evidence**

Different pair IDs can start an unlimited number of manual/watch/scheduled OS threads. Duplicate and sniffer scans add more blocking work. Per-pair exclusion prevents duplicate work for one pair but does not prevent 50 pairs from saturating the same disk, network share, CPU hashing pool, or SQLite writer. Cross-pair roots may also overlap.

The scheduler recomputes `schedule.upcoming(Utc).next()` from the current time on each loop, then checks whether that newly computed future value is `<= now`. That condition is normally impossible, so a due occurrence can be skipped indefinitely. This is a functional blocker discovered in the scalability path and must be repaired in this task.

**Files to change**

- `src-tauri/src/state.rs`
- `src-tauri/src/run_coordinator.rs`
- `src-tauri/src/commands/run.rs`, `preview.rs`, `duplicates.rs`, `sniffer.rs`
- `src-tauri/src/scheduler.rs`
- `src-tauri/src/watcher.rs`
- `src-tauri/Cargo.toml` if Tokio semaphore support is enabled

**Implementation recipe**

1. Add a shared `WorkCoordinator` to `AppState` with a bounded heavy-job semaphore. Default capacity: two; expose no UI setting in this task.
2. Represent jobs as manual run, automatic run, preview, duplicates, or sniffer. Manual work may be ahead of queued background work but must not cancel it.
3. Replace one unbounded `std::thread::spawn` per automatic pair with queued `spawn_blocking` work that acquires a permit before scanning.
4. Canonicalize job roots and prevent simultaneous writer jobs whose roots overlap. Read-only previews may share roots but still consume a global permit.
5. Deduplicate queued automatic jobs by pair and reason. A watch storm must not create an unbounded queue.
6. Make queue cancellation remove a job before it starts and retain the existing active-run cancellation behavior.
7. For the scheduler, retain each pair's actual next deadline across sleeps. When it becomes due, enqueue exactly once, then advance that schedule to the following occurrence.
8. Wake the scheduler through a notification/channel on configuration changes instead of polling a cancellation atomic every 100 ms.

**Acceptance criteria**

- Starting 20 background pairs results in no more than two heavy jobs executing simultaneously by default.
- Queue size is bounded by the number of configured pairs plus one duplicate and one sniffer job.
- Overlapping writer roots never execute concurrently, even under different pair IDs.
- A one-second test schedule fires once, advances once, and does not busy-loop or skip.
- Manual UI work remains responsive while background jobs are queued.
- No DB mutex is held while waiting for a work permit.

**Validation**

- Use barriers and atomic active-job counters; do not use timing-only tests.
- Add fake-clock scheduler tests covering due, missed-by-sleep, config refresh, and cancellation cases.
- Run a medium scan with 20 queued pairs and record disk queue/CPU behavior.

---

### PERF-005 — Replace full preview payloads with paged, reusable plan handles

**Priority:** P1  
**Dependencies:** PERF-006 and PERF-007; coordinate with PERF-008  
**Blocks:** PERF-012

**Problem and evidence**

`preview_pair` returns the entire `SyncPlan` over Tauri IPC and both preview components map every action before showing a page/window. One million representative actions form about 47.7 MiB of JSON and roughly 290 MiB of observed JavaScript heap in the isolated transport check.

Manual runs do not reuse a preview already shown in the pair UI. With `Ask`, `runSelectedPair` performs another preview for conflicts and `run_pair` then performs its own pre-scan. A preview followed by a manual run can therefore repeat full-tree work. Passing an arbitrary client plan back without validation would be unsafe.

**Files to change**

- Rust models and `src-tauri/src/commands/preview.rs`, `commands/run.rs`, `engine.rs`, `state.rs`
- `src/api/preview.ts`, `src/api/run.ts`
- plan types, pairs store, run store, preview components, conflict dialog
- tests on both sides

**Implementation recipe**

1. Change preview to return `PreviewSummary`: opaque `planId`, pair/config fingerprint, creation time, action totals by kind, scan integrity, conflicts count, and only the first action page.
2. Add `get_preview_actions(planId, cursor, limit)` with a hard maximum limit of 200 and stable path ordering.
3. Store plan actions backend-side in a bounded plan store. Prefer a temp SQLite table/file so large plans do not stay duplicated in Rust heap. Limit by count, bytes, and five-minute TTL; delete on run completion or explicit discard.
4. Add per-action preconditions captured at preview: expected source and target type/size/mtime/nanos. Validate immediately before copy/delete. If a precondition changed, stop safely and ask for a new preview.
5. Let `run_pair` accept `planId`. Verify pair ID and config fingerprint and reject expired/mismatched handles.
6. Reuse the plan shown by Pair Details and the conflict plan; do not rescan merely to rediscover conflicts.
7. Keep automatic runs able to pass an internal plan without IPC.
8. Update preview paging and conflict paging so the frontend never owns the full action list. Conflict choices should be sent as a bounded map/page or stored against the plan handle.

**Acceptance criteria**

- No preview/history API response contains more than 200 action/item records.
- A preview followed by a manual run performs two initial directory scans total, not four or six.
- An expired, changed-config, or changed-file plan fails safely before applying the affected action.
- Plan storage is bounded and cleaned on success, cancel, expiry, and app startup.
- A million-action synthetic preview does not allocate the full action set in the webview.

**Validation**

- Count scans across Preview -> View page 2 -> Run.
- Modify a source and a delete target after preview; both must reject stale actions.
- Verify paging has no duplicates or gaps and preserves ordering.

---

### PERF-006 — Cache content hashes and stop re-reading files during one plan

**Priority:** P1  
**Dependencies:** PERF-001 hash counters  
**Blocks:** PERF-007 and PERF-005

**Problem and evidence**

When two files have identical metadata and are under 50 MiB, `entries_differ` hashes both physical paths. Synchronize planning can call that comparison repeatedly for left/right and snapshot decisions. There is no per-plan cache. Snapshot entries have no physical side, yet comparisons involving a snapshot still route through left/right roots; this is both unnecessary I/O and a source of ambiguous behavior.

**Files to change**

- `src-tauri/src/diff.rs`
- `src-tauri/src/hashing.rs`
- snapshot model/persistence only if hashes are retained
- diff tests and benchmark counters

**Implementation recipe**

1. Give current entries an explicit side when resolving a physical path. Never infer a snapshot's path from argument position.
2. Add a per-plan hash cache keyed by side, relative path, size, modified seconds, and nanoseconds.
3. Hash each current physical file at most once per plan. Reuse the result in every left/right/snapshot decision.
4. Do not hash a snapshot entry unless it contains a previously stored content hash. Metadata-only snapshots must use documented metadata semantics.
5. Preserve warning/error information when a hash cannot be read; do not silently convert all hash failures to “same file.” For destructive modes, surface a scan-attention error if correctness depends on the failed hash.
6. Keep the size threshold configurable through one backend option; remove duplicated magic defaults.

**Acceptance criteria**

- An equal-metadata left/right file causes at most two physical hash calls in one plan.
- Repeated snapshot comparisons add zero physical reads.
- Hash failures are visible and cannot authorize a destructive action.
- Existing same-second-content-change behavior remains covered.

**Validation**

- Add fake hasher/path resolver tests with exact call counts.
- Benchmark a 100k-file no-op tree containing many equal-metadata files.

---

### PERF-007 — Make scan-to-diff processing linear and reduce entry cloning

**Priority:** P1  
**Dependencies:** PERF-001 and PERF-006  
**Blocks:** PERF-005 and PERF-012

**Problem and evidence**

Scans return sorted `Vec<FileEntry>`, but planning clones every entry into three `HashMap<String, FileEntry>` values, clones every key into a `BTreeSet`, then creates another path vector. Conflict actions clone entries again. `ensure_parent_dirs` repeatedly calls `has_action_for_path`, a linear scan for each needed directory. Peak memory has several full representations of the same tree, and parent planning can trend toward quadratic work.

**Files to change**

- `src-tauri/src/scanner.rs`
- `src-tauri/src/diff.rs`
- `src-tauri/src/models.rs` only if borrowed/internal planning types are introduced

**Implementation recipe**

1. Keep scanner output sorted by relative path.
2. Replace cloned entry maps and the `BTreeSet` union with a three-way merge iterator over sorted left, right, and snapshot sequences.
3. Pass borrowed entries through planning. Clone only fields that must survive in the final plan/action.
4. Track planned create-directory actions in `HashSet`s while building actions. Remove repeated `has_action_for_path` scans.
5. Generate parent paths without repeatedly allocating all prefixes where possible; intern or reuse path strings within one plan.
6. Preserve deterministic path order and current mode semantics.
7. Record peak RSS for old and new algorithms using identical fixtures.

**Acceptance criteria**

- Planning is O(L + R + S + A log A) or better; no nested action-list search remains.
- Medium-case peak RSS decreases materially (target at least 30%).
- The large synthetic metadata-only case completes without webview involvement and without exhausting an 8 GiB machine.
- Every existing diff test passes unchanged or with only API-adapter changes.

**Validation**

- Add worst-case deep-parent and all-conflict benchmarks.
- Property-test the merge implementation against the current implementation on small random trees before removing the old code.

---

### PERF-008 — Remove long work from the global SQLite mutex and add query indexes

**Priority:** P1  
**Dependencies:** PERF-003  
**Blocks:** PERF-009, PERF-010, and PERF-012

**Problem and evidence**

`AppState` exposes one `Arc<Mutex<Database>>` around one connection. WAL cannot provide concurrent readers when all access is serialized by that mutex. Snapshot and duplicate-result JSON is serialized/deserialized in database methods while the lock is held. Large run-item inserts also hold it for a transaction. Several synchronous Tauri commands perform reads directly under this lock.

Current indexes do not fully match ordered queries: history needs `(pair_id, started_at DESC)` and run detail paging needs `(run_id, path COLLATE NOCASE, id)`.

**Files to change**

- `src-tauri/src/state.rs`
- `src-tauri/src/persistence.rs`
- all command/coordinator callers of `state.db.lock()`
- migration and concurrency tests

**Implementation recipe**

1. Replace public mutex access with a `DatabaseManager` API. Keep one short-lived serialized writer connection and use separate read connections under WAL.
2. Return raw JSON/blob from the connection, release the DB resource, then serialize/deserialize outside the critical section until normalized storage replaces blobs.
3. Run potentially blocking DB commands in `spawn_blocking`; do not block the async runtime while waiting for SQLite.
4. Add indexes:
   - `runs(started_at DESC)`
   - `runs(pair_id, started_at DESC)`
   - `snapshots(pair_id, captured_at DESC)`
   - `run_items(run_id, path COLLATE NOCASE, id)`
5. Use `EXPLAIN QUERY PLAN` tests to confirm paging queries use the intended index.
6. Centralize transaction helpers and set WAL/busy timeout/foreign keys on every opened connection.
7. Never hold a DB connection while scanning, hashing, waiting for a permit, emitting an event, or sending a notification.

**Acceptance criteria**

- A history list/detail read can complete while another run is inserting a bounded batch.
- JSON parse/serialize time is outside the writer lock.
- No production module accesses a public `Mutex<Database>`.
- Query-plan tests show no full sort/table scan for capped history and paged run items.

**Validation**

- Add a barrier-based concurrent writer/reader test using a file-backed temp DB.
- Benchmark read latency during a 100k-item run.

---

### PERF-009 — Page run history, stream exports, and prune retained run data

**Priority:** P1  
**Dependencies:** PERF-003 and PERF-008

**Problem and evidence**

History list is capped, but `get_run_detail` loads every `run_item`, transfers all of them over IPC, and only then does React slice to 200 visible rows. CSV/JSON export also builds the complete detail in the webview. The `runs` and `run_items` tables grow without a retention policy.

**Files to change**

- `src-tauri/src/persistence.rs`, `commands/history.rs`
- history API/types/store/components and export helpers
- settings only if retention becomes user-configurable; a fixed documented default is enough initially

**Implementation recipe**

1. Add cursor paging to history list and run items. Use `(started_at, id)` and `(path COLLATE NOCASE, id)` as stable cursors.
2. Return summary totals separately from the item page so UI statistics do not require all items.
3. Hard-cap page size at 200 in Rust regardless of the client request.
4. Move CSV/JSON export to a backend command that streams DB rows directly to a selected file in bounded buffers.
5. Add retention: default 1,000 runs per pair or 90 days, whichever retains fewer, while preserving any running record. Document the chosen policy.
6. Delete old runs in small transactions so cascade deletion of items does not monopolize the writer.
7. Run pruning after completion or startup, never in the per-action loop.

**Acceptance criteria**

- Opening a 500,000-item run transfers at most 200 items initially.
- “Load more” fetches the next server page with no duplicates/gaps.
- Export memory remains bounded and cancellation removes or clearly marks a partial file.
- Retention has deterministic tests and does not remove running runs.

**Validation**

- Seed 1,001 runs and verify pruning boundaries.
- Seed duplicate paths plus IDs to prove stable cursor behavior.
- Export 500,000 synthetic items and sample peak RSS.

---

### PERF-010 — Normalize, page, and prune duplicate scan results

**Priority:** P1  
**Dependencies:** PERF-008  
**Blocks:** PERF-011

**Problem and evidence**

Duplicate scanning retains all candidates/groups in memory. On completion it serializes the entire result into `duplicate_scans.result_json`, and `get_duplicate_scan` deserializes and transfers that whole result. The React view renders every group and every file. Selecting one checkbox recomputes selected bytes by scanning all result files. Old duplicate scan rows are never pruned.

**Files to change**

- `src-tauri/src/duplicates.rs`, `commands/duplicates.rs`, `persistence.rs`
- duplicate API/types/view/tests

**Implementation recipe**

1. Keep job/progress summary in `duplicate_scans`, but move groups/files into normalized `duplicate_groups` and `duplicate_files` tables keyed by scan ID.
2. Insert results in bounded transactions as groups are finalized. Do not build one result JSON blob.
3. Add paged group and group-file commands with a maximum 100 groups/200 files per response.
4. Render a bounded page or virtualized flattened list. Do not mount every checkbox at once.
5. Build a file-size lookup only for the current page/selection and update selected byte totals incrementally rather than reducing the full result after every toggle.
6. Store selections by stable file ID, not only relative path.
7. Retain the newest three completed/interrupted scan summaries and cascade-delete older groups/files.
8. Keep cancellation checks in collection and hash loops; ensure partial normalized results are deleted or marked incomplete.

**Acceptance criteria**

- No duplicate API returns an unbounded group/file array.
- Completion does not serialize one full result blob.
- Toggling a checkbox is O(1) with respect to total scan size.
- Old scan storage is bounded by the documented retention rule.
- A million candidate metadata records can be processed without creating a million DOM nodes.

**Validation**

- Seed thousands of groups without real file hashing to test paging.
- Cancel midway and verify cleanup/recovery.
- Compare group totals and potential savings against the current implementation on the same small tree.

---

### PERF-011 — Do not load heavy hidden workspaces at application startup

**Priority:** P1  
**Dependencies:** PERF-010 for the final bounded duplicate fetch; can begin earlier

**Problem and evidence**

`App.tsx` mounts all four workspaces and hides inactive ones. This preserves UI state, but `DuplicatesView` immediately subscribes and calls `getDuplicateScan`, so a large stored result is parsed and transferred at app startup even when the user never opens Duplicates. All workspace code is also in one JS chunk, although the current 80.94 kB gzip bundle is acceptable.

**Files to change**

- `src/App.tsx`
- `DuplicatesView.tsx`, `FolderSnifferView.tsx`, and possibly `PairsPanel.tsx`
- Vite/frontend tests

**Implementation recipe**

1. Pass an `active` prop to mounted workspaces. Start heavy fetches/listeners only on first activation.
2. Preserve already loaded local state when switching tabs; activation must not restart an active backend scan.
3. Fetch only the duplicate job summary on activation, followed by paged results from PERF-010.
4. Use `React.lazy` for Sniffer and Duplicates if it produces clear separate chunks; do not split tiny shared utilities.
5. Keep background jobs independent of view mounting. Re-entering a view should query current status.

**Acceptance criteria**

- Cold startup on the Pairs tab makes no duplicate-result or sniffer scan request.
- Opening Duplicates performs one summary request and one bounded first-page request.
- Switching away and back does not lose selections or start a new scan.
- Main startup bundle does not grow; record chunk sizes.

**Validation**

- Mock invoke calls in a startup test and assert exact command counts before/after tab activation.

---

### PERF-012 — Introduce a persistent file index and incremental watch planning

**Priority:** P1 strategic  
**Dependencies:** PERF-004, PERF-005, PERF-007, and PERF-008

**Problem and evidence**

Every watch event is reduced to a pair ID. After debounce, the app scans both complete roots to identify changes. Even with plan reuse, an automatic changed-file run performs two preview scans plus two post-run scans. This makes watch mode O(total tree size) for a one-file edit and is not viable for million-entry trees or slow network shares. Snapshots are whole-tree JSON blobs and cannot be updated/query-streamed by path.

**Files to change**

- persistence schema and manager
- scanner/diff/engine
- watcher/state/run coordinator
- migration and recovery tests

**Implementation recipe**

1. Add a normalized latest-state table keyed by `(pair_id, side, relative_path)` with type, size, mtime seconds/nanos, and optional hash.
2. Build/rebuild the index in bounded batches. Mark an index generation complete only after both sides finish successfully.
3. Accumulate dirty relative paths per pair during debounce, including parents affected by rename/create/delete.
4. Stat and diff dirty paths against the index, producing a small plan handle. Update the index transactionally only after successful actions.
5. Fall back to a full reconciliation scan on watcher overflow, unknown rename pairing, missed events, filter/config changes, or an incomplete index generation.
6. Schedule periodic reconciliation (for example daily or after N incremental runs) so missed OS events cannot permanently hide drift.
7. Keep destructive scan integrity: an unreadable dirty path or incomplete full reconciliation must not authorize deletion.
8. Migrate from existing latest snapshot JSON by rebuilding from disk; do not trust a partial conversion.

**Acceptance criteria**

- A one-file watch event in a 100k-entry pair stats/diffs O(dirty paths), not both full trees.
- Watcher overflow forces a safe full reconciliation.
- Crash during index rebuild leaves the prior complete generation usable or forces a rebuild; it never exposes a partial generation as current.
- Full manual preview remains available as an explicit reconciliation path.

**Validation**

- Integration tests for create, modify, delete, rename, directory rename, filter change, overflow, and crash recovery.
- Benchmark one-file edits at 10k and 100k total entries and show near-flat planning time.

---

### PERF-013 — Avoid the second full file copy when replacing files on Windows

**Priority:** P1  
**Dependencies:** PERF-001 throughput benchmark  
**Can run in parallel with:** Most P1 tasks

**Problem and evidence**

`safe_copy_file` copies source to a temp file in the destination directory. `commit_temp_file` tries `fs::rename`; on Windows this fails when the destination exists, so it copies the complete temp file over the destination and deletes the temp. Updating an existing large file therefore performs a second full data copy. Hash verification can add additional full reads.

**Files to change**

- `src-tauri/src/engine.rs`
- Windows-specific helper module and `windows-sys` features
- copy/replace tests

**Implementation recipe**

1. Because the temp file is created beside the destination, use a Windows replace-existing primitive (`ReplaceFileW` or a carefully configured `MoveFileExW`) for the commit.
2. Preserve the original destination if commit fails and clean up the temp according to existing safety rules.
3. Keep Unix `rename` behavior.
4. For verification, evaluate a streaming copy that hashes source bytes while writing temp, then hashes temp once. Do not weaken verification semantics just to improve speed.
5. Preserve metadata intentionally or document current metadata behavior; do not accidentally regress timestamps/permissions.

**Acceptance criteria**

- Replacing an existing Windows destination does not call the full-file copy fallback in the normal same-directory case.
- A forced commit failure leaves original destination bytes intact.
- Verified and unverified copies pass correctness tests for zero-byte and multi-gigabyte sparse test files where supported.
- Benchmark shows bytes read/written per replacement are no longer doubled by commit.

**Validation**

- Use a test hook around copy/replace syscalls to assert call counts.
- Run Windows-only failure-injection tests.

---

### PERF-014 — Optimize watcher routing and make service updates incremental

**Priority:** P2  
**Dependencies:** PERF-004

**Problem and evidence**

For each filesystem event, the watcher clones the complete watched-root vector and compares every event path with both roots of every watched pair. Saving a pair restarts both watch and schedule services; the frontend then calls `set_schedule`, which saves again and restarts the scheduler again. Cost grows with pair count and produces avoidable service churn.

**Files to change**

- `src-tauri/src/watcher.rs`, `scheduler.rs`, `commands/pairs.rs`, `commands/schedule.rs`
- `src/store/pairsStore.ts`, `src/api/pairs.ts`

**Implementation recipe**

1. Replace vector cloning with an immutable root index behind `Arc`, swapped atomically on config change.
2. Route paths using canonical root components/prefix index; do not linearly compare every pair when pair counts are large.
3. Unwatch/watch only changed roots where the `notify` API permits. Otherwise rebuild once after the full save transaction.
4. Make `save_pair` persist schedule fields in the same call. Remove the immediate second `set_schedule` save from the frontend.
5. Notify the scheduler/watch service through update channels instead of tearing down and rebuilding unrelated state.
6. Remove expired entries from `watch_suppress_until` during normal access to prevent slow map growth after deleted pairs.

**Acceptance criteria**

- One pair edit causes one DB save and one configuration update notification.
- Routing an event does not clone all roots.
- Event routing benchmark scales sublinearly with pair count for non-overlapping roots.
- Existing shared/nested path normalization tests still pass.

**Validation**

- Benchmark 1, 100, and 1,000 configured pairs with a fixed event batch.
- Add command tests asserting a schedule-only edit does not rebuild the watcher.

---

### PERF-015 — Add bounded parallelism for independent file actions and duplicate hashing

**Priority:** P2  
**Dependencies:** PERF-004, PERF-005, PERF-007, and PERF-013

**Problem and evidence**

Actions within one pair and duplicate candidate hashes execute serially. This is safe but leaves throughput on the table for many small files or fast SSDs. Unbounded parallelism would make disk/network behavior worse and could violate directory/delete ordering, so this optimization comes only after admission control and plan preconditions.

**Files to change**

- engine action planner/executor
- duplicates hashing pipeline
- settings only if an advanced concurrency setting is later approved

**Implementation recipe**

1. Partition actions into ordered phases: create parents, copies/updates, file deletes, then deepest-first directory deletes.
2. Build dependencies by path so actions touching the same path/parent cannot overlap incorrectly.
3. Execute independent file copies with a small bounded worker count (start at two). Share PERF-004 permits so jobs do not multiply concurrency.
4. Hash duplicate candidates with the same bounded I/O pool, grouping by size first as today.
5. Keep progress and run-item persistence ordered by a stable sequence number even if completion order differs.
6. Stop scheduling new work promptly on cancellation or stop-on-error; safely join in-flight operations.

**Acceptance criteria**

- Active file operations never exceed the configured bound.
- Directory and delete ordering remains correct under adversarial nested paths.
- HDD/network-share benchmark is not materially worse; default remains conservative if no universal win exists.
- Reports contain one item per action in deterministic display order.

**Validation**

- Barrier tests prove the bound and real overlap.
- Stress nested create/copy/delete and injected failures.
- Benchmark many-small-file and few-large-file workloads separately.

---

### PERF-016 — Make Folder Sniffer iterative, cancellable, bounded, and cache-aware

**Priority:** P2  
**Dependencies:** PERF-001 and PERF-004

**Problem and evidence**

The sniffer uses a recursive, serial `measure` function with no cancellation. It can recurse deeply, follow filesystem indirections unexpectedly, and scans a top-level child's metadata twice. Opening a child rescans its subtree even though the parent scan just traversed it. The frontend cache is an unbounded `Map`; expanding files or “Other items” can create an unbounded DOM and the treemap layout can approach quadratic behavior for skewed sizes.

**Files to change**

- `src-tauri/src/commands/sniffer.rs`
- sniffer API/types/view/tests

**Implementation recipe**

1. Replace recursion with an explicit iterative walker or `jwalk` configured not to follow links/junction loops.
2. Add a scan job ID and cancellation token. Starting a new navigation cancels or supersedes the old scan; stale progress/results must be ignored by job ID.
3. Aggregate top-level totals in one traversal and avoid duplicate metadata calls.
4. Throttle progress to 10 events/second and include visited entry count even when top-level completion is unchanged.
5. Use a size/count-bounded LRU cache, for example 25 folders or 100 MiB of result estimates, whichever comes first.
6. Page/virtualize expanded file lists and cap treemap layout input. “Expand other” should page, not layout every child.
7. Replace the skew-sensitive recursive treemap splitter or enforce a small input cap so its worst case is bounded.

**Acceptance criteria**

- Deep directory trees cannot overflow the stack.
- Navigation cancellation prevents an old scan from replacing the new folder view.
- Cache size stays below its documented bound.
- No sniffer interaction mounts more than 200 file rows or the configured treemap cap.
- Link/junction cycles are skipped and counted as warnings.

**Validation**

- Test a synthetic 2,000-level logical walker without creating an OS path beyond platform limits.
- Test cancellation, stale job events, LRU eviction, junction/symlink behavior, and 100k direct children.

---

## Recommended delivery waves

### Wave 1 — Bound the existing architecture

1. PERF-001 harness
2. PERF-002 progress coalescing
3. PERF-003 run-item batching and pointer-vector removal
4. PERF-004 admission control and scheduler deadline repair

**Exit condition:** large runs cannot generate unbounded events, item buffers, transactions, threads, or background queue entries.

### Wave 2 — Bound API and persistence surfaces

1. PERF-006 hash cache
2. PERF-007 linear scan/diff
3. PERF-008 DB manager and indexes
4. PERF-009 history paging/retention
5. PERF-010 duplicate paging/retention
6. PERF-011 lazy heavy workspace activation
7. PERF-013 Windows replacement

**Exit condition:** no user-facing API loads an unbounded action/item/duplicate result, and Windows updates avoid the redundant commit copy.

### Wave 3 — Remove full-tree work from the steady state

1. PERF-005 reusable paged plan handles
2. PERF-012 persistent index/incremental watch
3. PERF-014 incremental watcher/scheduler configuration
4. PERF-015 bounded parallel execution
5. PERF-016 sniffer hardening

**Exit condition:** a one-file watched change is proportional to dirty paths, and optional parallelism remains globally bounded.

## Global definition of done for every task

Every implementation task is complete only when all of the following are true:

1. The task's acceptance criteria are covered by deterministic automated tests.
2. `pnpm run ci:local` passes.
3. The relevant PERF-001 before/after result is recorded with machine/build/fixture metadata.
4. New limits, TTLs, page sizes, retention rules, and concurrency values are named constants with comments explaining the safety reason.
5. Failure, cancellation, restart, and stale-response paths are tested, not only success.
6. No filesystem scan, hash, DB transaction, event emit, or notification occurs while an unrelated global lock is held.
7. The change does not weaken destructive-sync safeguards or stale-plan validation.
8. Documentation and API types are updated in the same change.

## Deferred/non-performance observations

- The scheduler due-time issue is included in PERF-004 because it blocks meaningful scheduled-load testing.
- Security posture (for example `csp: null`) was not assessed by this performance audit.
- Sync correctness semantics, Recycle Bin UX, and conflict policy design were reviewed only where they constrain safe optimization.

