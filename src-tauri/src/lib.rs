#[macro_export]
macro_rules! dlog {
    ($($t:tt)*) => { if cfg!(debug_assertions) { eprintln!($($t)*); } };
}

mod archives;
mod catalog;
mod presence;
pub mod store;
pub mod torrent;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use tauri::{AppHandle, Manager, State};
use walkdir::WalkDir;

const KIND_PC: &str = "pc";
const MAX_SCAN_FILES: usize = 2000;

fn default_kind() -> String { KIND_PC.to_string() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Game {
    pub id: String,
    pub name: String,
    pub exe_path: String,
    pub folder: String,
    pub source: String,
    pub source_folder: Option<String>,
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub system: Option<String>,
    #[serde(default)]
    pub last_played: Option<u64>,
    #[serde(default)]
    pub play_count: u32,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub size_bytes: Option<u64>,
    #[serde(default)]
    pub store_id: Option<String>,
    #[serde(default)]
    pub steam_app_id: Option<u32>,
    #[serde(default)]
    steam_art_checked_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Library {
    pub scan_folders: Vec<String>,
    pub games: Vec<Game>,
}

pub struct AppState {
    lib: Mutex<Library>,
}

fn is_junk_exe(file_name: &str) -> bool {
    let n = file_name.to_lowercase();
    const JUNK: &[&str] = &[
        "unins", "uninstall", "setup", "installer", "install_", "_setup",
        "crash", "reporter", "handler", "updater", "update_", "patcher",
        "redist", "vcredist", "dxsetup", "directx", "dotnet", "oalinst",
        "eac", "easyanticheat", "battleye", "beservice", "activation",
        "config", "settings_", "launcher_helper", "helper", "service",
        "prereq", "webview", "cef", "unitycrash", "bugreport", "sendrpt",
        "steam_", "gog_", "egs", "benchmark", "dedicated", "server.exe",
        "editor.exe", "sdk", "tool.exe", "console.exe",
    ];
    if n.starts_with("unins") { return true; }
    JUNK.iter().any(|j| n.contains(j))
}

fn clean_name(file_stem: &str) -> String {
    let mut s = file_stem.to_string();
    for suffix in ["-Win64-Shipping", "-Win32-Shipping", "-Shipping", "-Windows", "_Win64", "-Win64", "Win64", "Win32", "x64", "x86"] {
        if let Some(stripped) = s.strip_suffix(suffix) { s = stripped.to_string(); break; }
    }
    let spaced: String = s.chars().map(|c| if c == '_' || c == '-' || c == '.' { ' ' } else { c }).collect();
    let mut humps = String::with_capacity(spaced.len() + 4);
    let chars: Vec<char> = spaced.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if i > 0 && c.is_uppercase() {
            let prev = chars[i - 1];
            let next_lower = chars.get(i + 1).map(|n| n.is_lowercase()).unwrap_or(false);
            if prev.is_lowercase() || (prev.is_uppercase() && next_lower) {
                humps.push(' ');
            }
        }
        humps.push(*c);
    }
    let collapsed = humps.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed == collapsed.to_lowercase() || collapsed == collapsed.to_uppercase() {
        collapsed.split(' ').map(|w| {
            let mut ch = w.chars();
            match ch.next() { Some(f) => f.to_uppercase().collect::<String>() + ch.as_str(), None => String::new() }
        }).collect::<Vec<_>>().join(" ")
    } else { collapsed }
}

fn id_for(path: &Path) -> String { path.to_string_lossy().to_lowercase() }

fn library_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("library.json"))
}

