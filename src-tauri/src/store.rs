
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};

use crate::AppState;
use crate::catalog as cat;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
const CACHE_FILE: &str = "store-cache.json";
const CACHE_TTL_SECS: u64 = 12 * 3600;
const RESOLVE_HOST: &str = "dl-resolve.launcher";
const RESOLVE_LABEL: &str = "store-resolver";
const RESOLVE_TIMEOUT_SECS: u64 = 480;
const NATIVE_RESOLVE_GRACE_SECS: u64 = 60;
const HARVEST_LABEL: &str = "store-harvest";
const HARVEST_TIMEOUT_SECS: u64 = 180;
const HARVEST_CACHE_FILE: &str = "store-harvest-cache.json";
const EMIT_INTERVAL_MS: u128 = 200;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StoreGame {
    pub id: String,
    pub name: String,
    pub version: String,
    pub size_label: String,
    pub size_mb: f64,
    pub uploaded: u64,
    pub ext: String,
    pub page_url: String,
    pub uri: String,
    #[serde(default)]
    pub source: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StoreCatalog {
    pub fetched_at: u64,
    pub games: Vec<StoreGame>,
}

impl StoreCatalog {
    pub fn with_games(games: Vec<StoreGame>) -> Self {
        Self { fetched_at: cat::now_secs(), games }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct StoreEvent {
    pub game_id: String,
    pub phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downloaded: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bps: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl StoreEvent {
    fn phase(gid: &str, phase: &str) -> Self {
        StoreEvent { game_id: gid.into(), phase: phase.into(), downloaded: None, total: None, bps: None, error: None }
    }
}

pub struct StoreState {
    active: Mutex<Option<ActiveInstall>>,
}

struct ActiveInstall {
    cancel: Arc<AtomicBool>,
}

impl Default for StoreState {
    fn default() -> Self { StoreState { active: Mutex::new(None) } }
}

use crate::catalog::{HydraSource, RawCatalog};

#[cfg(test)]
fn parse_title(title: &str) -> (String, String) { cat::parse_title(title) }
#[cfg(test)]
fn parse_size_mb(label: &str) -> f64 { cat::parse_size_mb(label) }
#[cfg(test)]
fn parse_upload_date(date: &str) -> u64 { cat::parse_upload_date(date) }
fn split_uri(uri: &str) -> (String, String, String) { cat::split_uri(uri) }
#[cfg(test)]
fn extract_page_url(html: &str) -> String { cat::extract_page_url(html) }
fn now() -> u64 { cat::now_secs() }
#[cfg(test)]
fn build_catalog(raw: RawCatalog) -> StoreCatalog { cat::build_catalog(raw) }

fn cache_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join(CACHE_FILE))
}

fn hydra_sources() -> Vec<HydraSource> { cat::default_hydra_sources() }

fn fetch_one_source_blocking(source: &HydraSource) -> Result<StoreCatalog, String> {
    let resp = reqwest::blocking::Client::builder()
        .user_agent(UA)
        .timeout(Duration::from_secs(90))
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("Could not create the HTTP client: {e}"))?
        .get(&source.url)
        .send()
        .map_err(|e| format!("Could not reach {}: {e}", source.label))?;
    let is_cf = resp.status().as_u16() == 403;
    let mut body = String::new();
    if is_cf {
        return Err(format!("__cf__{}", source.url));
    }
    resp.error_for_status_ref().map_err(|e| format!("Could not reach {}: {e}", source.label))?;
    resp.take(96 * 1024 * 1024).read_to_string(&mut body).map_err(|e| e.to_string())?;
    let trimmed = body.trim_start();
    if !trimmed.starts_with('{') && !trimmed.starts_with('[') {
        return Err(format!("__cf__{}", source.url));
    }
    let raw: RawCatalog = serde_json::from_str(&body).map_err(|e| format!("Store catalog {} is malformed: {e}", source.label))?;
    Ok(cat::build_catalog_from_raw(raw, &source.label, cat::now_secs()))
}

fn fetch_source_capped(source: &HydraSource, cap: Duration) -> Result<StoreCatalog, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let src = source.clone();
    std::thread::spawn(move || {
        let _ = tx.send(fetch_one_source_blocking(&src));
    });
    match rx.recv_timeout(cap) {
        Ok(res) => res,
        Err(_) => Err(format!("__slow__{}: exceeded {:?}", source.label, cap)),
    }
}

fn fetch_all_sources_blocking(
    sources: &[HydraSource],
    fallback_json: &std::collections::HashMap<String, String>,
) -> (StoreCatalog, Vec<HydraSource>) {
    use std::sync::mpsc;
    const SOURCE_CAP: Duration = Duration::from_secs(120);

    let (tx, rx) = mpsc::channel::<(HydraSource, Result<StoreCatalog, String>)>();
    for src in sources {
        let tx = tx.clone();
        let src = src.clone();
        let fallback = fallback_json.clone();
        std::thread::spawn(move || {
            let one = match fetch_source_capped(&src, SOURCE_CAP) {
                Ok(c) => Ok(c),
                Err(e) => match fallback.get(&src.url) {
                    Some(json) => serde_json::from_str::<RawCatalog>(json)
                        .map(|raw| cat::build_catalog_from_raw(raw, &src.label, cat::now_secs()))
                        .map_err(|er| format!("Catalog {} harvested but malformed: {er}", src.label)),
                    None => Err(e),
                },
            };
            let _ = tx.send((src, one));
        });
    }
    drop(tx);

    let mut all_games: Vec<StoreGame> = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();
    let mut fetched_at = 0u64;
    let mut gated: Vec<HydraSource> = Vec::new();
    for (src, one) in rx {
        match one {
            Ok(mut cat) => {
                if cat.fetched_at > fetched_at { fetched_at = cat.fetched_at; }
                for g in cat.games.drain(..) {
                    if seen_ids.insert(g.id.clone()) { all_games.push(g); }
                }
            }
            Err(e) if e.starts_with("__cf__") => {
                crate::dlog!("[store] {} behind CF — will try the in-app harvester", src.label);
                gated.push(src);
            }
            Err(e) => crate::dlog!("[store] catalog source {} failed: {e}", src.label),
        }
    }
    all_games.sort_by(|a, b| b.uploaded.cmp(&a.uploaded));
    all_games.truncate(10_000);
    let ts = if fetched_at == 0 { cat::now_secs() } else { fetched_at };
    (StoreCatalog { fetched_at: ts, games: all_games }, gated)
}

fn fetch_catalog_blocking_with_harvested(
    harvested: &std::collections::HashMap<String, String>,
) -> Result<StoreCatalog, String> {
    let sources = hydra_sources();
    let (merged, _) = fetch_all_sources_blocking(&sources, harvested);
    if merged.games.is_empty() { return Err("All catalog sources failed or were blocked by Cloudflare. The cached store will be shown until a source is reachable.".into()); }
    Ok(merged)
}

