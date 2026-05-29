# S18 — Frontend performance (selectors, virtualization)

## Context (audit P2 frontend)

Full-store subscriptions and unbounded preview/history tables.

## Goal

Lean UI for large sync plans (10k+ actions).

## Acceptance criteria

- [ ] `usePairsStore(selector)` / `useRunStore(selector)` / `useHistoryStore(selector)` with shallow compare (or split subscriptions)
- [ ] `PairList` only subscribes to `pairs`, `selectedId`, `loading`
- [ ] `PreviewTable`: virtualize rows (`@tanstack/react-virtual` or windowed render); `useMemo` for formatted rows
- [ ] `RunDetail` items table virtualized or paginated (first 200 + load more)
- [ ] `RunProgress` / `PairEditor`: avoid duplicate full `useSyncProgress` re-renders (`React.memo` on `RunProgress`)
- [ ] `HistoryView`: selector for pair names only
- [ ] Optional: keep panels mounted in `App.tsx` (hidden) to avoid reload on tab switch
- [ ] `pnpm test` pass; no regressions in smoke tests

## Files likely touched

- `src/hooks/usePairsStore.ts`, `useSyncProgress.ts`, `useHistoryStore.ts`
- `src/components/preview/PreviewTable.tsx`
- `src/components/history/RunDetail.tsx`, `HistoryView.tsx`
- `src/components/pairs/PairEditor.tsx`, `PairList.tsx`
- `src/App.tsx`
- `package.json` (if adding `@tanstack/react-virtual`)

## Depends on

S17
