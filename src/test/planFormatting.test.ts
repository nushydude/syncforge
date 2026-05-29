import { describe, expect, it } from 'vitest';
import {
  actionLabel,
  formatAction,
  formatPlanSummary,
} from '../lib/planFormatting';
import type { SyncAction, SyncPlan } from '../types';

describe('planFormatting', () => {
  it('labels copy and delete actions', () => {
    expect(
      actionLabel({ kind: 'copyLeftToRight', path: 'a.txt' }),
    ).toBe('Copy → right');
    expect(actionLabel({ kind: 'deleteRight', path: 'b.txt' })).toBe(
      'Delete on right',
    );
  });

  it('formats conflict detail', () => {
    const action: SyncAction = {
      kind: 'conflict',
      path: 'both.txt',
      left: {
        relativePath: 'both.txt',
        size: 10,
        modifiedSecs: 1,
        isDir: false,
      },
      right: {
        relativePath: 'both.txt',
        size: 20,
        modifiedSecs: 2,
        isDir: false,
      },
    };
    const row = formatAction(action);
    expect(row.label).toBe('Conflict');
    expect(row.detail).toContain('L: 10 B');
    expect(row.tone).toBe('conflict');
  });

  it('summarizes plan counts', () => {
    const plan: SyncPlan = {
      pairId: 'p1',
      scannedLeft: 5,
      scannedRight: 6,
      actions: [
        { kind: 'copyLeftToRight', path: 'a.txt' },
        {
          kind: 'conflict',
          path: 'b.txt',
          left: {
            relativePath: 'b.txt',
            size: 1,
            modifiedSecs: 1,
            isDir: false,
          },
          right: {
            relativePath: 'b.txt',
            size: 2,
            modifiedSecs: 2,
            isDir: false,
          },
        },
      ],
    };
    expect(formatPlanSummary(plan)).toContain('2 actions');
    expect(formatPlanSummary(plan)).toContain('scanned L 5');
    expect(formatPlanSummary(plan)).toContain('1 conflict');
  });

  it('includes scan warning count in summary', () => {
    const plan: SyncPlan = {
      pairId: 'p1',
      scannedLeft: 1,
      scannedRight: 1,
      actions: [],
      scanWarnings: ['left: skipped 2 path(s) (permission denied or unreadable)'],
    };
    expect(formatPlanSummary(plan)).toContain('1 scan warning');
  });
});
