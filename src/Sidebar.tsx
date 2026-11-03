import { Icon } from "./icons";
import type { DeliveryCounts, Library, StoreDelivery } from "./types";
import type { AppTab } from "./TitleBar";

export type Filter = "all" | "favorites" | "recent";

interface Props {
  tab: AppTab;
  lib: Library;
  filter: Filter;
  onFilter: (filter: Filter) => void;

  storeSource: StoreDelivery;
  onStoreSource: (source: StoreDelivery) => void;
  deliveryCounts: DeliveryCounts | null;
  installableCount: number | null;
  installableOnly: boolean;
  onInstallableOnly: (value: boolean) => void;

  onAddFolder: () => void;
  onAddGame: () => void;
  onRescan: () => void;
  onOpenFolder: (path: string) => void;
  onRemoveFolder: (path: string) => void;
}

export default function Sidebar({
  tab,
  lib,
  filter,
  onFilter,
  storeSource,
  onStoreSource,
  deliveryCounts,
  installableCount,
  installableOnly,
  onInstallableOnly,
  onAddFolder,
  onAddGame,
  onRescan,
  onOpenFolder,
  onRemoveFolder,
}: Props) {
  const favorites = lib.games.filter((g) => g.favorite).length;

  return (
    <aside className="rail">
      <div className="rail-scroll">
        {tab === "library" ? (
          <>
            <div className="rail-group">
              <div className="rail-head">
                <span className="mlabel">Library</span>
              </div>
              <button className={filter === "all" ? "rail-item on" : "rail-item"} onClick={() => onFilter("all")}>
                <Icon name="library" size={15} />
                <span>All Games</span>
                <b>{lib.games.length}</b>
              </button>
              <button className={filter === "favorites" ? "rail-item on" : "rail-item"} onClick={() => onFilter("favorites")}>
                <Icon name="star" size={15} />
                <span>Favorites</span>
                <b>{favorites}</b>
              </button>
              <button className={filter === "recent" ? "rail-item on" : "rail-item"} onClick={() => onFilter("recent")}>
                <Icon name="clock" size={15} />
                <span>Recently Played</span>
              </button>
            </div>

            <div className="rail-group">
              <div className="rail-head">
                <span className="mlabel">Scan Folders</span>
              </div>
              {lib.scan_folders.length === 0 && (
                <p className="rail-note">
                  No folders yet. Add one and every game <code>.exe</code> inside it is found automatically.
                </p>
              )}
              {lib.scan_folders.map((folder) => (
                <div key={folder} className="source-row" title={folder}>
                  <Icon name="folder" size={13} />
                  <span>{folder}</span>
                  <div className="folder-actions">
                    <button className="icon-btn" title="Open in Explorer" onClick={() => onOpenFolder(folder)}>
                      <Icon name="external" size={12} />
                    </button>
                    <button
                      className="icon-btn danger"
                      title="Stop scanning this folder (removes its scanned entries)"
                      onClick={() => onRemoveFolder(folder)}
                    >
                      <Icon name="x" size={12} />
                    </button>
                  </div>
                </div>
              ))}
            </div>
          </>
        ) : (
          <>
            <div className="rail-group">
              <div className="rail-head">
                <span className="mlabel">Catalog</span>
              </div>
              <button className={storeSource === "all" ? "rail-item on" : "rail-item"} onClick={() => onStoreSource("all")}>
                <Icon name="store" size={15} />
                <span>All Titles</span>
                {deliveryCounts != null && <b>{(deliveryCounts.preinstalled + deliveryCounts.torrent).toLocaleString()}</b>}
              </button>
              <button className={installableOnly ? "rail-item on" : "rail-item"} onClick={() => onInstallableOnly(!installableOnly)}>
                <Icon name="download" size={15} />
                <span>Installable Only</span>
                {installableCount != null && <b>{installableCount.toLocaleString()}</b>}
              </button>
            </div>

            <div className="rail-group">
              <div className="rail-head">
                <span className="mlabel">Install Type</span>
              </div>
              {deliveryCounts == null && <p className="rail-note">Loading the catalog…</p>}
              <button className={storeSource === "preinstalled" ? "rail-item on" : "rail-item"} onClick={() => onStoreSource("preinstalled")}>
                <Icon name="package" size={15} />
                <span>Pre-installed</span>
                {deliveryCounts != null && <b>{deliveryCounts.preinstalled.toLocaleString()}</b>}
              </button>
              <button className={storeSource === "torrent" ? "rail-item on" : "rail-item"} onClick={() => onStoreSource("torrent")}>
                <Icon name="magnet" size={15} />
                <span>Torrent installs</span>
                {deliveryCounts != null && <b>{deliveryCounts.torrent.toLocaleString()}</b>}
              </button>
            </div>
          </>
        )}
      </div>

      {tab === "library" && (
        <div className="rail-foot">
          <button className="btn ghost sm wide" onClick={onAddFolder}>
            <Icon name="plus" size={13} /> Add Scan Folder
          </button>
          <button className="btn ghost sm wide" onClick={onAddGame}>
            <Icon name="file" size={13} /> Add Game (.exe)
          </button>
          <button className="btn ghost sm wide" onClick={onRescan} disabled={!lib.scan_folders.length}>
            <Icon name="refresh" size={13} /> Rescan Folders
          </button>
        </div>
      )}
    </aside>
  );
}
