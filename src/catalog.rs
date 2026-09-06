use std::collections::HashMap;
use std::fs;
use std::process::Command;

use anyhow::{Context, Result, bail};
use base64::Engine;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::log;
use crate::paths;
use crate::strategy::Strategy;

const SUBSCRIPTION_CURL_MAX_TIME: &str = "8";
const SUBSCRIPTION_CURL_CONNECT_TIMEOUT: &str = "3";
const SUBSCRIPTION_HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Catalog {
    pub nodes: Vec<Node>,
    pub excluded: Vec<Excluded>,
    #[serde(default)]
    pub subscription_urls: HashMap<String, String>,
    #[serde(default)]
    pub exclude_filter: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    pub name: String,
    pub subscription: String,
    pub raw: serde_yaml::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Excluded {
    pub name: String,
    pub subscription: String,
    pub reason: String,
}

impl Catalog {
    pub fn load() -> Result<Self> {
        let path = paths::catalog_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let data = fs::read_to_string(&path)?;
        serde_json::from_str(&data).context("parse catalog.json")
    }

    pub fn save(&self) -> Result<()> {
        let path = paths::catalog_path()?;
        fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

pub fn refresh(strategy: &Strategy) -> Result<Catalog> {
    let exclude = Regex::new(&strategy.exclude_filter)
        .context("invalid exclude_filter regex")
        .inspect_err(|err| log::error("catalog", format!("{err:#}")))?;
    let previous = Catalog::load().unwrap_or_default();
    let cache_filter_matches = previous.exclude_filter == strategy.exclude_filter;
    let mut catalog = Catalog::default();
    log::debug(
        "catalog",
        format!("refresh {} subscriptions", strategy.subscriptions.len()),
    );

    for sub in &strategy.subscriptions {
        match fetch_proxies(&sub.name, &sub.url) {
            Ok(proxies) => {
                let before = catalog.nodes.len();
                for raw in proxies {
                    let original = raw
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unnamed")
                        .to_string();
                    if exclude.is_match(&original) {
                        catalog.excluded.push(Excluded {
                            name: original,
                            subscription: sub.name.clone(),
                            reason: "exclude_filter".into(),
                        });
                        continue;
                    }
                    let mut prefixed = raw.clone();
                    let name = format!("{} · {}", sub.name, original);
                    if let serde_yaml::Value::Mapping(map) = &mut prefixed {
                        map.insert(
                            serde_yaml::Value::String("name".into()),
                            serde_yaml::Value::String(name.clone()),
                        );
                    }
                    catalog.nodes.push(Node {
                        name,
                        subscription: sub.name.clone(),
                        raw: prefixed,
                    });
                }
                log::debug(
                    "catalog",
                    format!("{} kept {} nodes", sub.name, catalog.nodes.len() - before),
                );
            }
            Err(err) => {
                log::warn("catalog", format!("fetch {} failed: {err:#}", sub.name));
                let reused = if cache_filter_matches
                    && previous.subscription_urls.get(&sub.name) == Some(&sub.url)
                {
                    previous
                        .nodes
                        .iter()
                        .filter(|node| node.subscription == sub.name)
                        .cloned()
                        .collect::<Vec<_>>()
                } else {
                    Vec::new()
                };
                if !reused.is_empty() {
                    log::info(
                        "catalog",
                        format!(
                            "{} reused {} cached nodes after fetch failure",
                            sub.name,
                            reused.len()
                        ),
                    );
                    catalog.nodes.extend(reused);
                }
                catalog.excluded.push(Excluded {
                    name: format!("<{}>", sub.name),
                    subscription: sub.name.clone(),
                    reason: format!("fetch failed: {err:#}"),
                });
            }
        }
    }

    catalog.exclude_filter = strategy.exclude_filter.clone();
    catalog.subscription_urls = strategy
        .subscriptions
        .iter()
        .map(|sub| (sub.name.clone(), sub.url.clone()))
        .collect();

    catalog.save()?;
    log::info(
        "catalog",
        format!(
            "refresh nodes={} excluded={}",
            catalog.nodes.len(),
            catalog.excluded.len()
        ),
    );
    Ok(catalog)
}

fn fetch_proxies(name: &str, url: &str) -> Result<Vec<serde_yaml::Value>> {
    let body = if let Some(path) = url.strip_prefix("file://") {
        fs::read_to_string(path).with_context(|| format!("read {name}"))?
    } else if url.starts_with("http://") || url.starts_with("https://") {
        fetch_http_body(name, url)?
    } else {
        fs::read_to_string(url).with_context(|| format!("read {name}"))?
    };
    parse_subscription(&body)
}

fn fetch_http_body(name: &str, url: &str) -> Result<String> {
    // ureq 2 is HTTP/1.1 only and has no Happy Eyeballs. Cloudflare AAAA
    // records that blackhole (Cunoe) hit the 30s read timeout whenever IPv6
    // is tried first. Prefer curl --ipv4 on macOS; fall back to ureq.
    match fetch_http_body_curl_ipv4(url) {
        Ok(body) => Ok(body),
        Err(err) => {
            log::warn(
                "catalog",
                format!("{name} IPv4 curl failed ({err:#}), trying default HTTP"),
            );
            ureq::get(url)
                .timeout(SUBSCRIPTION_HTTP_TIMEOUT)
                .set("User-Agent", "clash.meta")
                .call()
                .with_context(|| format!("GET {name}"))?
                .into_string()
                .with_context(|| format!("read {name}"))
        }
    }
}

fn fetch_http_body_curl_ipv4(url: &str) -> Result<String> {
    let output = Command::new("curl")
        .args([
            "-fsSL",
            "--ipv4",
            "-A",
            "clash.meta",
            "--max-time",
            SUBSCRIPTION_CURL_MAX_TIME,
            "--connect-timeout",
            SUBSCRIPTION_CURL_CONNECT_TIMEOUT,
            url,
        ])
        .output()
        .context("spawn curl")?;
    if !output.status.success() {
        bail!("curl IPv4 failed");
    }
    String::from_utf8(output.stdout).context("curl body is not UTF-8")
}

fn parse_subscription(body: &str) -> Result<Vec<serde_yaml::Value>> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        bail!("empty subscription body");
    }
    if let Ok(proxies) = proxies_from_yaml(trimmed) {
        return Ok(proxies);
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(trimmed.replace(['\n', '\r', ' '], ""))
        .or_else(|_| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(trimmed.replace(['\n', '\r', ' '], ""))
        });
    if let Ok(bytes) = decoded {
        if let Ok(text) = String::from_utf8(bytes) {
            if let Ok(proxies) = proxies_from_yaml(&text) {
                return Ok(proxies);
            }
        }
    }
    bail!("subscription is not Clash YAML or base64 YAML");
}

fn proxies_from_yaml(text: &str) -> Result<Vec<serde_yaml::Value>> {
    let value: serde_yaml::Value = serde_yaml::from_str(text)?;
    match value {
        serde_yaml::Value::Mapping(map) => {
            let proxies = map
                .get(serde_yaml::Value::String("proxies".into()))
                .cloned()
                .context("YAML has no proxies key")?;
            match proxies {
                serde_yaml::Value::Sequence(items) => Ok(items),
                _ => bail!("proxies is not a list"),
            }
        }
        serde_yaml::Value::Sequence(items) => Ok(items),
        _ => bail!("unexpected YAML root"),
    }
}

pub fn resolve_group_members(group: &crate::strategy::Group, catalog: &Catalog) -> Vec<String> {
    // Pin order is fallback/select priority. Do not alpha-sort: flag-prefixed
    // names would otherwise become the implicit first member.
    let mut names: Vec<String> = Vec::new();
    for pin in &group.include {
        if !catalog.nodes.iter().any(|node| node.name == *pin) {
            log::warn(
                "catalog",
                format!("group {} skip missing pin {}", group.name, pin),
            );
            continue;
        }
        if !names.iter().any(|n| n == pin) {
            names.push(pin.clone());
        }
    }
    for node in &catalog.nodes {
        if group_accepts(group, node) && !names.iter().any(|n| n == &node.name) {
            names.push(node.name.clone());
        }
    }
    names
}

pub fn count_group_members(group: &crate::strategy::Group, catalog: &Catalog) -> usize {
    let mut count = catalog
        .nodes
        .iter()
        .filter(|node| group_accepts(group, node))
        .count();
    for extra in &group.include {
        let in_catalog = catalog.nodes.iter().any(|node| node.name == *extra);
        if !in_catalog {
            continue;
        }
        let already = catalog
            .nodes
            .iter()
            .any(|node| &node.name == extra && group_accepts(group, node));
        if !already {
            count += 1;
        }
    }
    count
}

fn group_accepts(group: &crate::strategy::Group, node: &Node) -> bool {
    if group.exclude.iter().any(|n| n == &node.name) {
        return false;
    }
    if group.include.iter().any(|n| n == &node.name) {
        return true;
    }
    if !source_matches(group, node) {
        return false;
    }
    if group
        .name_excludes
        .iter()
        .any(|pattern| name_matches_pattern(&node.name, pattern))
    {
        return false;
    }
    if group.all_nodes {
        return true;
    }
    if group.name_contains.is_empty() {
        return false;
    }
    group
        .name_contains
        .iter()
        .any(|pattern| name_matches_pattern(&node.name, pattern))
}

fn name_matches_pattern(text: &str, pattern: &str) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return false;
    }
    let text = text.to_lowercase();
    let pattern = pattern.to_lowercase();
    if !pattern.contains('*') && !pattern.contains('?') {
        return text.contains(&pattern);
    }
    wildcard_match(
        &pattern.chars().collect::<Vec<_>>(),
        &text.chars().collect::<Vec<_>>(),
    )
}

fn wildcard_match(pattern: &[char], value: &[char]) -> bool {
    let (mut p, mut v) = (0usize, 0usize);
    let (mut star, mut star_v) = (None, 0usize);
    while v < value.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == value[v]) {
            p += 1;
            v += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            p += 1;
            star_v = v;
        } else if let Some(s) = star {
            p = s + 1;
            star_v += 1;
            v = star_v;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

fn source_matches(group: &crate::strategy::Group, node: &Node) -> bool {
    if group.sources.is_empty() {
        return true;
    }
    group.sources.iter().any(|source| {
        source.eq_ignore_ascii_case(&node.subscription) || source == &node.subscription
    })
}
