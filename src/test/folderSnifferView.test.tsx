import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { FolderSnifferView } from "../components/sniffer/FolderSnifferView";
import {
  getSnifferSummary,
  querySnifferEntries,
  refreshSnifferSubtree,
  startSnifferScan,
} from "../api/sniffer";
import type { SnifferScan } from "../types";

const eventHarness = vi.hoisted(() => ({
  handler: null as null | ((event: { payload: SnifferScan }) => void),
}));

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
  pinSnifferScan: vi.fn().mockResolvedValue(undefined),
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
  getSnifferNode: vi.fn(),
  querySnifferIssues: vi.fn(),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(
    (_name: string, handler: (event: { payload: SnifferScan }) => void) => {
      eventHarness.handler = handler;
      return Promise.resolve(() => undefined);
    },
  ),
}));
vi.mock("@tauri-apps/plugin-opener", () => ({
  openPath: vi.fn(),
  revealItemInDir: vi.fn(),
}));

describe("FolderSnifferView", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    eventHarness.handler = null;
    vi.mocked(getSnifferSummary).mockResolvedValue({
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
    });
  });

  it("buffers terminal progress that arrives before the start response", async () => {
    let resolveStart: (value: SnifferScan) => void = () => undefined;
    vi.mocked(startSnifferScan).mockReturnValue(
      new Promise((resolve) => {
        resolveStart = resolve;
      }),
    );
    vi.mocked(querySnifferEntries).mockResolvedValue({
      rows: [],
      nextCursor: null,
      matchCount: "0",
      matchedBytes: "0",
      directoryBytes: "0",
      revision: 3,
      coverageComplete: true,
      stale: false,
    });
    render(<FolderSnifferView />);
    await waitFor(() => expect(eventHarness.handler).not.toBeNull());
    fireEvent.click(screen.getByRole("button", { name: "Choose a folder" }));
    await waitFor(() => expect(startSnifferScan).toHaveBeenCalled());
    await act(async () => {
      eventHarness.handler?.({ payload: scan });
      resolveStart({ ...scan, status: "queued", revision: 1 });
    });
    expect(await screen.findByText("completed")).toBeInTheDocument();
  });

  it("retries until table and summary revisions match", async () => {
    vi.mocked(startSnifferScan).mockResolvedValue(scan);
    vi.mocked(querySnifferEntries)
      .mockResolvedValueOnce({
        rows: [],
        nextCursor: null,
        matchCount: "0",
        matchedBytes: "0",
        directoryBytes: "0",
        revision: 2,
        coverageComplete: true,
        stale: false,
      })
      .mockResolvedValue({
        rows: [],
        nextCursor: null,
        matchCount: "0",
        matchedBytes: "0",
        directoryBytes: "0",
        revision: 3,
        coverageComplete: true,
        stale: false,
      });
    vi.mocked(getSnifferSummary)
      .mockResolvedValueOnce({
        directory: {
          ...rows[0],
          nodeId: "1",
          parentId: null,
          kind: "directory",
        },
        logicalBytes: "0",
        files: "0",
        folders: "0",
        zeroSizeCount: "0",
        tiles: [],
        revision: 3,
        coverageComplete: true,
        stale: false,
      })
      .mockResolvedValue({
        directory: {
          ...rows[0],
          nodeId: "1",
          parentId: null,
          kind: "directory",
        },
        logicalBytes: "0",
        files: "0",
        folders: "0",
        zeroSizeCount: "0",
        tiles: [],
        revision: 3,
        coverageComplete: true,
        stale: false,
      });
    render(<FolderSnifferView />);
    fireEvent.click(screen.getByRole("button", { name: "Choose a folder" }));
    await screen.findByText(/0 matches/);
    expect(querySnifferEntries).toHaveBeenCalledTimes(2);
  });

  it("keeps the prior visible generation when refreshed results fail", async () => {
    vi.mocked(startSnifferScan).mockResolvedValue(scan);
    vi.mocked(refreshSnifferSubtree).mockResolvedValue({
      ...scan,
      id: "replacement",
      generationId: "replacement-generation",
      rootNodeId: "10",
      revision: 8,
    });
    vi.mocked(querySnifferEntries)
      .mockResolvedValueOnce({
        rows: [rows[0]],
        nextCursor: null,
        matchCount: "1",
        matchedBytes: "0",
        directoryBytes: "0",
        revision: 3,
        coverageComplete: true,
        stale: false,
      })
      .mockRejectedValueOnce({
        code: "failed",
        operation: "query",
        retryable: true,
        message: "replacement unavailable",
      })
      .mockResolvedValue({
        rows: [rows[0]],
        nextCursor: null,
        matchCount: "1",
        matchedBytes: "0",
        directoryBytes: "0",
        revision: 3,
        coverageComplete: true,
        stale: false,
      });

    render(<FolderSnifferView />);
    fireEvent.click(screen.getByRole("button", { name: "Choose a folder" }));
    await screen.findByText("file-0.txt");
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    expect(
      await screen.findByText("replacement unavailable"),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() =>
      expect(querySnifferEntries).toHaveBeenLastCalledWith(
        expect.objectContaining({
          scanId: "scan-1",
          generationId: "generation-1",
        }),
      ),
    );
  });

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

  it("commits navigation only after a cursor-free directory query succeeds", async () => {
    const folder = {
      ...rows[0],
      nodeId: "folder-1",
      name: "Nested",
      fullPath: "C:/files/Nested",
      relativePath: "Nested",
      kind: "directory" as const,
    };
    vi.mocked(startSnifferScan).mockResolvedValue(scan);
    vi.mocked(querySnifferEntries).mockResolvedValue({
      rows: [folder, ...rows.slice(1)],
      nextCursor: "next-page",
      matchCount: "250",
      matchedBytes: "0",
      directoryBytes: "0",
      revision: 3,
      coverageComplete: true,
      stale: false,
    });

    render(<FolderSnifferView />);
    fireEvent.click(screen.getByRole("button", { name: "Choose a folder" }));
    await screen.findByText("Nested");
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    await waitFor(() =>
      expect(querySnifferEntries).toHaveBeenCalledWith(
        expect.objectContaining({ cursor: "next-page" }),
      ),
    );

    fireEvent.doubleClick(screen.getByText("Nested"));
    await waitFor(() =>
      expect(querySnifferEntries).toHaveBeenCalledWith(
        expect.objectContaining({
          directoryId: "folder-1",
          cursor: undefined,
        }),
      ),
    );
    expect(screen.getByRole("button", { name: "Nested" })).toBeInTheDocument();
    expect(screen.getByText("Page 1")).toBeInTheDocument();
  });
});
