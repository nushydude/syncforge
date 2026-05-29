import { beforeEach, describe, expect, it, vi } from 'vitest';
import * as historyApi from '../api/history';
import type { RunDetail, RunReport } from '../types';
import {
  getHistoryState,
  loadHistory,
  resetHistoryStoreForTests,
  selectRun,
  setPairFilter,
} from '../store/historyStore';

vi.mock('../api/history', () => ({
  getHistory: vi.fn(),
  getRunDetail: vi.fn(),
}));

const runA: RunReport = {
  runId: 'run-a',
  pairId: 'pair-1',
  startedAt: 200,
  finishedAt: 300,
  status: 'completed',
  filesCopied: 1,
  filesDeleted: 0,
  bytesTransferred: 10,
  errors: [],
};

const runB: RunReport = {
  runId: 'run-b',
  pairId: 'pair-2',
  startedAt: 100,
  finishedAt: 150,
  status: 'failed',
  filesCopied: 0,
  filesDeleted: 0,
  bytesTransferred: 0,
  errors: ['disk full'],
};

const detail: RunDetail = {
  report: runA,
  items: [
    {
      id: 'item-1',
      runId: 'run-a',
      path: 'file.txt',
      action: 'copyLeftToRight',
      status: 'completed',
      bytes: 10,
    },
  ],
};

describe('historyStore', () => {
  beforeEach(() => {
    resetHistoryStoreForTests();
    vi.clearAllMocks();
  });

  it('loads runs sorted by the backend', async () => {
    vi.mocked(historyApi.getHistory).mockResolvedValue([runA, runB]);
    await loadHistory();
    expect(getHistoryState().runs).toEqual([runA, runB]);
    expect(historyApi.getHistory).toHaveBeenCalledWith(null);
  });

  it('filters by pair id', async () => {
    vi.mocked(historyApi.getHistory).mockResolvedValue([runA]);
    await setPairFilter('pair-1');
    expect(getHistoryState().pairFilter).toBe('pair-1');
    expect(historyApi.getHistory).toHaveBeenCalledWith('pair-1');
  });

  it('loads run detail on selection', async () => {
    vi.mocked(historyApi.getRunDetail).mockResolvedValue(detail);
    await selectRun('run-a');
    expect(getHistoryState().detail).toEqual(detail);
    expect(getHistoryState().selectedRunId).toBe('run-a');
  });
});
