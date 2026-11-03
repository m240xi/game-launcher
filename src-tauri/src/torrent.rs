
use std::path::Path;
use std::sync::atomic::AtomicBool;
#[cfg(any(test, feature = "torrent"))]
use std::sync::atomic::Ordering;
#[cfg(feature = "torrent")]
use std::time::{Duration, Instant};

use tauri::AppHandle;
#[cfg(feature = "torrent")]
use tauri::Emitter;

#[cfg(feature = "torrent")]
use crate::store::StoreEvent;

#[cfg(feature = "torrent")]
fn emit(app: &AppHandle, ev: StoreEvent) {
    let _ = app.emit("store-event", ev);
}

#[cfg(feature = "torrent")]
fn emit_phase(app: &AppHandle, gid: &str, phase: &str) {
    emit(app, StoreEvent { game_id: gid.into(), phase: phase.into(), downloaded: None, total: None, bps: None, error: None });
}

pub fn run_magnet_install(
    app: &AppHandle,
    game_id: &str,
    display_name: &str,
    magnet: &str,
    dest_root: &Path,
    cancel: &AtomicBool,
) -> Result<bool, String> {
    #[cfg(feature = "torrent")]
    {
        return run_magnet_install_real(app, game_id, display_name, magnet, dest_root, cancel);
    }
    #[cfg(not(feature = "torrent"))]
    {
        let _ = (app, game_id, display_name, magnet, dest_root, cancel);
        Err("This build has no torrent engine. Rebuild with --features torrent (adds librqbit + tokio). The magnet link itself is valid and can be opened in any torrent client.".into())
    }
}

#[cfg(feature = "torrent")]
fn run_magnet_install_real(
    app: &AppHandle,
    game_id: &str,
    _display_name: &str,
    magnet: &str,
    dest_root: &Path,
    cancel: &AtomicBool,
) -> Result<bool, String> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("Could not start torrent engine: {e}"))?;

    std::fs::create_dir_all(dest_root).map_err(|e| e.to_string())?;
    if !magnet.trim().to_lowercase().starts_with("magnet:") {
        return Err("Not a magnet link".into());
    }

    use librqbit::{AddTorrent, AddTorrentOptions, AddTorrentResponse, Session};

    let magnet_owned = magnet.to_string();
    let dest = dest_root.to_path_buf();

    let (session, handle) = rt.block_on(async {
        let session = Session::new(dest.clone())
            .await
            .map_err(|e| format!("Torrent session failed to start: {e}"))?;
        let resp = session
            .add_torrent(
                AddTorrent::from_url(magnet_owned),
                Some(AddTorrentOptions { overwrite: true, ..Default::default() }),
            )
            .await
            .map_err(|e| format!("Could not add torrent: {e}"))?;
        let h = match resp {
            AddTorrentResponse::Added(_, h) => h,
            AddTorrentResponse::AlreadyManaged(_, h) => h,
            AddTorrentResponse::ListOnly(_) => return Err("Torrent is list-only".into()),
        };
        Ok::<_, String>((session, h))
    })?;

    let mut last_emit = Instant::now();
    const EMIT_INTERVAL_MS: u128 = 400;

    loop {
        if cancel.load(Ordering::Relaxed) {
            let sid = session.clone();
            let hid = handle.clone();
            let _ = rt.block_on(async { sid.delete(librqbit::api::TorrentIdOrHash::Id(hid.id()), false).await });
            return Err("Canceled".into());
        }

        let stats = handle.stats();
        let downloaded = stats.progress_bytes;
        let total = if stats.total_bytes > 0 { Some(stats.total_bytes) } else { None };
        let finished = stats.finished;

        let bps = stats.live.as_ref().map(|l| l.download_speed.as_bytes()).unwrap_or(0);

        if last_emit.elapsed().as_millis() >= EMIT_INTERVAL_MS {
            if total.is_none() {
                emit(app, StoreEvent { game_id: game_id.into(), phase: "resolving".into(), downloaded: Some(downloaded), total, bps: Some(bps), error: None });
            } else {
                emit(app, StoreEvent { game_id: game_id.into(), phase: "downloading".into(), downloaded: Some(downloaded), total, bps: Some(bps), error: None });
            }
            last_emit = Instant::now();
        }

        if finished {
            let sid = session.clone();
            let hid = handle.clone();
            let _ = rt.block_on(async { sid.pause(&hid).await });
            emit_phase(app, game_id, "finishing");
            return Ok(true);
        }

        for _ in 0..4 {
            if cancel.load(Ordering::Relaxed) { break; }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancel_token_roundtrip() {
        let t = AtomicBool::new(false);
        assert!(!t.load(Ordering::Relaxed));
        t.store(true, Ordering::Relaxed);
        assert!(t.load(Ordering::Relaxed));
    }
}
