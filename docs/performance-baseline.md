# SyncForge performance baseline

The harness creates deterministic private fixture trees in the OS temp directory, prints the exact path, and removes only that generated directory on exit. Never point it at real data. Run the cases with:

```text
pnpm run perf:small -- --enforce-budgets
pnpm run perf:medium -- --enforce-budgets
```

Override the shape or seed when investigating a regression:

```text
node scripts/perf/workload.mjs --case small --entries 1000 --depth 4 --mean-file-size 256 --changed-percent 10 --seed 20260802 --enforce-budgets
```

At least one non-empty changed file is required. Separate generated Echo pairs exercise no-op and changed production runs. Changed destinations have different bytes and older modification times so the normal metadata comparison schedules copies. The harness requires completed status, expected copies and transferred bytes, no deletes, changed-run item persistence, unchanged no-op file content and timestamps, and matching changed output content and counts. These checks are always enforced.

The JSON report records production-run counters and timings from Rust. It also reports simplified JavaScript measurements for scan, plan, History, Duplicates, and Folder Sniffer. The test event emitter measures call counts. On Windows and Linux, `rustProcessPeakResidentBytesAtRunEnd` is the Rust test process's cumulative peak at the end of each run. The changed run's value can include the earlier no-op run. On other platforms the field is null. The Node parent has a separate sampled working set field. Cache state is unknown and timings use the working-tree test build.

With `--enforce-budgets`, each run's progress events are bounded by ten per second of that Rust run plus ten events for phase transitions, pending or terminal events, and scheduling tolerance. Run-item buffering is bounded at 1,000. CI enforces these counter budgets for both sizes. Elapsed time is reported and used to calculate the event allowance. Compare timings across equivalent machine, build, and cache conditions.
