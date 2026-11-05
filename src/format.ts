import type { Game } from "./types";

export function fmtSize(bytes: number | null): string {
  if (bytes == null) return "";
  if (bytes < 1024) return `${bytes} B`;
  const kb = bytes / 1024;
  if (kb < 1024) return `${kb < 10 ? kb.toFixed(1) : Math.round(kb)} KB`;
  const mb = kb / 1024;
  if (mb < 1024) return `${mb < 10 ? mb.toFixed(1) : Math.round(mb)} MB`;
  const gb = mb / 1024;
  if (gb < 1024) return `${gb.toFixed(1)} GB`;
  return `${(gb / 1024).toFixed(2)} TB`;
}

export function fmtAgo(ts: number | null): string {
  if (!ts) return "Never played";
  const seconds = Date.now() / 1000 - ts;
  if (seconds < 60) return "Just now";
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  if (seconds < 86400 * 2) return "Yesterday";
  if (seconds < 86400 * 30) return `${Math.floor(seconds / 86400)}d ago`;
  return new Date(ts * 1000).toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
}

export function fmtPlays(count: number): string {
  if (count <= 0) return "Never played";
  return count === 1 ? "1 play" : `${count} plays`;
}

export function fmtPlaytime(secs: number): string {
  if (secs <= 0) return "Never played";
  if (secs < 60) return "Less than a minute";
  if (secs < 3600) return `${Math.round(secs / 60)} min`;
  return `${(secs / 3600).toFixed(1).replace(/\.0$/, "")} hrs`;
}

export function sourceLabel(source: Game["source"]): string {
  if (source === "scan") return "Folder scan";
  if (source === "store") return "Store install";
  return "Added manually";
}

export function gameMeta(game: Game): string {
  const size = fmtSize(game.size_bytes);
  const plays = game.playtime_secs > 0 ? fmtPlaytime(game.playtime_secs) : fmtPlays(game.play_count);
  if (size && plays) return `${size} · ${plays}`;
  return size || plays;
}
