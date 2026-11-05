
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const DONE_MARKER: &str = ".ll-unpacked";

const MAX_PASSES: usize = 12;

const REPACK_PASSWORDS: &[&str] = &[
    "online-fix.me",
    "onlinefix.me.com",
    "OnlineFix.me",
    "onlinefix.me",
    "www.online-fix.me",
    "Online-Fix.me",
    "OFM",
    "ofm",
    "1234",
    "111111",
    "password",
    "repack",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Zip,
    Rar,
    SevenZ,
}

pub fn kind_of(path: &Path) -> Option<Kind> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    if name.ends_with(".zip") {
        Some(Kind::Zip)
    } else if name.ends_with(".rar") {
        Some(Kind::Rar)
    } else if name.ends_with(".7z") {
        Some(Kind::SevenZ)
    } else {
        None
    }
}

pub struct Plan {
    pub path: PathBuf,
    pub kind: Kind,
    pub volumes: Vec<PathBuf>,
}

fn rar_volume_info(name: &str) -> Option<(String, u32)> {
    let lower = name.to_lowercase();
    if let Some(pos) = lower.find(".part") {
        let rest = &lower[pos + 5..];
        if let Some(end) = rest.find(".rar") {
            if let Ok(idx) = rest[..end].parse::<u32>() {
                return Some((lower[..pos].to_string(), idx));
            }
        }
    }
    if lower.ends_with(".rar") {
        return Some((lower[..lower.len() - 4].to_string(), 1));
    }
    if let Some((stem, ext)) = lower.rsplit_once('.') {
        if ext.len() == 3 && ext.starts_with('r') && ext[1..].chars().all(|c| c.is_ascii_digit()) {
            if let Ok(idx) = ext[1..].parse::<u32>() {
                return Some((stem.to_string(), idx + 2));
            }
        }
    }
    None
}

pub fn find_archives(root: &Path) -> Vec<Plan> {
    let mut singles: Vec<(PathBuf, Kind)> = Vec::new();
    let mut rar_sets: BTreeMap<String, Vec<(u32, PathBuf)>> = BTreeMap::new();

    for entry in walkdir::WalkDir::new(root).max_depth(2).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.into_path();
        let Some(kind) = kind_of(&path) else { continue };
        if kind == Kind::Rar {
            let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
            if let Some((base, idx)) = rar_volume_info(&name) {
                rar_sets.entry(base).or_default().push((idx, path));
                continue;
            }
        }
        singles.push((path, kind));
    }

    let mut plans: Vec<Plan> = singles
        .into_iter()
        .map(|(path, kind)| Plan { path: path.clone(), kind, volumes: vec![path] })
        .collect();

    for (_, mut parts) in rar_sets {
        parts.sort_by_key(|(idx, _)| *idx);
        let Some((_, first)) = parts.first().cloned() else { continue };
        let volumes: Vec<PathBuf> = parts.into_iter().map(|(_, p)| p).collect();
        plans.push(Plan { path: first, kind: Kind::Rar, volumes });
    }
    plans.sort_by(|a, b| a.path.cmp(&b.path));
    plans
}

fn safe_join(dest: &Path, rel: &Path) -> Result<PathBuf, String> {
    let mut out = dest.to_path_buf();
    for comp in rel.components() {
        match comp {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            _ => return Err(format!("unsafe path in archive: {}", rel.display())),
        }
    }
    Ok(out)
}

pub struct Progress<'a> {
    done: u64,
    total: u64,
    cb: &'a mut dyn FnMut(u64, u64),
}

impl<'a> Progress<'a> {
    pub fn new(cb: &'a mut dyn FnMut(u64, u64)) -> Self {
        Progress { done: 0, total: 0, cb }
    }
    pub fn add_total(&mut self, bytes: u64) {
        self.total += bytes;
        (self.cb)(self.done, self.total);
    }
    pub fn add_done(&mut self, bytes: u64) {
        self.done += bytes;
        (self.cb)(self.done, self.total);
    }
}

pub fn extract_archive(
    plan: &Plan,
    archive_parent: &Path,
    root: &Path,
    cancel: &AtomicBool,
    progress: &mut Progress<'_>,
) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("Canceled".into());
    }
    match plan.kind {
        Kind::Zip => extract_zip(&plan.path, archive_parent, root, cancel, progress),
        Kind::Rar => extract_rar(plan, archive_parent, root, cancel, progress),
        Kind::SevenZ => extract_7z(&plan.path, archive_parent, root, cancel, progress),
    }
}