fn save(app: &AppHandle, lib: &Library) -> Result<(), String> {
    let p = library_path(app)?;
    std::fs::write(&p, serde_json::to_string_pretty(lib).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

fn load(app: &AppHandle) -> Library {
    let mut lib: Library = library_path(app)
        .and_then(|p| std::fs::read_to_string(p).map_err(|e| e.to_string()))
        .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
        .unwrap_or_default();
    lib.games.retain(|g| g.kind != "rom");
    lib
}

pub fn add_store_game(app: &AppHandle, exe: PathBuf, store_id: &str, display_name: &str) -> Result<Library, String> {
    if !exe.is_file() { return Err(format!("Executable no longer exists:\n{}", exe.display())); }
    let state = app.state::<AppState>();
    let mut lib = state.lib.lock().unwrap();
    let id = id_for(&exe);
    let mut game = game_from_path(&exe, "store", None, None);
    game.store_id = Some(store_id.to_string());
    if let Some(old) = lib.games.iter().find(|g| g.id == id) {            game.favorite = old.favorite;
            game.play_count = old.play_count;
            game.last_played = old.last_played;
            game.steam_app_id = old.steam_app_id;
            game.steam_art_checked_at = old.steam_art_checked_at;

    }
    if !display_name.trim().is_empty() { game.name = display_name.trim().to_string(); }
    if let Some(g) = lib.games.iter_mut().find(|g| g.id == id) { *g = game; }
    else { lib.games.push(game); }
    lib.games.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    let out = lib.clone();
    drop(lib);
    save(app, &out)?;
    Ok(out)
}

#[cfg(windows)]
fn extract_icon_data_url(exe: &Path) -> Option<String> {
    use base64::Engine;
    use windows::core::PCWSTR;
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::UI::Shell::*;
    use windows::Win32::UI::WindowsAndMessaging::*;

    let wide: Vec<u16> = exe.to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();

    unsafe {
        let mut info = SHFILEINFOW::default();
        let ok = SHGetFileInfoW(
            PCWSTR(wide.as_ptr()),
            windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL,
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        );
        if ok == 0 || info.hIcon.is_invalid() { return None; }
        let hicon = info.hIcon;

        let result = (|| -> Option<String> {
            let mut icon_info = ICONINFO::default();
            GetIconInfo(hicon, &mut icon_info).ok()?;

            let mut bm = BITMAP::default();
            GetObjectW(
                HGDIOBJ(icon_info.hbmColor.0),
                std::mem::size_of::<BITMAP>() as i32,
                Some(&mut bm as *mut _ as *mut _),
            );
            let (w, h) = (bm.bmWidth, bm.bmHeight);
            if w <= 0 || h <= 0 {
                let _ = DeleteObject(HGDIOBJ(icon_info.hbmColor.0));
                let _ = DeleteObject(HGDIOBJ(icon_info.hbmMask.0));
                return None;
            }

            let hdc = GetDC(None);
            let mut bmi = BITMAPINFO::default();
            bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = w;
            bmi.bmiHeader.biHeight = -h;
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            bmi.bmiHeader.biCompression = BI_RGB.0;
            let mut bgra = vec![0u8; (w * h * 4) as usize];
            let lines = GetDIBits(hdc, icon_info.hbmColor, 0, h as u32, Some(bgra.as_mut_ptr() as *mut _), &mut bmi, DIB_RGB_COLORS);
            ReleaseDC(None, hdc);
            let _ = DeleteObject(HGDIOBJ(icon_info.hbmColor.0));
            let _ = DeleteObject(HGDIOBJ(icon_info.hbmMask.0));
            if lines == 0 { return None; }

            let mut rgba = Vec::with_capacity(bgra.len());
            for px in bgra.chunks_exact(4) {
                let (b, g, r, a) = (px[0], px[1], px[2], px[3]);
                if a == 0 { rgba.extend_from_slice(&[0, 0, 0, 0]); }
                else { rgba.extend_from_slice(&[r, g, b, a]); }
            }

            let png = encode_png(w as u32, h as u32, &rgba)?;
            Some(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png)))
        })();

        let _ = DestroyIcon(hicon);
        result
    }
}

fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    fn crc32(data: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFF_FFFF;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 { crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 }; }
        }
        !crc
    }
    fn adler32(data: &[u8]) -> u32 {
        let (mut a, mut b): (u32, u32) = (1, 0);
        for &x in data { a = (a + x as u32) % 65521; b = (b + a) % 65521; }
        (b << 16) | a
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut crc_data = Vec::with_capacity(4 + data.len());
        crc_data.extend_from_slice(kind);
        crc_data.extend_from_slice(data);
        out.extend_from_slice(&crc32(&crc_data).to_be_bytes());
    }

    if rgba.len() != (w * h * 4) as usize { return None; }
    let mut raw = Vec::with_capacity((w * 4 + 1) as usize * h as usize);
    for y in 0..h as usize {
        raw.push(0u8);
        raw.extend_from_slice(&rgba[y * w as usize * 4..(y + 1) * w as usize * 4]);
    }
    let mut z = vec![0x78u8, 0x01u8];
    let mut off = 0;
    loop {
        let n = std::cmp::min(65535, raw.len() - off);
        let last = off + n == raw.len();
        z.push(if last { 1 } else { 0 });
        z.extend_from_slice(&(n as u16).to_le_bytes());
        z.extend_from_slice(&(!(n as u16)).to_le_bytes());
        z.extend_from_slice(&raw[off..off + n]);
        off += n;
        if last { break; }
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut png = vec![137u8, 80, 78, 71, 13, 10, 26, 10];
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &z);
    chunk(&mut png, b"IEND", &[]);
    Some(png)
}

