# S06 — Conflict resolution

## Goal

Detect conflicts using last-sync snapshots and resolve per policy.

## Acceptance criteria

- [ ] Snapshot distinguishes delete vs new file vs true conflict
- [ ] Policies: newer-wins, left, right, keep-both, ask
- [ ] `conflictPolicy.ts` service with tests
- [ ] `ConflictDialog` UI when policy is `ask`
- [ ] Diff engine surfaces `Conflict` actions in plan

## Depends on

S05 run engine and snapshots
