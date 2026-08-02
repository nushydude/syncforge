import { invoke } from "@tauri-apps/api/core";
import type { RunDetail, RunReport } from "../types";

export function getHistory(pairId?: string | null): Promise<RunReport[]> {
  return invoke<RunReport[]>("get_history", { pairId: pairId ?? null });
}

export function getRunDetail(runId: string): Promise<RunDetail | null> {
  return invoke<RunDetail | null>("get_run_detail", { runId });
}
