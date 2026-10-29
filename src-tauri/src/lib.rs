use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Manager, State};
use walkdir::WalkDir;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Game {
    pub id: String,
    pub name: String,
    pub exe_path: String,
    pub folder: String,
    pub source: String,
    pub source_folder: Option<String>,
    #[serde(default)]
    pub last_played: Option<u64>,
    #[serde(default)]
    pub play_count: u32,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default)]
    pub size_bytes: Option<u64>,
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
    library_path(app)
        .and_then(|p| std::fs::read_to_string(p).map_err(|e| e.to_string()))
        .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
        .unwrap_or_default()
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
            SHGFI_ICON | SHGFI_LARGEICON | SHGFI_USEFILEATTRIBUTES,
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
        if found.len() >= 1000 { break; }
        let p = entry.path();
        if !entry.file_type().is_file() { continue; }
        let name = entry.file_name().to_string_lossy();
        if !name.to_lowercase().ends_with(".exe") { continue; }
        if is_junk_exe(&name) { continue; }
        if entry.metadata().map(|m| m.len()).unwrap_or(0) < 200_000 { continue; }
        found.push(p.to_path_buf());
    }
    found.sort();
    found
}

fn game_from_path(exe: &Path, source: &str, source_folder: Option<String>, existing: Option<&Game>) -> Game {
    let stem = exe.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Game".into());
    Game {
        id: id_for(exe),
        name: existing.map(|g| g.name.clone()).unwrap_or_else(|| clean_name(&stem)),
        exe_path: exe.to_string_lossy().to_string(),
        folder: exe.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
        source: source.to_string(),
        source_folder,
        last_played: existing.and_then(|g| g.last_played),
        play_count: existing.map(|g| g.play_count).unwrap_or(0),
        favorite: existing.map(|g| g.favorite).unwrap_or(false),
        size_bytes: std::fs::metadata(exe).ok().map(|m| m.len()),
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
    let mut games: Vec<Game> = lib.games.iter().filter(|g| g.source == "manual").cloned().collect();
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
    extract_icon_data_url(Path::new(&exe_path))
}

#[tauri::command]
fn launch_game(app: AppHandle, state: State<AppState>, id: String, args: Option<String>) -> Result<Library, String> {
    let mut lib = state.lib.lock().unwrap();
    let g = lib.games.iter_mut().find(|g| g.id == id).ok_or("Game not found")?;
    let exe = PathBuf::from(&g.exe_path);
    if !exe.is_file() { return Err(format!("Executable no longer exists:\n{}", g.exe_path)); }
    let workdir = exe.parent().map(|p| p.to_path_buf()).unwrap_or_default();

    let mut cmd = std::process::Command::new(&exe);
    cmd.current_dir(&workdir);
    if let Some(a) = args.as_deref().map(str::trim).filter(|a| !a.is_empty()) {
        cmd.args(a.split_whitespace());
    }
    cmd.spawn().map_err(|e| format!("Failed to launch {}: {e}", exe.display()))?;

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
    fn scan_finds_deeply_nested_exes_and_prunes_junk_dirs() {
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

        assert_eq!(found, ["DeepGame.exe", "ShallowGame.exe"].into_iter().map(String::from).collect());

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn names_clean_up() {
        assert_eq!(clean_name("CoolGame-Win64-Shipping"), "Cool Game");
        assert_eq!(clean_name("my_cool.game"), "My Cool Game");
        assert_eq!(clean_name("Hades2"), "Hades2");
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let lib = load(app.handle());
            app.manage(AppState { lib: Mutex::new(lib) });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_library, add_scan_folder, remove_scan_folder, rescan_all,
            add_game, remove_game, rename_game, toggle_favorite,
            get_icon, launch_game, open_folder, reveal_exe
        ])
        .run(tauri::generate_context!())
        .expect("error while running Hate.bio Launcher");
}
