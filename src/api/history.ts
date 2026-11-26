import { invoke } from "@tauri-apps/api/core";
import type { RunActionFilter, RunDetail, RunReport } from "../types";

export function getHistory(pairId?: string | null): Promise<RunReport[]> {
  return invoke<RunReport[]>("get_history", { pairId: pairId ?? null });
}

export function getRunDetail(
  runId: string,
  cursor = 0,
  limit = 200,
  actionFilter: RunActionFilter | null = null,
): Promise<RunDetail | null> {
  return invoke<RunDetail | null>("get_run_detail", {
    runId,
    cursor,
    limit,
    actionFilter,
  });
}

export function exportHistoryRun(
  runId: string,
  format: "json" | "csv",
): Promise<boolean> {
  return invoke<boolean>("export_history_run", { runId, format });
}
