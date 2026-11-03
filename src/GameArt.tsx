import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Game } from "./types";

const inTauri = () => "__TAURI_INTERNALS__" in window;

const steamArtCache = new Map<string, string>();
const steamArtRequests = new Map<string, Promise<string | null>>();

export function loadSteamArt(gameId: string, title: string): Promise<string | null> {
  const key = `${gameId}:${title}`;
  const cached = steamArtCache.get(key);
  if (cached) return Promise.resolve(cached);
  const pending = steamArtRequests.get(key);
  if (pending) return pending;
  const request = invoke<string | null>("get_game_art", { gameId })
    .then((image) => {
      if (image) steamArtCache.set(key, image);
      return image;
    })
    .catch(() => null)
    .finally(() => steamArtRequests.delete(key));
  steamArtRequests.set(key, request);
  return request;
}

const PLACEHOLDER_GRADIENTS = [
  ["#243447", "#131922"],
  ["#2d3a30", "#141b16"],
  ["#3a2f3f", "#1a1520"],
  ["#3a3327", "#1b1813"],
  ["#273a3a", "#121b1b"],
  ["#3b2b2b", "#1c1414"],
  ["#2f2f42", "#15151f"],
  ["#23384a", "#101820"],
];

export function placeholderGradient(seed: string) {
  let hash = 0;
  for (let i = 0; i < seed.length; i++) hash = (hash * 31 + seed.charCodeAt(i)) >>> 0;
  const pair = PLACEHOLDER_GRADIENTS[hash % PLACEHOLDER_GRADIENTS.length];
  return `linear-gradient(145deg, ${pair[0]}, ${pair[1]})`;
}

export default function GameArt({
  game,
  variant = "cover",
}: {
  game: Game;
  variant?: "cover" | "icon";
}) {
  const [node, setNode] = useState<Element | null>(null);
  const [near, setNear] = useState(false);
  const [steamArt, setSteamArt] = useState<string | null>(null);
  const [icon, setIcon] = useState<string | null>(null);
  const [iconFailed, setIconFailed] = useState(false);

  useEffect(() => {
    if (!node) return;
    if (!("IntersectionObserver" in window)) {
      setNear(true);
      return;
    }
    const observer = new IntersectionObserver(
      ([entry]) => {
        if (entry.isIntersecting) {
          setNear(true);
          observer.disconnect();
        }
      },
      { rootMargin: "200px" },
    );
    observer.observe(node);
    return () => observer.disconnect();
  }, [node]);

  useEffect(() => {
    let live = true;
    setSteamArt(null);
    setIcon(null);
    setIconFailed(false);
    if (!near || !inTauri()) return;
    void loadSteamArt(game.id, game.name).then((image) => {
      if (live && image) setSteamArt(image);
    });
    invoke<string | null>("get_icon", { exePath: game.exe_path })
      .then((value) => {
        if (!live) return;
        if (value) setIcon(value);
        else setIconFailed(true);
      })
      .catch(() => {
        if (live) setIconFailed(true);
      });
    return () => {
      live = false;
    };
  }, [near, game.id, game.name, game.exe_path]);

  const letter = game.name.trim().charAt(0).toUpperCase() || "?";

  if (variant === "cover" && steamArt) {
    return <img ref={setNode} className="art-cover" src={steamArt} alt="" draggable={false} />;
  }
  if (icon && !iconFailed) {
    return (
      <img
        ref={setNode}
        className="art-icon"
        src={icon}
        alt=""
        draggable={false}
        onError={() => setIconFailed(true)}
      />
    );
  }
  if (variant === "icon" && steamArt) {
    return <img ref={setNode} className="art-cover" src={steamArt} alt="" draggable={false} />;
  }
  return (
    <span ref={setNode} className="art-letter" style={{ background: placeholderGradient(game.name) }}>
      {letter}
    </span>
  );
}
