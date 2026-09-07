use std::fs;
use std::time::Duration;

use anyhow::{Context, Result};
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
    if !domains.is_empty() {
        return domains;
    }
    if let Err(err) = fetch_list() {
        log::warn("gfw", format!("fetch: {err:#}"));
        return Vec::new();
    }
    load_domains()
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
                domain == value || domain.ends_with(&format!(".{value}"))
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
        .trim_start_matches('.')
        .to_ascii_lowercase();
    if item.is_empty() || item.contains('/') || item.contains(':') {
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
    fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
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
