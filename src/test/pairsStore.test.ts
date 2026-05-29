import { beforeEach, describe, expect, it, vi } from "vitest";
import * as pairsApi from "../api/pairs";
import * as previewApi from "../api/preview";
import type { FolderPair, SyncPlan } from "../types";
import {
  getPairsState,
  loadPairs,
  previewSelectedPair,
  resetPairsStoreForTests,
  saveEditing,
  selectPair,
  startNewPair,
  updateEditing,
} from "../store/pairsStore";

vi.mock("../api/pairs", () => ({
  listPairs: vi.fn(),
  savePair: vi.fn(),
  deletePair: vi.fn(),
  pickFolder: vi.fn(),
  pathExists: vi.fn(),
  pathsEqual: vi.fn(),
}));

vi.mock("../api/preview", () => ({
  previewPair: vi.fn(),
}));

const samplePair: FolderPair = {
  id: "pair-1",
  name: "Docs",
  leftPath: "C:\\left",
  rightPath: "D:\\right",
  mode: "echo",
  filters: { include: ["*.txt"], exclude: [] },
  conflictPolicy: "newerWins",
  enabled: true,
  watchEnabled: false,
  createdAt: 1,
  updatedAt: 2,
};

describe("pairsStore", () => {
  beforeEach(() => {
    resetPairsStoreForTests();
    vi.clearAllMocks();
  });

  it("loads pairs from the backend", async () => {
    vi.mocked(pairsApi.listPairs).mockResolvedValue([samplePair]);
    await loadPairs();
    expect(getPairsState().pairs).toEqual([samplePair]);
    expect(getPairsState().loading).toBe(false);
  });

  it("surfaces load errors", async () => {
    vi.mocked(pairsApi.listPairs).mockRejectedValue(new Error("db offline"));
    await loadPairs();
    expect(getPairsState().error).toBe("db offline");
  });

  it("starts a new empty draft", () => {
    startNewPair();
    const { editing } = getPairsState();
    expect(editing).not.toBeNull();
    expect(editing?.id).toBe("");
    expect(editing?.name).toBe("");
  });

  it("selects a pair for editing", async () => {
    vi.mocked(pairsApi.listPairs).mockResolvedValue([samplePair]);
    await loadPairs();
    selectPair("pair-1");
    expect(getPairsState().selectedId).toBe("pair-1");
    expect(getPairsState().editing?.name).toBe("Docs");
  });

  it("validates before save", async () => {
    startNewPair();
    updateEditing({
      name: "",
      leftPath: "C:\\missing",
      rightPath: "D:\\missing",
    });
    vi.mocked(pairsApi.pathExists).mockResolvedValue(false);
    vi.mocked(pairsApi.pathsEqual).mockResolvedValue(false);

    const ok = await saveEditing();
    expect(ok).toBe(false);
    expect(pairsApi.savePair).not.toHaveBeenCalled();
    expect(getPairsState().validationErrors.length).toBeGreaterThan(0);
  });

  it("saves a valid pair", async () => {
    startNewPair();
    updateEditing({
      name: "Backup",
      leftPath: "C:\\left",
      rightPath: "D:\\right",
    });
    vi.mocked(pairsApi.pathExists).mockResolvedValue(true);
    vi.mocked(pairsApi.pathsEqual).mockResolvedValue(false);
    vi.mocked(pairsApi.savePair).mockResolvedValue({
      ...samplePair,
      id: "new-id",
      name: "Backup",
    });

    const ok = await saveEditing();
    expect(ok).toBe(true);
    expect(pairsApi.savePair).toHaveBeenCalled();
    expect(getPairsState().pairs).toHaveLength(1);
    expect(getPairsState().selectedId).toBe("new-id");
  });

  it("loads preview for selected pair", async () => {
    const plan: SyncPlan = {
      pairId: "pair-1",
      scannedLeft: 1,
      scannedRight: 1,
      actions: [{ kind: "copyLeftToRight", path: "a.txt" }],
    };
    vi.mocked(pairsApi.listPairs).mockResolvedValue([samplePair]);
    vi.mocked(previewApi.previewPair).mockResolvedValue(plan);
    await loadPairs();
    selectPair("pair-1");
    updateEditing({ mode: "synchronize" });
    await previewSelectedPair();
    expect(previewApi.previewPair).toHaveBeenCalledWith({
      ...samplePair,
      mode: "synchronize",
    });
    expect(getPairsState().previewPlan).toEqual(plan);
    expect(getPairsState().previewLoading).toBe(false);
    expect(getPairsState().previewError).toBeNull();
  });

  it("surfaces preview errors", async () => {
    vi.mocked(pairsApi.listPairs).mockResolvedValue([samplePair]);
    vi.mocked(previewApi.previewPair).mockRejectedValue(new Error("scan left failed"));
    await loadPairs();
    selectPair("pair-1");
    await previewSelectedPair();
    expect(getPairsState().previewError).toBe("scan left failed");
    expect(getPairsState().previewPlan).toBeNull();
  });

  it("rejects identical paths", async () => {
    startNewPair();
    updateEditing({
      name: "Same",
      leftPath: "C:\\folder",
      rightPath: "c:\\folder",
    });
    vi.mocked(pairsApi.pathExists).mockResolvedValue(true);
    vi.mocked(pairsApi.pathsEqual).mockResolvedValue(true);

    const ok = await saveEditing();
    expect(ok).toBe(false);
    expect(getPairsState().validationErrors).toContain(
      "Left and right folders must be different",
    );
  });
});
