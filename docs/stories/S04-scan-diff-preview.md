# S04 — Scan, diff, and preview

## Goal

Scan both sides of a pair, diff against last snapshot, and show a preview table of planned actions.

## Acceptance criteria

- [x] `scanner.rs`: parallel walk (jwalk), `FileEntry`, glob include/exclude filters
- [x] `diff.rs`: mode-aware plan vs snapshot; heavy `#[cfg(test)]` coverage
- [x] `preview_pair` command returns `SyncPlan`
- [x] Frontend: `filterMatching.ts`, `planFormatting.ts`, `PreviewTable` component
- [x] Vitest tests for filter matching and plan formatting
- [x] Preview does not execute copies/deletes

## Depends on

S02 models; pair must exist (S03 helpful but not required for backend-only test)
