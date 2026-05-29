import { describe, expect, it } from 'vitest';
import { runToCsv, runToJson } from '../lib/exportRun';
import type { RunDetail } from '../types';

const detail: RunDetail = {
  report: {
    runId: 'run-1',
    pairId: 'pair-1',
    startedAt: 100,
    finishedAt: 200,
    status: 'completed',
    filesCopied: 1,
    filesDeleted: 0,
    bytesTransferred: 42,
    errors: [],
  },
  items: [
    {
      id: 'item-1',
      runId: 'run-1',
      path: 'notes, draft.txt',
      action: 'copyLeftToRight',
      status: 'completed',
      bytes: 42,
      message: 'ok',
    },
  ],
};

describe('exportRun', () => {
  it('serializes run detail to JSON', () => {
    const json = runToJson(detail);
    const parsed = JSON.parse(json) as RunDetail;
    expect(parsed.report.runId).toBe('run-1');
    expect(parsed.items).toHaveLength(1);
  });

  it('serializes run detail to CSV with escaped fields', () => {
    const csv = runToCsv(detail);
    expect(csv).toContain('section,key,value');
    expect(csv).toContain('"notes, draft.txt"');
    expect(csv).toContain('item,"notes, draft.txt"');
  });
});
