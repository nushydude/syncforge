import { useSyncExternalStore } from 'react';
import {
  getRunState,
  subscribeRun,
  type RunStoreState,
} from '../store/runStore';

export function useSyncProgress(): RunStoreState {
  return useSyncExternalStore(subscribeRun, getRunState, getRunState);
}
