import { describe, expect, it } from 'vitest';
import {
  formatBytes,
  formatDuration,
  statsFromItems,
  statsFromReport,
} from '../lib/syncStats';
import type { RunItem, RunReport } from '../types';

const sampleReport: RunReport = {
  runId: 'run-1',
  pairId: 'pair-1',
  startedAt: 1_000,
  finishedAt: 4_500,
  status: 'completed',
  filesCopied: 3,
  filesDeleted: 1,
  bytesTransferred: 2048,
  errors: ['warning'],
};

describe('syncStats', () => {
  it('aggregates files, bytes, and duration from a report', () => {
    const stats = statsFromReport(sampleReport);
    expect(stats.filesCopied).toBe(3);
    expect(stats.filesDeleted).toBe(1);
    expect(stats.filesChanged).toBe(4);
    expect(stats.bytesTransferred).toBe(2048);
    expect(stats.durationMs).toBe(3500);
    expect(stats.errorCount).toBe(1);
  });

  it('sums item bytes when items are present', () => {
    const items: RunItem[] = [
      {
        id: '1',
        runId: 'run-1',
        path: 'a.txt',
        action: 'copyLeftToRight',
        status: 'completed',
        bytes: 100,
      },
      {
        id: '2',
        runId: 'run-1',
        path: 'b.txt',
        action: 'deleteRight',
        status: 'completed',
        bytes: 50,
      },
      {
        id: '3',
        runId: 'run-1',
        path: 'c.txt',
        action: 'copyLeftToRight',
        status: 'failed',
        bytes: 10,
      },
    ];

    const stats = statsFromItems(sampleReport, items);
    expect(stats.bytesTransferred).toBe(150);
    expect(stats.filesChanged).toBe(2);
  });

  it('formats duration and bytes for display', () => {
    expect(formatDuration(500)).toBe('500 ms');
    expect(formatDuration(45_000)).toBe('45s');
    expect(formatDuration(125_000)).toBe('2m 5s');
    expect(formatBytes(512)).toBe('512 B');
    expect(formatBytes(1536)).toBe('1.5 KiB');
  });
});