async fn fetch_catalog_auto(app: &AppHandle) -> Result<StoreCatalog, String> {
    let mut map = load_harvest_cache(app);
    let map2 = map.clone();
    let (mut catalog, gated) = tauri::async_runtime::spawn_blocking(move || {
        fetch_all_sources_blocking(&hydra_sources(), &map2)
    })
    .await
    .map_err(|e| e.to_string())?;
    if !gated.is_empty() {
        let mut got = false;
        for src in &gated {
            match harvest_source_webview(app, src).await {
                Ok(json) => {
                    map.insert(src.url.clone(), json);
                    got = true;
                }
                Err(e) => crate::dlog!("[store] auto-harvest {} failed: {e}", src.label),
            }
        }
        if got {
            let (merged, _) = tauri::async_runtime::spawn_blocking(move || {
                fetch_all_sources_blocking(&hydra_sources(), &map)
            })
            .await
            .map_err(|e| e.to_string())?;
            if !merged.games.is_empty() {
                catalog = merged;
            }
        }
    }
    if catalog.games.is_empty() {
        return Err("All catalog sources failed or were blocked by Cloudflare. The cached store will be shown until a source is reachable.".into());
    }
    Ok(catalog)
}

fn slugify(name: &str, code: &str) -> String {
    let mut s = String::new();
    for ch in name.chars() {
        if ch.is_alphanumeric() || ch == ' ' { s.extend(ch.to_lowercase()); }
        else if ch == '-' || ch == '_' { s.push(' '); }
    }
    let mut s: String = s.split_whitespace().collect::<Vec<_>>().join("-");
    if s.len() > 48 { s.truncate(48); }
    if s.is_empty() { s = code.to_string(); }
    s
}

fn store_root(_app: &AppHandle) -> Result<PathBuf, String> {
    let base = dirs::document_dir()
        .ok_or_else(|| "Could not locate the Documents folder.".to_string())?;
    Ok(base.join("Hate.bio Launcher").join("Store Games"))
}

const RESOLVER_SCRIPT: &str = r##"(function () {
  if (window.__llResolver) return;
  window.__llResolver = true;
  try { window.close = function () {}; } catch (e) {}
  function report(u) {
    var b64 = btoa(unescape(encodeURIComponent(u)));
    try { location.href = 'https://dl-resolve.launcher/#' + encodeURIComponent(b64); } catch (e) {}
  }
  var of = window.fetch;
  if (of) {
    window.fetch = function (input, init) {
      var isDl = false;
      try {
        var h = init && init.headers;
        var m = h instanceof Headers ? h : (h ? new Headers(h) : new Headers());
        isDl = (init && init.method === 'POST' && (m.get('X-Dn-Dl') === '1' || m.get('x-dn-dl') === '1'));
      } catch (e) {}
      if (!isDl) return of.apply(this, arguments);
      return of.apply(this, arguments).then(function (resp) {
        try {
          return resp.clone().text().then(function (t) {
            var y;
            try { y = JSON.parse(t); } catch (e) { return resp.clone(); }
            if (y && y.url) {
              var direct;
              try { direct = decodeURIComponent(y.url); } catch (e) { direct = y.url; }
              report(direct);
              return new Response(JSON.stringify({ error: '' }), { status: 200, headers: { 'content-type': 'application/json' } });
            }
            return new Response(t, { status: resp.status, headers: { 'content-type': 'application/json' } });
          });
        } catch (e) { return resp.clone(); }
      });
    };
  }
  function visible(b) { return b && b.offsetParent !== null; }
  function born() { return performance.now(); }
  var firstSeen = 0;
  function clickGate() {
    var btns = Array.prototype.slice.call(document.querySelectorAll("#downloadForm button[type=submit], button#method_free"));
    var target = btns.find(function (b) { return /(continue|free download)/i.test((b.textContent || '').trim()); });
    if (!target) { firstSeen = 0; return; }
    if (!firstSeen) firstSeen = born();
    if (target.disabled && born() - firstSeen > 20000) {
      firstSeen = born();
      target.disabled = false;
    }
    try { target.click(); } catch (e) {}
  }
  function clickCountdown() {
    var btns = Array.prototype.slice.call(document.querySelectorAll("button, a"));
    var target = btns.filter(visible).find(function (b) { return /start download/i.test((b.textContent || "").trim()); });
    var main = btns.filter(visible).find(function (b) { return /free download standard/i.test((b.textContent || "").trim()); });
    target = target || main;
    if (target) { try { target.click(); } catch (e) {} }
  }
  window.setTimeout(function tick() {
    try {
      if (document.getElementById('downloadForm')) {
        clickGate();
      } else {
        firstSeen = 0;
        clickCountdown();
      }
    } catch (e) {}
    window.setTimeout(tick, 1200);
  }, 900);
})();
"##;

static RESOLVE_TX: Mutex<Option<mpsc::Sender<String>>> = Mutex::new(None);

fn hide_resolver(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(RESOLVE_LABEL) {
        let _ = w.hide();
    }
}

fn clear_resolver(app: &AppHandle) {
    hide_resolver(app);
    *RESOLVE_TX.lock().unwrap() = None;
}

pub fn ensure_resolver_window(app: &AppHandle) -> Result<tauri::WebviewWindow, String> {
    if let Some(w) = app.get_webview_window(RESOLVE_LABEL) {
        return Ok(w);
    }
    let url: tauri::Url = "https://datanodes.to/".parse().map_err(|e| format!("Bad link: {e}"))?;
    let window = WebviewWindowBuilder::new(app, RESOLVE_LABEL, WebviewUrl::External(url))
        .title("Preparing download — complete the check if asked")
        .inner_size(520.0, 680.0)
        .resizable(true)
        .visible(false)
        .initialization_script(RESOLVER_SCRIPT)
        .on_navigation(move |nav| {
            crate::dlog!("[store] nav: {}", nav.as_str().chars().take(110).collect::<String>());
            if nav.host_str() == Some(RESOLVE_HOST) {
                let raw = nav.fragment().unwrap_or("");
                let decoded = percent_decode(raw);
                let inner = base64::engine::general_purpose::STANDARD
                    .decode(decoded.as_bytes())
                    .ok()
                    .and_then(|b| String::from_utf8(b).ok())
                    .or_else(|| Some(decoded.clone()).filter(|s| s.starts_with("http")));
                if let Some(u) = inner {
                    if let Some(tx) = RESOLVE_TX.lock().unwrap().as_ref() {
                        let _ = tx.send(u);
                    }
                }
                return false;
            }
            true
        })
        .build()
        .map_err(|e| format!("Could not open the download preparation window: {e}"))?;
    window.on_window_event(|e| match e {
        tauri::WindowEvent::CloseRequested { .. } => crate::dlog!("[store] resolver close-requested"),
        tauri::WindowEvent::Destroyed => crate::dlog!("[store] resolver destroyed"),
        _ => {}
    });
    Ok(window)
}

const HARVEST_SCRIPT: &str = r##"(function () {
  if (window.__llHarvest) return;
  window.__llHarvest = true;
  try { window.close = function () {}; } catch (e) {}
  function report(t) {
    var b64 = btoa(unescape(encodeURIComponent(t)));
    try { location.href = 'https://dl-resolve.launcher/#' + encodeURIComponent(b64); } catch (e) {}
  }
  function looksJson(s) { return s.startsWith('{') || s.startsWith('['); }
  function inspect() {
    try {
      var t = document.body ? document.body.innerText : '';
      t = (t || '').trim();
      if (looksJson(t)) { report(t); return true; }
      var pre = document.querySelector('pre');
      if (pre && pre.innerText) {
        var pt = pre.innerText.trim();
        if (looksJson(pt)) { report(pt); return true; }
      }
    } catch (e) {}
    return false;
  }
  window.setTimeout(function tick() {
    if (!inspect()) window.setTimeout(tick, 500);
  }, 400);
})();
"##;

