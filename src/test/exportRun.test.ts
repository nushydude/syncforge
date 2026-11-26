import { invoke } from "@tauri-apps/api/core";
import { describe, expect, it, vi } from "vitest";
import { exportHistoryRun } from "../api/history";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

describe("History export", () => {
  it("exports the full run by ID, independently of the current page or filter", async () => {
    vi.mocked(invoke).mockResolvedValueOnce(true);
    await expect(exportHistoryRun("run-1", "json")).resolves.toBe(true);
    expect(invoke).toHaveBeenLastCalledWith("export_history_run", {
      runId: "run-1",
      format: "json",
    });
  });

  it("returns the native save dialog cancellation", async () => {
    vi.mocked(invoke).mockResolvedValueOnce(false);
    await expect(exportHistoryRun("run-1", "csv")).resolves.toBe(false);
    expect(invoke).toHaveBeenLastCalledWith("export_history_run", {
      runId: "run-1",
      format: "csv",
    });
  });
});
