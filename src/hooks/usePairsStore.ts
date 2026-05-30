import { createStoreHook } from "./createStoreHook";
import {
  getPairsState,
  subscribePairs,
  type PairsStoreState,
} from "../store/pairsStore";

export const usePairsStore = createStoreHook<PairsStoreState>(
  subscribePairs,
  getPairsState,
);

/** Stable when only editing/preview/etc. change; derive id+name via useMemo. */
export const selectPairsList = (s: PairsStoreState) => s.pairs;

export type { PairsStoreState };
