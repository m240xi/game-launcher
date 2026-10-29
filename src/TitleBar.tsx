import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Icon } from "./icons";

const inTauri = () => "__TAURI_INTERNALS__" in window;

const winProps = {
  width: 10,
  height: 10,
  viewBox: "0 0 10 10",
  stroke: "currentColor",
  strokeWidth: 1,
  fill: "none",
} as const;

export default function TitleBar() {
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    if (!inTauri()) return;
    const win = getCurrentWindow();
    win.isMaximized().then(setMaximized).catch(() => {});
    let unlisten: (() => void) | undefined;
    win
      .onResized(() => {
        win.isMaximized().then(setMaximized).catch(() => {});
      })
      .then((u) => {
        unlisten = u;
      })
      .catch(() => {});
    return () => unlisten?.();
  }, []);

  const minimize = () => {
    if (inTauri()) getCurrentWindow().minimize().catch(() => {});
  };
  const toggleMaximize = () => {
    if (inTauri()) getCurrentWindow().toggleMaximize().catch(() => {});
  };
  const close = () => {
    if (inTauri()) getCurrentWindow().close().catch(() => {});
  };

  return (
    <header className="titlebar" data-tauri-drag-region>
      <div className="tb-brand" data-tauri-drag-region>
        <span className="tb-mark" data-tauri-drag-region>
          <Icon name="play" size={9} fill />
        </span>
        <span className="tb-title" data-tauri-drag-region>
          Hate.bio <span data-tauri-drag-region>Launcher</span>
        </span>
      </div>
      <div className="tb-spacer" data-tauri-drag-region />

      <button className="tb-btn" title="Minimize" aria-label="Minimize" onClick={minimize}>
        <svg {...winProps} shapeRendering="crispEdges">
          <path d="M0 5.5h10" />
        </svg>
      </button>
      <button
        className="tb-btn"
        title={maximized ? "Restore" : "Maximize"}
        aria-label={maximized ? "Restore" : "Maximize"}
        onClick={toggleMaximize}
      >
        {maximized ? (
          <svg {...winProps} shapeRendering="crispEdges">
            <path d="M2.5 2.5V0.5H9.5V7.5H7.5" />
            <rect x="0.5" y="2.5" width="7" height="7" />
          </svg>
        ) : (
          <svg {...winProps} shapeRendering="crispEdges">
            <rect x="0.5" y="0.5" width="9" height="9" />
          </svg>
        )}
      </button>
      <button className="tb-btn tb-close" title="Close" aria-label="Close" onClick={close}>
        <svg {...winProps} shapeRendering="geometricPrecision">
          <path d="M0 0l10 10M10 0L0 10" />
        </svg>
      </button>
    </header>
  );
}