#[cfg(not(windows))]
fn extract_icon_data_url(_exe: &Path) -> Option<String> { None }

fn is_skipped_dir(entry: &walkdir::DirEntry, depth: usize) -> bool {
    if depth == 0 || !entry.file_type().is_dir() { return false; }
    let name = entry.file_name().to_string_lossy().to_lowercase();
    if name.starts_with('.') { return true; }
    const SKIP: &[&str] = &[
        "node_modules", "$recycle.bin", "system volume information",
        "windows", "winnt", "program files", "program files (x86)",
        "programdata", "appdata", "__pycache__", "venv", ".venv",
        "msvc", "sdk", "sdks", "toolchain", "cargo", "rustup",
        "dotnet", "nuget", "packages", "vendor", "third_party",
        "redist", "_redist", "redistributable", "directx",
        "__install", "_commonredist", "commonredist", "support", "bonus",
        "crashreporter", "crashpad", "symbols", "debug", "docs",
    ];
    SKIP.contains(&name.as_str())
}

fn scan_folder(root: &Path, max_depth: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in WalkDir::new(root)
        .max_depth(max_depth)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !is_skipped_dir(e, e.depth()))
        .filter_map(|e| e.ok())
    {
        if found.len() >= MAX_SCAN_FILES { break; }
        let p = entry.path();
        if !entry.file_type().is_file() { continue; }
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        let name = entry.file_name().to_string_lossy();
        if !name.to_lowercase().ends_with(".exe") { continue; }
        if is_junk_exe(&name) { continue; }
        if size < 200_000 { continue; }
        found.push(p.to_path_buf());
    }
    found.sort();
    found
}

fn game_from_path(path: &Path, source: &str, source_folder: Option<String>, existing: Option<&Game>) -> Game {
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Game".into());
    Game {
        id: id_for(path),
        name: existing.map(|g| g.name.clone()).unwrap_or_else(|| clean_name(&stem)),
        exe_path: path.to_string_lossy().to_string(),
        folder: path.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
        source: source.to_string(),
        source_folder,
        kind: KIND_PC.into(),
        system: None,
        last_played: existing.and_then(|g| g.last_played),
        play_count: existing.map(|g| g.play_count).unwrap_or(0),
        favorite: existing.map(|g| g.favorite).unwrap_or(false),
        size_bytes: std::fs::metadata(path).ok().map(|m| m.len()),
        store_id: existing.and_then(|g| g.store_id.clone()),
        steam_app_id: existing.and_then(|g| g.steam_app_id),
        steam_art_checked_at: existing.and_then(|g| g.steam_art_checked_at),
    }
}

#[tauri::command]
fn get_library(state: State<AppState>) -> Library {
    state.lib.lock().unwrap().clone()
}

#[tauri::command]
fn add_scan_folder(app: AppHandle, state: State<AppState>, folder: String) -> Result<Library, String> {
    let root = PathBuf::from(&folder);
    if !root.is_dir() { return Err(format!("Folder does not exist: {folder}")); }
    let mut lib = state.lib.lock().unwrap();
    if !lib.scan_folders.iter().any(|f| f.eq_ignore_ascii_case(&folder)) {
        lib.scan_folders.push(folder.clone());
    }
    let existing: std::collections::HashMap<String, Game> =
        lib.games.iter().map(|g| (g.id.clone(), g.clone())).collect();
    let mut seen: HashSet<String> = lib.games.iter().map(|g| g.id.clone()).collect();
    for exe in scan_folder(&root, 10) {
        let id = id_for(&exe);
        if seen.insert(id.clone()) {
            lib.games.push(game_from_path(&exe, "scan", Some(folder.clone()), existing.get(&id)));
        }
    }
    lib.games.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    let out = lib.clone();
    drop(lib);
    save(&app, &out)?;
    Ok(out)
}

#[tauri::command]
fn remove_scan_folder(app: AppHandle, state: State<AppState>, folder: String) -> Result<Library, String> {
    let mut lib = state.lib.lock().unwrap();
    lib.scan_folders.retain(|f| !f.eq_ignore_ascii_case(&folder));
    lib.games.retain(|g| !(g.source == "scan" && g.source_folder.as_deref().map(|f| f.eq_ignore_ascii_case(&folder)).unwrap_or(false)));
    let out = lib.clone();
    drop(lib);
    save(&app, &out)?;
    Ok(out)
}

