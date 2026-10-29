export interface Game {
  id: string;
  name: string;
  exe_path: string;
  folder: string;
  source: "scan" | "manual";
  source_folder: string | null;
  last_played: number | null;
  play_count: number;
  favorite: boolean;
  size_bytes: number | null;
}

export interface Library {
  scan_folders: string[];
  games: Game[];
}
