import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import type { Game, Library } from "./types";
import { Icon } from "./icons";
import TitleBar from "./TitleBar";

type View = "grid" | "list";
type Sort = "name" | "recent" | "played" | "size";
type Filter = "all" | "favorites" | "recent";

const inTauri = () => "__TAURI_INTERNALS__" in window;

function fmtSize(b: number | null) {
  if (b == null) return "";
  if (b < 1e6) return `${(b / 1e3).toFixed(0)} KB`;
  if (b < 1e9) return `${(b / 1e6).toFixed(0)} MB`;
  return `${(b / 1e9).toFixed(1)} GB`;
}
function fmtAgo(ts: number | null) {
  if (!ts) return "Never played";
  const s = Date.now() / 1000 - ts;
  if (s < 3600) return "Played just now";
  if (s < 86400) return `Played ${Math.floor(s / 3600)}h ago`;
  if (s < 86400 * 30) return `Played ${Math.floor(s / 86400)}d ago`;
  return `Played ${new Date(ts * 1000).toLocaleDateString()}`;
}
function GameIcon({ game, size }: { game: Game; size: number }) {
  const [src, setSrc] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let live = true;
    if (!inTauri()) return;
    invoke<string | null>("get_icon", { exePath: game.exe_path })
      .then((v) => { if (live) { v ? setSrc(v) : setFailed(true); } })
      .catch(() => live && setFailed(true));
    return () => { live = false; };
  }, [game.exe_path]);

  if (src && !failed)
    return <img src={src} alt="" width={size} height={size} style={{ borderRadius: 10, flexShrink: 0, filter: "drop-shadow(0 4px 10px rgba(0,0,0,.5))" }} onError={() => setFailed(true)} draggable={false} />;
  return (
    <div style={{
      width: size, height: size, borderRadius: 10, flexShrink: 0, display: "grid", placeItems: "center",
      fontWeight: 800, fontSize: size * 0.42, color: "#e4e4e7", userSelect: "none",
      background: "linear-gradient(145deg,#3f3f46,#18181b)", border: "1px solid rgba(255,255,255,.1)",
    }}>{game.name.trim().charAt(0).toUpperCase() || "?"}</div>
  );
}

