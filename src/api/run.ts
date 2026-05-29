import { invoke } from '@tauri-apps/api/core';
import type { FolderPair, RunPairOptions, RunReport } from '../types';

export function runPair(
  pair: FolderPair,
  options: RunPairOptions = {},
): Promise<RunReport> {
  return invoke<RunReport>('run_pair', { pair, options });
}

/** Cancel a sync for one pair, or all active runs when `pairId` is omitted. */
export function cancelRun(pairId?: string): Promise<void> {
  return invoke<void>('cancel_run', { pairId: pairId ?? null });
}
