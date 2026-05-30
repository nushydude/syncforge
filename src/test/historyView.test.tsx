import { render } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { HistoryView } from "../components/history/HistoryView";

const loadHistory = vi.fn().mockResolvedValue(undefined);
const loadPairs = vi.fn().mockResolvedValue(undefined);

vi.mock("../store/historyStore", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../store/historyStore")>();
  return {
    ...actual,
    loadHistory: (...args: Parameters<typeof loadHistory>) => loadHistory(...args),
  };
});

vi.mock("../store/pairsStore", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../store/pairsStore")>();
  return {
    ...actual,
    loadPairs: (...args: Parameters<typeof loadPairs>) => loadPairs(...args),
  };
});

describe("HistoryView", () => {
  beforeEach(() => {
    loadHistory.mockClear();
    loadPairs.mockClear();
  });

  it("does not load pairs or history while inactive", () => {
    render(<HistoryView active={false} />);
    expect(loadPairs).not.toHaveBeenCalled();
    expect(loadHistory).not.toHaveBeenCalled();
  });

  it("loads history once when the tab becomes active", () => {
    const { rerender } = render(<HistoryView active={false} />);
    expect(loadHistory).not.toHaveBeenCalled();

    rerender(<HistoryView active />);
    expect(loadHistory).toHaveBeenCalledTimes(1);
    expect(loadPairs).not.toHaveBeenCalled();

    rerender(<HistoryView active={false} />);
    rerender(<HistoryView active />);
    expect(loadHistory).toHaveBeenCalledTimes(1);
  });
});
