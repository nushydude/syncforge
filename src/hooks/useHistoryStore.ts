import { useSyncExternalStore } from 'react';
import {
  getHistoryState,
  subscribeHistory,
  type HistoryStoreState,
} from '../store/historyStore';

export function useHistoryStore(): HistoryStoreState {
  return useSyncExternalStore(
    subscribeHistory,
    getHistoryState,
    getHistoryState,
  );
}
