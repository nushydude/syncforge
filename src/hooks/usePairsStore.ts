import { useSyncExternalStore } from "react";
import {
  getPairsState,
  subscribePairs,
  type PairsStoreState,
} from "../store/pairsStore";

export function usePairsStore(): PairsStoreState {
  return useSyncExternalStore(subscribePairs, getPairsState, getPairsState);
}
