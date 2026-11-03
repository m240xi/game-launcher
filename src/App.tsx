import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { MouseEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import type { DeliveryCounts, Game, Library, StoreDelivery } from "./types";
import { Icon } from "./icons";
import TitleBar, { type AppTab } from "./TitleBar";
import Sidebar, { type Filter } from "./Sidebar";
import Inspector from "./Inspector";
import Store, { type StoreSort } from "./Store";
import { ContinueCard, GameRow, GameTile } from "./GameTile";
import GameArt from "./GameArt";

type View = "grid" | "list";
type Sort = "name" | "recent" | "played" | "size";

const inTauri = () => "__TAURI_INTERNALS__" in window;

const TORRENT_WARN_KEY = "store-torrent-warning-v1";

const TITLE_BY_FILTER: Record<Filter, string> = {
  all: "All Games",
  favorites: "Favorites",
  recent: "Recently Played",
};

export default function App() {
  const [tab, setTab] = useState<AppTab>("library");
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
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [ctx, setCtx] = useState<{ game: Game; x: number; y: number } | null>(null);

  const [storeQuery, setStoreQuery] = useState("");
  const [storeSort, setStoreSort] = useState<StoreSort>("newest");
  const [storeZipOnly, setStoreZipOnly] = useState(false);
  const [storeSource, setStoreSource] = useState<StoreDelivery>("all");
  const [torrentWarn, setTorrentWarn] = useState<StoreDelivery | null>(null);
  const [deliveryCounts, setDeliveryCounts] = useState<DeliveryCounts | null>(null);
  const [installableCount, setInstallableCount] = useState<number | null>(null);

  const toastTimer = useRef<number>(0);

  const torrentWarned = (() => {
    try {
      return localStorage.getItem(TORRENT_WARN_KEY) === "1";
    } catch {
      return false;
    }
  })();

  const ackTorrentWarn = () => {
    try {
      localStorage.setItem(TORRENT_WARN_KEY, "1");
    } catch {
    }
    if (torrentWarn) setStoreSource(torrentWarn);
    setTorrentWarn(null);
  };

  const handleStoreSource = (next: StoreDelivery) => {
    if (next !== "preinstalled" && !torrentWarned) {
      setTorrentWarn(next);
      return;
    }
    setStoreSource(next);
  };

  useEffect(() => {
    if (tab === "store" && !torrentWarned && storeSource !== "preinstalled") {
      setTorrentWarn(storeSource);
    }
  }, [tab]);

  const flash = useCallback((msg: string) => {
    setToast(msg);
    window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), 3200);
  }, []);

  const call = useCallback(
    async <T,>(cmd: string, args?: Record<string, unknown>): Promise<T | null> => {
      if (!inTauri()) {
        flash("Run the desktop app to use this feature");
        return null;
      }
      try {
        setError(null);
        return await invoke<T>(cmd, args);
      } catch (e) {
        setError(String(e));
        return null;
      }
    },
    [flash],
  );

  const applyLib = (next: Library | null) => {
    if (next) {
      setLib(next);
      setSelected((current) => next.games.find((g) => g.id === current?.id) ?? current);
    }
  };

  const refetchLibrary = useCallback(async (): Promise<Library | null> => {
    if (!inTauri()) return null;
    try {
      const next = await invoke<Library>("get_library");
      setLib(next);
      return next;
    } catch {
      return null;
    }
  }, []);

  useEffect(() => {
    (async () => {
      if (!inTauri()) {
        setLoading(false);
        return;
      }
      const next = await call<Library>("get_library");
      if (next) setLib(next);
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
      multiple: true,
      title: "Select game executable(s)",
      filters: [{ name: "Executable", extensions: ["exe"] }],
    });
    const files = typeof picked === "string" ? [picked] : Array.isArray(picked) ? picked : [];
    if (!files.length) return;
    setBusy("Adding…");
    let next: Library | null = null;
    for (const file of files) next = await call<Library>("add_game", { exePath: file });
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
    if (next) {
      applyLib(next);
      flash(`Launched ${game.name}`);
    }
  }, []);

  const toggleFavorite = useCallback(
    async (game: Game) => {
      applyLib(await call<Library>("toggle_favorite", { id: game.id }));
    },
    [call],
  );

  const renameGame = useCallback(
    async (game: Game, name: string) => {
      applyLib(await call<Library>("rename_game", { id: game.id, name }));
      flash(`Renamed to ${name}`);
    },
    [call],
  );

  const removeGame = useCallback(
    async (game: Game) => {
      applyLib(await call<Library>("remove_game", { id: game.id }));
      setSelected((current) => (current?.id === game.id ? null : current));
      flash("Removed from library (files untouched)");
    },
    [call],
  );

  const uninstallGame = useCallback(
    async (game: Game) => {
      if (!game.store_id) return;
      setBusy(`Uninstalling ${game.name}…`);
      const next = await call<Library>("store_uninstall", { storeId: game.store_id });
      setBusy(null);
      applyLib(next);
      setSelected((current) => (current?.id === game.id ? null : current));
      flash(`Uninstalled ${game.name} (files removed)`);
    },
    [call, flash],
  );

  const openCtx = (event: MouseEvent, game: Game) => {
    event.preventDefault();
    event.stopPropagation();
    setCtx({ game, x: event.clientX, y: event.clientY });
  };

  useEffect(() => {
    if (!ctx) return;
    const close = () => setCtx(null);
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
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
      sort === "name"
        ? a.name.localeCompare(b.name)
        : sort === "recent"
          ? (b.last_played ?? 0) - (a.last_played ?? 0)
          : sort === "played"
            ? b.play_count - a.play_count
            : (b.size_bytes ?? 0) - (a.size_bytes ?? 0),
    );
    if (sort === "name") list.sort((a, b) => Number(b.favorite) - Number(a.favorite));
    return list;
  }, [lib.games, query, sort, filter]);

  const recent = useMemo(
    () =>
      [...lib.games]
        .filter((g) => g.last_played)
        .sort((a, b) => (b.last_played ?? 0) - (a.last_played ?? 0))
        .slice(0, 6),
    [lib.games],
  );

  const handleCatalogMeta = useCallback(
    (meta: { counts: DeliveryCounts; installable: number }) => {
      setDeliveryCounts(meta.counts);
      setInstallableCount(meta.installable);
    },
    [],
  );

  const suppressCtx = (event: MouseEvent) => {
    if (!(event.target as HTMLElement).closest("input,textarea,[contenteditable]")) event.preventDefault();
  };

  return (
    <div className="shell" onContextMenu={suppressCtx}>
      <TitleBar tab={tab} onTab={setTab} />

      <div className="workbench">
        <Sidebar
          tab={tab}
          lib={lib}
          filter={filter}
          onFilter={(next) => {
            setTab("library");
            setFilter(next);
          }}
          storeSource={storeSource}
          onStoreSource={handleStoreSource}
          deliveryCounts={deliveryCounts}
          installableCount={installableCount}
          installableOnly={storeZipOnly}
          onInstallableOnly={setStoreZipOnly}
          onAddFolder={addFolder}
          onAddGame={addGame}
          onRescan={rescan}
          onOpenFolder={(path) => call("open_folder", { path })}
          onRemoveFolder={async (folder) => applyLib(await call<Library>("remove_scan_folder", { folder }))}
        />

        {tab === "store" ? (
          <Store
            lib={lib}
            onLibrary={applyLib}
            refetchLibrary={refetchLibrary}
            flash={flash}
            query={storeQuery}
            onQuery={setStoreQuery}
            sort={storeSort}
            onSort={setStoreSort}
            zipOnly={storeZipOnly}
            onZipOnly={setStoreZipOnly}
            source={storeSource}
            onSource={handleStoreSource}
            onCatalogMeta={handleCatalogMeta}
          />
        ) : (
          <>
            <main className="stage">
              <header className="stage-head">
                <div className="stage-titles">
                  <h1>{TITLE_BY_FILTER[filter]}</h1>
                  <span className="stage-count">
                    {games.length} {games.length === 1 ? "game" : "games"}
                  </span>
                </div>
                <div className="stage-tools">
                  <div className="field search">
                    <Icon name="search" size={13} />
                    <input
                      placeholder="Search your library…"
                      value={query}
                      onChange={(e) => setQuery(e.target.value)}
                      spellCheck={false}
                    />
                  </div>
                  <div className="select">
                    <select value={sort} onChange={(e) => setSort(e.target.value as Sort)} title="Sort library">
                      <option value="name">Sort: Name</option>
                      <option value="recent">Sort: Last played</option>
                      <option value="played">Sort: Most played</option>
                      <option value="size">Sort: Size</option>
                    </select>
                    <Icon name="chevron-down" size={13} />
                  </div>
                  <div className="seg">
                    <button
                      className={view === "grid" ? "on" : ""}
                      onClick={() => setView("grid")}
                      title="Grid view"
                      aria-label="Grid view"
                    >
                      <Icon name="grid" size={13} />
                    </button>
                    <button
                      className={view === "list" ? "on" : ""}
                      onClick={() => setView("list")}
                      title="List view"
                      aria-label="List view"
                    >
                      <Icon name="list" size={13} />
                    </button>
                  </div>
                </div>
              </header>

              <div className="stage-body">
                {error && (
                  <div className="banner error">
                    <Icon name="alert" size={14} />
                    <span>{error}</span>
                    <button onClick={() => setError(null)} aria-label="Dismiss">
                      <Icon name="x" size={13} />
                    </button>
                  </div>
                )}
                {busy && (
                  <div className="banner busy">
                    <span className="spinner" />
                    {busy}
                  </div>
                )}

                {loading && (
                  <div className="loading-block">
                    <span className="spinner" /> Loading your library…
                  </div>
                )}

                {!loading && games.length === 0 && (
                  <div className="blank">
                    <h2>{lib.games.length === 0 ? "Your library is empty" : "No games match"}</h2>
                    {lib.games.length === 0 ? (
                      <>
                        <p>
                          Add a folder such as <code>D:\Games</code> and Hate.bio Launcher will find every game
                          executable inside it — including ones buried in nested subfolders. You can also add
                          individual games by hand.
                        </p>
                        <div className="row">
                          <button className="btn-play" onClick={addFolder}>
                            <Icon name="plus" size={13} /> Add Scan Folder
                          </button>
                          <button className="btn" onClick={addGame}>
                            <Icon name="file" size={13} /> Add Game (.exe)
                          </button>
                        </div>
                      </>
                    ) : (
                      <>
                        <p>Nothing in your library matches that. Clear the search or pick another filter on the left.</p>
                        <div className="row">
                          <button className="btn" onClick={() => setQuery("")}>
                            Clear search
                          </button>
                          <button className="btn" onClick={() => setFilter("all")}>
                            Show all games
                          </button>
                        </div>
                      </>
                    )}
                  </div>
                )}

                {!loading && games.length > 0 && (
                  <>
                    {filter === "all" && recent.length > 0 && (
                      <section className="section">
                        <div className="section-head">
                          <span className="mlabel">Continue playing</span>
                        </div>
                        <div className="continue-row">
                          {recent.map((game) => (
                            <ContinueCard key={game.id} game={game} onSelect={setSelected} onPlay={play} />
                          ))}
                        </div>
                      </section>
                    )}

                    {view === "grid" ? (
                      <div className="tiles">
                        {games.map((game) => (
                          <GameTile
                            key={game.id}
                            game={game}
                            selected={selected?.id === game.id}
                            onSelect={setSelected}
                            onPlay={play}
                            onContextMenu={openCtx}
                            onToggleFavorite={toggleFavorite}
                          />
                        ))}
                      </div>
                    ) : (
                      <div className="ltable">
                        <div className="ltable-head">
                          <span>Title</span>
                          <span>Last played</span>
                          <span className="col-size">Size</span>
                          <span className="col-src">Added via</span>
                          <span className="r" />
                        </div>
                        {games.map((game) => (
                          <GameRow
                            key={game.id}
                            game={game}
                            selected={selected?.id === game.id}
                            onSelect={setSelected}
                            onPlay={play}
                            onContextMenu={openCtx}
                          />
                        ))}
                      </div>
                    )}
                  </>
                )}
              </div>
            </main>

            {selected && (
              <Inspector
                game={selected}
                renaming={renamingId === selected.id}
                onRenameStart={() => setRenamingId(selected.id)}
                onRenameCancel={() => setRenamingId(null)}
                onClose={() => {
                  setSelected(null);
                  setRenamingId(null);
                }}
                onPlay={play}
                onRename={renameGame}
                onToggleFavorite={toggleFavorite}
                onOpenFolder={(path) => call("open_folder", { path })}
                onReveal={(path) => call("reveal_exe", { exePath: path })}
                onRemove={removeGame}
                onUninstall={uninstallGame}
              />
            )}
          </>
        )}
      </div>

      {ctx &&
        (() => {
          const game = ctx.game;
          const W = 212;
          const H = 258;
          const x = Math.min(ctx.x, window.innerWidth - W - 8);
          const y = Math.min(ctx.y, window.innerHeight - H - 8);
          const act = (fn: () => void | Promise<unknown>) => async (event: MouseEvent) => {
            event.stopPropagation();
            setCtx(null);
            await fn();
          };
          return (
            <div
              className="ctx-menu"
              style={{ left: x, top: y }}
              onMouseDown={(event) => event.stopPropagation()}
              onContextMenu={(event) => event.preventDefault()}
            >
              <div className="ctx-head">
                <GameArt game={game} variant="icon" />
                <span>{game.name}</span>
              </div>
              <button className="ctx-item" onClick={act(() => play(game))}>
                <Icon name="play" size={13} fill /> Play
              </button>
              <button className="ctx-item" onClick={act(() => toggleFavorite(game))}>
                <Icon name="star" size={13} fill={game.favorite} />{" "}
                {game.favorite ? "Remove from favorites" : "Add to favorites"}
              </button>
              <div className="ctx-sep" />
              <button
                className="ctx-item"
                onClick={act(() => {
                  setSelected(game);
                  setRenamingId(game.id);
                })}
              >
                <Icon name="pencil" size={13} /> Rename
              </button>
              <button className="ctx-item" onClick={act(() => call("open_folder", { path: game.folder }))}>
                <Icon name="folder" size={13} /> Open game folder
              </button>
              <button className="ctx-item" onClick={act(() => call("reveal_exe", { exePath: game.exe_path }))}>
                <Icon name="external" size={13} /> Show in Explorer
              </button>
              <div className="ctx-sep" />
              {game.store_id ? (
                <button className="ctx-item danger" onClick={act(() => uninstallGame(game))}>
                  <Icon name="trash" size={13} /> Uninstall
                </button>
              ) : (
                <button className="ctx-item danger" onClick={act(() => removeGame(game))}>
                  <Icon name="trash" size={13} /> Remove from library
                </button>
              )}
            </div>
          );
        })()}

      {torrentWarn && (
        <div className="modal-backdrop" onMouseDown={ackTorrentWarn}>
          <section
            className="modal"
            role="dialog"
            aria-modal="true"
            aria-label="Torrent catalog notice"
            onMouseDown={(event) => event.stopPropagation()}
          >
            <button className="icon-btn modal-close" aria-label="Close" onClick={ackTorrentWarn}>
              <Icon name="x" size={14} />
            </button>
            <header className="modal-head">
              <div>
                <h2>Torrent catalog notice</h2>
                <div className="sub">Community-sourced listings</div>
              </div>
            </header>
            <div className="modal-body">
              <p className="modal-note">
                This section lists thousands of community-maintained entries. Game artwork, Steam pages, and some
                links may not resolve because of where they're hosted.
              </p>
              <ul className="consent-list">
                <li>Titles and metadata can be rough, mismatched, or missing images.</li>
                <li>Links and file hosts can change or go offline without warning.</li>
                <li>Use your own due diligence when installing anything from this section.</li>
              </ul>
              <p className="modal-fine">
                This notice applies only to torrent installs — every Pre-installed title is checked and safe to install.
              </p>
            </div>
            <div className="modal-foot">
              <button className="btn-play" onClick={ackTorrentWarn} autoFocus>
                Got it
              </button>
            </div>
          </section>
        </div>
      )}
      {toast && <div className="toast">{toast}</div>}
    </div>
  );
}
