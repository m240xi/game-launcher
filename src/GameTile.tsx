import type { MouseEvent } from "react";
import type { Game } from "./types";
import { Icon } from "./icons";
import GameArt from "./GameArt";
import { fmtAgo, fmtPlaytime, fmtSize, gameMeta, sourceLabel } from "./format";

interface Common {
  game: Game;
  selected: boolean;
  onSelect: (game: Game) => void;
  onPlay: (game: Game) => void;
  onContextMenu: (event: MouseEvent, game: Game) => void;
  onToggleFavorite: (game: Game) => void;
}

export function GameTile({ game, selected, onSelect, onPlay, onContextMenu, onToggleFavorite }: Common) {
  return (
    <div
      className={selected ? "tile on" : "tile"}
      onClick={() => onSelect(game)}
      onDoubleClick={() => onPlay(game)}
      onContextMenu={(event) => onContextMenu(event, game)}
      title={game.name}
    >
      <div className="tile-art">
        <GameArt game={game} variant="cover" />
        <button
          className={game.favorite ? "tile-fav on" : "tile-fav"}
          title={game.favorite ? "Remove from favorites" : "Add to favorites"}
          onClick={(event) => {
            event.stopPropagation();
            onToggleFavorite(game);
          }}
        >
          <Icon name="star" size={13} fill={game.favorite} />
        </button>
        <div className="tile-hover">
          <button
            className="btn-play"
            onClick={(event) => {
              event.stopPropagation();
              onPlay(game);
            }}
          >
            <Icon name="play" size={10} fill /> Play
          </button>
        </div>
      </div>
      <div className="tile-name">{game.name}</div>
      <div className="tile-meta">{gameMeta(game)}</div>
    </div>
  );
}

export function GameRow({ game, selected, onSelect, onPlay, onContextMenu }: Omit<Common, "onToggleFavorite">) {
  return (
    <div
      className={selected ? "lrow on" : "lrow"}
      onClick={() => onSelect(game)}
      onDoubleClick={() => onPlay(game)}
      onContextMenu={(event) => onContextMenu(event, game)}
    >
      <div className="lrow-main">
        <div className="lrow-art">
          <GameArt game={game} variant="icon" />
        </div>
        <div className="lrow-name" title={game.name}>
          {game.name}
          {game.favorite && <Icon name="star" size={11} fill />}
        </div>
      </div>
      <div className="lrow-cell">
        {fmtAgo(game.last_played)}
        {game.playtime_secs > 0 ? ` · ${fmtPlaytime(game.playtime_secs)}` : ""}
      </div>
      <div className="lrow-cell col-size">{game.size_bytes != null ? fmtSize(game.size_bytes) : "—"}</div>
      <div className="lrow-cell col-src">{sourceLabel(game.source)}</div>
      <div className="lrow-act">
        <button
          className="btn-play sm"
          onClick={(event) => {
            event.stopPropagation();
            onPlay(game);
          }}
        >
          <Icon name="play" size={10} fill /> Play
        </button>
      </div>
    </div>
  );
}

export function ContinueCard({
  game,
  onSelect,
  onPlay,
}: {
  game: Game;
  onSelect: (game: Game) => void;
  onPlay: (game: Game) => void;
}) {
  return (
    <div className="ccard" onClick={() => onSelect(game)} onDoubleClick={() => onPlay(game)} title={game.name}>
      <div className="ccard-art">
        <GameArt game={game} variant="icon" />
      </div>
      <div className="ccard-body">
        <div className="ccard-name">{game.name}</div>
        <div className="ccard-meta">Played {fmtAgo(game.last_played).replace(/^Played /, "")}</div>
      </div>
      <button
        className="btn-play sm"
        onClick={(event) => {
          event.stopPropagation();
          onPlay(game);
        }}
      >
        <Icon name="play" size={10} fill />
      </button>
    </div>
  );
}
