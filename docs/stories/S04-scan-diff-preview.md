# S04 — Scan, diff, and preview

## Goal

Scan both sides of a pair, diff against last snapshot, and show a preview table of planned actions.

## Acceptance criteria

- [ ] `scanner.rs`: parallel walk (jwalk), `FileEntry`, glob include/exclude filters
- [ ] `diff.rs`: mode-aware plan vs snapshot; heavy `#[cfg(test)]` coverage
- [ ] `preview_pair` command returns `SyncPlan`
- [ ] Frontend: `filterMatching.ts`, `planFormatting.ts`, `PreviewTable` component
- [ ] Vitest tests for filter matching and plan formatting
- [ ] Preview does not execute copies/deletes

## Depends on

S02 models; pair must exist (S03 helpful but not required for backend-only test)
