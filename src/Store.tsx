import { useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { DeliveryCounts, Library, StoreCatalog, StoreDelivery, StoreGame, StoreProgress } from "./types";
import { Icon } from "./icons";
import { placeholderGradient } from "./GameArt";

const inTauri = () => "__TAURI_INTERNALS__" in window;
const PAGE_SIZE = 60;

export type StoreSort = "newest" | "name" | "size";

const ACTIVE_PHASES = new Set(["resolving", "downloading", "extracting", "finishing"]);
const CONSENT_KEY = "store-torrent-consent-v1";
const isMagnet = (uri: string) => uri.trim().toLowerCase().startsWith("magnet:");
const isGameInstallable = (g: StoreGame) => isMagnet(g.uri) || g.ext === "zip";

interface StoreSteamArtwork {
  app_id: number;
  name: string;
  cover: string | null;
}

const storeArtworkCache = new Map<string, StoreSteamArtwork | null>();
const storeArtworkRequests = new Map<string, Promise<StoreSteamArtwork | null>>();
const storeScreenshotsCache = new Map<number, string[]>();

function getStoreArtwork(title: string): Promise<StoreSteamArtwork | null> {
  const key = title.toLowerCase();
  if (storeArtworkCache.has(key)) return Promise.resolve(storeArtworkCache.get(key) ?? null);
  const pending = storeArtworkRequests.get(key);
  if (pending) return pending;
  const request = invoke<StoreSteamArtwork | null>("get_store_game_art", { title })
    .then((art) => {
      storeArtworkCache.set(key, art);
      return art;
    })
    .catch(() => null)
    .finally(() => storeArtworkRequests.delete(key));
  storeArtworkRequests.set(key, request);
  return request;
}

function openStorePage(title: string) {
  void getStoreArtwork(title).then((art) =>
    invoke("open_url", {
      url: art?.app_id
        ? `https://store.steampowered.com/app/${art.app_id}`
        : `https://store.steampowered.com/search/?term=${encodeURIComponent(title)}`,
    }),
  );
}

function StoreArt({ game, className }: { game: StoreGame; className?: string }) {
  const ref = useRef<HTMLDivElement | null>(null);
  const [visible, setVisible] = useState(false);
  const [cover, setCover] = useState<string | null>(null);

  useEffect(() => {
    const node = ref.current;
    if (!node) return;
    if (!("IntersectionObserver" in window)) {
      setVisible(true);
      return;
    }
    const observer = new IntersectionObserver(
      ([entry]) => {
        if (entry.isIntersecting) {
          setVisible(true);
          observer.disconnect();
        }
      },
      { rootMargin: "220px" },
    );
    observer.observe(node);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    if (!visible || !inTauri()) return;
    let live = true;
    void getStoreArtwork(game.name).then((art) => {
      if (live && art?.cover) setCover(art.cover);
    });
    return () => {
      live = false;
    };
  }, [visible, game.name]);

  return (
    <div ref={ref} className={className ?? "stcard-art"}>
      {cover ? (
        <img className="art-cover" src={cover} alt="" draggable={false} />
      ) : (
        <span className="art-letter" style={{ background: placeholderGradient(game.name) }}>
          {game.name.trim().charAt(0).toUpperCase() || "?"}
        </span>
      )}
    </div>
  );
}

function StoreGameDetails({ game, onClose }: { game: StoreGame; onClose: () => void }) {
  const [art, setArt] = useState<StoreSteamArtwork | null>(null);
  const [screenshots, setScreenshots] = useState<string[]>([]);
  const [activeImage, setActiveImage] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let live = true;
    setLoading(true);
    void getStoreArtwork(game.name).then(async (found) => {
      if (!live) return;
      setArt(found);
      if (!found) {
        setLoading(false);
        return;
      }
      const cached = storeScreenshotsCache.get(found.app_id);
      if (cached) {
        setScreenshots(cached);
        setActiveImage(cached[0] ?? found.cover);
        setLoading(false);
        return;
      }
      try {
        const images = await invoke<string[]>("get_store_game_screenshots", { appId: found.app_id });
        if (live) {
          storeScreenshotsCache.set(found.app_id, images);
          setScreenshots(images);
          setActiveImage(images[0] ?? found.cover);
        }
      } catch {
        if (live) setActiveImage(found.cover);
      } finally {
        if (live) setLoading(false);
      }
    });
    return () => {
      live = false;
    };
  }, [game.id, game.name]);

  const stepImage = (dir: number) => {
    if (screenshots.length < 2) return;
    const current = activeImage ? screenshots.indexOf(activeImage) : -1;
    const next = current < 0 ? 0 : (current + dir + screenshots.length) % screenshots.length;
    setActiveImage(screenshots[next]);
  };

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
      if (event.key === "ArrowLeft") stepImage(-1);
      if (event.key === "ArrowRight") stepImage(1);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose, screenshots, activeImage]);

  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <section
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-label={`${game.name} details`}
        onMouseDown={(event) => event.stopPropagation()}
      >
        <button className="icon-btn modal-close" aria-label="Close details" onClick={onClose}>
          <Icon name="x" size={15} />
        </button>
        <header className="modal-head">
          <StoreArt game={game} className="modal-art" />
          <div>
            <h2>{game.name}</h2>
            <div className="sub">
              {game.version || game.size_label} · {game.source || "ankergames"}
            </div>
          </div>
        </header>
        <div className="modal-body">
          {loading && (
            <div className="loading-block">
              <span className="spinner" /> Looking for Steam artwork…
            </div>
          )}
          {!loading && !art && (
            <p className="modal-note">
              No exact Steam match was found for this title, so there is no artwork to show. The store download is
              still available.
            </p>
          )}
          {art && (
            <p className="modal-note">
              Steam match: <strong>{art.name}</strong> · App {art.app_id}
            </p>
          )}
          {activeImage && (
            <div className="hero-frame">
              <img className="modal-hero" src={activeImage} alt={`${game.name} artwork`} onError={() => setActiveImage(art?.cover ?? null)} />
              {screenshots.length > 1 && (
                <>
                  <button className="hero-nav prev" aria-label="Previous image" onClick={() => stepImage(-1)}>
                    <Icon name="chevron-left" size={18} />
                  </button>
                  <button className="hero-nav next" aria-label="Next image" onClick={() => stepImage(1)}>
                    <Icon name="chevron-right" size={18} />
                  </button>
                </>
              )}
            </div>
          )}
          {screenshots.length > 0 && (
            <div className="shot-strip" aria-label="Game screenshots">
              {screenshots.map((image, index) => (
                <button
                  key={`${index}-${image}`}
                  className={activeImage === image ? "shot on" : "shot"}
                  onClick={() => setActiveImage(image)}
                  aria-label={`Show screenshot ${index + 1}`}
                >
                  <img src={image} alt="" />
                </button>
              ))}
            </div>
          )}
        </div>
        <div className="modal-foot">
          <button className="btn" onClick={() => openStorePage(game.name)}>
            <Icon name="external" size={13} /> Open game page
          </button>
          <button className="btn-play" onClick={onClose}>
            Done
          </button>
        </div>
      </section>
    </div>
  );
}

