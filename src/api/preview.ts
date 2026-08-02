import { invoke } from "@tauri-apps/api/core";
import type { FolderPair, SyncPlan } from "../types";

export function previewPair(pair: FolderPair): Promise<SyncPlan> {
  return invoke<SyncPlan>("preview_pair", { pair });
}
