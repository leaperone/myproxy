use std::collections::{HashMap, HashSet};
use std::fs;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use anyhow::{bail, Context, Result};
use regex::RegexBuilder;
use serde::Deserialize;

use crate::strategy::Strategy;

const MAX_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Deserialize)]
struct Export {
    schema: u32,
    sites: HashMap<String, Vec<Domain>>,
    ips: HashMap<String, Vec<String>>,
}

#[derive(Deserialize)]
struct Domain {
    kind: String,
    value: String,
}

#[derive(Default)]
struct Site {
    exact: HashSet<String>,
    suffix: HashSet<String>,
    keywords: Vec<String>,
    expressions: Vec<regex::Regex>,
}

#[derive(Default)]
struct Networks {
    v4: Vec<(u128, u128)>,
    v6: Vec<(u128, u128)>,
}

struct Database {
    sites: HashMap<String, Site>,
    ips: HashMap<String, Networks>,
}

struct Cached {
    path: PathBuf,
    modified: SystemTime,
    length: u64,
    database: Arc<Database>,
}

fn database() -> Result<Arc<Database>> {
    static CACHE: OnceLock<Mutex<Option<Cached>>> = OnceLock::new();
    let path = crate::paths::data_dir()?.join("rulesets/mclash-geodata.json");
    let metadata = fs::metadata(&path).context("规则数据库缺失，请先导入地区和域名规则数据")?;
    if metadata.len() > MAX_BYTES {
        bail!("规则数据库超过 32 MiB");
    }
    let modified = metadata.modified()?;
    let mut cache = CACHE.get_or_init(Mutex::default).lock().expect("geo cache");
    if let Some(cached) = cache.as_ref() {
        if cached.path == path && cached.modified == modified && cached.length == metadata.len() {
            return Ok(cached.database.clone());
        }
    }
    let parsed = Arc::new(Database::parse(&fs::read(&path)?)?);
    *cache = Some(Cached { path, modified, length: metadata.len(), database: parsed.clone() });
    Ok(parsed)
}

pub fn validate_rules(strategy: &Strategy) -> Result<()> {
    let required = strategy.rule_sets.iter().flat_map(|set| &set.matchers)
        .filter(|matcher| matches!(matcher.kind.as_str(), "geo-site" | "geo-ip"))
        .collect::<Vec<_>>();
    if required.is_empty() {
        return Ok(());
    }
    let database = database()?;
    for matcher in required {
        let key = matcher.value.to_ascii_lowercase();
        let exists = match matcher.kind.as_str() {
            "geo-site" => database.sites.contains_key(&key),
            _ => database.ips.contains_key(&key),
        };
        if !exists {
            bail!("规则数据库不包含所选分类：{}", matcher.value);
        }
    }
    Ok(())
}

pub fn matches(kind: &str, category: &str, host: &str) -> Result<bool> {
    let database = database()?;
    Ok(database.matches(kind, category, host))
}

