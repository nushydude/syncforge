import { invoke } from '@tauri-apps/api/core';
import type { FolderPair, RunPairOptions, RunReport } from '../types';

export function runPair(
  pair: FolderPair,
  options: RunPairOptions = {},
): Promise<RunReport> {
  return invoke<RunReport>('run_pair', { pair, options });
}

export function cancelRun(): Promise<void> {
  return invoke<void>('cancel_run');
}
