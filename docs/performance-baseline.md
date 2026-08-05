# SyncForge performance baseline

The PERF-001 harness creates deterministic fixtures in the OS temp directory, prints the exact path before use, measures the current workload phases, and removes only that directory on exit. It never stores generated trees in Git.

Run the reproducible cases from a clean checkout:

```text
pnpm run perf:small
pnpm run perf:medium
```

Override the fixture shape or seed when investigating a regression:

```text
node scripts/perf/workload.mjs --case small --entries 10000 --depth 6 --mean-file-size 256 --changed-percent 10 --seed 20260802
```

The JSON report includes machine/build/cache metadata, phase wall times, peak working set (or process RSS when the platform sampler is unavailable), and test-only workload counters. The initial safety budgets are ten progress events per second plus five terminal/phase events and a maximum buffered run-item count of 1,000. Time is reported for comparison only; CI should gate on the bounded counters and generous machine-specific ceilings rather than elapsed time alone.

The current harness measures the deterministic workload shape and its scan/plan/history/duplicate/sniffer data-flow proxies. PERF-002 and PERF-003 will replace the corresponding counters with production test hooks as those paths are bounded.