fn extract_zip(
    path: &Path,
    archive_parent: &Path,
    _root: &Path,
    cancel: &AtomicBool,
    progress: &mut Progress<'_>,
) -> Result<(), String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file))
        .map_err(|e| format!("corrupt zip: {e}"))?;

    let total: u64 = (0..archive.len())
        .map(|i| archive.by_index(i).map(|e| e.size()).unwrap_or(0))
        .sum();
    progress.add_total(total);

    let dest = archive_parent;
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;

    for i in 0..archive.len() {
        if cancel.load(Ordering::Relaxed) {
            return Err("Canceled".into());
        }
        let mut entry = archive.by_index(i).map_err(|e| format!("corrupt zip: {e}"))?;
        let Some(rel) = entry.enclosed_name().map(|p| p.to_path_buf()) else {
            continue;
        };
        let out = safe_join(dest, &rel)?;
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut writer = std::fs::File::create(&out).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut writer).map_err(|e| format!("extraction interrupted: {e}"))?;
        progress.add_done(entry.size());
    }
    Ok(())
}

fn rar_first_header(path: &Path, password: Option<&str>) -> Result<bool, String> {
    let listing = match password {
        Some(pw) => unrar::Archive::with_password(path, pw).open_for_listing(),
        None => unrar::Archive::new(path).open_for_listing(),
    }
    .map_err(|e| e.to_string())?;
    let cursor = listing;
    match cursor.read_header() {
        Ok(Some(_)) => Ok(true),
        Ok(None) => Ok(true),
        Err(e) => Err(e.to_string()),
    }
}

fn rar_password(path: &Path) -> Result<Option<&'static str>, String> {
    match rar_first_header(path, None) {
        Ok(_) => return Ok(None),
        Err(e) if !e.to_lowercase().contains("password") => {
            return Err(format!("cannot read rar: {e}"));
        }
        Err(_) => {}
    }
    for pw in REPACK_PASSWORDS {
        if rar_first_header(path, Some(pw)).is_ok() {
            return Ok(Some(pw));
        }
    }
    Err("this archive is password-protected and no known password worked".into())
}

fn extract_rar(
    plan: &Plan,
    archive_parent: &Path,
    _root: &Path,
    cancel: &AtomicBool,
    progress: &mut Progress<'_>,
) -> Result<(), String> {
    let password = rar_password(&plan.path)?;

    let mut total = 0u64;
    {
        let listing = match password {
            Some(pw) => unrar::Archive::with_password(&plan.path, pw).open_for_listing(),
            None => unrar::Archive::new(&plan.path).open_for_listing(),
        }
        .map_err(|e| format!("cannot read rar: {e}"))?;
        let mut cursor = listing;
        while let Some(header) = cursor.read_header().map_err(|e| format!("rar: {e}"))? {
            total += header.entry().unpacked_size;
            cursor = header.skip().map_err(|e| format!("rar: {e}"))?;
        }
    }
    progress.add_total(total);
    let dest = archive_parent;
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;

    let archive = match password {
        Some(pw) => unrar::Archive::with_password(&plan.path, pw).open_for_processing(),
        None => unrar::Archive::new(&plan.path).open_for_processing(),
    }
    .map_err(|e| format!("cannot read rar: {e}"))?;
    let mut cursor = archive;
    while let Some(header) = cursor.read_header().map_err(|e| format!("rar: {e}"))? {
        if cancel.load(Ordering::Relaxed) {
            return Err("Canceled".into());
        }
        let name = header.entry().filename.clone();
        let size = header.entry().unpacked_size;
        if header.entry().is_directory() {
            let out = safe_join(dest, &name)?;
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            cursor = header.skip().map_err(|e| format!("rar: {e}"))?;
            continue;
        }
        let out = safe_join(dest, &name)?;
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        cursor = header
            .extract_to(&out)
            .map_err(|e| format!("rar: {e}"))?;
        progress.add_done(size);
    }
    Ok(())
}

fn extract_7z(
    path: &Path,
    archive_parent: &Path,
    _root: &Path,
    cancel: &AtomicBool,
    progress: &mut Progress<'_>,
) -> Result<(), String> {
    let mut password: Option<&'static str> = None;
    let archive = match sevenz_rust::Archive::open(path) {
        Ok(a) => a,
        Err(first) => {
            let mut found = None;
            for pw in REPACK_PASSWORDS {
                if let Ok(a) = sevenz_rust::Archive::open_with_password(path, &sevenz_rust::Password::from(*pw)) {
                    found = Some((a, *pw));
                    break;
                }
            }
            match found {
                Some((a, pw)) => { password = Some(pw); a }
                None => return Err(format!("cannot open 7z: {first:?}")),
            }
        }
    };
    let total: u64 = archive.files.iter().map(|e| e.size()).sum();
    progress.add_total(total);

    let dest = archive_parent;
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;

    let mut done = 0u64;
    let mut cancelled = false;
    let extract_fn = |entry: &sevenz_rust::SevenZArchiveEntry,
                      reader: &mut dyn std::io::Read,
                      _crate_path: &PathBuf| {
        if cancel.load(Ordering::Relaxed) {
            cancelled = true;
            return Err(sevenz_rust::Error::other("canceled"));
        }
        let name = PathBuf::from(entry.name());
        let out = match safe_join(&dest, &name) {
            Ok(p) => p,
            Err(_) => return Ok(true),
        };
        if entry.is_directory() {
            std::fs::create_dir_all(&out).map_err(sevenz_rust::Error::io)?;
            return Ok(true);
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(sevenz_rust::Error::io)?;
        }
        let mut writer = std::fs::File::create(&out).map_err(sevenz_rust::Error::io)?;
        std::io::copy(reader, &mut writer).map_err(sevenz_rust::Error::io)?;
        done += entry.size();
        Ok(true)
    };

    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    match password {
        Some(pw) => sevenz_rust::decompress_with_extract_fn_and_password(
            &mut file,
            &dest,
            sevenz_rust::Password::from(pw),
            extract_fn,
        ),
        None => sevenz_rust::decompress_with_extract_fn(&mut file, &dest, extract_fn),
    }
    .map_err(|e| format!("7z: {e:?}"))?;

    if cancelled {
        return Err("Canceled".into());
    }
    progress.add_done(done);
    Ok(())
}

