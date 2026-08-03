import { invoke } from "@tauri-apps/api/core";
import type {
  FolderPair,
  PreviewActionPage,
  PreviewSummary,
  SyncAction,
} from "../types";

interface BackendPreviewSummary extends Omit<PreviewSummary, "actions"> {
  firstPage: SyncAction[];
}

export async function previewPair(pair: FolderPair): Promise<PreviewSummary> {
  const summary = await invoke<BackendPreviewSummary>("preview_pair", { pair });
  return { ...summary, actions: summary.firstPage };
}

export function getPreviewActions(
  pair: FolderPair,
  planId: string,
  cursor = 0,
  limit = 200,
): Promise<PreviewActionPage> {
  return invoke<PreviewActionPage>("get_preview_actions", {
    pair,
    planId,
    cursor,
    limit,
  });
}