function fmtSpeed(bps: number) {
  if (bps >= 1e9) return `${(bps / 1e9).toFixed(1)} GB/s`;
  if (bps >= 1e6) return `${(bps / 1e6).toFixed(1)} MB/s`;
  return `${(bps / 1e3).toFixed(0)} KB/s`;
}

function fmtEta(seconds: number) {
  const s = Math.max(0, Math.round(seconds));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  return `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, "0")}m`;
}

const PHASE_LABEL: Record<string, string> = {
  resolving: "Preparing link…",
  downloading: "Downloading",
  extracting: "Unpacking",
  finishing: "Adding to library…",
};

export default function Store({
  lib,
  onLibrary,
  refetchLibrary,
  flash,
  query,
  onQuery,
  sort,
  onSort,
  zipOnly,
  onZipOnly,
  source,
  onSource,
  onCatalogMeta,
}: {
  lib: Library;
  onLibrary: (next: Library | null) => void;
  refetchLibrary: () => Promise<Library | null>;
  flash: (msg: string) => void;
  query: string;
  onQuery: (value: string) => void;
  sort: StoreSort;
  onSort: (value: StoreSort) => void;
  zipOnly: boolean;
  onZipOnly: (value: boolean) => void;
  source: StoreDelivery;
  onSource: (value: StoreDelivery) => void;
  onCatalogMeta?: (meta: { counts: DeliveryCounts; installable: number }) => void;
}) {
  const [catalog, setCatalog] = useState<StoreCatalog | null>(null);
  const [catalogError, setCatalogError] = useState<string | null>(null);
  const [catalogLoading, setCatalogLoading] = useState(true);
  const [page, setPage] = useState(1);
  const [progress, setProgress] = useState<Record<string, StoreProgress>>({});
  const [installError, setInstallError] = useState<string | null>(null);
  const [detailsGame, setDetailsGame] = useState<StoreGame | null>(null);
  const [showConsent, setShowConsent] = useState(false);
  const [pendingMagnet, setPendingMagnet] = useState<StoreGame | null>(null);
  const [repairing, setRepairing] = useState(false);

  const catalogRef = useRef<StoreCatalog | null>(null);
  useEffect(() => {
    catalogRef.current = catalog;
  }, [catalog]);

  const installed = useMemo(() => new Set(lib.games.map((g) => g.store_id).filter(Boolean) as string[]), [lib.games]);
  const busyId = useMemo(
    () => Object.entries(progress).find(([, p]) => ACTIVE_PHASES.has(p.phase))?.[0] ?? null,
    [progress],
  );

  const nameFor = (gid: string) => catalogRef.current?.games.find((g) => g.id === gid)?.name ?? "game";

  useEffect(() => {
    let live = true;
    (async () => {
      const cat = await invoke<StoreCatalog>("store_catalog")
        .then((v) => {
          setCatalog(v);
          return v;
        })
        .catch((e) => {
          setCatalogError(String(e));
          return null;
        });
      if (!live) return;
      setCatalogLoading(false);
      if (cat && cat.games.length === 0) setCatalogError("The store catalog is empty right now — try refreshing later.");
    })();
    const unlisten = listen<StoreProgress>("store-event", (e) => {
      const p = e.payload;
      if (!p?.game_id) return;
      setProgress((prev) => ({ ...prev, [p.game_id]: p }));
      if (p.phase === "done") {
        flash(`Installed ${nameFor(p.game_id)}`);
        void refetchLibrary();
      }
      if (p.phase === "error" && p.error) setInstallError(p.error);
    });
    return () => {
      live = false;
      unlisten.then((u) => u()).catch(() => {});
    };
  }, []);

  const filtered = useMemo(() => {
    let list = catalog?.games ?? [];
    const q = query.trim().toLowerCase();
    if (q) list = list.filter((g) => g.name.toLowerCase().includes(q));
    if (zipOnly) list = list.filter(isGameInstallable);
    if (source === "preinstalled") list = list.filter((g) => !isMagnet(g.uri));
    if (source === "torrent") list = list.filter((g) => isMagnet(g.uri));
    const sorted = [...list];
    sorted.sort((a, b) => {
      const grp = (isMagnet(a.uri) ? 1 : 0) - (isMagnet(b.uri) ? 1 : 0);
      if (grp !== 0) return grp;
      return sort === "name" ? a.name.localeCompare(b.name) : sort === "size" ? b.size_mb - a.size_mb : b.uploaded - a.uploaded;
    });
    return sorted;
  }, [catalog, query, zipOnly, source, sort]);

  useEffect(() => {
    if (!catalog) return;
    let torrent = 0;
    for (const g of catalog.games) {
      if (isMagnet(g.uri)) torrent++;
    }
    onCatalogMeta?.({
      counts: { preinstalled: catalog.games.length - torrent, torrent },
      installable: catalog.games.filter(isGameInstallable).length,
    });
  }, [catalog, onCatalogMeta]);

  const pageCount = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE));
  const safePage = Math.min(page, pageCount);
  const pageItems = filtered.slice((safePage - 1) * PAGE_SIZE, safePage * PAGE_SIZE);

  useEffect(() => {
    setPage(1);
  }, [query, sort, zipOnly, source, catalog]);

  const startInstall = async (g: StoreGame) => {
    setInstallError(null);
    try {
      await invoke("store_install", {
        gameId: g.id,
        name: g.name,
        uri: g.uri,
        ext: g.ext,
        sizeBytes: g.size_mb > 0 ? Math.round(g.size_mb * 1048576) : null,
      });
      setProgress((prev) => ({ ...prev, [g.id]: { game_id: g.id, phase: "resolving" } }));
    } catch (e) {
      setInstallError(String(e));
    }
  };

  const consentAccepted = (() => {
    try {
      return localStorage.getItem(CONSENT_KEY) === "1";
    } catch {
      return false;
    }
  })();

  const install = (g: StoreGame) => {
    if (!inTauri()) {
      flash("Run the desktop app to install from the store");
      return;
    }
    if (isMagnet(g.uri) && !consentAccepted) {
      setPendingMagnet(g);
      setShowConsent(true);
      return;
    }
    void startInstall(g);
  };

  const acceptConsent = () => {
    try {
      localStorage.setItem(CONSENT_KEY, "1");
    } catch {
    }
    setShowConsent(false);
    const g = pendingMagnet;
    setPendingMagnet(null);
    if (g) void startInstall(g);
  };

  const declineConsent = () => {
    setShowConsent(false);
    setPendingMagnet(null);
  };

  const repairInstalls = async () => {
    if (!inTauri() || repairing) return;
    setRepairing(true);
    setInstallError(null);
    try {
      const names = await invoke<string[]>("store_repair");
      flash(
        names.length
          ? `Unpacked and re-registered ${names.length} install${names.length > 1 ? "s" : ""}: ${names.join(", ")}`
          : "Every install is already unpacked — nothing to fix",
      );
      void refetchLibrary();
    } catch (e) {
      setInstallError(String(e));
    } finally {
      setRepairing(false);
    }
  };

  const cancelInstall = async () => {
    try {
      await invoke("store_cancel");
    } catch {
    }
  };

  const uninstall = async (g: StoreGame) => {
    try {
      const next = await invoke<Library>("store_uninstall", { storeId: g.id });
      onLibrary(next);
      flash("Game uninstalled (files removed)");
    } catch (e) {
      setInstallError(String(e));
    }
  };

  const total = catalog?.games.length ?? 0;

  return (
    <div className="stage">
      <header className="stage-head">
        <div className="stage-titles">
          <h1>Store</h1>
          <span className="stage-count">
            {filtered.length.toLocaleString()}
            {filtered.length !== total ? ` of ${total.toLocaleString()}` : ""} titles
          </span>
        </div>
        <div className="stage-tools">
          <div className="field search wide">
            <Icon name="search" size={13} />
            <input
              placeholder="Search the store…"
              value={query}
              onChange={(e) => onQuery(e.target.value)}
              spellCheck={false}
            />
          </div>
          <div className="select">
            <select value={sort} onChange={(e) => onSort(e.target.value as StoreSort)} title="Sort store">
              <option value="newest">Sort: Newest</option>
              <option value="name">Sort: Name</option>
              <option value="size">Sort: Size</option>
            </select>
            <Icon name="chevron-down" size={13} />
          </div>
          <button
            className="btn sm"
            onClick={() => void repairInstalls()}
            disabled={repairing || !inTauri()}
            title="Unpack archives still bundled inside installed games and re-register them"
          >
            {repairing ? <span className="spinner" /> : <Icon name="refresh" size={13} />} Fix installs
          </button>
        </div>
      </header>

      <div className="stage-body">
        {catalogError && (
          <div className="banner error">
            <Icon name="alert" size={14} />
            <span>{catalogError}</span>
            <button
              onClick={() => {
                setCatalogError(null);
                setCatalogLoading(true);
                invoke<StoreCatalog>("store_catalog")
                  .then(setCatalog)
                  .catch((e) => setCatalogError(String(e)))
                  .finally(() => setCatalogLoading(false));
              }}
            >
              <Icon name="x" size={13} />
            </button>
          </div>
        )}
        {installError && (
          <div className="banner error">
            <Icon name="alert" size={14} />
            <span>{installError}</span>
            <button onClick={() => setInstallError(null)} aria-label="Dismiss">
              <Icon name="x" size={13} />
            </button>
          </div>
        )}

        {catalogLoading && (
          <div className="loading-block">
            <span className="spinner big" /> Loading the store…
          </div>
        )}

        {!catalogLoading && filtered.length === 0 && (
          <div className="blank">
            <h2>No titles match</h2>
            <p>
              {query
                ? `Nothing in the catalog matches “${query}”. Try a shorter search or clear the filters in the left panel.`
                : "The store catalog could not be loaded. Check your connection and try again — some sources may ask you to complete a quick check in a pop-up window."}
            </p>
            <div className="row">
              {query && (
                <button className="btn" onClick={() => onQuery("")}>
                  Clear search
                </button>
              )}
              {source !== "all" && (
                <button className="btn" onClick={() => onSource("all")}>
                  All titles
                </button>
              )}
              {zipOnly && (
                <button className="btn" onClick={() => onZipOnly(false)}>
                  Include all titles
                </button>
              )}
            </div>
          </div>
        )}

        {!catalogLoading && filtered.length > 0 && (
          <div className="store-tiles">
            {pageItems.map((g) => {
              const p = progress[g.id];
              const isActive = p ? ACTIVE_PHASES.has(p.phase) : false;
              const isInstalled = installed.has(g.id);
              const magnet = isMagnet(g.uri);
              const installable = magnet || g.ext === "zip";
              const pct = p && p.downloaded != null && p.total ? Math.min(100, Math.round((p.downloaded / p.total) * 100)) : null;
              const eta =
                pct != null && p?.bps && p.total && p.downloaded != null
                  ? fmtEta((p.total - p.downloaded) / p.bps)
                  : null;
              const disabledButton = !installable || (busyId != null && busyId !== g.id);
              const buttonTitle = magnet
                ? "Torrent install — downloads and unpacks fully in-app"
                : installable
                  ? "Downloads and installs automatically in the background"
                  : "This game ships as a RAR archive — open its page to grab it manually";

              return (
                <article
                  key={g.id}
                  className="stcard"
                  onClick={() => setDetailsGame(g)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter" || event.key === " ") {
                      event.preventDefault();
                      setDetailsGame(g);
                    }
                  }}
                  role="button"
                  tabIndex={0}
                  aria-label={`${g.name} details`}
                >
                  <StoreArt game={g} className="stcard-art" />
                  <div className="stcard-body">
                    <div className="stcard-name" title={g.name}>
                      {g.name}
                    </div>
                    <div className="stcard-meta">
                      <span>{g.size_label || "—"}</span>
                    </div>

                    {p?.error && (
                      <div className="stcard-error" title={p.error}>
                        {p.error}
                      </div>
                    )}

                    {isActive ? (
                      <div className="stcard-progress">
                        <div className="bar">
                          <i style={{ width: `${pct ?? 0}%` }} />
                        </div>
                        <div className="row">
                          <span>{PHASE_LABEL[p.phase] ?? "Working…"}</span>
                          {p.phase === "downloading" && p.downloaded != null && (
                            <span>
                              {pct != null ? `${pct}%` : ""}
                              {p.bps ? ` · ${fmtSpeed(p.bps)}` : ""}
                              {eta ? ` · ${eta} left` : ""}
                            </span>
                          )}
                        </div>
                        <button className="btn sm wide" onClick={(event) => { event.stopPropagation(); void cancelInstall(); }}>
                          <Icon name="x" size={12} /> Cancel
                        </button>
                      </div>
                    ) : isInstalled ? (
                      <button
                        className="btn sm wide"
                        onClick={(event) => {
                          event.stopPropagation();
                          void uninstall(g);
                        }}
                        title="Uninstall (removes the downloaded files)"
                      >
                        <Icon name="check" size={13} /> Installed — Uninstall
                      </button>
                    ) : (
                      <div className="stcard-actions">
                        <button
                          className="btn-play sm"
                          disabled={disabledButton}
                          onClick={(event) => {
                            event.stopPropagation();
                            install(g);
                          }}
                          title={buttonTitle}
                        >
                          <Icon name="download" size={13} /> {magnet ? "Torrent install" : installable ? "Install" : "RAR only"}
                        </button>
                        {!installable && (
                          <button
                            className="btn sm"
                            onClick={(event) => {
                              event.stopPropagation();
                              openStorePage(g.name);
                            }}
                            title="Open the game's Steam page"
                          >
                            <Icon name="external" size={12} />
                          </button>
                        )}
                      </div>
                    )}
                  </div>
                </article>
              );
            })}
          </div>
        )}

        {pageCount > 1 && (
          <div className="pager">
            <button disabled={safePage <= 1} onClick={() => setPage(safePage - 1)} aria-label="Previous page">
              ‹
            </button>
            {(() => {
              const nums: ReactNode[] = [];
              const end = Math.min(pageCount, Math.max(1, safePage - 2) + 4);
              for (let n = Math.max(1, end - 4); n <= end; n++) {
                nums.push(
                  <button key={n} className={n === safePage ? "on" : ""} onClick={() => setPage(n)}>
                    {n}
                  </button>,
                );
              }
              return nums;
            })()}
            <button disabled={safePage >= pageCount} onClick={() => setPage(safePage + 1)} aria-label="Next page">
              ›
            </button>
            <span className="total">{filtered.length.toLocaleString()} titles</span>
          </div>
        )}
      </div>

      {detailsGame && <StoreGameDetails game={detailsGame} onClose={() => setDetailsGame(null)} />}

      {showConsent && (
        <div className="modal-backdrop" onMouseDown={declineConsent}>
          <section
            className="modal"
            role="dialog"
            aria-modal="true"
            aria-label="Enable torrent installs"
            onMouseDown={(event) => event.stopPropagation()}
          >
            <header className="modal-head">
              <div>
                <h2>Enable torrent installs?</h2>
                <div className="sub">Peer-to-peer downloads</div>
              </div>
            </header>
            <div className="modal-body">
              <p className="modal-note">
                Some titles ship as BitTorrent magnets. The launcher downloads them itself with a built-in open-source
                torrent engine.
              </p>
              <ul className="consent-list">
                <li>Fetching from torrent swarms connects your PC to other peers. Your IP is visible to those peers while downloading.</li>
                <li>Downloading files you have not acquired legally may be illegal where you live — you are responsible for what you download.</li>
                <li>The launcher stops sharing once a download finishes (no permanent seeding).</li>
              </ul>
            </div>
            <div className="modal-foot">
              <button className="btn" onClick={declineConsent}>
                Not now
              </button>
              <button className="btn-play" onClick={acceptConsent} autoFocus>
                <Icon name="download" size={13} /> I understand — enable
              </button>
            </div>
          </section>
        </div>
      )}
    </div>
  );
}