export default function App() {
  const [lib, setLib] = useState<Library>({ scan_folders: [], games: [] });
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [view, setView] = useState<View>("grid");
  const [sort, setSort] = useState<Sort>("name");
  const [filter, setFilter] = useState<Filter>("all");
  const [selected, setSelected] = useState<Game | null>(null);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [renameVal, setRenameVal] = useState("");
  const [launchArgs, setLaunchArgs] = useState("");
  const [ctx, setCtx] = useState<{ game: Game; x: number; y: number } | null>(null);
  const toastTimer = useRef<number>(0);

  const flash = (msg: string) => {
    setToast(msg);
    window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), 3200);
  };
  const call = async <T,>(cmd: string, args?: Record<string, unknown>): Promise<T | null> => {
    if (!inTauri()) { flash("Run the desktop app to use this feature"); return null; }
    try { setError(null); return await invoke<T>(cmd, args); }
    catch (e) { setError(String(e)); return null; }
  };
  const applyLib = (next: Library | null) => { if (next) { setLib(next); setSelected((s) => next.games.find((g) => g.id === s?.id) ?? s); } };

  useEffect(() => {
    (async () => {
      if (!inTauri()) { setLoading(false); return; }
      const l = await call<Library>("get_library");
      if (l) setLib(l);
      setLoading(false);
    })();
  }, []);

  const addFolder = async () => {
    const picked = await open({ directory: true, multiple: false, title: "Choose a folder to scan for games" });
    if (typeof picked !== "string") return;
    setBusy("Scanning folder…");
    applyLib(await call<Library>("add_scan_folder", { folder: picked }));
    setBusy(null);
    flash(`Scanned ${picked}`);
  };
  const addGame = async () => {
    const picked = await open({
      multiple: true, title: "Select game executable(s)",
      filters: [{ name: "Executable", extensions: ["exe"] }],
    });
    const files = typeof picked === "string" ? [picked] : Array.isArray(picked) ? picked : [];
    if (!files.length) return;
    setBusy("Adding…");
    let next: Library | null = null;
    for (const f of files) next = await call<Library>("add_game", { exePath: f });
    applyLib(next);
    setBusy(null);
    flash(files.length > 1 ? `Added ${files.length} games` : "Game added");
  };
  const rescan = async () => {
    setBusy("Rescanning…");
    applyLib(await call<Library>("rescan_all"));
    setBusy(null);
    flash("Library rescanned");
  };
  const play = useCallback(async (game: Game, args?: string) => {
    setBusy(`Launching ${game.name}…`);
    const next = await call<Library>("launch_game", { id: game.id, args: args ?? null });
    setBusy(null);
    if (next) { applyLib(next); flash(`Launched ${game.name}`); }
  }, []);

  const openCtx = (e: React.MouseEvent, game: Game) => {
    e.preventDefault();
    e.stopPropagation();
    setSelected(game);
    setCtx({ game, x: e.clientX, y: e.clientY });
  };

  useEffect(() => {
    if (!ctx) return;
    const close = () => setCtx(null);
    const key = (e: KeyboardEvent) => { if (e.key === "Escape") close(); };
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", key);
    window.addEventListener("blur", close);
    window.addEventListener("scroll", close, true);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", key);
      window.removeEventListener("blur", close);
      window.removeEventListener("scroll", close, true);
    };
  }, [ctx]);

  const games = useMemo(() => {
    let list = [...lib.games];
    if (filter === "favorites") list = list.filter((g) => g.favorite);
    if (filter === "recent") list = list.filter((g) => g.last_played);
    const q = query.trim().toLowerCase();
    if (q) list = list.filter((g) => g.name.toLowerCase().includes(q) || g.exe_path.toLowerCase().includes(q));
    list.sort((a, b) =>
      sort === "name" ? a.name.localeCompare(b.name)
      : sort === "recent" ? (b.last_played ?? 0) - (a.last_played ?? 0)
      : sort === "played" ? b.play_count - a.play_count
      : (b.size_bytes ?? 0) - (a.size_bytes ?? 0));
    if (sort === "name") list.sort((a, b) => Number(b.favorite) - Number(a.favorite));
    return list;
  }, [lib.games, query, sort, filter]);

  const recent = useMemo(() => [...lib.games].filter((g) => g.last_played).sort((a, b) => (b.last_played ?? 0) - (a.last_played ?? 0)).slice(0, 5), [lib.games]);

  const suppressCtx = (e: React.MouseEvent) => {
    if (!(e.target as HTMLElement).closest("input,textarea,[contenteditable]")) e.preventDefault();
  };

  return (
    <div className="shell" onContextMenu={suppressCtx}>
      <TitleBar />
      <div className="app">
      <aside className="sidebar">
        <nav className="nav">
          <button className={filter === "all" ? "nav-item active" : "nav-item"} onClick={() => setFilter("all")}><Icon name="library" /><span>All Games</span><b>{lib.games.length}</b></button>
          <button className={filter === "favorites" ? "nav-item active" : "nav-item"} onClick={() => setFilter("favorites")}><Icon name="star" /><span>Favorites</span><b>{lib.games.filter((g) => g.favorite).length}</b></button>
          <button className={filter === "recent" ? "nav-item active" : "nav-item"} onClick={() => setFilter("recent")}><Icon name="clock" /><span>Recently Played</span></button>
        </nav>

        {recent.length > 0 && (
          <>
            <div className="side-label">Recent</div>
            {recent.map((g) => (
              <button key={g.id} className="recent-item" onClick={() => setSelected(g)} title={g.exe_path}>
                <GameIcon game={g} size={22} /><span>{g.name}</span>
              </button>
            ))}
          </>
        )}

        <div className="side-label">Scan Folders</div>
        {lib.scan_folders.length === 0 && <p className="muted small">No folders yet. Add one and every game .exe inside is found automatically.</p>}
        {lib.scan_folders.map((f) => (
          <div key={f} className="folder-row" title={f}>
            <Icon name="folder" size={14} />
            <span className="folder-path">{f}</span>
            <span className="folder-actions">
              <button className="icon-btn" title="Open in Explorer" onClick={() => call("open_folder", { path: f })}><Icon name="external" size={13} /></button>
              <button className="icon-btn danger" title="Stop scanning this folder (removes its scanned games)" onClick={async () => applyLib(await call<Library>("remove_scan_folder", { folder: f }))}><Icon name="x" size={13} /></button>
            </span>
          </div>
        ))}
        <button className="btn ghost wide" onClick={addFolder}><Icon name="plus" size={14} /> Add Scan Folder…</button>
        <button className="btn ghost wide" onClick={addGame}><Icon name="file" size={14} /> Add Game (.exe)…</button>
        <button className="btn ghost wide" onClick={rescan} disabled={!lib.scan_folders.length}><Icon name="refresh" size={14} /> Rescan Folders</button>
      </aside>

      <main className="main">
        <header className="topbar">
          <div className="search-wrap">
            <Icon name="search" size={15} />
            <input className="search" placeholder="Search your library…" value={query} onChange={(e) => setQuery(e.target.value)} autoFocus />
          </div>
          <div className="select-wrap">
            <select className="select" value={sort} onChange={(e) => setSort(e.target.value as Sort)} title="Sort">
              <option value="name">Sort: Name</option>
              <option value="recent">Sort: Recently played</option>
              <option value="played">Sort: Most played</option>
              <option value="size">Sort: Size</option>
            </select>
            <Icon name="chevron-down" size={14} />
          </div>
          <div className="view-toggle">
            <button className={view === "grid" ? "on" : ""} onClick={() => setView("grid")} title="Grid view"><Icon name="grid" size={15} /></button>
            <button className={view === "list" ? "on" : ""} onClick={() => setView("list")} title="List view"><Icon name="list" size={15} /></button>
          </div>
        </header>

        {error && <div className="banner error"><Icon name="alert" size={15} /><span>{error}</span><button onClick={() => setError(null)}><Icon name="x" size={14} /></button></div>}
        {busy && <div className="banner busy"><span className="spinner" />{busy}</div>}
        {loading && <div className="empty">Loading your library…</div>}

        {!loading && games.length === 0 && (
          <div className="empty">
            <div className="empty-art"><Icon name="play" size={34} fill /></div>
            <h2>{lib.games.length === 0 ? "Your library is empty" : "No games match"}</h2>
            {lib.games.length === 0 ? (
              <>
                <p>Add a folder (e.g. <code>D:\Games</code>) and Hate.bio Launcher will find every game .exe inside it — including ones buried in nested subfolders — or add a specific game .exe yourself.</p>
                <div className="row center">
                  <button className="btn primary" onClick={addFolder}><Icon name="plus" size={14} /> Add Scan Folder…</button>
                  <button className="btn" onClick={addGame}><Icon name="file" size={14} /> Add Game (.exe)…</button>
                </div>
              </>
            ) : <p>Try a different search or filter.</p>}
          </div>
        )}

        {!loading && games.length > 0 && (
          <div className="section-head">
            <h2>{filter === "all" ? "All Games" : filter === "favorites" ? "Favorites" : "Recently Played"}</h2>
            <span className="muted small">{games.length} {games.length === 1 ? "game" : "games"}</span>
          </div>
        )}

        {!loading && games.length > 0 && view === "grid" && (
          <div className="grid">
            {games.map((g, i) => (
              <div key={g.id} className={`card ${selected?.id === g.id ? "sel" : ""}`} style={{ "--i": i } as CSSProperties} onClick={() => setSelected(g)} onDoubleClick={() => play(g)} onContextMenu={(e) => openCtx(e, g)}>
                <div className="card-art"><GameIcon game={g} size={64} /></div>
                <div className="card-name" title={g.name}>{g.name}</div>
                <div className="card-sub">{fmtAgo(g.last_played)}{g.size_bytes ? ` · ${fmtSize(g.size_bytes)}` : ""}</div>
                <button className={`fav ${g.favorite ? "on" : ""}`} title={g.favorite ? "Unfavorite" : "Favorite"}
                  onClick={async (e) => { e.stopPropagation(); applyLib(await call<Library>("toggle_favorite", { id: g.id })); }}><Icon name="star" size={15} fill={g.favorite} /></button>
                <button className="play-fab" title={`Play ${g.name}`} onClick={(e) => { e.stopPropagation(); play(g); }}><Icon name="play" size={15} fill /></button>
              </div>
            ))}
          </div>
        )}

        {!loading && games.length > 0 && view === "list" && (
          <div className="list">
            {games.map((g) => (
              <div key={g.id} className={`row-item ${selected?.id === g.id ? "sel" : ""}`} onClick={() => setSelected(g)} onDoubleClick={() => play(g)} onContextMenu={(e) => openCtx(e, g)}>
                <GameIcon game={g} size={40} />
                <div className="row-main">
                  <div className="row-name">{g.name} {g.favorite && <Icon name="star" size={13} fill />}</div>
                  <div className="muted small">{g.exe_path}</div>
                </div>
                <span className="muted small">{fmtAgo(g.last_played)}</span>
                <span className="muted small">{fmtSize(g.size_bytes)}</span>
                <button className="btn primary sm" onClick={(e) => { e.stopPropagation(); play(g); }}><Icon name="play" size={13} fill /> Play</button>
              </div>
            ))}
          </div>
        )}
      </main>

      {selected && (
        <aside className="detail">
          <button className="icon-btn close" onClick={() => setSelected(null)}><Icon name="x" size={15} /></button>
          <GameIcon game={selected} size={80} />
          {renaming === selected.id ? (
            <form onSubmit={async (e) => { e.preventDefault(); applyLib(await call<Library>("rename_game", { id: selected.id, name: renameVal })); setRenaming(null); }}>
              <input className="search" value={renameVal} onChange={(e) => setRenameVal(e.target.value)} autoFocus />
              <div className="row"><button className="btn primary sm" type="submit">Save</button><button className="btn sm" type="button" onClick={() => setRenaming(null)}>Cancel</button></div>
            </form>
          ) : (
            <h2 className="detail-name">{selected.name}</h2>
          )}
          <dl className="meta">
            <dt>Last played</dt><dd>{fmtAgo(selected.last_played)}</dd>
            <dt>Times played</dt><dd>{selected.play_count}</dd>
            <dt>Size</dt><dd>{fmtSize(selected.size_bytes) || "—"}</dd>
            <dt>Added via</dt><dd>{selected.source === "scan" ? "Folder scan" : "Added manually"}</dd>
            <dt>Location</dt><dd className="path">{selected.exe_path}</dd>
          </dl>

          <button className="btn play-big" onClick={() => play(selected)}><Icon name="play" size={17} fill /> PLAY</button>
          <input className="search" placeholder="Launch arguments (optional, e.g. -windowed)" value={launchArgs} onChange={(e) => setLaunchArgs(e.target.value)} />
          {launchArgs.trim() && <button className="btn wide" onClick={() => play(selected, launchArgs)}><Icon name="play" size={14} fill /> Play with arguments</button>}

          <div className="detail-actions">
            <button className="btn ghost wide" onClick={async () => applyLib(await call<Library>("toggle_favorite", { id: selected.id }))}><Icon name="star" size={15} fill={selected.favorite} /> {selected.favorite ? "Unfavorite" : "Add to Favorites"}</button>
            <button className="btn ghost wide" onClick={() => { setRenaming(selected.id); setRenameVal(selected.name); }}><Icon name="pencil" size={15} /> Rename</button>
            <button className="btn ghost wide" onClick={() => call("open_folder", { path: selected.folder })}><Icon name="folder" size={15} /> Open Game Folder</button>
            <button className="btn ghost wide" onClick={() => call("reveal_exe", { exePath: selected.exe_path })}><Icon name="external" size={15} /> Show in Explorer</button>
            <button className="btn ghost wide danger-text" onClick={async () => { applyLib(await call<Library>("remove_game", { id: selected.id })); setSelected(null); flash("Removed from library (files untouched)"); }}><Icon name="trash" size={15} /> Remove from Library</button>
          </div>
          <p className="muted small">Removing only removes it from the launcher — your game files are never touched. Double-click a game to play instantly.</p>
        </aside>
      )}

      {ctx && (() => {
        const g = ctx.game;
        const MW = 214, MH = 262;
        const x = Math.min(ctx.x, window.innerWidth - MW - 8);
        const y = Math.min(ctx.y, window.innerHeight - MH - 8);
        const act = (fn: () => void | Promise<unknown>) => async (e: React.MouseEvent) => {
          e.stopPropagation();
          setCtx(null);
          await fn();
        };
        return (
          <div className="ctx-menu" style={{ left: x, top: y }} onMouseDown={(e) => e.stopPropagation()} onContextMenu={(e) => e.preventDefault()}>
            <div className="ctx-head"><GameIcon game={g} size={20} /><span>{g.name}</span></div>
            <button className="ctx-item" onClick={act(() => play(g))}><Icon name="play" size={14} fill /> Play</button>
            <button className="ctx-item" onClick={act(async () => applyLib(await call<Library>("toggle_favorite", { id: g.id })))}><Icon name="star" size={14} fill={g.favorite} /> {g.favorite ? "Unfavorite" : "Add to Favorites"}</button>
            <div className="ctx-sep" />
            <button className="ctx-item" onClick={act(() => { setRenaming(g.id); setRenameVal(g.name); })}><Icon name="pencil" size={14} /> Rename</button>
            <button className="ctx-item" onClick={act(() => call("open_folder", { path: g.folder }))}><Icon name="folder" size={14} /> Open Game Folder</button>
            <button className="ctx-item" onClick={act(() => call("reveal_exe", { exePath: g.exe_path }))}><Icon name="external" size={14} /> Show in Explorer</button>
            <div className="ctx-sep" />
            <button className="ctx-item danger" onClick={act(async () => { applyLib(await call<Library>("remove_game", { id: g.id })); setSelected((s) => s?.id === g.id ? null : s); flash("Removed from library (files untouched)"); })}><Icon name="trash" size={14} /> Remove from Library</button>
          </div>
        );
      })()}

      {toast && <div className="toast">{toast}</div>}
      </div>
    </div>
  );
}
