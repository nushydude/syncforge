import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { DuplicatesView } from "../components/duplicates/DuplicatesView";
import { getDuplicateScan, startDuplicateScan } from "../api/duplicates";

vi.mock("../api/duplicates", () => ({
  getDuplicateScan: vi.fn(),
  startDuplicateScan: vi.fn(),
  resumeDuplicateScan: vi.fn(),
  cancelDuplicateScan: vi.fn(),
  removeDuplicates: vi.fn(),
}));

vi.mock("../api/pairs", () => ({
  pickFolder: vi.fn(),
}));

describe("DuplicatesView", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getDuplicateScan).mockResolvedValue(null);
  });

  it("scans a folder and renders duplicate groups", async () => {
    vi.mocked(startDuplicateScan).mockResolvedValue({
      id: "scan-1",
      root: "C:/Documents",
      mode: "hash",
      status: "completed",
      phase: null,
      filesFound: 3,
      totalFiles: 3,
      hashedFiles: 2,
      hashTotal: 24,
      bytesProcessed: 24,
      bytesTotal: 24,
      currentPath: null,
      cancelRequested: false,
      error: null,
      startedAt: 1,
      updatedAt: 2,
      result: {
        root: "C:/Documents",
        mode: "hash",
        scannedFiles: 3,
        hashedFiles: 2,
        skippedFiles: 0,
        potentialSavingsBytes: 12,
        warnings: [],
        groups: [
          {
            key: "abc1234567890abc",
            potentialSavingsBytes: 12,
            files: [
              { relativePath: "one.txt", name: "one.txt", size: 12 },
              {
                relativePath: "archive/one.txt",
                name: "one.txt",
                size: 12,
              },
            ],
          },
        ],
      },
    });

    render(<DuplicatesView />);
    await waitFor(() => {
      expect(getDuplicateScan).toHaveBeenCalled();
    });
    fireEvent.change(screen.getByLabelText(/folder to scan/i), {
      target: { value: "C:/Documents" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Scan folder" }));

    await waitFor(() => {
      expect(screen.getByText("archive/one.txt")).toBeInTheDocument();
    });
    expect(screen.getByText("duplicate groups")).toBeInTheDocument();
    expect(startDuplicateScan).toHaveBeenCalledWith("C:/Documents", "hash");
  });

  it("offers recovery for an interrupted scan", async () => {
    vi.mocked(getDuplicateScan).mockResolvedValue({
      id: "scan-1",
      root: "C:/Documents",
      mode: "hash",
      status: "interrupted",
      phase: "hashing",
      filesFound: 120,
      totalFiles: 240,
      hashedFiles: 80,
      hashTotal: 1024,
      bytesProcessed: 512,
      bytesTotal: 1024,
      currentPath: "nested/current.bin",
      cancelRequested: false,
      error: "The previous scan was interrupted. Resume it to continue.",
      startedAt: 1,
      updatedAt: 2,
      result: null,
    });

    render(<DuplicatesView />);

    await waitFor(() => {
      expect(
        screen.getByRole("heading", { name: "Previous scan interrupted" }),
      ).toBeInTheDocument();
    });
    expect(
      screen.getByRole("button", { name: "Resume scan" }),
    ).toBeInTheDocument();
    expect(
      screen.getByText(/The previous scan was interrupted/),
    ).toBeInTheDocument();
  });
});
