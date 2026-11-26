import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { listen } from "@tauri-apps/api/event";
import { DuplicatesView } from "../components/duplicates/DuplicatesView";
import {
  getDuplicateScan,
  getDuplicateGroupsPage,
  getDuplicateMembersPage,
  revealDuplicate,
} from "../api/duplicates";
import type { DuplicateScanJob } from "../types";

vi.mock("../api/duplicates", () => ({
  getDuplicateScan: vi.fn(),
  startDuplicateScan: vi.fn(),
  resumeDuplicateScan: vi.fn(),
  cancelDuplicateScan: vi.fn(),
  revealDuplicate: vi.fn(),
  getDuplicateGroupsPage: vi.fn(),
  getDuplicateMembersPage: vi.fn(),
}));
vi.mock("../api/pairs", () => ({ pickFolder: vi.fn() }));

function job(sequence: number, revision = "r1"): DuplicateScanJob {
  return {
    id: "j1",
    attempt: 1,
    sequence,
    root: "C:/fixtures",
    mode: "hash",
    status: "completed",
    phase: null,
    filesFound: 2,
    totalFiles: 2,
    hashedFiles: 2,
    hashTotal: 8,
    bytesProcessed: 8,
    bytesTotal: 8,
    currentPath: null,
    cancelRequested: false,
    error: null,
    startedAt: 1,
    updatedAt: 2,
    summary: {
      revision,
      cleanupAvailable: true,
      root: "C:/fixtures",
      mode: "hash",
      scannedFiles: 2,
      hashedFiles: 2,
      skippedFiles: 0,
      potentialSavingsBytes: 4,
      groupCount: 1,
      warnings: [],
    },
  };
}

describe("duplicate result paging", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(listen).mockResolvedValue(() => {});
    vi.mocked(getDuplicateScan).mockResolvedValue(job(2));
    vi.mocked(getDuplicateGroupsPage).mockResolvedValue({
      revision: "r1",
      groups: [
        {
          id: "g1",
          key: "abc",
          fileCount: 2,
          potentialSavingsBytes: 4,
          firstSize: 4,
        },
      ],
      nextCursor: null,
    });
    vi.mocked(getDuplicateMembersPage).mockResolvedValue({
      revision: "r1",
      groupId: "g1",
      files: [
        { id: "a", fingerprint: "v1", name: "a", relativePath: "a", size: 4 },
        { id: "b", fingerprint: "v2", name: "b", relativePath: "b", size: 4 },
      ],
      nextCursor: 50,
    });
  });
  it("loads bounded pages and clears selection when switching pages", async () => {
    render(<DuplicatesView />);
    const box = await screen.findByRole("radio", { name: /^b\s/i });
    fireEvent.click(box);
    expect(box).toBeChecked();
    fireEvent.click(screen.getByRole("button", { name: "Next files" }));
    await waitFor(() =>
      expect(getDuplicateMembersPage).toHaveBeenCalledWith(
        "j1",
        "r1",
        "g1",
        50,
      ),
    );
    expect(
      screen.queryByRole("button", { name: /open in explorer/i }),
    ).not.toBeInTheDocument();
  });
  it("ignores a saved snapshot that arrives after a newer event", async () => {
    let resolve!: (value: DuplicateScanJob) => void;
    vi.mocked(getDuplicateScan).mockReturnValue(
      new Promise((r) => {
        resolve = r;
      }),
    );
    render(<DuplicatesView />);
    const calls = vi.mocked(listen).mock.calls;
    const callback = calls[calls.length - 1][1];
    act(() => callback({ event: "progress", id: 1, payload: job(3) }));
    await screen.findByText("duplicate groups");
    act(() => resolve({ ...job(2), status: "running", summary: null }));
    await waitFor(() =>
      expect(
        screen.queryByRole("heading", { name: "Finding files" }),
      ).not.toBeInTheDocument(),
    );
  });
  it("sends exactly one reviewed ID to Explorer and describes the request accurately", async () => {
    vi.mocked(revealDuplicate).mockResolvedValue();
    render(<DuplicatesView />);
    fireEvent.click(await screen.findByRole("radio", { name: /^a\s/i }));
    fireEvent.click(screen.getByRole("radio", { name: /^b\s/i }));
    fireEvent.click(screen.getByRole("button", { name: "Open in Explorer" }));
    await waitFor(() =>
      expect(revealDuplicate).toHaveBeenCalledWith("j1", "r1", "b"),
    );
    expect(
      await screen.findByText(
        "Explorer request sent. Find and review the file there before removing it.",
      ),
    ).toBeInTheDocument();
  });
  it("shows opener failure without claiming the request succeeded", async () => {
    vi.mocked(revealDuplicate).mockRejectedValue(
      "Could not send the Explorer request: unavailable",
    );
    render(<DuplicatesView />);
    fireEvent.click(await screen.findByRole("radio", { name: /^b\s/i }));
    fireEvent.click(screen.getByRole("button", { name: "Open in Explorer" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Could not send the Explorer request",
    );
    expect(
      screen.queryByText(/Explorer request sent/i),
    ).not.toBeInTheDocument();
  });
  it("refreshes a running snapshot if progress subscription fails", async () => {
    vi.mocked(listen).mockRejectedValue(new Error("listener unavailable"));
    vi.mocked(getDuplicateScan)
      .mockResolvedValueOnce({ ...job(1), status: "running", summary: null })
      .mockResolvedValue(job(2));
    render(<DuplicatesView />);
    expect(await screen.findByText("duplicate groups")).toBeInTheDocument();
    expect(getDuplicateScan).toHaveBeenCalledTimes(2);
  });
  it("shows an attempt-scoped persistence failure without overriding a durable terminal event", async () => {
    const running = { ...job(1), status: "running" as const, summary: null };
    vi.mocked(getDuplicateScan).mockResolvedValue(running);
    render(<DuplicatesView />);
    await screen.findByRole("heading", { name: "Preparing scan" });
    const calls = vi.mocked(listen).mock.calls;
    const failure = calls.find(
      ([name]) => name === "syncforge://duplicates-persistence-error",
    )![1];
    const progress = calls.find(
      ([name]) => name === "syncforge://duplicates-progress",
    )![1];
    act(() =>
      failure({
        event: "failure",
        id: 1,
        payload: { jobId: "j1", attempt: 2, message: "wrong attempt" },
      }),
    );
    expect(screen.queryByText("wrong attempt")).not.toBeInTheDocument();
    act(() =>
      failure({
        event: "failure",
        id: 1,
        payload: {
          jobId: "j1",
          attempt: 1,
          message:
            "Scan stopped before its results could be saved. Scan again.",
        },
      }),
    );
    expect(
      await screen.findByText(/Scan stopped before its results could be saved/),
    ).toBeInTheDocument();
    act(() => progress({ event: "progress", id: 2, payload: job(3) }));
    await screen.findByText("duplicate groups");
    act(() =>
      failure({
        event: "failure",
        id: 3,
        payload: { jobId: "j1", attempt: 1, message: "late failure" },
      }),
    );
    expect(screen.queryByText("late failure")).not.toBeInTheDocument();
  });
});
