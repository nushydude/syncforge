import { useRef, useSyncExternalStore } from "react";
import { shallowEqual } from "../lib/shallowEqual";

type Listener = () => void;

export function createStoreHook<TState>(
  subscribe: (listener: Listener) => () => void,
  getState: () => TState,
) {
  function useStore(): TState;
  function useStore<TSelected>(
    selector: (state: TState) => TSelected,
  ): TSelected;
  function useStore<TSelected>(
    selector?: (state: TState) => TSelected,
  ): TState | TSelected {
    const selectorRef = useRef(selector);
    selectorRef.current = selector;

    const cacheRef = useRef<{ snapshot: TSelected | TState } | null>(null);

    const getSnapshot = (): TState | TSelected => {
      const state = getState();
      const sel = selectorRef.current;
      if (!sel) {
        return state;
      }
      const next = sel(state);
      const cached = cacheRef.current;
      if (cached && shallowEqual(cached.snapshot, next)) {
        return cached.snapshot;
      }
      cacheRef.current = { snapshot: next };
      return next;
    };

    return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
  }

  return useStore;
}
