import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Condition = "intact" | "recoverable" | "probable" | "overwritten" | "unsupported";
export type Integrity = "unchecked" | "valid" | "damaged";

export interface DeviceInfo {
  path: string;
  size: number | null;
  model: string | null;
  removable: boolean | null;
  rotational: boolean | null;
  is_partition: boolean;
}

export interface SourceInfo {
  path: string;
  size: number;
  is_device: boolean;
}

export interface ScanProgress {
  phase: "quick" | "rebuild" | "deep" | "verify";
  position: number;
  total: number;
  found: number;
}

export interface ScanDone {
  cancelled: boolean;
  volumes: number;
  files: number;
  deleted: number;
  carved: number;
}

export interface VolumeInfo {
  index: number;
  name: string;
  fs: string | null;
  size: number;
  error: string | null;
}

export interface Item {
  key: string;
  origin: "fs" | "carved";
  name: string;
  folder: string;
  volume: number | null;
  size: number;
  modified: number | null;
  utc_time: boolean;
  deleted: boolean;
  condition: Condition | null;
  integrity: Integrity;
  category: string;
  ext: string;
}

export interface Query {
  only_deleted: boolean;
  category: string;
  text: string;
  node: string | null;
  sort: "name" | "size" | "modified" | "folder";
  asc: boolean;
}

export interface Page {
  total: number;
  offset: number;
  rows: Item[];
  counts: Record<string, number>;
  deleted_total: number;
}

export interface TreeNode {
  id: string;
  label: string;
  count: number;
  has_children: boolean;
}

/** Tutti i file della query `all`, più `include`, meno `exclude`. */
export interface Selection {
  all: Query | null;
  include: string[];
  exclude: string[];
}

export interface Preview {
  kind: "image" | "text" | "none";
  mime: string;
  data: string;
  truncated: boolean;
}

export interface RecoverProgress {
  done: number;
  total: number;
  current: string;
}

export interface RecoverDone {
  ok: number;
  failed: string[];
  dest: string;
}

export type ScanMode = "quick" | "deep" | "both";

export const api = {
  listDevices: () => invoke<DeviceInfo[]>("list_devices"),
  openSource: (path: string) => invoke<SourceInfo>("open_source", { path }),
  startScan: (mode: ScanMode, kinds: string[]) => invoke<void>("start_scan", { mode, kinds }),
  cancel: () => invoke<void>("cancel"),
  volumes: () => invoke<VolumeInfo[]>("volumes"),
  query: (q: Query, offset: number, limit: number) => invoke<Page>("query", { q, offset, limit }),
  tree: (q: Query, parent: string | null) => invoke<TreeNode[]>("tree", { q, parent }),
  selectionInfo: (sel: Selection) => invoke<{ count: number; size: number }>("selection_info", { sel }),
  preview: (key: string) => invoke<Preview>("preview", { key }),
  recover: (sel: Selection, dest: string) => invoke<void>("recover", { sel, dest }),
  /** Rinomina una cartella dei risultati; ritorna l'id del nodo rinominato. */
  renameFolder: (node: string, name: string) => invoke<string>("rename_folder", { node, name }),
};

export function onEvent<T>(name: string, cb: (payload: T) => void): Promise<UnlistenFn> {
  return listen<T>(name, (e) => cb(e.payload));
}
