use std::fs;

use anyhow::{Context, Result, bail};
use base64::Engine;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::paths;
use crate::strategy::Strategy;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Catalog {
    pub nodes: Vec<Node>,
    pub excluded: Vec<Excluded>,
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
        Ok(serde_json::from_str(&data).unwrap_or_default())
    }

    pub fn save(&self) -> Result<()> {
        let path = paths::catalog_path()?;
        fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

pub fn refresh(strategy: &Strategy) -> Result<Catalog> {
    let exclude = Regex::new(&strategy.exclude_filter)
        .context("invalid exclude_filter regex")?;
    let mut catalog = Catalog::default();

    for sub in &strategy.subscriptions {
        match fetch_proxies(&sub.url) {
            Ok(proxies) => {
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
            }
            Err(err) => {
                catalog.excluded.push(Excluded {
                    name: format!("<{}>", sub.name),
                    subscription: sub.name.clone(),
                    reason: format!("fetch failed: {err:#}"),
                });
            }
        }
    }

    catalog.save()?;
    Ok(catalog)
}

fn fetch_proxies(url: &str) -> Result<Vec<serde_yaml::Value>> {
    let body = if let Some(path) = url.strip_prefix("file://") {
        fs::read_to_string(path).with_context(|| format!("read {path}"))?
    } else if url.starts_with("http://") || url.starts_with("https://") {
        ureq::get(url)
            .timeout(std::time::Duration::from_secs(30))
            .set("User-Agent", "clash.meta")
            .call()
            .with_context(|| format!("GET {url}"))?
            .into_string()?
    } else {
        fs::read_to_string(url).with_context(|| format!("read {url}"))?
    };
    parse_subscription(&body)
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
    let filter = if group.filter.trim().is_empty() {
        None
    } else {
        Regex::new(&group.filter).ok()
    };
    let mut names: Vec<String> = catalog
        .nodes
        .iter()
        .filter(|node| {
            if group.exclude.iter().any(|n| n == &node.name) {
                return false;
            }
            if group.include.iter().any(|n| n == &node.name) {
                return true;
            }
            match &filter {
                None => true,
                Some(re) => re.is_match(&node.name),
            }
        })
        .map(|n| n.name.clone())
        .collect();

    for extra in &group.include {
        if !names.iter().any(|n| n == extra) {
            names.push(extra.clone());
        }
    }
    names.sort();
    names.dedup();
    names
}
