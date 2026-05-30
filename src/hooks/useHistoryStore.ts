import { createStoreHook } from "./createStoreHook";
import {
  getHistoryState,
  subscribeHistory,
  type HistoryStoreState,
} from "../store/historyStore";

export const useHistoryStore = createStoreHook<HistoryStoreState>(
  subscribeHistory,
  getHistoryState,
);

export type { HistoryStoreState };