#[tauri::command]
fn rescan_all(app: AppHandle, state: State<AppState>) -> Result<Library, String> {
    let folders = state.lib.lock().unwrap().scan_folders.clone();
    let mut lib = state.lib.lock().unwrap();
    let existing: std::collections::HashMap<String, Game> =
        lib.games.iter().map(|g| (g.id.clone(), g.clone())).collect();
    let mut games: Vec<Game> = lib.games.iter().filter(|g| g.source == "manual" || g.source == "store").cloned().collect();
    let mut seen: HashSet<String> = games.iter().map(|g| g.id.clone()).collect();
    for folder in &folders {
        let root = PathBuf::from(folder);
        if !root.is_dir() { continue; }
        for exe in scan_folder(&root, 10) {
            let id = id_for(&exe);
            if seen.insert(id.clone()) {
                games.push(game_from_path(&exe, "scan", Some(folder.clone()), existing.get(&id)));
            }
        }
    }
    games.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    lib.games = games;
    let out = lib.clone();
    drop(lib);
    save(&app, &out)?;
    Ok(out)
}

#[tauri::command]
fn add_game(app: AppHandle, state: State<AppState>, exe_path: String) -> Result<Library, String> {
    let exe = PathBuf::from(&exe_path);
    if !exe.is_file() { return Err(format!("File does not exist: {exe_path}")); }
    let mut lib = state.lib.lock().unwrap();
    let id = id_for(&exe);
    if let Some(g) = lib.games.iter_mut().find(|g| g.id == id) {
        g.source = "manual".into();
    } else {
        let existing = lib.games.iter().find(|g| g.id == id).cloned();
        lib.games.push(game_from_path(&exe, "manual", None, existing.as_ref()));
    }
    lib.games.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    let out = lib.clone();
    drop(lib);
    save(&app, &out)?;
    Ok(out)
}

#[tauri::command]
fn remove_game(app: AppHandle, state: State<AppState>, id: String) -> Result<Library, String> {
    let mut lib = state.lib.lock().unwrap();
    lib.games.retain(|g| g.id != id);
    let out = lib.clone();
    drop(lib);
    save(&app, &out)?;
    Ok(out)
}

#[tauri::command]
fn rename_game(app: AppHandle, state: State<AppState>, id: String, name: String) -> Result<Library, String> {
    let name = name.trim().to_string();
    if name.is_empty() { return Err("Name cannot be empty".into()); }
    let mut lib = state.lib.lock().unwrap();
    let g = lib.games.iter_mut().find(|g| g.id == id).ok_or("Game not found")?;
    if g.name != name {
        g.steam_app_id = None;
        g.steam_art_checked_at = None;
    }
    g.name = name;
    lib.games.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    let out = lib.clone();
    drop(lib);
    save(&app, &out)?;
    Ok(out)
}

#[tauri::command]
fn toggle_favorite(app: AppHandle, state: State<AppState>, id: String) -> Result<Library, String> {
    let mut lib = state.lib.lock().unwrap();
    let g = lib.games.iter_mut().find(|g| g.id == id).ok_or("Game not found")?;
    g.favorite = !g.favorite;
    let out = lib.clone();
    drop(lib);
    save(&app, &out)?;
    Ok(out)
}

#[tauri::command]
fn get_icon(exe_path: String) -> Option<String> {
    if !Path::new(&exe_path).is_file() { return None; }
    extract_icon_data_url(Path::new(&exe_path))
}

#[derive(Deserialize)]
struct SteamSearchResponse {
    #[serde(default)]
    items: Vec<SteamSearchItem>,
}

#[derive(Deserialize)]
struct SteamSearchItem {
    id: u32,
    name: String,
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    platforms: Option<SteamPlatforms>,
}

#[derive(Deserialize)]
struct SteamPlatforms {
    #[serde(default)]
    windows: bool,
}

#[derive(Debug, Clone, Serialize)]
struct SteamArtwork {
    app_id: u32,
    name: String,
    cover: Option<String>,
}

#[derive(Deserialize)]
struct SteamDetailsResponse {
    #[serde(flatten)]
    apps: std::collections::HashMap<String, SteamDetailsResult>,
}

#[derive(Deserialize)]
struct SteamDetailsResult {
    success: bool,
    data: Option<SteamDetailsData>,
}

#[derive(Deserialize)]
struct SteamDetailsData {
    #[serde(default)]
    name: String,
    #[serde(default)]
    screenshots: Vec<SteamScreenshot>,
}

#[derive(Deserialize)]
struct SteamScreenshot {
    path_full: String,
}

