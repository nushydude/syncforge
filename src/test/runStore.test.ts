import { beforeEach, describe, expect, it, vi } from 'vitest';
import * as previewApi from '../api/preview';
import * as runApi from '../api/run';
import type { FolderPair, RunReport } from '../types';
import {
  confirmConflictResolutionAndRun,
  getRunState,
  resetRunStoreForTests,
  runSelectedPair,
  setConflictResolution,
} from '../store/runStore';

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock('../api/preview', () => ({
  previewPair: vi.fn(),
}));

vi.mock('../api/run', () => ({
  runPair: vi.fn(),
  cancelRun: vi.fn(),
}));

const samplePair: FolderPair = {
  id: 'pair-1',
  name: 'Docs',
  leftPath: 'C:\\left',
  rightPath: 'D:\\right',
  mode: 'echo',
  filters: { include: [], exclude: [] },
  conflictPolicy: 'newerWins',
  enabled: true,
  createdAt: 1,
  updatedAt: 2,
};

const sampleReport: RunReport = {
  runId: 'run-1',
  pairId: 'pair-1',
  startedAt: 1,
  finishedAt: 2,
  status: 'completed',
  filesCopied: 1,
  filesDeleted: 0,
  bytesTransferred: 100,
  errors: [],
};

describe('runStore', () => {
  beforeEach(() => {
    resetRunStoreForTests();
    vi.clearAllMocks();
  });

  it('runs a pair and stores the report', async () => {
    vi.mocked(runApi.runPair).mockResolvedValue(sampleReport);
    const report = await runSelectedPair(samplePair);
    expect(report).toEqual(sampleReport);
    expect(getRunState().running).toBe(false);
    expect(getRunState().lastReport).toEqual(sampleReport);
  });

  it('surfaces run errors', async () => {
    vi.mocked(runApi.runPair).mockRejectedValue(new Error('disk full'));
    const report = await runSelectedPair(samplePair);
    expect(report).toBeNull();
    expect(getRunState().error).toBe('disk full');
  });

  it('ignores run without pair id', async () => {
    const report = await runSelectedPair({ ...samplePair, id: '' });
    expect(report).toBeNull();
    expect(runApi.runPair).not.toHaveBeenCalled();
  });

  it('opens conflict dialog when ask policy finds conflicts', async () => {
    vi.mocked(previewApi.previewPair).mockResolvedValue({
      pairId: 'pair-1',
      scannedLeft: 1,
      scannedRight: 1,
      actions: [
        {
          kind: 'conflict',
          path: 'both.txt',
          left: {
            relativePath: 'both.txt',
            size: 1,
            modifiedSecs: 1,
            isDir: false,
          },
          right: {
            relativePath: 'both.txt',
            size: 2,
            modifiedSecs: 2,
            isDir: false,
          },
        },
      ],
    });

    const report = await runSelectedPair({
      ...samplePair,
      mode: 'synchronize',
      conflictPolicy: 'ask',
    });
    expect(report).toBeNull();
    expect(runApi.runPair).not.toHaveBeenCalled();
    expect(getRunState().pendingConflicts?.conflicts).toHaveLength(1);
  });

  it('runs with user conflict resolutions after confirm', async () => {
    vi.mocked(previewApi.previewPair).mockResolvedValue({
      pairId: 'pair-1',
      scannedLeft: 1,
      scannedRight: 1,
      actions: [
        {
          kind: 'conflict',
          path: 'both.txt',
          left: {
            relativePath: 'both.txt',
            size: 1,
            modifiedSecs: 1,
            isDir: false,
          },
          right: {
            relativePath: 'both.txt',
            size: 2,
            modifiedSecs: 2,
            isDir: false,
          },
        },
      ],
    });
    vi.mocked(runApi.runPair).mockResolvedValue(sampleReport);

    await runSelectedPair({
      ...samplePair,
      mode: 'synchronize',
      conflictPolicy: 'ask',
    });
    setConflictResolution('both.txt', 'left');
    const report = await confirmConflictResolutionAndRun();

    expect(report).toEqual(sampleReport);
    expect(runApi.runPair).toHaveBeenCalledWith(
      expect.objectContaining({ id: 'pair-1' }),
      expect.objectContaining({
        conflictResolutions: { 'both.txt': 'left' },
      }),
    );
  });
});
