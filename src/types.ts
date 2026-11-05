export type GameKind = "pc" | "rom";

export interface Game {
  id: string;
  name: string;
  exe_path: string;
  folder: string;
  source: "scan" | "manual" | "store";
  source_folder: string | null;
  kind: GameKind;
  system: string | null;
  last_played: number | null;
  play_count: number;
  playtime_secs: number;
  favorite: boolean;
  size_bytes: number | null;
  store_id: string | null;
  steam_app_id: number | null;
}

export interface Library {
  scan_folders: string[];
  games: Game[];
}

export interface StoreGame {
  id: string;
  name: string;
  version: string;
  size_label: string;
  size_mb: number;
  uploaded: number;
  ext: string;
  page_url: string;
  uri: string;
  source: string;
}

export interface StoreCatalog {
  fetched_at: number;
  games: StoreGame[];
}

export type StoreDelivery = "all" | "preinstalled" | "torrent";

export interface DeliveryCounts {
  preinstalled: number;
  torrent: number;
}

export interface StoreSources {
  sources: { label: string; url: string }[];
}

export interface StoreProgress {
  game_id: string;
  phase: "resolving" | "downloading" | "extracting" | "finishing" | "done" | "canceled" | "error";
  downloaded?: number;
  total?: number;
  bps?: number;
  error?: string;
}