fn normalize_steam_title(title: &str) -> String {
    title.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

fn is_dotted_version_token(token: &str) -> bool {
    let core = token.strip_prefix(['v', 'V']).unwrap_or(token);
    let core = core.split(['-', '+']).next().unwrap_or(core);
    core.contains('.')
        && core.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

fn strip_one_version_suffix(title: &str) -> &str {
    let trimmed = title.trim_end();
    let Some((head, last)) = trimmed.rsplit_once(char::is_whitespace) else { return trimmed };
    if last.is_empty() { return trimmed; }
    if is_dotted_version_token(last) { return head.trim_end(); }
    if let Some(rest) = last.strip_prefix(['v', 'V']) {
        if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) { return head.trim_end(); }
    }
    if let Some((before_last, prev)) = head.rsplit_once(char::is_whitespace) {
        let prev_lower = prev.to_lowercase();
        let last_lower = last.to_lowercase();
        if ["build", "update", "patch", "hotfix"].contains(&prev_lower.as_str())
            && (last_lower.chars().all(|c| c.is_ascii_digit()) || is_dotted_version_token(last)) {
            return before_last.trim_end();
        }
        if (prev == "v" || prev == "V") && last.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
            return before_last.trim_end();
        }
    }
    trimmed
}

fn steam_search_title(title: &str) -> String {
    let mut current = title.trim();
    loop {
        let stripped = strip_one_version_suffix(current);
        if stripped.len() == current.len() { return stripped.to_string(); }
        current = stripped;
        if current.is_empty() { return title.trim().to_string(); }
    }
}

fn matching_steam_app_id(title: &str, items: &[SteamSearchItem]) -> Option<u32> {
    let target = normalize_steam_title(title);
    if target.is_empty() { return None; }
    let eligible = items.iter().filter(|item| {
        item.kind == "app"
            && item.platforms.as_ref().map(|p| p.windows).unwrap_or(true)
    }).collect::<Vec<_>>();
    let mut matches = eligible.iter().filter(|item| normalize_steam_title(&item.name) == target)
        .map(|item| item.id).collect::<Vec<_>>();
    if matches.is_empty() {
        let base = title.trim().to_lowercase();
        matches = eligible.iter().filter(|item| {
            let name = item.name.trim().to_lowercase();
            name.strip_prefix(&base).map(|suffix| {
                let suffix = suffix.trim_start();
                matches!(suffix.chars().next(), Some('-' | '\u{2012}' | '\u{2013}' | '\u{2014}' | ':' | '|' | '('))
            }).unwrap_or(false)
        }).map(|item| item.id).collect();
    }
    matches.sort_unstable();
    matches.dedup();
    if matches.len() == 1 { matches.first().copied() } else { None }
}

const MAX_STEAM_ART_BYTES: usize = 8 * 1024 * 1024;
const STEAM_ART_RETRY_SECS: u64 = 30 * 24 * 60 * 60;

fn cached_steam_art(app_id: u32, cache_dir: &Path) -> Option<String> {
    for (ext, mime) in [("jpg", "image/jpeg"), ("png", "image/png")] {
        let path = cache_dir.join(format!("{app_id}.{ext}"));
        let Ok(metadata) = std::fs::metadata(&path) else { continue; };
        if metadata.len() as usize > MAX_STEAM_ART_BYTES { continue; }
        let Ok(bytes) = std::fs::read(path) else { continue; };
        let valid = if mime == "image/jpeg" { bytes.starts_with(&[0xFF, 0xD8, 0xFF]) } else { bytes.starts_with(b"\x89PNG\r\n\x1a\n") };
        if bytes.len() <= MAX_STEAM_ART_BYTES && valid {
            return Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)));
        }
    }
    None
}

fn search_steam_app(title: &str) -> Result<Option<u32>, ()> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("Hate.bio Launcher")
        .timeout(std::time::Duration::from_secs(8))
        .build().map_err(|_| ())?;
    let query = steam_search_title(title);
    let mut url = reqwest::Url::parse("https://store.steampowered.com/api/storesearch/").map_err(|_| ())?;
    url.query_pairs_mut().append_pair("term", &query).append_pair("l", "english").append_pair("cc", "US");
    let response = client.get(url).send().map_err(|_| ())?.error_for_status().map_err(|_| ())?;
    let body = response.text().map_err(|_| ())?;
    let result = serde_json::from_str::<SteamSearchResponse>(&body).map_err(|_| ())?;
    Ok(matching_steam_app_id(&query, &result.items))
}

