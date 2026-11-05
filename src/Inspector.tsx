import { useEffect, useState } from "react";
import type { Game } from "./types";
import { Icon } from "./icons";
import GameArt from "./GameArt";
import { fmtAgo, fmtPlaytime, fmtSize, sourceLabel } from "./format";

interface Props {
  game: Game;
  renaming: boolean;
  onRenameStart: () => void;
  onRenameCancel: () => void;
  onClose: () => void;
  onPlay: (game: Game, args?: string) => void;
  onRename: (game: Game, name: string) => void;
  onToggleFavorite: (game: Game) => void;
  onOpenFolder: (path: string) => void;
  onReveal: (path: string) => void;
  onRemove: (game: Game) => void;
  onUninstall: (game: Game) => void;
}

export default function Inspector({
  game,
  renaming,
  onRenameStart,
  onRenameCancel,
  onClose,
  onPlay,
  onRename,
  onToggleFavorite,
  onOpenFolder,
  onReveal,
  onRemove,
  onUninstall,
}: Props) {
  const [args, setArgs] = useState("");
  const [draft, setDraft] = useState(game.name);

  useEffect(() => {
    setDraft(game.name);
  }, [game.id, game.name]);

  return (
    <aside className="inspector">
      <header className="insp-head">
        <div className="insp-art">
          <GameArt game={game} variant="icon" />
        </div>
        <div className="insp-titles">
          {renaming ? (
            <form
              onSubmit={(event) => {
                event.preventDefault();
                const next = draft.trim();
                if (next && next !== game.name) onRename(game, next);
                onRenameCancel();
              }}
            >
              <input
                className="insp-rename"
                value={draft}
                onChange={(event) => setDraft(event.target.value)}
                autoFocus
                onFocus={(event) => event.currentTarget.select()}
              />
              <div className="insp-rename-row">
                <button className="btn-play sm" type="submit">
                  Save
                </button>
                <button className="btn sm" type="button" onClick={onRenameCancel}>
                  Cancel
                </button>
              </div>
            </form>
          ) : (
            <>
              <h2>{game.name}</h2>
              <div className="insp-sub">{sourceLabel(game.source)}</div>
            </>
          )}
        </div>
        <button className="icon-btn insp-close" title="Close details" aria-label="Close details" onClick={onClose}>
          <Icon name="x" size={14} />
        </button>
      </header>

      <div className="insp-body">
        <button className="btn-play lg wide" onClick={() => onPlay(game)}>
          <Icon name="play" size={13} fill /> PLAY
        </button>

        <div className="insp-args">
          <span className="mlabel">Launch Options</span>
          <input
            placeholder="-windowed -novid"
            value={args}
            onChange={(event) => setArgs(event.target.value)}
            spellCheck={false}
          />
          {args.trim() && (
            <button className="btn wide" onClick={() => onPlay(game, args.trim())}>
              <Icon name="play" size={11} fill /> Play with options
            </button>
          )}
        </div>

        <section className="insp-section">
          <span className="mlabel">Item Details</span>
          <dl className="props">
            <div>
              <dt>Last played</dt>
              <dd>{fmtAgo(game.last_played)}</dd>
            </div>
            <div>
              <dt>Playtime</dt>
              <dd>{fmtPlaytime(game.playtime_secs)}</dd>
            </div>
            <div>
              <dt>Times played</dt>
              <dd>{game.play_count}</dd>
            </div>
            <div>
              <dt>Size</dt>
              <dd>{fmtSize(game.size_bytes) || <em>Unknown</em>}</dd>
            </div>
            <div>
              <dt>Added via</dt>
              <dd>{sourceLabel(game.source)}</dd>
            </div>
            <div>
              <dt>Location</dt>
              <dd className="path">{game.exe_path}</dd>
            </div>
          </dl>
        </section>

        <section className="insp-section">
          <span className="mlabel">Actions</span>
          <button className="act-row" onClick={() => onToggleFavorite(game)}>
            <Icon name="star" size={14} fill={game.favorite} />
            <span>{game.favorite ? "Remove from favorites" : "Add to favorites"}</span>
          </button>
          <button className="act-row" onClick={onRenameStart}>
            <Icon name="pencil" size={14} />
            <span>Rename</span>
          </button>
          <button className="act-row" onClick={() => onOpenFolder(game.folder)}>
            <Icon name="folder" size={14} />
            <span>Open game folder</span>
          </button>
          <button className="act-row" onClick={() => onReveal(game.exe_path)}>
            <Icon name="external" size={14} />
            <span>Show in Explorer</span>
          </button>
          {game.store_id ? (
            <button className="act-row danger" onClick={() => onUninstall(game)}>
              <Icon name="trash" size={14} />
              <span>Uninstall</span>
            </button>
          ) : (
            <button className="act-row danger" onClick={() => onRemove(game)}>
              <Icon name="trash" size={14} />
              <span>Remove from library</span>
            </button>
          )}
        </section>

        <p className="insp-note">
          {game.store_id
            ? "Uninstall deletes the downloaded game files and removes the entry from your library."
            : "Removing only removes the entry from the launcher — your game files are never touched. Double-click any title to play it instantly."}
        </p>
      </div>
    </aside>
  );
}
