import { act, fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { getPreviewActions } from "../api/preview";
import { PreviewResults } from "../components/preview/PreviewResults";
import { PreviewScanProgress } from "../components/preview/PreviewScanProgress";
import type { FolderPair, PreviewActionPage, SyncPlan } from "../types";

vi.mock("../api/preview", () => ({ getPreviewActions: vi.fn() }));
const pair: FolderPair = {
  id: "pair-1",
  name: "Test",
  leftPath: "C:/left",
  rightPath: "C:/right",
  mode: "contribute",
  filters: { include: [], exclude: [] },
  conflictPolicy: "ask",
  enabled: true,
  watchEnabled: false,
  scheduleEnabled: false,
  createdAt: 0,
  updatedAt: 0,
};

const plan: SyncPlan = {
  pairId: "pair-1",
  scannedLeft: 100,
  scannedRight: 100,
  actions: Array.from({ length: 51 }, (_, index) => ({
    kind: "copyLeftToRight" as const,
    path: `file-${index}.txt`,
  })),
};

describe("PreviewResults", () => {
  it("keeps the current page on failure and allows retry", async () => {
    vi.mocked(getPreviewActions).mockRejectedValueOnce(new Error("offline"));
    render(
      <PreviewResults
        pair={pair}
        plan={{ ...plan, planId: "old", actionCount: 201 }}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("offline");
    expect(screen.getByText("Page 1 of 2")).toBeInTheDocument();
    expect(screen.getByText("file-0.txt")).toBeInTheDocument();
    vi.mocked(getPreviewActions).mockResolvedValueOnce({
      planId: "old",
      cursor: 200,
      actions: [{ kind: "deleteLeft", path: "next.txt" }],
    });
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(await screen.findByText("next.txt")).toBeInTheDocument();
    expect(screen.getByText("Page 2 of 2")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("ignores a pending page response after the preview changes", async () => {
    let resolve!: (page: PreviewActionPage) => void;
    vi.mocked(getPreviewActions).mockReturnValueOnce(
      new Promise((done) => {
        resolve = done;
      }),
    );
    const { rerender } = render(
      <PreviewResults
        pair={pair}
        plan={{ ...plan, planId: "old", actionCount: 201 }}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    rerender(
      <PreviewResults
        pair={pair}
        plan={{
          ...plan,
          planId: "new",
          actions: [{ kind: "copyLeftToRight", path: "fresh.txt" }],
        }}
      />,
    );
    await act(async () => {
      resolve({
        planId: "old",
        cursor: 200,
        actions: [{ kind: "deleteLeft", path: "stale.txt" }],
      });
    });
    expect(screen.getByText("fresh.txt")).toBeInTheDocument();
    expect(screen.queryByText("stale.txt")).not.toBeInTheDocument();
  });

  it("paginates large result sets", async () => {
    const { getByRole } = render(<PreviewResults plan={plan} />);
    expect(screen.getByText("Page 1 of 2")).toBeInTheDocument();
    expect(screen.getByText("file-0.txt")).toBeInTheDocument();
    expect(screen.queryByText("file-50.txt")).not.toBeInTheDocument();

    fireEvent.click(getByRole("button", { name: "Next" }));
    expect(screen.getByText("Page 2 of 2")).toBeInTheDocument();
    expect(screen.getByText("file-50.txt")).toBeInTheDocument();
  });
});

describe("PreviewScanProgress", () => {
  it("announces an active scan", () => {
    render(<PreviewScanProgress loading />);
    expect(screen.getByText(/scanning both folders/i)).toBeInTheDocument();
    expect(screen.getByText(/comparing files/i)).toBeInTheDocument();
    expect(screen.getByRole("region")).toHaveAttribute("aria-busy", "true");
  });
});
