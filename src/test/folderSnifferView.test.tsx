import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { FolderSnifferView } from "../components/sniffer/FolderSnifferView";
import { querySnifferEntries, startSnifferScan } from "../api/sniffer";

const scan = {
  id: "scan-1",
  generationId: "generation-1",
  root: "C:/files",
  rootNodeId: "1",
  status: "completed" as const,
  revision: 3,
  filesVisited: "250",
  foldersVisited: "1",
  logicalBytes: "0",
  issueCount: "2",
  coverageComplete: false,
  stale: false,
  currentDirectory: null,
  startedAt: Date.now() - 500,
  finishedAt: Date.now(),
  error: null,
};

const rows = Array.from({ length: 100 }, (_, index) => ({
  nodeId: String(index + 2),
  parentId: "1",
  name: `file-${index}.txt`,
  fullPath: `C:/files/file-${index}.txt`,
  relativePath: `file-${index}.txt`,
  kind: "file" as const,
  logicalSize: "0",
  files: "1",
  folders: "0",
  modifiedAt: null,
  status: "complete",
}));

vi.mock("../api/pairs", () => ({
  pickFolder: vi.fn().mockResolvedValue("C:/files"),
}));
vi.mock("../api/sniffer", () => ({
  getSnifferScan: vi.fn().mockResolvedValue(null),
  startSnifferScan: vi.fn(),
  cancelSnifferScan: vi.fn(),
  querySnifferEntries: vi.fn(),
  getSnifferSummary: vi.fn().mockResolvedValue({
    directory: {
      nodeId: "1",
      parentId: null,
      name: "files",
      fullPath: "C:/files",
      relativePath: "",
      kind: "directory",
      logicalSize: "0",
      files: "250",
      folders: "0",
      modifiedAt: null,
      status: "unreadable",
    },
    logicalBytes: "0",
    files: "250",
    folders: "0",
    zeroSizeCount: "250",
    tiles: [],
    revision: 3,
    coverageComplete: false,
    stale: false,
  }),
  refreshSnifferSubtree: vi.fn(),
  prepareSnifferAction: vi.fn(),
  executeSnifferAction: vi.fn(),
  showSnifferItemProperties: vi.fn(),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => undefined),
}));
vi.mock("@tauri-apps/plugin-opener", () => ({
  openPath: vi.fn(),
  revealItemInDir: vi.fn(),
}));

describe("FolderSnifferView", () => {
  it("queries bounded indexed pages and labels incomplete zero-size results", async () => {
    vi.mocked(startSnifferScan).mockResolvedValue(scan);
    vi.mocked(querySnifferEntries).mockResolvedValue({
      rows,
      nextCursor: "next-page",
      matchCount: "250",
      matchedBytes: "0",
      directoryBytes: "0",
      revision: 3,
      coverageComplete: false,
      stale: false,
    });
    const { container } = render(<FolderSnifferView />);
    fireEvent.click(screen.getByRole("button", { name: "Choose a folder" }));
    expect(await screen.findByText(/Partial/)).toBeInTheDocument();
    expect(
      screen.getAllByText("250", { selector: ".sniffer-summary strong" }),
    ).toHaveLength(2);
    expect(container.querySelectorAll("tbody tr")).toHaveLength(100);
    expect(screen.getByText(/250 matches/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    await waitFor(() =>
      expect(querySnifferEntries).toHaveBeenLastCalledWith(
        expect.objectContaining({ cursor: "next-page", limit: 100 }),
      ),
    );
  });
});
