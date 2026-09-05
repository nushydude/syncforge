import { invoke } from "@tauri-apps/api/core";
import type {
  SnifferActionReview,
  SnifferEntry,
  SnifferEntryPage,
  SnifferIssuePage,
  SnifferQuery,
  SnifferScan,
  SnifferSummary,
} from "../types/sniffer";

export const startSnifferScan = (root: string) =>
  invoke<SnifferScan>("start_sniffer_scan", { root });
export const getSnifferScan = (scanId?: string) =>
  invoke<SnifferScan | null>("get_sniffer_scan", { scanId });
export const pinSnifferScan = (scanId: string | null) =>
  invoke<void>("pin_sniffer_scan", { scanId });
export const cancelSnifferScan = (scanId: string) =>
  invoke<SnifferScan>("cancel_sniffer_scan", { scanId });
export const querySnifferEntries = (request: SnifferQuery) =>
  invoke<SnifferEntryPage>("query_sniffer_entries", { request });
export const getSnifferSummary = (
  scanId: string,
  generationId: string,
  directoryId: string,
  request?: SnifferQuery,
) =>
  invoke<SnifferSummary>("get_sniffer_summary", {
    scanId,
    generationId,
    directoryId,
    request,
  });
export const getSnifferNode = (
  scanId: string,
  generationId: string,
  nodeId: string,
) =>
  invoke<SnifferEntry>("get_sniffer_node", {
    scanId,
    generationId,
    nodeId,
  });
export const refreshSnifferSubtree = (
  scanId: string,
  generationId: string,
  directoryId: string,
) =>
  invoke<SnifferScan>("refresh_sniffer_subtree", {
    scanId,
    generationId,
    directoryId,
  });
export const prepareSnifferAction = (request: {
  scanId: string;
  generationId: string;
  nodeId: string;
  action: "rename" | "recycle";
  newName?: string;
}) => invoke<SnifferActionReview>("prepare_sniffer_action", { request });
export const executeSnifferAction = (token: string) =>
  invoke<{
    action: string;
    path: string;
    newPath: string | null;
    warning: string | null;
  }>("execute_sniffer_action", { token });
export const showSnifferItemProperties = (scanId: string, nodeId: string) =>
  invoke<void>("show_sniffer_item_properties", { scanId, nodeId });
export const querySnifferIssues = (
  scanId: string,
  category?: string,
  cursor?: string,
) =>
  invoke<SnifferIssuePage>("query_sniffer_issues", {
    scanId,
    category,
    cursor,
    limit: 100,
  });
