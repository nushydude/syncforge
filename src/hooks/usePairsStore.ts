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

export type { PairsStoreState };
