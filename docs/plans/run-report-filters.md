# Run report access and action filters

Agreed 22 November 2026.

```text
Completed with errors           [View run report] [Dismiss]
1,182,743 copied · 230 deleted · 7 errors
                         ↓
Sync history › Main Data Backup
Items                        Action [Deletions ▾]
Path                     Action                 Status
…                        Delete from right      Completed
                                                  [Load more]
```

- Open the exact saved run by ID from the completion card, including when the history sidebar is filtered to a different pair or the run is outside the listed page.
- Keep the run's summary and errors visible. Filter item pages in SQLite by all actions, both deletion directions together, or an individual action. Reset pagination on filter changes and ignore stale responses.
- Give rows readable action labels. Show an empty state when a filter matches no items. Keep full-report export semantics.
- Keep recovery in Windows. SyncForge history records the action and result. It does not record whether an individual item is still recoverable from the Recycle Bin.

Verify the direct navigation, an empty filter, sparse deletion matches beyond the first unfiltered page, stable pagination, and unchanged run totals. Run the full local quality gate before handoff.

Execute with the orchestrate skill. Use GPT-6.2 Sol at medium for coordination and extra high for independent review. Check current repository rules before implementation.
