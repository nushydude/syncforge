import { invoke } from "@tauri-apps/api/core";
import type { FolderPair } from "../types";

export function listPairs(): Promise<FolderPair[]> {
  return invoke<FolderPair[]>("list_pairs");
}

export function savePair(pair: FolderPair): Promise<FolderPair> {
  return invoke<FolderPair>("save_pair", { pair });
}

export function deletePair(id: string): Promise<void> {
  return invoke("delete_pair", { id });
}

export function pickFolder(): Promise<string | null> {
  return invoke<string | null>("pick_folder");
}

export function pathExists(path: string): Promise<boolean> {
  return invoke<boolean>("path_exists", { path });
}

export function pathsEqual(a: string, b: string): Promise<boolean> {
  return invoke<boolean>("paths_equal", { a, b });
}
