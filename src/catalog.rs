use std::collections::HashMap;
use std::fs;
use std::process::Command;

use anyhow::{bail, Context, Result};
use base64::Engine;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::log;
use crate::paths;
use crate::strategy::Strategy;

const SUBSCRIPTION_CURL_MAX_TIME: &str = "8";
const SUBSCRIPTION_CURL_CONNECT_TIMEOUT: &str = "3";

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
        let catalog: Self = serde_json::from_str(&data).context("parse catalog.json")?;
        catalog.validate()?;
        Ok(catalog)
    }

    pub fn save(&self) -> Result<()> {
        self.validate()?;
        let path = paths::catalog_path()?;
        paths::atomic_write(&path, serde_json::to_string_pretty(self)?.as_bytes())?;
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        let mut names = HashMap::new();
        for node in &self.nodes {
            if node.name.trim().is_empty()
                || node.name != node.name.trim()
                || node.name.chars().any(|ch| ch == ',' || ch.is_control())
                || crate::strategy::reserved_proxy_name(&node.name)
            {
                bail!("invalid node name in subscription {}", node.subscription);
            }
            if let Some(previous) = names.insert(&node.name, &node.subscription) {
                bail!(
                    "duplicate node name {} in subscriptions {} and {}",
                    node.name,
                    previous,
                    node.subscription
                );
            }
            if node.raw.get("name").and_then(serde_yaml::Value::as_str) != Some(node.name.as_str())
            {
                bail!(
                    "catalog node name does not match proxy config in subscription {}",
                    node.subscription
                );
            }
        }
        Ok(())
    }

    pub fn subscription_warning(&self, subscription: &str) -> Option<String> {
        if !self.excluded.iter().any(|item| {
            item.subscription == subscription && item.reason.starts_with("fetch failed:")
        }) {
            return None;
        }
        let cached = self
            .nodes
            .iter()
            .filter(|node| node.subscription == subscription)
            .count();
        Some(if cached == 0 {
            "刷新失败，无可用缓存节点".into()
        } else {
            format!("刷新失败，沿用 {cached} 个缓存节点")
        })
    }

    pub fn fetch_failure_count(&self) -> usize {
        let mut names: Vec<_> = self.subscription_urls.keys().collect();
        names.sort();
        names
            .into_iter()
            .filter(|name| self.subscription_warning(name).is_some())
            .count()
    }

    pub fn filter_excluded_count(&self) -> usize {
        self.excluded
            .iter()
            .filter(|item| !item.reason.starts_with("fetch failed:"))
            .count()
    }

    pub fn refresh_warnings(&self) -> Vec<String> {
        let mut subscriptions: Vec<_> = self.subscription_urls.keys().collect();
        subscriptions.sort();
        subscriptions
            .into_iter()
            .filter_map(|name| {
                self.subscription_warning(name)
                    .map(|message| format!("{name}: {message}"))
            })
            .collect()
    }

    pub fn matches_strategy(&self, strategy: &Strategy) -> bool {
        if strategy.subscriptions.is_empty() && self.nodes.is_empty() {
            return true;
        }
        self.exclude_filter == strategy.exclude_filter
            && self.subscription_urls.len() == strategy.subscriptions.len()
            && strategy
                .subscriptions
                .iter()
                .all(|sub| self.subscription_urls.get(&sub.name) == Some(&sub.url))
    }
}

pub fn refresh(strategy: &Strategy) -> Result<Catalog> {
    strategy.validate()?;
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
                        .and_then(serde_yaml::Value::as_str)
                        .filter(|name| !name.trim().is_empty())
                        .with_context(|| {
                            format!("subscription {} contains a proxy without a name", sub.name)
                        })?
                        .to_string();
                    if !strategy.exclude_filter.is_empty() && exclude.is_match(&original) {
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
                let unique_name = strategy
                    .subscriptions
                    .iter()
                    .filter(|candidate| candidate.name == sub.name)
                    .count()
                    == 1;
                let reused = if cache_filter_matches
                    && unique_name
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
    // curl is available on macOS and lets us force IPv4. Do not fall back to
    // a second HTTP client here: a broken DNS/IPv6 path would otherwise pay
    // the full timeout twice for every subscription and make Apply appear
    // hung. The previous catalog remains available to the caller.
    fetch_http_body_curl_ipv4(url).with_context(|| format!("GET {name} via IPv4 curl"))
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
        if group.exclude.iter().any(|excluded| excluded == pin) {
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
    resolve_group_members(group, catalog).len()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_failures_are_not_counted_as_filter_excludes() {
        let catalog = Catalog {
            nodes: vec![Node {
                name: "HK".into(),
                subscription: "A".into(),
                raw: serde_yaml::from_str("name: HK").expect("yaml"),
            }],
            excluded: vec![
                Excluded {
                    name: "*".into(),
                    subscription: "A".into(),
                    reason: "fetch failed: timeout".into(),
                },
                Excluded {
                    name: "广告".into(),
                    subscription: "B".into(),
                    reason: "exclude_filter".into(),
                },
            ],
            subscription_urls: HashMap::from([
                ("A".into(), "https://example.invalid/a".into()),
                ("B".into(), "https://example.invalid/b".into()),
            ]),
            exclude_filter: String::new(),
        };
        assert_eq!(catalog.fetch_failure_count(), 1);
        assert_eq!(catalog.filter_excluded_count(), 1);
        assert!(catalog.subscription_warning("A").is_some());
        assert!(catalog.subscription_warning("B").is_none());
    }
}