fn close_harvester(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(HARVEST_LABEL) {
        let _ = w.close();
    }
}

fn emit(app: &AppHandle, ev: StoreEvent) {
    let _ = app.emit("store-event", ev);
}

pub fn is_magnet_uri(uri: &str) -> bool {
    uri.trim().to_lowercase().starts_with("magnet:")
}

fn sleep_cancelable(secs: u64, cancel: &AtomicBool) -> Result<(), String> {
    for _ in 0..(secs * 10) {
        if cancel.load(Ordering::Relaxed) {
            return Err("Canceled".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

fn safe_local_name(name: &str) -> String {
    let mapped: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || " ._-()[]+".contains(c) { c } else { '_' })
        .collect();
    let trimmed = mapped.trim_start_matches(['.', ' ']).trim_end_matches(['.', ' ']);
    let capped: String = trimmed.chars().take(120).collect();
    if capped.is_empty() { "download".to_string() } else { capped }
}

fn url_safe_name(name: &str) -> String {
    fn safe(b: u8) -> bool { b.is_ascii_alphanumeric() || b"-._~".contains(&b) }
    if name.bytes().all(|b| safe(b) || b == b'%') {
        return name.to_string();
    }
    name.bytes()
        .map(|b| if safe(b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

fn resolve_gate(app: &AppHandle, code: &str, fname: &str, cancel: &AtomicBool) -> Result<String, String> {
    let gate = format!("https://datanodes.to/{code}/{}", url_safe_name(fname));
    let url: tauri::Url = gate.parse().map_err(|e| format!("Bad link: {e}"))?;
    *RESOLVE_TX.lock().unwrap() = None;
    let window = ensure_resolver_window(app)?;
    let _ = window.hide();
    window.navigate(url).map_err(|e| format!("Could not prepare the download: {e}"))?;
    let (tx, rx) = mpsc::channel::<String>();
    *RESOLVE_TX.lock().unwrap() = Some(tx);

    let started = Instant::now();
    let deadline = started + Duration::from_secs(RESOLVE_TIMEOUT_SECS);
    let mut window_shown = false;
    let mut resolved: Option<String> = None;
    loop {
        if cancel.load(Ordering::Relaxed) {
            *RESOLVE_TX.lock().unwrap() = None;
            return Err("Canceled".into());
        }
        if Instant::now() > deadline {
            break;
        }
        if !window_shown && started.elapsed() >= Duration::from_secs(NATIVE_RESOLVE_GRACE_SECS) {
            window_shown = true;
            if let Some(w) = app.get_webview_window(RESOLVE_LABEL) {
                crate::dlog!("[store] auto-resolve pending after {}s — revealing the check window", NATIVE_RESOLVE_GRACE_SECS);
                let _ = w.show();
                let _ = w.set_focus();
            }
        }
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(u) => {
                resolved = Some(u);
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if app.get_webview_window(RESOLVE_LABEL).is_none() {
                    match rx.recv_timeout(Duration::from_millis(400)) {
                        Ok(u) => {
                            resolved = Some(u);
                            break;
                        }
                        Err(_) => break,
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    *RESOLVE_TX.lock().unwrap() = None;
    resolved.ok_or_else(|| {
        "Could not prepare the download link. Complete the verification in the window, then try again.".to_string()
    })
}

enum DownloadError {
    Canceled,
    Transient(i32),
    RangeUnsupported,
    HttpError(i32),
    Other(String),
}

fn download_file(
    app: &AppHandle,
    gid: &str,
    url: &str,
    dest: &Path,
    cancel: &AtomicBool,
    resolver: &dyn Fn() -> Result<String, String>,
) -> Result<(), String> {
    let mut current_url = url.to_string();
    let mut transient_attempts = 0usize;
    let mut resolve_attempts = 0usize;
    let mut restart_pending = false;
    loop {
        if !is_web_url(&current_url) {
            return Err("The download link was invalid. Try again.".into());
        }
        if restart_pending {
            let _ = std::fs::remove_file(dest);
            restart_pending = false;
        }
        let resume = std::fs::metadata(dest).map(|m| m.len() > 0).unwrap_or(false);
        match download_file_once(app, gid, &current_url, dest, cancel, resume) {
            Ok(()) => return Ok(()),
            Err(DownloadError::Canceled) => return Err("Canceled".into()),
            Err(DownloadError::Transient(code)) => {
                if cancel.load(Ordering::Relaxed) {
                    return Err("Canceled".into());
                }
                transient_attempts += 1;
                if transient_attempts > 40 {
                    return Err(format!("The download kept failing (curl exit {code}). Check your connection and try again."));
                }
                let waits = [5u64, 10, 20, 30, 45, 60];
                let wait = waits[(transient_attempts - 1).min(waits.len() - 1)];
                crate::dlog!("[store] transient curl exit {} for {} — retrying in {}s (attempt {})", code, gid, wait, transient_attempts);
                sleep_cancelable(wait, cancel)?;
            }
            Err(DownloadError::RangeUnsupported) => {
                transient_attempts += 1;
                if transient_attempts > 10 {
                    return Err("The server rejected resuming the download. Try again.".into());
                }
                restart_pending = true;
            }
            Err(DownloadError::HttpError(code)) => {
                if cancel.load(Ordering::Relaxed) {
                    return Err("Canceled".into());
                }
                resolve_attempts += 1;
                if resolve_attempts > 6 {
                    return Err("The host kept refusing the direct download (the link may have expired). Try again from the Store.".into());
                }
                crate::dlog!("[store] direct link refused (curl exit {}) for {} — refreshing the link", code, gid);
                sleep_cancelable(3, cancel)?;
                current_url = resolver()?;
                transient_attempts = 0;
                if std::fs::metadata(dest).map(|m| m.len() == 0).unwrap_or(true) {
                    restart_pending = true;
                }
            }
            Err(DownloadError::Other(msg)) => return Err(msg),
        }
    }
}

fn download_file_once(
    app: &AppHandle,
    gid: &str,
    url: &str,
    dest: &Path,
    cancel: &AtomicBool,
    resume: bool,
) -> Result<(), DownloadError> {
    let Some(curl_path) = std::env::var_os("SystemRoot")
        .map(|r| std::path::PathBuf::from(r).join("System32\\curl.exe"))
        .filter(|p| p.exists())
    else {
        return Err(DownloadError::Other(
            "Windows' built-in curl.exe was not found (Windows 10 1803 or newer is required).".into(),
        ));
    };
    let mut cmd = Command::new(&curl_path);
    cmd.arg("-L")
        .arg("--proto")
        .arg("=http,https")
        .arg("--proto-redir")
        .arg("=http,https")
        .arg("--silent")
        .arg("--show-error")
        .arg("--fail")
        .arg("--http1.1")
        .arg("--connect-timeout")
        .arg("20")
        .arg("--speed-limit")
        .arg("1024")
        .arg("--speed-time")
        .arg("120");
    if resume {
        cmd.arg("--continue-at").arg("-");
    }
    cmd.arg("--output").arg(dest).arg("--url").arg(url);
    cmd.stdin(Stdio::null()).stdout(Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    let err_log = std::env::temp_dir().join("ll-curl-stderr.log");
    if let Ok(f) = std::fs::File::create(&err_log) {
        cmd.stderr(Stdio::from(f));
    }
    crate::dlog!("[store] spawning {:?} for {} -> {} (resume {})", curl_path, gid, dest.display(), resume);
    let mut child = cmd
        .spawn()
        .map_err(|e| {
            crate::dlog!("[store] curl spawn failed for {}: {e}", gid);
            DownloadError::Other(format!("Could not start the download: {e}"))
        })?;

    let mut downloaded: u64 = 0;
    let mut last_emit = Instant::now();
    let mut last_bytes: u64 = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            return Err(DownloadError::Canceled);
        }
        if let Some(status) = child.try_wait().map_err(|e| {
            crate::dlog!("[store] try_wait failed for {}: {e}", gid);
            DownloadError::Other(format!("Download failed: {e}"))
        })? {
            if let Ok(md) = std::fs::metadata(dest) {
                downloaded = md.len();
            }
            emit(app, StoreEvent {
                game_id: gid.into(),
                phase: "downloading".into(),
                downloaded: Some(downloaded),
                total: Some(downloaded),
                bps: None,
                error: None,
            });
            if status.success() {
                if downloaded == 0 {
                    crate::dlog!("[store] curl exited OK but wrote nothing for {}", gid);
                    return Err(DownloadError::Other("Download failed: the server returned no data.".into()));
                }
                crate::dlog!("[store] curl OK, {} bytes for {}", downloaded, gid);
                return Ok(());
            }
            let code = status.code().unwrap_or(-1);
            crate::dlog!("[store] curl exit {} for {}", code, gid);
            return Err(match code {
                22 => DownloadError::HttpError(22),
                33 => DownloadError::RangeUnsupported,
                16 | 18 | 28 | 35 | 52 | 55 | 56 => DownloadError::Transient(code),
                _ => DownloadError::Other(format!("Download failed (curl exit {code}). Reload the link by trying the install again.")),
            });
        }
        if let Ok(md) = std::fs::metadata(dest) {
            downloaded = md.len();
            if last_emit.elapsed().as_millis() >= EMIT_INTERVAL_MS {
                let elapsed = last_emit.elapsed().as_millis();
                let bps = ((downloaded - last_bytes) as f64 / (elapsed as f64 / 1000.0)) as u64;
                last_bytes = downloaded;
                last_emit = Instant::now();
                emit(app, StoreEvent {
                    game_id: gid.into(),
                    phase: "downloading".into(),
                    downloaded: Some(downloaded),
                    total: None,
                    bps: Some(bps),
                    error: None,
                });
            }
        }
        std::thread::sleep(Duration::from_millis(400));
    }
}

fn extract_zip(app: &AppHandle, gid: &str, zip_path: &Path, root: &Path, cancel: &AtomicBool) -> Result<(), String> {
    let file = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file))
        .map_err(|e| format!("Corrupt archive: {e}"))?;

    let total: u64 = (0..archive.len())
        .map(|i| archive.by_index(i).map(|e| e.size()).unwrap_or(0))
        .sum();
    let mut done: u64 = 0;
    let mut last_emit = Instant::now();

    for i in 0..archive.len() {
        if cancel.load(Ordering::Relaxed) {
            return Err("Canceled".into());
        }
        let mut entry = archive.by_index(i).map_err(|e| format!("Corrupt archive: {e}"))?;
        if entry.is_dir() { continue; }
        let Some(rel) = entry.enclosed_name().map(|p| p.to_path_buf()) else { continue };
        let out = root.join(&rel);
        let Some(out_dir) = out.parent() else { continue };
        if !out_dir.starts_with(root) { continue; }
        std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
        let mut writer = std::fs::File::create(&out).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut writer).map_err(|e| format!("Extraction interrupted: {e}"))?;
        done += entry.size();
        if last_emit.elapsed().as_millis() >= EMIT_INTERVAL_MS {
            last_emit = Instant::now();
            emit(app, StoreEvent {
                game_id: gid.into(),
                phase: "extracting".into(),
                downloaded: Some(done),
                total: Some(total),
                bps: None,
                error: None,
            });
        }
    }
    Ok(())
}

pub fn extract_archives_with_progress(
    app: &AppHandle,
    gid: &str,
    root: &Path,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let mut last_emit = Instant::now();
    let mut cb = |done: u64, total: u64| {
        if last_emit.elapsed().as_millis() >= EMIT_INTERVAL_MS {
            last_emit = Instant::now();
            emit(app, StoreEvent {
                game_id: gid.into(),
                phase: "extracting".into(),
                downloaded: Some(done),
                total: Some(total),
                bps: None,
                error: None,
            });
        }
    };
    let mut progress = crate::archives::Progress::new(&mut cb);
    crate::archives::extract_all(root, cancel, &mut progress)
}

pub fn nested_unpack_pending(dir: &Path) -> bool {
    !dir.join(crate::archives::DONE_MARKER).exists() || crate::archives::has_archives(dir)
}

pub fn store_game_dir(app: &AppHandle, exe: &Path) -> Option<PathBuf> {
    let root = store_root(app).ok()?;
    let rel = exe.strip_prefix(&root).ok()?;
    let first = rel.components().next()?;
    Some(root.join(first))
}

fn exe_name_excluded(lower_name: &str) -> bool {
    const JUNK: &[&str] = &["readme", "license", "instructions", "ankergames", "installguide", "install-guide", "cover", "screenshot", "crashreport"];
    JUNK.iter().any(|j| lower_name.contains(j))
}

fn find_game_exe(dir: &Path, game_name: &str) -> Option<PathBuf> {
    let mut cands: Vec<(PathBuf, usize, u64)> = Vec::new();
    for entry in walkdir::WalkDir::new(dir).max_depth(6).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() { continue; }
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.to_lowercase().ends_with(".exe") { continue; }
        if crate::is_junk_exe(&name) { continue; }
        if exe_name_excluded(&name.to_lowercase()) { continue; }
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        let depth = entry.depth();
        cands.push((entry.into_path(), depth, size));
    }
    if cands.is_empty() { return None; }
    let key = |g: &str| {
        let mut n = String::new();
        for c in g.chars() { if c.is_alphanumeric() { n.extend(c.to_lowercase()); } }
        n
    };
    let gk = key(game_name);
    cands.sort_by(|a, b| {
        let am = a.0.file_stem().map(|s| key(&s.to_string_lossy()).contains(&gk)).unwrap_or(false);
        let bm = b.0.file_stem().map(|s| key(&s.to_string_lossy()).contains(&gk)).unwrap_or(false);
        bm.cmp(&am)
            .then(a.1.cmp(&b.1))
            .then(b.2.cmp(&a.2))
    });
    if let Some(big) = cands.iter().find(|c| c.2 >= 300_000) {
        if cands.len() == 1 || big.1 <= cands[0].1 + 1 { return Some(big.0.clone()); }
    }
    Some(cands[0].0.clone())
}

#[tauri::command]
pub async fn store_install(
    app: AppHandle,
    state: State<'_, StoreState>,
    game_id: String,
    name: String,
    uri: String,
    ext: String,
) -> Result<(), String> {
    if is_magnet_uri(&uri) {
        {
            let active = state.active.lock().unwrap();
            if active.is_some() {
                return Err("Another download is already running. Cancel it first, then start this one.".into());
            }
        }
        let (code, _fname, _) = split_uri(&uri);
        if code.is_empty() {
            return Err("That store entry has no usable download link.".into());
        }
        let root = store_root(&app)?;
        let game_dir = root.join(slugify(&name, &code));
        let _ = std::fs::remove_dir_all(&game_dir);
        if std::fs::create_dir_all(&game_dir).is_err() {
            return Err("Could not create the download folder. Check that the Documents folder is writable.".into());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *state.active.lock().unwrap() = Some(ActiveInstall { cancel: cancel.clone() });
        let app_t = app.clone();
        let gid_t = game_id.clone();
        let name_t = name.clone();
        emit(&app_t, StoreEvent::phase(&gid_t, "resolving"));
        tauri::async_runtime::spawn_blocking(move || {
            let res = crate::torrent::run_magnet_install(&app_t, &gid_t, &name_t, &uri, &game_dir, &cancel);
            match res {
                Ok(true) => {
                    emit(&app_t, StoreEvent::phase(&gid_t, "extracting"));
                    if let Err(e) = extract_archives_with_progress(&app_t, &gid_t, &game_dir, &cancel) {
                        crate::dlog!("[store] nested extract incomplete for {}: {}", gid_t, e);
                    }
                    let exe = find_game_exe(&game_dir, &name_t);
                    match exe {
                        Some(e) => {
                            emit(&app_t, StoreEvent::phase(&gid_t, "finishing"));
                            match crate::add_store_game(&app_t, e, &gid_t, &name_t) {
                                Ok(_) => emit(&app_t, StoreEvent::phase(&gid_t, "done")),
                                Err(err) => emit(&app_t, StoreEvent { game_id: gid_t.clone(), phase: "error".into(), downloaded: None, total: None, bps: None, error: Some(err) }),
                            }
                        }
                        None => {
                            emit(&app_t, StoreEvent { game_id: gid_t.clone(), phase: "error".into(), downloaded: None, total: None, bps: None, error: Some("Downloaded, but no game executable was found inside. Check the Store Games folder.".into()) });
                            let _ = std::process::Command::new("explorer").arg(&game_dir).spawn();
                        }
                    }
                }
                Ok(false) => {
                    emit(&app_t, StoreEvent { game_id: gid_t.clone(), phase: "error".into(), downloaded: None, total: None, bps: None, error: Some("Torrent engine not enabled in this build.".into()) });
                }
                Err(e) if e == "Canceled" => {
                    emit(&app_t, StoreEvent::phase(&gid_t, "canceled"));
                }
                Err(e) => {
                    emit(&app_t, StoreEvent { game_id: gid_t.clone(), phase: "error".into(), downloaded: None, total: None, bps: None, error: Some(e) });
                }
            }
            *app_t.state::<StoreState>().active.lock().unwrap() = None;
        });
        return Ok(());
    }

    if ext != "zip" {
        return Err("This game ships as a RAR archive, which the launcher cannot unpack yet. Download it manually from its game page.".into());
    }
    {
        let active = state.active.lock().unwrap();
        if active.is_some() {
            return Err("Another download is already running. Cancel it first, then start this one.".into());
        }
    }
    let (code, fname, _) = split_uri(&uri);
    if code.is_empty() || fname.is_empty() {
        return Err("That store entry has no usable download link.".into());
    }

    let root = store_root(&app)?;
    let game_dir = root.join(slugify(&name, &code));
    let _ = std::fs::remove_dir_all(&game_dir);
    if std::fs::create_dir_all(&game_dir).is_err() {
        return Err("Could not create the download folder. Check that the Documents folder is writable.".into());
    }

    let cancel = Arc::new(AtomicBool::new(false));
    *state.active.lock().unwrap() = Some(ActiveInstall { cancel: cancel.clone() });

    let app_t = app.clone();
    let gid_t = game_id.clone();
    let name_t = name.clone();
    let code_t = code.clone();
    let fname_t = fname.clone();
    let cancel_t = cancel.clone();
    tauri::async_runtime::spawn_blocking(move || {
        emit(&app_t, StoreEvent::phase(&gid_t, "resolving"));
        let direct_url = match resolve_gate(&app_t, &code_t, &fname_t, &cancel_t) {
            Ok(u) => {
                crate::dlog!("[store] resolved direct link ({} chars)", u.len());
                u
            }
            Err(e) if e == "Canceled" => {
                clear_resolver(&app_t);
                cleanup_target(&app_t, &game_dir);
                emit(&app_t, StoreEvent::phase(&gid_t, "canceled"));
                *app_t.state::<StoreState>().active.lock().unwrap() = None;
                return;
            }
            Err(e) => {
                clear_resolver(&app_t);
                cleanup_target(&app_t, &game_dir);
                emit(&app_t, StoreEvent {
                    game_id: gid_t.clone(),
                    phase: "error".into(),
                    downloaded: None, total: None, bps: None,
                    error: Some(e),
                });
                *app_t.state::<StoreState>().active.lock().unwrap() = None;
                return;
            }
        };
        clear_resolver(&app_t);

        let zip_path = game_dir.join(format!("{}.part", safe_local_name(&fname_t)));
        emit(&app_t, StoreEvent::phase(&gid_t, "downloading"));
        let resolver = || resolve_gate(&app_t, &code_t, &fname_t, &cancel_t);
        if let Err(e) = download_file(&app_t, &gid_t, &direct_url, &zip_path, &cancel_t, &resolver) {
            crate::dlog!("[store] download failed for {} , status-err: {}", gid_t, e);
            if e == "Canceled" {
                cleanup_target(&app_t, &game_dir);
                emit(&app_t, StoreEvent::phase(&gid_t, "canceled"));
            } else {
                cleanup_target(&app_t, &zip_path);
                emit(&app_t, StoreEvent { game_id: gid_t.clone(), phase: "error".into(), downloaded: None, total: None, bps: None, error: Some(e) });
            }
            *app_t.state::<StoreState>().active.lock().unwrap() = None;
            return;
        }
        let final_zip = zip_path.with_extension("zip");
        crate::dlog!("[store] download finished, renaming {:?} -> {:?}", zip_path, final_zip);
        if let Err(e) = std::fs::rename(&zip_path, &final_zip) {
            cleanup_target(&app_t, &game_dir);
            emit(&app_t, StoreEvent { game_id: gid_t.clone(), phase: "error".into(), downloaded: None, total: None, bps: None, error: Some(format!("Could not finalize the download: {e}")) });
            *app_t.state::<StoreState>().active.lock().unwrap() = None;
            return;
        }

        emit(&app_t, StoreEvent::phase(&gid_t, "extracting"));
        if let Err(e) = extract_zip(&app_t, &gid_t, &final_zip, &game_dir, &cancel_t) {
            crate::dlog!("[store] extract failed for {}: {}", gid_t, e);
            if e == "Canceled" {
                cleanup_target(&app_t, &game_dir);
                emit(&app_t, StoreEvent::phase(&gid_t, "canceled"));
            } else {
                emit(&app_t, StoreEvent { game_id: gid_t.clone(), phase: "error".into(), downloaded: None, total: None, bps: None, error: Some(e) });
            }
            cleanup_target(&app_t, &final_zip);
            *app_t.state::<StoreState>().active.lock().unwrap() = None;
            return;
        }
        if let Err(e) = extract_archives_with_progress(&app_t, &gid_t, &game_dir, &cancel_t) {
            crate::dlog!("[store] nested extract incomplete for {}: {}", gid_t, e);
        }
        let _ = std::fs::remove_file(&final_zip);

        emit(&app_t, StoreEvent::phase(&gid_t, "finishing"));
        let exe = match find_game_exe(&game_dir, &name_t) {
            Some(e) => e,
            None => {
                emit(&app_t, StoreEvent { game_id: gid_t.clone(), phase: "error".into(), downloaded: None, total: None, bps: None, error: Some("Downloaded and unpacked, but no game executable was found inside. The files are in your Store Games folder.".into()) });
                let _ = std::process::Command::new("explorer").arg(&game_dir).spawn();
                *app_t.state::<StoreState>().active.lock().unwrap() = None;
                return;
            }
        };
        match crate::add_store_game(&app_t, exe, &gid_t, &name_t) {
            Ok(_) => emit(&app_t, StoreEvent::phase(&gid_t, "done")),
            Err(e) => emit(&app_t, StoreEvent { game_id: gid_t.clone(), phase: "error".into(), downloaded: None, total: None, bps: None, error: Some(e) }),
        }
        *app_t.state::<StoreState>().active.lock().unwrap() = None;
    });

    Ok(())
}

fn cleanup_target(app: &AppHandle, target: &Path) {
    if let Ok(root) = store_root(app) {
        if target.starts_with(&root) {
            if target.is_file() { let _ = std::fs::remove_file(target); }
            else if target.exists() { let _ = std::fs::remove_dir_all(target); }
        }
    }
}

#[tauri::command]
pub fn store_cancel(app: AppHandle, state: State<StoreState>) -> Result<(), String> {
    let active = state.active.lock().unwrap().take();
    let Some(active) = active else { return Ok(()) };
    active.cancel.store(true, Ordering::Relaxed);
    clear_resolver(&app);
    Ok(())
}

#[tauri::command]
pub fn store_uninstall(app: AppHandle, store_id: String) -> Result<Option<crate::Library>, String> {
    let root = store_root(&app)?;
    let root = std::fs::canonicalize(&root).unwrap_or(root);
    let app_state = app.state::<AppState>();
    let mut lib = app_state.lib.lock().unwrap();
    for g in lib.games.iter().filter(|g| g.store_id.as_deref() == Some(store_id.as_str())).cloned().collect::<Vec<_>>() {
        let folder = PathBuf::from(&g.folder);
        let Ok(meta) = std::fs::symlink_metadata(&folder) else { continue };
        if meta.file_type().is_symlink() { continue; }
        let Ok(canon) = std::fs::canonicalize(&folder) else { continue };
        if canon == root || !canon.starts_with(&root) { continue; }
        let _ = std::fs::remove_dir_all(&canon);
    }
    lib.games.retain(|g| g.store_id.as_deref() != Some(store_id.as_str()));
    let out = lib.clone();
    drop(lib);
    crate::save(&app, &out)?;
    Ok(Some(out))
}

fn is_web_url(url: &str) -> bool {
    let Ok(u) = tauri::Url::parse(url.trim()) else { return false; };
    matches!(u.scheme(), "http" | "https") && u.host_str().map(|h| !h.is_empty()).unwrap_or(false)
}

#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        if !is_web_url(&url) {
            return Err("Only web links can be opened.".into());
        }
        use windows::core::PCWSTR;
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let open: Vec<u16> = "open".encode_utf16().chain(std::iter::once(0)).collect();
        let wide: Vec<u16> = url.trim().encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let r = ShellExecuteW(
                None,
                PCWSTR(open.as_ptr()),
                PCWSTR(wide.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            );
            if (r.0 as usize) <= 32 {
                return Err(format!("Could not open the link (code {}).", r.0 as usize));
            }
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = url;
        Err("Only supported on Windows".into())
    }
}

#[tauri::command]
pub async fn store_catalog(app: AppHandle) -> Result<StoreCatalog, String> {
    let _ = ensure_resolver_window(&app);
    let path = cache_path(&app);
    let cached: Option<StoreCatalog> = path.as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|txt| serde_json::from_str::<StoreCatalog>(&txt).ok());
    if let Some(c) = cached.as_ref() {
        if now().saturating_sub(c.fetched_at) < CACHE_TTL_SECS {
            return Ok(c.clone());
        }
    }
    match cached {
        Some(c) => {
            let bg_path = path.clone();
            let app2 = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Ok(fresh) = fetch_catalog_auto(&app2).await {
                    if !fresh.games.is_empty() {
                        if let Some(p) = bg_path {
                            if let Ok(txt) = serde_json::to_string(&fresh) {
                                let _ = std::fs::write(p, txt);
                            }
                        }
                    }
                }
            });
            Ok(c)
        }
        None => {
            let fetched = fetch_catalog_auto(&app).await;
            match fetched {
                Ok(c) => {
                    if let Some(p) = path {
                        if let Ok(txt) = serde_json::to_string(&c) {
                            let _ = std::fs::write(p, txt);
                        }
                    }
                    Ok(c)
                }
                Err(e) => {
                    if let Some(p) = path.as_ref() {
                        if let Ok(txt) = std::fs::read_to_string(p) {
                            if let Ok(c) = serde_json::from_str::<StoreCatalog>(&txt) {
                                return Ok(c);
                            }
                        }
                    }
                    Err(e)
                }
            }
        }
    }
}

fn norm_key(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()
}

fn match_catalog_entry(app: &AppHandle, folder: &str) -> (String, String) {
    let key = norm_key(folder);
    if let Some(p) = cache_path(app) {
        if let Ok(txt) = std::fs::read_to_string(p) {
            if let Ok(cat) = serde_json::from_str::<StoreCatalog>(&txt) {
                for g in &cat.games {
                    let gk = norm_key(&g.name);
                    if !gk.is_empty() && (gk == key || key.starts_with(&gk) || gk.starts_with(&key)) {
                        return (g.id.clone(), g.name.clone());
                    }
                }
            }
        }
    }
    (folder.to_string(), folder.replace('-', " "))
}

#[tauri::command]
pub async fn store_repair(app: AppHandle) -> Result<Vec<String>, String> {
    let root = store_root(&app)?;
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .map_err(|e| format!("Could not read the Store Games folder: {e}"))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();

    let mut repaired: Vec<String> = Vec::new();
    for dir in dirs {
        let folder = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if crate::archives::has_archives(&dir) {
            let cancel = AtomicBool::new(false);
            if let Err(e) = extract_archives_with_progress(&app, &folder, &dir, &cancel) {
                crate::dlog!("[store] repair unpack incomplete for {}: {}", folder, e);
            }
        }
        if let Some(exe) = find_game_exe(&dir, &folder) {
            let (store_id, name) = match_catalog_entry(&app, &folder);
            crate::add_store_game(&app, exe, &store_id, &name)?;
            repaired.push(name);
        }
    }
    Ok(repaired)
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StoreSources {
    pub sources: Vec<cat::HydraSource>,
}

#[tauri::command]
pub fn store_sources() -> StoreSources {
    StoreSources { sources: hydra_sources() }
}

#[tauri::command]
pub async fn store_catalog_harvested(
    app: AppHandle,
    harvested: std::collections::HashMap<String, String>,
) -> Result<StoreCatalog, String> {
    let mut map = load_harvest_cache(&app);
    for (url, json) in harvested.iter() {
        map.insert(url.clone(), json.clone());
    }
    for (url, json) in map.iter() {
        save_harvest_entry(&app, url.clone(), json.clone());
    }
    let path = cache_path(&app);
    let result = tauri::async_runtime::spawn_blocking(move || fetch_catalog_blocking_with_harvested(&map))
        .await
        .map_err(|e| e.to_string())?;
    let fetched = result?;
    if fetched.games.is_empty() {
        return Err("No games in merged catalog".into());
    }
    if let Some(p) = path { if let Ok(txt) = serde_json::to_string(&fetched) { let _ = std::fs::write(p, txt); } }
    Ok(fetched)
}

fn harvest_cache_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok().map(|d| d.join(HARVEST_CACHE_FILE))
}

fn load_harvest_cache(app: &AppHandle) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    if let Some(p) = harvest_cache_path(app) {
        if let Ok(txt) = std::fs::read_to_string(p) {
            if let Ok(m) = serde_json::from_str::<std::collections::HashMap<String, String>>(&txt) {
                map = m;
            }
        }
    }
    map
}

