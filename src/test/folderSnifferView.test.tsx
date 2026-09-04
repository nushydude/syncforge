import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { FolderSnifferView } from "../components/sniffer/FolderSnifferView";
import { scanFolderSizes } from "../api/sniffer";

vi.mock("../api/pairs", () => ({
  pickFolder: vi.fn().mockResolvedValue("C:/files"),
}));
vi.mock("../api/sniffer", () => ({ scanFolderSizes: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({
  openPath: vi.fn(),
  revealItemInDir: vi.fn(),
}));

describe("FolderSnifferView", () => {
  it("bounds expanded file rows, includes zero-byte files, and shows skipped items", async () => {
    vi.mocked(scanFolderSizes).mockResolvedValue({
      path: "C:/files",
      size: 0,
      files: 250,
      folders: 0,
      skipped: 2,
      entries: Array.from({ length: 250 }, (_, index) => ({
        name: `file-${index}.txt`,
        path: `C:/files/file-${index}.txt`,
        size: 0,
        isFolder: false,
        childCount: 0,
      })),
    });
    const { container } = render(<FolderSnifferView />);
    fireEvent.click(screen.getByRole("button", { name: "Choose a folder" }));
    expect(await screen.findByText(/2 items were skipped/)).toBeInTheDocument();
    expect(
      container.querySelectorAll(".form-warning, .sniffer-warning"),
    ).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: /Other files/ }));
    expect(container.querySelectorAll(".sniffer-file-row")).toHaveLength(100);
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(screen.getByText("file-100.txt")).toBeInTheDocument();
    expect(screen.queryByText("file-0.txt")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(container.querySelectorAll(".sniffer-file-row")).toHaveLength(50);
    expect(screen.getByRole("button", { name: "Next" })).toBeDisabled();
  });
});
