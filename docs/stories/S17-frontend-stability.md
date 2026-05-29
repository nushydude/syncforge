# S17 — Frontend stability (preview, progress, history races)

## Context (audit P0/P1 frontend)

Stale preview, background progress hijacking UI, history race, cancel UX.

## Goal

LightFrame-style request tokens and run ownership in stores.

## Acceptance criteria

- [ ] `pairsStore`: `previewRequestId`; ignore stale preview responses; reset `previewLoading` on `selectPair` / `cancelEdit`
- [ ] `runStore`: track `activeRunId` + `activePairId` for UI-initiated runs; progress listener ignores events when not matching
- [ ] Background watch/schedule progress does not set `running` or hijack pair editor unless user started run
- [ ] `historyStore`: request id guard on `selectRun` and `loadHistory`
- [ ] `cancelActiveRun`: optimistic UI clear; recover on error
- [ ] `runSelectedPair`: guard against double-click (in-flight from preview through execute)
- [ ] Block run while `previewLoading`
- [ ] Debounce path-exists checks (~300ms) in `pairsStore`
- [ ] Vitest tests for preview token stale ignore and progress filtering (mock listen)
- [ ] `pnpm test` pass

## Files likely touched

- `src/store/pairsStore.ts`
- `src/store/runStore.ts`
- `src/store/historyStore.ts`
- `src/components/pairs/PairEditor.tsx`
- `src/test/` new tests

## Depends on

S10 (cancel_run fix) for cancel E2E
