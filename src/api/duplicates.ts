import { invoke } from "@tauri-apps/api/core";
import type {
  DuplicateCleanupResult,
  DuplicateMatchMode,
  DuplicateScanJob,
  DuplicateScanResult,
} from "../types";

export function startDuplicateScan(
  root: string,
  mode: DuplicateMatchMode,
): Promise<DuplicateScanJob> {
  return invoke<DuplicateScanJob>("start_duplicate_scan", { root, mode });
}

export function getDuplicateScan(): Promise<DuplicateScanJob | null> {
  return invoke<DuplicateScanJob | null>("get_duplicate_scan");
}

export function resumeDuplicateScan(id: string): Promise<DuplicateScanJob> {
  return invoke<DuplicateScanJob>("resume_duplicate_scan", { id });
}

export function cancelDuplicateScan(
  id: string,
): Promise<DuplicateScanJob | null> {
  return invoke<DuplicateScanJob | null>("cancel_duplicate_scan", { id });
}

export function findDuplicates(
  root: string,
  mode: DuplicateMatchMode,
): Promise<DuplicateScanResult> {
  return invoke<DuplicateScanResult>("find_duplicates", { root, mode });
}

export function removeDuplicates(
  root: string,
  paths: string[],
): Promise<DuplicateCleanupResult> {
  return invoke<DuplicateCleanupResult>("remove_duplicates", { root, paths });
}
