
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::store::{StoreCatalog, StoreGame};

#[derive(Deserialize)]
pub struct RawCatalog {
    pub downloads: Vec<RawEntry>,
}

#[derive(Deserialize, Clone)]
pub struct RawEntry {
    pub title: String,
    #[serde(default)]
    pub uris: Vec<String>,
    #[serde(default, rename = "uploadDate")]
    pub upload_date: Option<String>,
    #[serde(default, rename = "fileSize")]
    pub file_size: Option<String>,
    #[serde(default, rename = "descriptionHtml")]
    pub description_html: Option<String>,
}

pub fn parse_title(title: &str) -> (String, String) {
    const DASHES: [char; 3] = ['\u{2013}', '\u{2014}', '\u{2012}'];
    for d in DASHES {
        if let Some(idx) = title.find(d) {
            let name = title[..idx].trim().to_string();
            let version = title[idx + d.len_utf8()..].trim().to_string();
            if !name.is_empty() {
                return (name, version);
            }
        }
    }
    (title.trim().to_string(), String::new())
}

pub fn parse_size_mb(label: &str) -> f64 {
    let s = label.trim();
    if s.is_empty() {
        return 0.0;
    }
    let num: String = s
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',')
        .collect();
    let num = num.replace(',', ".");
    let value: f64 = match num.parse() {
        Ok(v) => v,
        Err(_) => return 0.0,
    };
    let unit = s.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ',');
    let unit = unit.trim().to_lowercase();
    if unit.starts_with('t') {
        value * 1024.0 * 1024.0
    } else if unit.starts_with('g') {
        value * 1024.0
    } else if unit.starts_with('m') {
        value
    } else if unit.starts_with('k') {
        value / 1024.0
    } else {
        value / (1024.0 * 1024.0)
    }
}

pub fn parse_upload_date(date: &str) -> u64 {
    let b: Vec<u8> = date.bytes().collect();
    if b.len() < 19 {
        return 0;
    }
    let digits = |r: std::ops::Range<usize>| -> Option<i64> {
        let s: String = b[r].iter().map(|&c| c as char).collect();
        s.parse().ok()
    };
    let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(se)) = (
        digits(0..4),
        digits(5..7),
        digits(8..10),
        digits(11..13),
        digits(14..16),
        digits(17..19),
    ) else {
        return 0;
    };
    let y = if mo <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719_468;
    let secs = days * 86400 + h * 3600 + mi * 60 + se;
    u64::try_from(secs).unwrap_or(0)
}

pub fn split_uri(uri: &str) -> (String, String, String) {
    let trimmed = uri.trim();
    if trimmed.to_lowercase().starts_with("magnet:") {
        let dn = extract_magnet_param(trimmed, "dn")
            .unwrap_or_default()
            .trim()
            .to_string();
        let hash = extract_magnet_param(trimmed, "xt")
            .map(|v| {
                v.rsplit(':').next().unwrap_or(&v).to_string()
            })
            .unwrap_or_default()
            .to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>();
        let short = if !hash.is_empty() {
            hash[..hash.len().min(12)].to_string()
        } else if !dn.is_empty() {
            slug_from_dn(&dn)
        } else {
            String::new()
        };
        let fname = if dn.is_empty() {
            short.clone()
        } else {
            dn
        };
        let ext = fname
            .rsplit_once('.')
            .map(|(_, e)| e.to_lowercase())
            .unwrap_or_default();
        return (short, fname, ext);
    }
    let path = trimmed
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let Some(rest_start) = path.find('/') else {
        return (String::new(), String::new(), String::new());
    };
    let rest = &path[rest_start + 1..];
    let (code, fname) = match rest.find('/') {
        Some(i) => (rest[..i].to_string(), rest[i + 1..].to_string()),
        None => (rest.to_string(), String::new()),
    };
    let ext = fname
        .rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default();
    (code, fname, ext)
}

fn extract_magnet_param(magnet: &str, key: &str) -> Option<String> {
    let q = magnet.find('?')?;
    let query = &magnet[q + 1..];
    for part in query.split('&') {
        if let Some((k, v)) = part.split_once('=') {
            if k.eq_ignore_ascii_case(key) || (key == "xt" && k.eq_ignore_ascii_case("xt")) {
                return Some(percent_decode(v));
            }
        }
    }
    if key == "xt" {
        for part in query.split('&') {
            if part.to_lowercase().starts_with("xt") {
                if let Some((_, v)) = part.split_once('=') {
                    return Some(percent_decode(v));
                }
            }
        }
    }
    None
}

