import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as pairsApi from "../api/pairs";
import * as runApi from "../api/run";
import { PairsPanel } from "../components/pairs/PairsPanel";
import { resetPairsStoreForTests } from "../store/pairsStore";
import { getRunState, resetRunStoreForTests } from "../store/runStore";
import {
  resetSettingsForTests,
  updateAppSettings,
} from "../store/settingsStore";
import type { FolderPair } from "../types";

vi.mock("../api/pairs", () => ({
  listPairs: vi.fn(),
  savePair: vi.fn(),
  deletePair: vi.fn(),
  setSchedule: vi.fn(),
  pickFolder: vi.fn(),
  pathExists: vi.fn().mockResolvedValue(true),
  pathsEqual: vi.fn().mockResolvedValue(false),
}));

vi.mock("../api/preview", () => ({
  previewPair: vi.fn(),
  getPreviewActions: vi.fn(),
  getPreviewConflicts: vi.fn(),
}));

vi.mock("../api/run", () => ({
  runPair: vi.fn(),
  cancelRun: vi.fn(),
}));

vi.mock("../api/history", () => ({
  listRuns: vi.fn().mockResolvedValue([]),
  getRunDetail: vi.fn(),
}));

const basePair: FolderPair = {
  id: "pair-1",
  name: "Docs",
  leftPath: "C:\\left",
  rightPath: "D:\\right",
  mode: "echo",
  filters: { include: [], exclude: [] },
  conflictPolicy: "newerWins",
  enabled: true,
  watchEnabled: false,
  scheduleEnabled: false,
  scheduleCron: null,
  createdAt: 1,
  updatedAt: 2,
};

const photosPair: FolderPair = { ...basePair, id: "pair-2", name: "Photos" };

describe("sync queue view", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    resetRunStoreForTests();
    resetPairsStoreForTests();
    resetSettingsForTests();
    updateAppSettings({ confirmBeforeRun: false });
    vi.mocked(pairsApi.listPairs).mockResolvedValue([basePair, photosPair]);
    vi.mocked(runApi.runPair).mockImplementation(() => new Promise(() => {}));
  });

  it("queues a second pair while the first one runs", async () => {
    render(<PairsPanel />);
    await screen.findByRole("button", { name: /Docs/ });

    fireEvent.click(screen.getByRole("button", { name: /^Docs/ }));
    fireEvent.click(await screen.findByRole("button", { name: "Run sync" }));

    // Selecting another pair mid-run is allowed, and queues independently.
    fireEvent.click(screen.getByRole("button", { name: /^Photos/ }));
    fireEvent.click(await screen.findByRole("button", { name: "Run sync" }));

    await waitFor(() => {
      expect(getRunState().queue.map((j) => j.pairId)).toEqual(["pair-2"]);
    });
    expect(getRunState().activePairId).toBe("pair-1");
    expect(runApi.runPair).toHaveBeenCalledTimes(1);
    expect(screen.getByLabelText("Work queue")).toBeInTheDocument();
    expect(screen.getByText("1 running · 1 waiting")).toBeInTheDocument();
  });

  it("queues a scan behind a running sync instead of scanning in parallel", async () => {
    const previewApi = await import("../api/preview");
    vi.mocked(previewApi.previewPair).mockImplementation(
      () => new Promise(() => {}),
    );

    render(<PairsPanel />);
    await screen.findByRole("button", { name: /^Docs/ });

    fireEvent.click(screen.getByRole("button", { name: /^Docs/ }));
    fireEvent.click(await screen.findByRole("button", { name: "Run sync" }));

    fireEvent.click(screen.getByRole("button", { name: /^Photos/ }));
    fireEvent.click(
      await screen.findByRole("button", { name: "Preview sync" }),
    );

    await waitFor(() => {
      expect(getRunState().queue).toHaveLength(1);
    });
    expect(getRunState().queue[0]).toMatchObject({
      pairId: "pair-2",
      kind: "preview",
    });
    // The sync owns the disk; the scan has not started.
    expect(previewApi.previewPair).not.toHaveBeenCalled();
    expect(
      await screen.findByText(/Waiting to scan .Photos./),
    ).toBeInTheDocument();
  });

  it("reports identical folders without a results button", async () => {
    const previewApi = await import("../api/preview");
    vi.mocked(previewApi.previewPair).mockResolvedValue({
      pairId: "pair-1",
      planId: "plan-1",
      scannedLeft: 12,
      scannedRight: 12,
      actions: [],
      actionCount: 0,
      scanWarnings: [],
    });

    render(<PairsPanel />);
    await screen.findByRole("button", { name: /^Docs/ });
    fireEvent.click(screen.getByRole("button", { name: /^Docs/ }));
    fireEvent.click(
      await screen.findByRole("button", { name: "Preview sync" }),
    );

    expect(await screen.findByText("Nothing to sync")).toBeInTheDocument();
    expect(
      screen.getByText("Both folders are already identical."),
    ).toBeInTheDocument();
    // Nothing to page through, so no dead-end click.
    expect(
      screen.queryByRole("button", { name: "View results" }),
    ).not.toBeInTheDocument();
  });

  it("still offers results when a clean scan produced warnings", async () => {
    const previewApi = await import("../api/preview");
    vi.mocked(previewApi.previewPair).mockResolvedValue({
      pairId: "pair-1",
      planId: "plan-1",
      scannedLeft: 12,
      scannedRight: 12,
      actions: [],
      actionCount: 0,
      scanWarnings: ["locked.txt: metadata unavailable"],
    });

    render(<PairsPanel />);
    await screen.findByRole("button", { name: /^Docs/ });
    fireEvent.click(screen.getByRole("button", { name: /^Docs/ }));
    fireEvent.click(
      await screen.findByRole("button", { name: "Preview sync" }),
    );

    expect(
      await screen.findByRole("button", { name: "View results" }),
    ).toBeInTheDocument();
  });

  it("shows run status above the pair details grid", async () => {
    const { container } = render(<PairsPanel />);
    await screen.findByRole("button", { name: /^Docs/ });
    fireEvent.click(screen.getByRole("button", { name: /^Docs/ }));
    fireEvent.click(await screen.findByRole("button", { name: "Run sync" }));

    const details = container.querySelector(".pair-details");
    const progress = container.querySelector(".run-progress");
    const grid = container.querySelector(".pair-details-grid");
    expect(details).toBeTruthy();
    expect(progress).toBeTruthy();
    expect(grid).toBeTruthy();
    // DOCUMENT_POSITION_FOLLOWING === 4: the grid comes after the progress card.
    expect(
      progress!.compareDocumentPosition(grid!) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
  });

  it("keeps each pair's run result on its own pair", async () => {
    vi.mocked(runApi.runPair).mockResolvedValue({
      runId: "run-1",
      pairId: "pair-1",
      startedAt: 1,
      finishedAt: 2,
      status: "completed",
      filesCopied: 3,
      filesDeleted: 0,
      bytesTransferred: 2048,
      errors: [],
    });

    render(<PairsPanel />);
    await screen.findByRole("button", { name: /^Docs/ });
    fireEvent.click(screen.getByRole("button", { name: /^Docs/ }));
    fireEvent.click(await screen.findByRole("button", { name: "Run sync" }));

    await screen.findByText(/Copied 3/);

    // Switching pairs must not carry the first pair's report over.
    fireEvent.click(screen.getByRole("button", { name: /^Photos/ }));
    expect(screen.queryByText(/Copied 3/)).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /^Docs/ }));
    expect(screen.getByText(/Copied 3/)).toBeInTheDocument();
  });
});
