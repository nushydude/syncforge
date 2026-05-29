# S09 — History and export

## Goal

View past sync runs and export reports.

## Acceptance criteria

- [x] Commands: `get_history`, `get_run_detail`
- [x] `historyStore`, `HistoryView`, `RunDetail` components
- [x] `syncStats.ts` aggregates files/bytes/duration; tests
- [x] Export run to CSV and JSON from UI
- [x] Run list sorted by date, filterable by pair

## Depends on

S05 run engine writing `runs` / `run_items`
