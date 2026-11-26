import { invoke } from "@tauri-apps/api/core";
import type {
  DuplicateMatchMode,
  DuplicateGroupsPage,
  DuplicateMembersPage,
  DuplicateScanJob,
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
  attempt: number,
): Promise<DuplicateScanJob | null> {
  return invoke<DuplicateScanJob | null>("cancel_duplicate_scan", {
    id,
    attempt,
  });
}

export function revealDuplicate(
  jobId: string,
  resultRevision: string,
  itemId: string,
): Promise<void> {
  return invoke("reveal_duplicate", { jobId, resultRevision, itemId });
}

export function getDuplicateGroupsPage(
  jobId: string,
  revision: string,
  cursor: number,
  limit = 50,
): Promise<DuplicateGroupsPage> {
  return invoke("duplicate_groups_page", { jobId, revision, cursor, limit });
}
export function getDuplicateMembersPage(
  jobId: string,
  revision: string,
  groupId: string,
  cursor: number,
  limit = 50,
): Promise<DuplicateMembersPage> {
  return invoke("duplicate_members_page", {
    jobId,
    revision,
    groupId,
    cursor,
    limit,
  });
}