fn slug_from_dn(dn: &str) -> String {
    let mut s = String::new();
    for ch in dn.chars() {
        if ch.is_alphanumeric() || ch == ' ' {
            s.extend(ch.to_lowercase());
        } else if ch == '-' || ch == '_' || ch == '.' {
            s.push(' ');
        }
    }
    let joined: String = s.split_whitespace().collect::<Vec<_>>().join("-");
    if joined.is_empty() {
        dn.chars()
            .filter(|c| c.is_alphanumeric())
            .take(16)
            .collect::<String>()
            .to_lowercase()
    } else {
        joined.chars().take(48).collect()
    }
}

pub fn extract_page_url(html: &str) -> String {
    const NEEDLE: &str = "href=\"";
    let Some(start) = html.find(NEEDLE) else {
        return String::new();
    };
    let rest = &html[start + NEEDLE.len()..];
    let end = rest.find('"').unwrap_or(0);
    let url = rest[..end].trim().to_string();
    if url.starts_with("http://") || url.starts_with("https://") {
        url
    } else {
        String::new()
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UriKind {
    Magnet,
    Datanodes,
    OtherHttp,
    Unknown,
}

pub fn classify_uri(uri: &str) -> UriKind {
    let t = uri.trim().to_lowercase();
    if t.starts_with("magnet:") {
        UriKind::Magnet
    } else if t.contains("datanodes.to/") {
        UriKind::Datanodes
    } else if t.starts_with("http://") || t.starts_with("https://") {
        UriKind::OtherHttp
    } else {
        UriKind::Unknown
    }
}

pub fn percent_decode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn build_catalog_from_raw(raw: RawCatalog, source_label: &str, now: u64) -> StoreCatalog {
    let mut games = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for entry in raw.downloads {
        let mut chosen: Option<String> = None;
        for u in entry.uris.iter().map(|u| u.trim()) {
            if classify_uri(u) == UriKind::Magnet {
                chosen = Some(u.to_string());
                break;
            }
        }
        if chosen.is_none() {
            chosen = entry
                .uris
                .iter()
                .map(|u| u.trim())
                .find(|u| classify_uri(u) == UriKind::Datanodes)
                .map(|s| s.to_string());
        }
        let Some(uri) = chosen else { continue };
        let kind = classify_uri(&uri);
        let (code, _fname, ext) = split_uri(&uri);
        if code.is_empty() || !seen.insert(format!("{source_label}:{code}")) {
            continue;
        }
        if kind == UriKind::Datanodes && !matches!(ext.as_str(), "zip" | "rar" | "7z") {
            continue;
        }
        let (name, version) = parse_title(&entry.title);
        let name = name.trim().to_string();
        if name.chars().filter(|c| c.is_alphanumeric()).count() < 2 {
            continue;
        }
        let size_mb = parse_size_mb(entry.file_size.as_deref().unwrap_or(""));
        if size_mb <= 0.0 {
            continue;
        }
        games.push(StoreGame {
            id: format!("{source_label}:{code}"),
            name,
            version,
            size_label: entry.file_size.unwrap_or_default(),
            size_mb,
            uploaded: parse_upload_date(entry.upload_date.as_deref().unwrap_or("")),
            ext: if ext.is_empty() && kind == UriKind::Magnet {
                String::new()
            } else {
                ext
            },
            page_url: entry
                .description_html
                .as_deref()
                .map(extract_page_url)
                .unwrap_or_default(),
            uri: uri.clone(),
            source: source_label.to_string(),
        });
    }
    games.truncate(6000);
    StoreCatalog {
        fetched_at: now,
        games,
    }
}

#[cfg(test)]
pub fn build_catalog(raw: RawCatalog) -> StoreCatalog {
    build_catalog_from_raw(raw, "ankergames", now_secs())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HydraSource {
    pub label: String,
    pub url: String,
}

impl HydraSource {
    pub fn new(label: &str, url: &str) -> Self {
        Self {
            label: label.to_string(),
            url: url.to_string(),
        }
    }
}

pub fn default_hydra_sources() -> Vec<HydraSource> {
    vec![
        HydraSource::new("ankergames", "https://wkeynhk.online/ankergames.json"),
        HydraSource::new("rutor", "https://wkeynhk.online/rutor.json"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magnet_split_extracts_hash_and_dn() {
        let magnet = "magnet:?xt=urn:btih:ABCDEF1234567890ABCDEF1234567890ABCDEF12&dn=My%20Game%20Repack";
        let (code, fname, _ext) = split_uri(magnet);
        assert_eq!(code.len(), 12);
        assert!(fname.to_lowercase().contains("my game"));
    }

    #[test]
    fn magnet_percent_decoded_dn_used_as_fname() {
        let m = "magnet:?xt=urn:btih:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa&dn=The%20Game%20v1.2";
        let (_, fname, _) = split_uri(m);
        assert_eq!(fname, "The Game v1.2");
    }
}
