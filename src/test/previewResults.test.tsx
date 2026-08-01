import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { PreviewResults } from "../components/preview/PreviewResults";
import { PreviewScanProgress } from "../components/preview/PreviewScanProgress";
import type { SyncPlan } from "../types";

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
