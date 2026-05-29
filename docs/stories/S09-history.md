# S09 — History and export

## Goal

View past sync runs and export reports.

## Acceptance criteria

- [ ] Commands: `get_history`, `get_run_detail`
- [ ] `historyStore`, `HistoryView`, `RunDetail` components
- [ ] `syncStats.ts` aggregates files/bytes/duration; tests
- [ ] Export run to CSV and JSON from UI
- [ ] Run list sorted by date, filterable by pair

## Depends on

S05 run engine writing `runs` / `run_items`