fn download_steam_art(app_id: u32, cache_dir: &Path) -> Option<String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("Hate.bio Launcher")
        .timeout(std::time::Duration::from_secs(10))
        .build().ok()?;
    let urls = [
        format!("https://cdn.akamai.steamstatic.com/steam/apps/{app_id}/library_600x900.jpg"),
        format!("https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/{app_id}/library_600x900.jpg"),
    ];
    let mut downloaded = None;
    for url in urls {
        let Ok(response) = client.get(url).send().and_then(reqwest::blocking::Response::error_for_status) else { continue; };
        if response.content_length().map(|n| n as usize > MAX_STEAM_ART_BYTES).unwrap_or(false) { continue; }
        let Some(mime) = response.headers().get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .map(str::trim)
            .filter(|mime| *mime == "image/jpeg" || *mime == "image/png")
            .map(str::to_owned) else { continue; };
        let mut bytes = Vec::new();
        if response.take((MAX_STEAM_ART_BYTES + 1) as u64).read_to_end(&mut bytes).is_err()
            || bytes.is_empty() || bytes.len() > MAX_STEAM_ART_BYTES { continue; }
        let image = if mime == "image/jpeg" && bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some((bytes, "jpg", "image/jpeg"))
        } else if mime == "image/png" && bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some((bytes, "png", "image/png"))
        } else { None };
        if image.is_some() { downloaded = image; break; }
    }
    let (bytes, ext, mime) = downloaded?;

    std::fs::create_dir_all(cache_dir).ok()?;
    let path = cache_dir.join(format!("{app_id}.{ext}"));
    let temp = cache_dir.join(format!("{app_id}.{ext}.tmp"));
    std::fs::write(&temp, &bytes).ok()?;
    std::fs::rename(&temp, &path).ok()?;
    Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

fn save_steam_match(app: &AppHandle, game_id: &str, title: &str, app_id: Option<u32>) {
    let state = app.state::<AppState>();
    let mut library = state.lib.lock().unwrap();
    let Some(game) = library.games.iter_mut().find(|g| g.id == game_id && g.name == title) else { return; };
    game.steam_app_id = app_id;
    game.steam_art_checked_at = Some(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0));
    let _ = save(app, &library);
}

fn load_steam_art(app: &AppHandle, game: &Game) -> Option<String> {
    let cache_dir = app.path().app_data_dir().ok()?.join("steam-artwork");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    let recently_checked = game.steam_art_checked_at
        .map(|t| now.saturating_sub(t) < STEAM_ART_RETRY_SECS)
        .unwrap_or(false);
    if let Some(app_id) = game.steam_app_id {
        if let Some(image) = cached_steam_art(app_id, &cache_dir) { return Some(image); }
        if recently_checked { return None; }
        let image = download_steam_art(app_id, &cache_dir);
        if image.is_none() { save_steam_match(app, &game.id, &game.name, Some(app_id)); }
        return image;
    }
    if recently_checked { return None; }
    let app_id = match search_steam_app(&game.name) {
        Ok(id) => id,
        Err(()) => return None,
    };
    save_steam_match(app, &game.id, &game.name, app_id);
    download_steam_art(app_id?, &cache_dir)
}

fn fetch_steam_screenshots(app_id: u32) -> (String, Vec<String>) {
    let Ok(client) = reqwest::blocking::Client::builder()
        .user_agent("Hate.bio Launcher")
        .timeout(std::time::Duration::from_secs(8))
        .build() else { return (String::new(), Vec::new()); };
    let url = format!("https://store.steampowered.com/api/appdetails?appids={app_id}&l=english");
    let Ok(response) = client.get(url).send().and_then(reqwest::blocking::Response::error_for_status) else {
        return (String::new(), Vec::new());
    };
    let Ok(body) = response.text() else { return (String::new(), Vec::new()); };
    let Ok(result) = serde_json::from_str::<SteamDetailsResponse>(&body) else { return (String::new(), Vec::new()); };
    let Some(data) = result.apps.get(&app_id.to_string()).filter(|r| r.success).and_then(|r| r.data.as_ref()) else {
        return (String::new(), Vec::new());
    };
    let screenshots = data.screenshots.iter()
        .map(|s| s.path_full.as_str())
        .filter(|raw| {
            reqwest::Url::parse(raw).map(|u| u.scheme() == "https" && matches!(u.host_str(), Some("shared.akamai.steamstatic.com" | "cdn.akamai.steamstatic.com" | "store.cloudflare.steamstatic.com"))).unwrap_or(false)
        })
        .take(8)
        .map(str::to_owned)
        .collect();
    (data.name.clone(), screenshots)
}

