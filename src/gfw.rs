use std::fs;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::log;
use crate::paths;

pub const LIST_URL: &str = "https://cdn.jsdelivr.net/gh/Loyalsoldier/clash-rules@release/gfw.txt";
pub const PROVIDER: &str = "gfw";
pub const RULESET_REL: &str = "./ruleset/gfw.yaml";

#[derive(Deserialize)]
struct GfwFile {
    #[serde(default)]
    payload: Vec<String>,
}

pub fn list_path() -> Result<std::path::PathBuf> {
    Ok(paths::ruleset_dir()?.join("gfw.yaml"))
}

pub fn load_domains() -> Vec<String> {
    let path = match list_path() {
        Ok(path) => path,
        Err(err) => {
            log::warn("gfw", format!("ruleset path: {err:#}"));
            return Vec::new();
        }
    };
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => return Vec::new(),
    };
    parse_payload(&text)
}

pub fn ensure_domains() -> Vec<String> {
    let domains = load_domains();
    let fresh = list_path()
        .ok()
        .and_then(|path| fs::metadata(path).ok())
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age < Duration::from_secs(24 * 60 * 60));
    if !domains.is_empty() && fresh {
        return domains;
    }
    match refresh_domains() {
        Ok(refreshed) => refreshed,
        Err(err) => {
            if domains.is_empty() {
                log::warn("gfw", format!("list unavailable: {err:#}"));
            } else {
                log::warn(
                    "gfw",
                    "refresh failed; retaining the last valid cached list",
                );
            }
            domains
        }
    }
}

/// Reused by explicit subscription/list refresh; a bad download never replaces
/// the last valid file consumed by either the host or Mihomo rule provider.
pub fn refresh_domains() -> Result<Vec<String>> {
    fetch_list()?;
    Ok(load_domains())
}

pub fn parse_payload(text: &str) -> Vec<String> {
    if let Ok(file) = serde_yaml::from_str::<GfwFile>(text) {
        let mut out: Vec<String> = file
            .payload
            .iter()
            .filter_map(|item| normalize_entry(item))
            .collect();
        out.sort();
        out.dedup();
        return out;
    }
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("payload:") {
            continue;
        }
        let item = line.trim_start_matches('-').trim().trim_matches('"');
        if let Some(domain) = normalize_entry(item) {
            out.push(domain);
        }
    }
    out.sort();
    out.dedup();
    out
}

pub fn gfw_group(via: &str) -> Option<&str> {
    let via = via.trim();
    let lower = via.to_ascii_lowercase();
    for prefix in ["gfw:", "gfwlist:"] {
        if lower.starts_with(prefix) {
            let group = via[prefix.len()..].trim();
            if !group.is_empty() {
                return Some(group);
            }
        }
    }
    None
}

pub fn domain_matches(kind: &str, value: &str, domain: &str) -> bool {
    let domain = domain.trim().trim_start_matches('.').to_ascii_lowercase();
    let value = value.trim().to_ascii_lowercase();
    if domain.is_empty() || value.is_empty() {
        return false;
    }
    match kind {
        "keyword" => domain.contains(&value),
        "suffix" => suffix_match(value.trim_start_matches('.'), &domain),
        "domain" => {
            if let Some(suffix) = value.strip_prefix("*.") {
                suffix_match(suffix, &domain)
            } else {
                domain == value
            }
        }
        _ => false,
    }
}

fn suffix_match(suffix: &str, domain: &str) -> bool {
    let suffix = suffix.trim_start_matches('.');
    domain == suffix || domain.ends_with(&format!(".{suffix}"))
}

fn normalize_entry(item: &str) -> Option<String> {
    let item = item.trim().trim_matches('\'').trim_matches('"');
    let item = item
        .trim_start_matches("+.")
        .trim_matches('.')
        .to_ascii_lowercase();
    if item.is_empty()
        || item.len() > 253
        || !item.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|ch| ch.is_alphanumeric() || ch == '-')
        })
    {
        return None;
    }
    Some(item)
}

fn fetch_list() -> Result<()> {
    let path = list_path()?;
    let body = ureq::get(LIST_URL)
        .timeout(Duration::from_secs(8))
        .call()
        .with_context(|| format!("GET {LIST_URL}"))?
        .into_string()
        .context("read gfw list")?;
    if parse_payload(&body).is_empty() {
        bail!("GFWList download contains no valid domains");
    }
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    fs::write(&temporary, body).context("write GFWList candidate")?;
    if let Err(error) = fs::rename(&temporary, &path) {
        let _ = fs::remove_file(&temporary);
        return Err(error).context("publish GFWList candidate");
    }
    log::info("gfw", format!("cached {}", path.display()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_payload_plus_dot() {
        let domains = parse_payload("payload:\n  - '+.google.com'\n  - github.com\n");
        assert_eq!(domains, vec!["github.com", "google.com"]);
    }

    #[test]
    fn gfw_group_prefix() {
        assert_eq!(gfw_group("gfw:Default"), Some("Default"));
        assert_eq!(gfw_group("GFWList:Telegram"), Some("Telegram"));
        assert_eq!(gfw_group("Default"), None);
    }

    #[test]
    fn domain_matchers() {
        assert!(domain_matches("suffix", "google.com", "www.google.com"));
        assert!(domain_matches("domain", "*.google.com", "mail.google.com"));
        assert!(domain_matches("keyword", "github", "api.github.com"));
        assert!(!domain_matches("suffix", "google.com", "notgoogle.com"));
    }
}
