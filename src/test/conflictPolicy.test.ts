import { describe, expect, it } from 'vitest';
import {
  allConflictsResolved,
  applyConflictChoice,
  applyResolutionsToPlan,
  classifyPathChange,
  entriesEqual,
  listConflictActions,
  pathChangeKindLabel,
  resolveConflictAction,
} from '../lib/conflictPolicy';
import type { FileEntry, SyncPlan } from '../types';

function file(
  path: string,
  size: number,
  modifiedSecs: number,
): FileEntry {
  return { relativePath: path, size, modifiedSecs, isDir: false };
}

describe('conflictPolicy', () => {
  it('detects equal entries', () => {
    const a = file('a.txt', 1, 1);
    expect(entriesEqual(a, { ...a })).toBe(true);
    expect(entriesEqual(a, file('a.txt', 2, 1))).toBe(false);
  });

  it('classifies new file without snapshot', () => {
    expect(classifyPathChange(file('n.txt', 1, 1), undefined, undefined)).toBe(
      'newOnLeft',
    );
    expect(classifyPathChange(undefined, file('n.txt', 1, 1), undefined)).toBe(
      'newOnRight',
    );
  });

  it('classifies delete vs new using snapshot', () => {
    const snap = file('gone.txt', 1, 1);
    expect(
      classifyPathChange(file('gone.txt', 1, 1), undefined, snap),
    ).toBe('deletedOnRight');
    expect(
      classifyPathChange(undefined, file('gone.txt', 1, 1), snap),
    ).toBe('deletedOnLeft');
    expect(
      classifyPathChange(file('new.txt', 2, 2), undefined, undefined),
    ).toBe('newOnLeft');
  });

  it('classifies true conflict when both sides changed', () => {
    const snap = file('both.txt', 1, 1);
    expect(
      classifyPathChange(file('both.txt', 10, 10), file('both.txt', 20, 20), snap),
    ).toBe('bothChanged');
    expect(pathChangeKindLabel('bothChanged')).toBe('Conflict');
  });

  it('resolves policies to sync actions', () => {
    const left = file('x.txt', 1, 100);
    const right = file('x.txt', 2, 200);
    expect(resolveConflictAction('left', 'x.txt', left, right)).toEqual({
      kind: 'copyLeftToRight',
      path: 'x.txt',
    });
    expect(resolveConflictAction('ask', 'x.txt', left, right).kind).toBe(
      'conflict',
    );
    expect(resolveConflictAction('newerWins', 'x.txt', left, right).kind).toBe(
      'copyRightToLeft',
    );
  });

  it('applies per-path user choices to a plan', () => {
    const plan: SyncPlan = {
      pairId: 'p1',
      scannedLeft: 1,
      scannedRight: 1,
      actions: [
        {
          kind: 'conflict',
          path: 'a.txt',
          left: file('a.txt', 1, 1),
          right: file('a.txt', 2, 2),
        },
        { kind: 'copyLeftToRight', path: 'b.txt' },
      ],
    };
    expect(listConflictActions(plan)).toHaveLength(1);
    const resolved = applyResolutionsToPlan(plan, { 'a.txt': 'right' });
    expect(resolved.actions[0]).toEqual({
      kind: 'copyRightToLeft',
      path: 'a.txt',
    });
    expect(applyConflictChoice(plan.actions[0] as never, 'keepBoth').kind).toBe(
      'skip',
    );
    expect(allConflictsResolved(listConflictActions(plan), {})).toBe(false);
    expect(
      allConflictsResolved(listConflictActions(plan), { 'a.txt': 'left' }),
    ).toBe(true);
  });
});