#[tauri::command]
async fn get_store_game_art(app: AppHandle, title: String) -> Result<Option<SteamArtwork>, String> {
    let artwork = tauri::async_runtime::spawn_blocking(move || {
        let app_id = search_steam_app(&title).ok().flatten()?;
        let cache_dir = app.path().app_data_dir().ok()?.join("steam-artwork");
        let cover = cached_steam_art(app_id, &cache_dir).or_else(|| download_steam_art(app_id, &cache_dir));
        Some(SteamArtwork {
            app_id,
            name: title,
            cover,
        })
    }).await.map_err(|e| e.to_string())?;
    Ok(artwork)
}

#[tauri::command]
async fn get_store_game_screenshots(app_id: u32) -> Result<Vec<String>, String> {
    let screenshots = tauri::async_runtime::spawn_blocking(move || fetch_steam_screenshots(app_id).1)
        .await
        .map_err(|e| e.to_string())?;
    Ok(screenshots)
}

#[tauri::command]
async fn get_game_art(app: AppHandle, state: State<'_, AppState>, game_id: String) -> Result<Option<String>, String> {
    let game = state.lib.lock().unwrap().games.iter().find(|g| g.id == game_id).cloned();
    let Some(game) = game else { return Ok(None); };
    let art = tauri::async_runtime::spawn_blocking(move || load_steam_art(&app, &game))
        .await
        .map_err(|e| e.to_string())?;
    Ok(art)
}

#[tauri::command]
fn launch_game(app: AppHandle, state: State<AppState>, id: String, args: Option<String>) -> Result<Library, String> {
    let mut lib = state.lib.lock().unwrap();
    let g = lib.games.iter_mut().find(|g| g.id == id).ok_or("Game not found")?;

    let exe = PathBuf::from(&g.exe_path);
    if !exe.is_file() { return Err(format!("Executable no longer exists:\n{}", g.exe_path)); }
    let workdir = exe.parent().map(|p| p.to_path_buf()).unwrap_or_default();

    if g.source == "store" {
        if let Some(dir) = crate::store::store_game_dir(&app, &exe) {
            if crate::store::nested_unpack_pending(&dir) {
                let cancel = std::sync::atomic::AtomicBool::new(false);
                if let Err(e) = crate::store::extract_archives_with_progress(&app, &g.id, &dir, &cancel) {
                    crate::dlog!("[launch] nested unpack incomplete for {}: {}", g.name, e);
                }
            }
        }
    }

    let mut cmd = std::process::Command::new(&exe);
    cmd.current_dir(&workdir);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    if let Some(a) = args.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
        cmd.args(a.split_whitespace());
    }
    let mut child = cmd.spawn().map_err(|e| format!("Failed to launch {}: {e}", exe.display()))?;
    let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    presence::set(presence::PresenceMsg::Playing { name: g.name.clone(), since });
    std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let _ = child.wait();
        if started.elapsed() >= std::time::Duration::from_secs(90) {
            presence::set(presence::PresenceMsg::Idle);
        }
    });

    g.play_count += 1;
    g.last_played = Some(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0));
    let out = lib.clone();
    drop(lib);
    save(&app, &out)?;
    Ok(out)
}

#[tauri::command]
fn open_folder(path: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        if !Path::new(&path).is_dir() { return Err("Folder does not exist".into()); }
        std::process::Command::new("explorer").arg(&path).spawn().map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err("Only supported on Windows".into())
    }
}