fn save_harvest_entry(app: &AppHandle, url: String, json: String) {
    let mut map = load_harvest_cache(app);
    map.insert(url, json);
    if let Some(p) = harvest_cache_path(app) {
        if let Ok(txt) = serde_json::to_string(&map) {
            let _ = std::fs::write(p, txt);
        }
    }
}

fn fetch_source_body_blocking(source: &HydraSource) -> Result<String, String> {
    let resp = reqwest::blocking::Client::builder()
        .user_agent(UA)
        .timeout(Duration::from_secs(90))
        .build()
        .map_err(|e| format!("Could not create the HTTP client: {e}"))?
        .get(&source.url)
        .send()
        .map_err(|e| format!("Could not reach {}: {e}", source.label))?;
    if resp.status().as_u16() == 403 {
        return Err(format!("__cf__{}", source.url));
    }
    resp.error_for_status_ref().map_err(|e| format!("Could not reach {}: {e}", source.label))?;
    let mut body = String::new();
    resp.take(96 * 1024 * 1024).read_to_string(&mut body).map_err(|e| e.to_string())?;
    let trimmed = body.trim_start();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        Ok(body)
    } else {
        Err(format!("__cf__{}", source.url))
    }
}

#[tauri::command]
pub async fn store_harvest_source(app: AppHandle, url: String) -> Result<String, String> {
    let source = hydra_sources()
        .into_iter()
        .find(|s| s.url == url)
        .ok_or_else(|| "Unknown store source.".to_string())?;

    let s2 = source.clone();
    let direct = tauri::async_runtime::spawn_blocking(move || fetch_source_body_blocking(&s2))
        .await
        .map_err(|e| e.to_string())?;
    match direct {
        Ok(body) => {
            save_harvest_entry(&app, url, body.clone());
            return Ok(body);
        }
        Err(e) if !e.starts_with("__cf__") => return Err(e),
        _ => {}
    }

    harvest_source_webview(&app, &source).await
}