pub fn extract_all(
    root: &Path,
    cancel: &AtomicBool,
    progress: &mut Progress<'_>,
) -> Result<(), String> {
    let mut failures: Vec<String> = Vec::new();
    let mut attempted: Vec<PathBuf> = Vec::new();

    for _ in 0..MAX_PASSES {
        if cancel.load(Ordering::Relaxed) {
            return Err("Canceled".into());
        }
        let plans = find_archives(root);
        let mut worked_this_pass = false;
        for plan in plans {
            if cancel.load(Ordering::Relaxed) {
                return Err("Canceled".into());
            }
            if attempted.contains(&plan.path) {
                continue;
            }
            attempted.push(plan.path.clone());
            let dest = plan.path.parent().unwrap_or(root).to_path_buf();
            match extract_archive(&plan, &dest, root, cancel, progress) {
                Ok(()) => {
                    worked_this_pass = true;
                    for vol in &plan.volumes {
                        let _ = std::fs::remove_file(vol);
                    }
                }
                Err(e) if e == "Canceled" => return Err(e),
                Err(e) => {
                    let msg = format!("{}: {}", plan.path.file_name().unwrap_or_default().to_string_lossy(), e);
                    failures.push(msg);
                }
            }
        }
        if !worked_this_pass {
            break;
        }
    }

    if failures.is_empty() {
        let _ = std::fs::write(root.join(DONE_MARKER), b"ok");
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

pub fn has_archives(root: &Path) -> bool {
    !find_archives(root).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rar_volume_names_are_grouped() {
        assert_eq!(rar_volume_info("Game.part1.rar"), Some(("game".into(), 1)));
        assert_eq!(rar_volume_info("Game.part12.rar"), Some(("game".into(), 12)));
        assert_eq!(rar_volume_info("Game.rar"), Some(("game".into(), 1)));
        assert_eq!(rar_volume_info("Game.r00"), Some(("game".into(), 2)));
        assert_eq!(rar_volume_info("Game.rar"), Some(("game".into(), 1)));
        assert_eq!(rar_volume_info("notes.txt"), None);
    }

    #[test]
    fn archive_kinds_are_detected() {
        assert_eq!(kind_of(Path::new("a/b.zip")), Some(Kind::Zip));
        assert_eq!(kind_of(Path::new("a/b.RAR")), Some(Kind::Rar));
        assert_eq!(kind_of(Path::new("a/b.7z")), Some(Kind::SevenZ));
        assert_eq!(kind_of(Path::new("a/b.exe")), None);
    }

    #[test]
    fn unsafe_entry_names_are_rejected() {
        let dest = Path::new("/tmp/dest");
        assert!(safe_join(dest, Path::new("../evil.txt")).is_err());
        assert!(safe_join(dest, Path::new("/etc/passwd")).is_err());
        assert!(safe_join(dest, Path::new("game/data.bin")).is_ok());
    }

    #[test]
    #[ignore]
    fn extract_real_archive() {
        let src = std::env::var("LL_TEST_ARCHIVE").expect("set LL_TEST_ARCHIVE");
        let dest = std::env::temp_dir().join("ll-archive-test");
        let _ = std::fs::remove_dir_all(&dest);
        std::fs::create_dir_all(&dest).unwrap();
        let path = PathBuf::from(&src);
        let kind = kind_of(&path).expect("unsupported archive");
        let plan = Plan { path: path.clone(), kind, volumes: vec![path.clone()] };
        let mut cb = |done: u64, total: u64| {
            println!("[extract] {done}/{total}");
        };
        let mut progress = Progress::new(&mut cb);
        extract_archive(&plan, &dest, &dest, &AtomicBool::new(false), &mut progress).unwrap();
        let mut exes = Vec::new();
        for e in walkdir::WalkDir::new(&dest).into_iter().filter_map(|e| e.ok()) {
            if e.file_type().is_file() {
                let n = e.file_name().to_string_lossy().to_string();
                if n.to_lowercase().ends_with(".exe") {
                    exes.push(e.path().to_string_lossy().to_string());
                }
            }
        }
        println!("[extract] exes found: {exes:#?}");
        assert!(!exes.is_empty(), "no exe in extracted archive");
    }
}