#[tauri::command]
fn reveal_exe(exe_path: String) -> Result<(), String> {
    #[cfg(windows)]
    {
        if !Path::new(&exe_path).is_file() { return Err("File does not exist".into()); }
        std::process::Command::new("explorer").arg(format!("/select,{exe_path}")).spawn().map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = exe_path;
        Err("Only supported on Windows".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fake_exe(dir: &Path, name: &str, size: usize) {
        fs::create_dir_all(dir).unwrap();
        let mut f = fs::File::create(dir.join(name)).unwrap();
        std::io::Write::write_all(&mut f, &vec![0u8; size]).unwrap();
    }

    #[test]
    fn scan_finds_deeply_nested_games_and_roms_and_prunes_junk_dirs() {
        let base = std::env::temp_dir().join(format!("ll_scan_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);

        fake_exe(&base.join(r"Deep\Nested\Folders\Inside\This\Tree"), "DeepGame.exe", 300_000);
        fake_exe(&base.join(r"Shallow\Inner"), "ShallowGame.exe", 300_000);

        fake_exe(&base.join(r"Evil\node_modules"), "Nope.exe", 300_000);
        fake_exe(&base.join(".hidden"), "Nope.exe", 300_000);
        fake_exe(&base.join("Game"), "unins000.exe", 300_000);
        fake_exe(&base.join("Game"), "tiny.exe", 100);

        let found: std::collections::HashSet<String> = scan_folder(&base, 10)
            .iter().map(|p| p.file_name().unwrap().to_string_lossy().to_string()).collect();

        assert_eq!(found, ["DeepGame.exe", "ShallowGame.exe"]
            .into_iter().map(String::from).collect());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn names_clean_up() {
        assert_eq!(clean_name("CoolGame-Win64-Shipping"), "Cool Game");
        assert_eq!(clean_name("my_cool.game"), "My Cool Game");
        assert_eq!(clean_name("Hades2"), "Hades2");
    }

    #[test]
    fn steam_art_matches_only_exact_windows_app_titles() {
        let results = vec![
            SteamSearchItem { id: 620, name: "Portal 2".into(), kind: "app".into(), platforms: Some(SteamPlatforms { windows: true }) },
            SteamSearchItem { id: 400, name: "Portal".into(), kind: "app".into(), platforms: Some(SteamPlatforms { windows: true }) },
            SteamSearchItem { id: 999, name: "Portal".into(), kind: "app".into(), platforms: Some(SteamPlatforms { windows: false }) },
            SteamSearchItem { id: 1000, name: "Portal".into(), kind: "sub".into(), platforms: Some(SteamPlatforms { windows: true }) },
        ];
        assert_eq!(matching_steam_app_id("Portal", &results), Some(400));
        assert_eq!(matching_steam_app_id("Portal 2", &results), Some(620));
        assert_eq!(matching_steam_app_id("Portal Companion", &results), None);
        let subtitle = vec![
            SteamSearchItem { id: 4348910, name: "Probably Stolen - Cyberpunk Shopkeeper Sim".into(), kind: "app".into(), platforms: Some(SteamPlatforms { windows: true }) },
            SteamSearchItem { id: 5113870, name: "Probably Stolen Soundtrack".into(), kind: "app".into(), platforms: Some(SteamPlatforms { windows: true }) },
        ];
        assert_eq!(matching_steam_app_id("Probably Stolen", &subtitle), Some(4348910));
        assert_eq!(matching_steam_app_id("Probably", &subtitle), None);
    }

    #[test]
    fn steam_search_titles_shed_repack_versions() {
        assert_eq!(steam_search_title("Moon Sweepers 1.0.4"), "Moon Sweepers");
        assert_eq!(steam_search_title("Anymaker 0.1.27"), "Anymaker");
        assert_eq!(steam_search_title("Game v1.2"), "Game");
        assert_eq!(steam_search_title("Game V2"), "Game");
        assert_eq!(steam_search_title("Game V2.3-beta"), "Game");
        assert_eq!(steam_search_title("Game Build 16245418"), "Game");
        assert_eq!(steam_search_title("Game V 100-REV3"), "Game");
        assert_eq!(steam_search_title("Game v1.2 Build 123"), "Game");
        assert_eq!(steam_search_title("Portal 2"), "Portal 2");
        assert_eq!(steam_search_title("House Flipper 2"), "House Flipper 2");
        assert_eq!(steam_search_title("Battlefield 1942"), "Battlefield 1942");
        assert_eq!(steam_search_title("7 Days to Die"), "7 Days to Die");
        assert_eq!(steam_search_title("Grand Theft Auto V"), "Grand Theft Auto V");
        assert_eq!(steam_search_title("We Were Here Tomorrow"), "We Were Here Tomorrow");
    }

    #[test]
    fn old_library_entries_default_to_pc() {
        let json = r#"{"id":"x","name":"Old Game","exe_path":"C:\\Old\\game.exe","folder":"C:\\Old","source":"scan","source_folder":"C:\\Old","last_played":null,"play_count":0,"favorite":false,"size_bytes":null}"#;
        let game: Game = serde_json::from_str(json).unwrap();
        assert_eq!(game.kind, "pc");
        assert_eq!(game.system, None);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let _ = store::ensure_resolver_window(app.handle());
            presence::init();
            let lib = load(app.handle());
            app.manage(AppState { lib: Mutex::new(lib) });
            app.manage(store::StoreState::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_library, add_scan_folder, remove_scan_folder, rescan_all,
            add_game, remove_game, rename_game, toggle_favorite,
            get_icon, get_game_art, get_store_game_art, get_store_game_screenshots, launch_game,
            open_folder, reveal_exe,
            store::store_catalog, store::store_catalog_harvested, store::store_sources,
            store::store_harvest_source, store::store_install, store::store_cancel,
            store::store_uninstall, store::store_repair, store::open_url
        ])
        .run(tauri::generate_context!())
        .expect("error while running Hate.bio Launcher");
}