async fn harvest_source_webview(app: &AppHandle, source: &HydraSource) -> Result<String, String> {
    let (tx, rx) = mpsc::channel::<String>();
    let nav_url: tauri::Url = source.url.parse().map_err(|e| format!("Bad source URL: {e}"))?;
    let tx_for_nav = tx.clone();
    let build_result = WebviewWindowBuilder::new(app, HARVEST_LABEL, WebviewUrl::External(nav_url))
        .title(format!("Verifying {} — complete the check if asked", source.label))
        .inner_size(560.0, 640.0)
        .resizable(true)
        .visible(false)
        .initialization_script(HARVEST_SCRIPT)
        .on_navigation(move |nav| {
            crate::dlog!("[store] harvest nav: {}", nav.as_str().chars().take(110).collect::<String>());
            if nav.host_str() == Some(RESOLVE_HOST) {
                let raw = nav.fragment().unwrap_or("");
                let decoded = percent_decode(raw);
                let body = base64::engine::general_purpose::STANDARD
                    .decode(decoded.as_bytes())
                    .ok()
                    .and_then(|b| String::from_utf8(b).ok())
                    .unwrap_or(decoded);
                let body = body.trim().to_string();
                if !body.is_empty() {
                    let _ = tx_for_nav.send(body);
                }
                return false;
            }
            true
        })
        .build();
    if let Err(e) = build_result {
        return Err(format!("Could not open the verification window: {e}"));
    }

    let app_t = app.clone();
    let payload = tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
        let started = Instant::now();
        let deadline = started + Duration::from_secs(HARVEST_TIMEOUT_SECS);
        let mut window_shown = false;
        loop {
            if Instant::now() > deadline {
                return Err("Timed out waiting for the source to load. Complete the check in the window and try again.".into());
            }
            if !window_shown && started.elapsed() >= Duration::from_secs(20) {
                window_shown = true;
                if let Some(w) = app_t.get_webview_window(HARVEST_LABEL) {
                    crate::dlog!("[store] harvest auto-check pending after 20s — revealing the verification window");
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(body) => return Ok(body),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if app_t.get_webview_window(HARVEST_LABEL).is_none() {
                        match rx.recv_timeout(Duration::from_millis(400)) {
                            Ok(body) => return Ok(body),
                            Err(_) => return Err("Verification was canceled before the source loaded.".into()),
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("Verification window closed unexpectedly.".into());
                }
            }
        }
    })
    .await
    .map_err(|e| e.to_string())?;
    close_harvester(app);
    let body = payload?;
    if !(body.trim_start().starts_with('{') || body.trim_start().starts_with('[')) {
        return Err("The source did not return catalog JSON. Try again.".into());
    }
    save_harvest_entry(app, source.url.clone(), body.clone());
    Ok(body)
}

