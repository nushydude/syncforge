import { createStoreHook } from "./createStoreHook";
import {
  getRunState,
  subscribeRun,
  type RunStoreState,
} from "../store/runStore";

export const useRunStore = createStoreHook<RunStoreState>(
  subscribeRun,
  getRunState,
);

export type { RunStoreState };
