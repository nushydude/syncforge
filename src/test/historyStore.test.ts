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

  it('ignores stale loadHistory results', async () => {
    let resolveFirst!: (runs: RunReport[]) => void;
    let resolveSecond!: (runs: RunReport[]) => void;
    let call = 0;
    vi.mocked(historyApi.getHistory).mockImplementation(() => {
      call += 1;
      if (call === 1) {
        return new Promise((resolve) => {
          resolveFirst = resolve;
        });
      }
      return new Promise((resolve) => {
        resolveSecond = resolve;
      });
    });

    const first = loadHistory();
    const second = loadHistory('pair-1');

    resolveFirst([runB]);
    await first;
    expect(getHistoryState().loading).toBe(true);

    resolveSecond([runA]);
    await second;
    expect(getHistoryState().runs).toEqual([runA]);
    expect(getHistoryState().pairFilter).toBe('pair-1');
  });

  it('ignores stale selectRun detail', async () => {
    const detailB: RunDetail = { report: runB, items: [] };
    let resolveFirst!: (detail: RunDetail) => void;
    let resolveSecond!: (detail: RunDetail) => void;
    let call = 0;
    vi.mocked(historyApi.getRunDetail).mockImplementation(() => {
      call += 1;
      if (call === 1) {
        return new Promise((resolve) => {
          resolveFirst = resolve;
        });
      }
      return new Promise((resolve) => {
        resolveSecond = resolve;
      });
    });

    const first = selectRun('run-a');
    const second = selectRun('run-b');

    resolveFirst(detail);
    await first;
    expect(getHistoryState().detail).toBeNull();
    expect(getHistoryState().selectedRunId).toBe('run-b');

    resolveSecond(detailB);
    await second;
    expect(getHistoryState().detail).toEqual(detailB);
  });

  it('loads run detail on selection', async () => {
    vi.mocked(historyApi.getRunDetail).mockResolvedValue(detail);
    await selectRun('run-a');
    expect(getHistoryState().detail).toEqual(detail);
    expect(getHistoryState().selectedRunId).toBe('run-a');
  });
});