impl Database {
    fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() as u64 > MAX_BYTES {
            bail!("规则数据库超过 32 MiB");
        }
        let export: Export = serde_json::from_slice(bytes).context("规则数据库格式不正确")?;
        if export.schema != 1 {
            bail!("不支持此规则数据库版本");
        }
        let mut sites = HashMap::new();
        let mut count = 0usize;
        for (name, entries) in export.sites {
            let mut site = Site::default();
            for entry in entries {
                count += 1;
                if count > 1_000_000 || entry.value.is_empty() || entry.value.len() > 2048 {
                    bail!("规则数据库包含过多或无效条目");
                }
                let value = entry.value.to_ascii_lowercase();
                match entry.kind.as_str() {
                    "domain" => { site.exact.insert(value); }
                    "suffix" => { site.suffix.insert(value); }
                    "keyword" => site.keywords.push(value),
                    "regex" => site.expressions.push(RegexBuilder::new(&entry.value).case_insensitive(true).build().context("域名规则表达式无效")?),
                    _ => bail!("规则数据库包含未知匹配类型"),
                }
            }
            sites.insert(name.to_ascii_lowercase(), site);
        }
        let mut ips = HashMap::new();
        for (name, entries) in export.ips {
            let mut networks = Networks::default();
            for entry in entries {
                count += 1;
                if count > 1_000_000 {
                    bail!("规则数据库包含过多条目");
                }
                let (address, bits) = entry.split_once('/').context("无效的网段规则")?;
                let address: IpAddr = address.parse().context("无效的网段地址")?;
                let bits: u32 = bits.parse().context("无效的网段前缀")?;
                let (value, width, ranges) = match address {
                    IpAddr::V4(ip) => (u128::from(u32::from(ip)), 32, &mut networks.v4),
                    IpAddr::V6(ip) => (u128::from(ip), 128, &mut networks.v6),
                };
                if bits > width { bail!("无效的网段前缀"); }
                let host_bits = width - bits;
                let host_mask = if host_bits == 128 { u128::MAX } else { (1u128 << host_bits) - 1 };
                ranges.push((value & !host_mask, value | host_mask));
            }
            merge_ranges(&mut networks.v4);
            merge_ranges(&mut networks.v6);
            ips.insert(name.to_ascii_lowercase(), networks);
        }
        Ok(Self { sites, ips })
    }

    fn matches(&self, kind: &str, category: &str, host: &str) -> bool {
        let category = category.to_ascii_lowercase();
        if kind == "geo-ip" {
            let Some(networks) = self.ips.get(&category) else { return false; };
            return match host.parse::<IpAddr>() {
                Ok(IpAddr::V4(ip)) => contains(&networks.v4, u128::from(u32::from(ip))),
                Ok(IpAddr::V6(ip)) => contains(&networks.v6, u128::from(ip)),
                Err(_) => false,
            };
        }
        let Some(site) = self.sites.get(&category) else { return false; };
        let host = host.trim().trim_matches('.').to_ascii_lowercase();
        if site.exact.contains(&host) { return true; }
        let mut suffix = host.as_str();
        loop {
            if site.suffix.contains(suffix) { return true; }
            let Some((_, rest)) = suffix.split_once('.') else { break; };
            suffix = rest;
        }
        site.keywords.iter().any(|value| host.contains(value))
            || site.expressions.iter().any(|pattern| pattern.is_match(&host))
    }
}

fn merge_ranges(ranges: &mut Vec<(u128, u128)>) {
    ranges.sort_unstable();
    let mut merged: Vec<(u128, u128)> = Vec::new();
    for &(start, end) in ranges.iter() {
        if let Some(previous) = merged.last_mut() {
            if start <= previous.1.saturating_add(1) {
                previous.1 = previous.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }
    *ranges = merged;
}

fn contains(ranges: &[(u128, u128)], value: u128) -> bool {
    let index = ranges.partition_point(|(start, _)| *start <= value);
    index > 0 && value <= ranges[index - 1].1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imported_geo_snapshot_preserves_domain_types_and_cidr_boundaries() {
        let data = br#"{"schema":1,"sites":{"sample":[{"kind":"domain","value":"exact.example"},{"kind":"suffix","value":"suffix.example"},{"kind":"keyword","value":"tracker"},{"kind":"regex","value":"^api[0-9]+\\.example$"}]},"ips":{"sample":["192.0.2.0/25","192.0.2.128/25","2001:db8::/32"]}}"#;
        let database = Database::parse(data).unwrap();
        for (host, expected) in [("exact.example",true),("sub.exact.example",false),("sub.suffix.example",true),("notsuffix.example",false),("tracker.invalid",true),("api12.example",true),("www.api12.example",false)] {
            assert_eq!(database.matches("geo-site","SAMPLE",host),expected,"{host}");
        }
        for (host, expected) in [("192.0.2.0",true),("192.0.2.255",true),("192.0.3.0",false),("2001:db8::1234",true),("2001:db9::",false),("www.example",false)] {
            assert_eq!(database.matches("geo-ip","sample",host),expected,"{host}");
        }
        assert_eq!(database.ips["sample"].v4.len(),1);
    }

    #[test]
    fn malformed_geo_snapshot_is_rejected() {
        for json in [r#"{"schema":2,"sites":{},"ips":{}}"#,r#"{"schema":1,"sites":{},"ips":{"cn":["192.0.2.0/33"]}}"#,r#"{"schema":1,"sites":{"cn":[{"kind":"regex","value":"["}]},"ips":{}}"#] {
            assert!(Database::parse(json.as_bytes()).is_err());
        }
    }
}