fn percent_decode(s: &str) -> String { cat::percent_decode(s) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_split_on_en_dash() {
        let (n, v) = parse_title("Probably Stolen – V 100-REV3 / Build 26284217");
        assert_eq!(n, "Probably Stolen");
        assert_eq!(v, "V 100-REV3 / Build 26284217");

        let (n, v) = parse_title("House Flipper 2 – V 1.13.0.0 / Build 26251253");
        assert_eq!(n, "House Flipper 2");
        assert_eq!(v, "V 1.13.0.0 / Build 26251253");

        let (n, v) = parse_title("ANIMAL WELL – Build 16245418");
        assert_eq!(n, "ANIMAL WELL");
        assert_eq!(v, "Build 16245418");

        let (n, v) = parse_title("JUST PLAIN");
        assert_eq!(n, "JUST PLAIN");
        assert!(v.is_empty());
    }

    #[test]
    fn sizes_parse_across_units() {
        assert_eq!(parse_size_mb("378.9 MB"), 378.9);
        assert_eq!(parse_size_mb("1.8 GB"), 1843.2);
        assert_eq!(parse_size_mb("30.7 MB"), 30.7);
        assert_eq!(parse_size_mb("21.8 GB"), 22323.2);
        assert_eq!(parse_size_mb(""), 0.0);
        assert_eq!(parse_size_mb("garbage"), 0.0);
    }

    #[test]
    fn dates_convert_to_unix_seconds() {
        let expected: u64 = 1_767_225_600 + 301 * 86_400 + 16 * 3_600 + 18 * 60;
        assert_eq!(parse_upload_date("2026-10-29T16:18:00Z"), expected);
        assert_eq!(parse_upload_date(""), 0);
        assert_eq!(parse_upload_date("not-a-date"), 0);
    }

    #[test]
    fn uri_splits_code_and_filename() {
        let (code, fname, ext) = split_uri("https://datanodes.to/01l3gbq6lz3q/7-Days-to-End-with-You-AnkerGames.zip");
        assert_eq!(code, "01l3gbq6lz3q");
        assert_eq!(fname, "7-Days-to-End-with-You-AnkerGames.zip");
        assert_eq!(ext, "zip");
        let (code, _, ext) = split_uri("https://datanodes.to/r9s07mymkzbt/Animal-Well-AnkerGames.rar");
        assert_eq!(ext, "rar");
        assert!(!code.is_empty());
    }

    #[test]
    fn page_urls_extract_from_html() {
        assert_eq!(
            extract_page_url(r#"<a href="https://ankergames.net/game/probably-stolen">Site</a>"#),
            "https://ankergames.net/game/probably-stolen"
        );
        assert_eq!(extract_page_url("no link here"), "");
    }

    #[test]
    fn catalog_builds_from_sample() {
        let raw: RawCatalog = serde_json::from_str(
            r#"{"name":"AnkerGames","downloads":[
              {"title":"Drone Sector – V 0.6.1 / Build 26284649",
               "uris":["https://datanodes.to/r8c7h7q63gb4/Drone-Sector-AnkerGames.zip"],
               "uploadDate":"2026-10-29T16:05:00Z","fileSize":"1.8 GB",
               "descriptionHtml":"<a href=\"https://ankergames.net/game/drone-sector\">Website</a>"},
              {"title":"Pixeldrain only","uris":["https://pixeldrain.com/u/x"],"uploadDate":"","fileSize":"1 MB"},
              {"title":"No size","uris":["https://datanodes.to/abc/File.zip"],"uploadDate":"","fileSize":""}
            ]}"#,
        ).unwrap();
        let cat = build_catalog(raw);
        assert_eq!(cat.games.len(), 1);
        let g = &cat.games[0];
        assert_eq!(g.id, "ankergames:r8c7h7q63gb4");
        assert_eq!(g.source, "ankergames");
        assert_eq!(g.name, "Drone Sector");
        assert_eq!(g.ext, "zip");
        assert_eq!(g.page_url, "https://ankergames.net/game/drone-sector");
    }

    #[test]
    fn harvested_source_json_is_merged_into_catalog() {
        let sources = vec![cat::HydraSource::new("gog", "https://hydralinks.cloud/sources/gog.json")];
        let mut fallback = std::collections::HashMap::new();
        fallback.insert(
            "https://hydralinks.cloud/sources/gog.json".to_string(),
            r#"{"downloads":[{"title":"Merged Game – V 1.0","uris":["magnet:?xt=urn:btih:abcdef0123456789abcdef0123456789abcdef01&dn=Merged.Game"],"uploadDate":"2026-10-30T10:00:00Z","fileSize":"1.4 GB"}]}"#.to_string(),
        );
        let (merged, _) = fetch_all_sources_blocking(&sources, &fallback);
        assert_eq!(merged.games.len(), 1);
        assert_eq!(merged.games[0].source, "gog");
        assert_eq!(merged.games[0].id, "gog:abcdef012345");
        assert!(merged.games[0].uri.starts_with("magnet:"));
    }

    #[test]
    #[ignore]
    fn live_source_timing() {
        for src in hydra_sources() {
            let t = std::time::Instant::now();
            match fetch_one_source_blocking(&src) {
                Ok(c) => println!("[live] {} -> {} games in {:.1}s", src.label, c.games.len(), t.elapsed().as_secs_f32()),
                Err(e) => println!("[live] {} -> {} after {:.1}s", src.label, if e.starts_with("__cf__") { "CF-GATED" } else { e.as_str() }, t.elapsed().as_secs_f32()),
            }
        }
    }

    #[test]
    #[ignore]
    fn live_sources_report_status() {
        for src in hydra_sources() {
            match fetch_one_source_blocking(&src) {
                Ok(c) => println!("[live] {} -> {} games", src.label, c.games.len()),
                Err(e) => println!("[live] {} -> {}", src.label, if e.starts_with("__cf__") { "CF-GATED" } else { e.as_str() }),
            }
        }
    }

    #[test]
    fn magnet_entries_are_kept() {
        let raw: RawCatalog = serde_json::from_str(
            r#"{"downloads":[
              {"title":"Test Game – Build 123","uris":["magnet:?xt=urn:btih:ABCDEF1234567890ABCDEF1234567890ABCDEF12&dn=Test.Game.Build.123"],"uploadDate":"2026-10-29T16:05:00Z","fileSize":"2.1 GB"}
            ]}"#,
        ).unwrap();
        let cat = cat::build_catalog_from_raw(raw, "xatab", cat::now_secs());
        assert_eq!(cat.games.len(), 1);
        assert!(cat.games[0].uri.starts_with("magnet:"));
        assert_eq!(cat.games[0].source, "xatab");
    }

    #[test]
    fn nested_zips_extract_recursively() {
        use std::io::Write;
        let root = std::env::temp_dir().join(format!("ll-nested-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Game")).unwrap();

        let mut inner: Vec<u8> = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut inner));
            let opts = zip::write::SimpleFileOptions::default();
            w.start_file("data/inner.txt", opts).unwrap();
            w.write_all(b"hello").unwrap();
            w.finish().unwrap();
        }
        let outer_path = root.join("outer.zip");
        {
            let f = std::fs::File::create(&outer_path).unwrap();
            let mut w = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            w.start_file("Game/inner.zip", opts).unwrap();
            w.write_all(&inner).unwrap();
            w.start_file("Game/game.exe", opts).unwrap();
            w.write_all(b"exe").unwrap();
            w.finish().unwrap();
        }

        let mut cb = |_d: u64, _t: u64| {};
        let mut progress = crate::archives::Progress::new(&mut cb);
        crate::archives::extract_all(&root, &AtomicBool::new(false), &mut progress).unwrap();

        let txt = root.join("Game").join("data").join("inner.txt");
        assert_eq!(std::fs::read_to_string(&txt).unwrap(), "hello");
        assert!(root.join("Game").join("game.exe").exists());
        let left: Vec<_> = walkdir::WalkDir::new(&root)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|x| x.eq_ignore_ascii_case("zip")).unwrap_or(false))
            .collect();
        assert!(left.is_empty(), "zips left: {left:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn slugify_makes_safe_folder_names() {
        assert_eq!(slugify("House Flipper 2", "abc"), "house-flipper-2");
        assert_eq!(slugify("Game: with <> bad | chars", "abc123"), "game-with-bad-chars");
    }

    #[test]
    fn percent_decode_round_trips() {
        assert_eq!(percent_decode("a%20b%2Fc"), "a b/c");
        assert_eq!(percent_decode("plain"), "plain");
        assert_eq!(percent_decode("bad%2"), "bad%2");
    }

    #[test]
    fn local_names_are_safe() {
        assert_eq!(safe_local_name("../../x.zip"), "_.._x.zip");
        assert_eq!(safe_local_name("a\\b.zip"), "a_b.zip");
        assert_eq!(safe_local_name("C:evil.zip"), "C_evil.zip");
        assert_eq!(safe_local_name("normal-Game_v1.2.zip"), "normal-Game_v1.2.zip");
        assert_eq!(safe_local_name(""), "download");
    }

    #[test]
    fn only_web_urls_pass() {
        assert!(is_web_url("https://example.com/x"));
        assert!(is_web_url("http://datanodes.to/f"));
        assert!(!is_web_url("file:///C:/x"));
        assert!(!is_web_url("powershell.exe"));
        assert!(!is_web_url("\\\\server\\share"));
        assert!(!is_web_url("javascript:alert(1)"));
    }
}
