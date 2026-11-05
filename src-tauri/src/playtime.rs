use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use tauri::{AppHandle, Emitter, Manager};

use crate::presence;

static TRACKING: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn tracked() -> &'static Mutex<Option<HashSet<String>>> {
    &TRACKING
}

pub fn start_session(app: AppHandle, game_id: String, mut child: std::process::Child, root: PathBuf, exact_exe: Option<PathBuf>) {
    {
        let mut guard = tracked().lock().unwrap();
        let set = guard.get_or_insert_with(HashSet::new);
        if !set.insert(game_id.clone()) {
            return;
        }
    }
    std::thread::spawn(move || {
        let start = Instant::now();
        let mut last = start;
        let mut unflushed: u64 = 0;
        let mut misses = 0u32;
        let mut child_open = true;
        loop {
            std::thread::sleep(std::time::Duration::from_secs(5));
            let now = Instant::now();
            let alive = is_alive(&mut child, &mut child_open, &root, exact_exe.as_deref());
            if alive {
                let delta = now.duration_since(last).as_secs().min(30);
                unflushed += delta;
                misses = 0;
            } else if now.duration_since(start).as_secs() >= 45 {
                misses += 1;
            }
            last = now;
            if unflushed >= 60 {
                flush(&app, &game_id, &mut unflushed);
            }
            if now.duration_since(start).as_secs() >= 45 && misses >= 2 {
                break;
            }
        }
        flush(&app, &game_id, &mut unflushed);
        presence::set(presence::PresenceMsg::Idle);
        if let Some(set) = tracked().lock().unwrap().as_mut() {
            set.remove(&game_id);
        }
    });
}

fn flush(app: &AppHandle, game_id: &str, unflushed: &mut u64) {
    let secs = *unflushed;
    if secs == 0 { return; }
    let state = app.state::<crate::AppState>();
    let mut lib = state.lib.lock().unwrap();
    let Some(g) = lib.games.iter_mut().find(|g| g.id == game_id) else { return; };
    g.playtime_secs = g.playtime_secs.saturating_add(secs);
    let out = lib.clone();
    drop(lib);
    if crate::save(app, &out).is_ok() {
        *unflushed = 0;
        let _ = app.emit("library-changed", &out);
    }
}

fn is_alive(child: &mut std::process::Child, child_open: &mut bool, root: &Path, exact_exe: Option<&Path>) -> bool {
    if *child_open {
        match child.try_wait() {
            Ok(None) => return true,
            _ => *child_open = false,
        }
    }
    path_matches_running(root, exact_exe)
}

#[cfg(windows)]
fn path_matches_running(root: &Path, exact_exe: Option<&Path>) -> bool {
    running_image_paths().iter().any(|p| image_matches(p, root, exact_exe))
}

#[cfg(not(windows))]
fn path_matches_running(_root: &Path, _exact_exe: Option<&Path>) -> bool {
    false
}

fn image_matches(image: &str, root: &Path, exact_exe: Option<&Path>) -> bool {
    if let Some(exe) = exact_exe {
        return norm_path(&exe.to_string_lossy()) == norm_path(image);
    }
    let nroot = norm_path(&root.to_string_lossy());
    let nimg = norm_path(image);
    nimg.len() > nroot.len() && nimg.starts_with(&nroot) && nimg.as_bytes()[nroot.len()] == b'\\'
}

fn norm_path(p: &str) -> String {
    p.replace('/', "\\").to_lowercase()
}

#[cfg(windows)]
fn running_image_paths() -> Vec<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};

    let mut out = Vec::new();
    let own = std::process::id();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut entry).is_ok() {
            loop {
                if entry.th32ProcessID != own {
                    if let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, entry.th32ProcessID) {
                        let mut buf = [0u16; 1024];
                        let mut len = buf.len() as u32;
                        if QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut len).is_ok() && len > 0 {
                            out.push(String::from_utf16_lossy(&buf[..len as usize]));
                        }
                        let _ = CloseHandle(h);
                    }
                }
                if Process32NextW(snap, &mut entry).is_err() { break; }
            }
        }
        let _ = CloseHandle(snap);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_prefix_respects_components() {
        let root = Path::new("C:\\Games\\Foo");
        assert!(image_matches("c:/games/foo/game.exe", root, None));
        assert!(!image_matches("C:\\Games\\FooBar\\x.exe", root, None));
        assert!(!image_matches("C:\\Games\\Foo", root, None));
        assert!(!image_matches("D:\\other\\x.exe", root, None));
    }

    #[test]
    fn exact_mode_matches_only_exe() {
        let exe = PathBuf::from("C:\\Games\\x.exe");
        assert!(image_matches("c:\\games\\x.exe", Path::new("C:\\Games"), Some(&exe)));
        assert!(!image_matches("C:\\Games\\other\\y.exe", Path::new("C:\\Games"), Some(&exe)));
    }

    #[test]
    fn shallow_root_uses_exact() {
        fn depth(p: &Path) -> usize {
            p.components().filter(|c| matches!(c, std::path::Component::Normal(_))).count()
        }
        assert!(depth(Path::new("C:\\")) < 2);
        assert!(depth(Path::new("C:\\Games")) < 2);
        assert!(depth(Path::new("C:\\Games\\Foo")) >= 2);
    }
}
