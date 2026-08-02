import { invoke } from "@tauri-apps/api/core";
import type { FolderSizeResult } from "../types";

export function scanFolderSizes(path: string): Promise<FolderSizeResult> {
  return invoke<FolderSizeResult>("scan_folder_sizes", { path });
}

export function renameSnifferItem(path: string, newName: string): Promise<string> {
  return invoke<string>("rename_sniffer_item", { path, newName });
}

export function deleteSnifferItem(path: string): Promise<void> {
  return invoke("delete_sniffer_item", { path });
}

export function showSnifferItemProperties(path: string): Promise<void> {
  return invoke("show_sniffer_item_properties", { path });
}
