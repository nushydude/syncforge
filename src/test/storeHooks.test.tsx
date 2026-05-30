import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { createStoreHook } from "../hooks/createStoreHook";
import { selectPairsList, usePairsStore } from "../hooks/usePairsStore";
import { startNewPair, updateEditing } from "../store/pairsStore";
import { resetPairsStoreForTests } from "../store/pairsStore";

type MockState = { count: number; label: string };

function createMockStore() {
  let state: MockState = { count: 0, label: "a" };
  const listeners = new Set<() => void>();
  return {
    getState: () => state,
    subscribe: (listener: () => void) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    setState: (patch: Partial<MockState>) => {
      state = { ...state, ...patch };
      listeners.forEach((l) => l());
    },
  };
}

describe("createStoreHook", () => {
  it("re-renders only when the selected slice changes", () => {
    const store = createMockStore();
    const useMockStore = createStoreHook(store.subscribe, store.getState);

    let countRenders = 0;
    let labelRenders = 0;

    renderHook(() => {
      countRenders++;
      return useMockStore((s) => s.count);
    });
    renderHook(() => {
      labelRenders++;
      return useMockStore((s) => s.label);
    });

    expect(countRenders).toBe(1);
    expect(labelRenders).toBe(1);

    act(() => store.setState({ label: "b" }));
    expect(countRenders).toBe(1);
    expect(labelRenders).toBe(2);

    act(() => store.setState({ count: 1 }));
    expect(countRenders).toBe(2);
    expect(labelRenders).toBe(2);
  });
});

describe("usePairsStore selectors", () => {
  beforeEach(() => {
    resetPairsStoreForTests();
  });

  it("skips re-render when only editing changes and pairs is selected", () => {
    let pairsRenders = 0;
    let editingRenders = 0;

    renderHook(() => {
      pairsRenders++;
      return usePairsStore((s) => s.pairs);
    });
    renderHook(() => {
      editingRenders++;
      return usePairsStore((s) => s.editing);
    });

    expect(pairsRenders).toBe(1);
    expect(editingRenders).toBe(1);

    act(() => startNewPair());
    expect(pairsRenders).toBe(1);
    expect(editingRenders).toBe(2);

    act(() => updateEditing({ name: "Docs" }));
    expect(pairsRenders).toBe(1);
    expect(editingRenders).toBe(3);
  });

  it("skips re-render when only editing changes and selectPairsList is used (HistoryView pattern)", () => {
    let pairsListRenders = 0;

    renderHook(() => {
      pairsListRenders++;
      return usePairsStore(selectPairsList);
    });

    expect(pairsListRenders).toBe(1);

    act(() => startNewPair());
    expect(pairsListRenders).toBe(1);

    act(() => updateEditing({ name: "Docs" }));
    expect(pairsListRenders).toBe(1);
  });
});
