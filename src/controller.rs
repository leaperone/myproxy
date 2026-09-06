use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::compile::{controller_port, CONTROLLER_SECRET};
use crate::log;
use crate::paths;
use crate::strategy::Strategy;

const FETCH_TIMEOUT: Duration = Duration::from_millis(800);

#[derive(Clone, Debug, Default)]
pub struct TrafficSnapshot {
    pub connections: Vec<LiveConnection>,
    pub upload_total: u64,
    pub download_total: u64,
}

#[derive(Clone, Debug)]
pub struct LiveConnection {
    pub id: String,
    pub process: String,
    pub destination: String,
    pub network: String,
    pub chain: String,
    pub upload: u64,
    pub download: u64,
    pub duration: String,
}

#[derive(Clone, Debug, Default)]
pub struct LiveGroup {
    pub name: String,
    pub kind: String,
    pub now: String,
    pub members: Vec<LiveMember>,
}

#[derive(Clone, Debug)]
pub struct LiveMember {
    pub name: String,
    pub delay: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct LiveSnapshot {
    pub traffic: TrafficSnapshot,
    pub groups: Vec<LiveGroup>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LiveReach {
    #[default]
    Unknown,
    Down,
    Unreachable,
    Ok,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PublishedLive {
    pub reach: LiveReach,
    pub proxy_now: String,
}

static PUBLISHED: Mutex<PublishedLive> = Mutex::new(PublishedLive {
    reach: LiveReach::Unknown,
    proxy_now: String::new(),
});

pub fn published_live() -> PublishedLive {
    PUBLISHED
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .clone()
}

pub fn publish_live(live: PublishedLive) {
    *PUBLISHED.lock().unwrap_or_else(|err| err.into_inner()) = live;
}

#[derive(Deserialize)]
struct ConnectionsResponse {
    #[serde(default, rename = "uploadTotal")]
    upload_total: u64,
    #[serde(default, rename = "downloadTotal")]
    download_total: u64,
    #[serde(default)]
    connections: Vec<RawConnection>,
}

#[derive(Deserialize)]
struct RawConnection {
    #[serde(default)]
    id: String,
    #[serde(default)]
    metadata: RawMetadata,
    #[serde(default)]
    upload: u64,
    #[serde(default)]
    download: u64,
    #[serde(default)]
    start: String,
    #[serde(default)]
    chains: Vec<String>,
}

#[derive(Deserialize, Default)]
struct RawMetadata {
    #[serde(default)]
    network: String,
    #[serde(default)]
    host: String,
    #[serde(default, rename = "destinationIP")]
    destination_ip: String,
    #[serde(default, rename = "destinationPort")]
    destination_port: Value,
    #[serde(default)]
    process: String,
    #[serde(default, rename = "processPath")]
    process_path: String,
}

#[derive(Deserialize, Default)]
struct ProxiesResponse {
    #[serde(default)]
    proxies: HashMap<String, RawProxy>,
}

#[derive(Deserialize, Default)]
struct RawProxy {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    now: String,
    #[serde(default)]
    all: Vec<String>,
    #[serde(default)]
    history: Vec<RawHistory>,
}

#[derive(Deserialize, Default)]
struct RawHistory {
    #[serde(default)]
    delay: u32,
}

pub fn fetch(mixed_port: u16) -> Result<TrafficSnapshot> {
    let url = format!(
        "http://127.0.0.1:{}/connections",
        controller_port(mixed_port)
    );
    let body = ureq::get(&url)
        .timeout(FETCH_TIMEOUT)
        .set("Authorization", &format!("Bearer {CONTROLLER_SECRET}"))
        .call()
        .with_context(|| format!("GET connections :{}", controller_port(mixed_port)))?
        .into_string()
        .context("read connections body")?;
    let parsed: ConnectionsResponse =
        serde_json::from_str(&body).context("parse connections json")?;
    let mut connections: Vec<LiveConnection> = parsed
        .connections
        .into_iter()
        .map(LiveConnection::from_raw)
        .collect();
    connections.sort_by(|a, b| {
        (b.upload.saturating_add(b.download)).cmp(&(a.upload.saturating_add(a.download)))
    });
    log::debug("controller", format!("connections n={}", connections.len()));
    Ok(TrafficSnapshot {
        connections,
        upload_total: parsed.upload_total,
        download_total: parsed.download_total,
    })
}

pub fn close_one(mixed_port: u16, id: &str) -> Result<()> {
    if id.is_empty() {
        return Ok(());
    }
    let url = format!(
        "http://127.0.0.1:{}/connections/{}",
        controller_port(mixed_port),
        id
    );
    ureq::delete(&url)
        .timeout(FETCH_TIMEOUT)
        .set("Authorization", &format!("Bearer {CONTROLLER_SECRET}"))
        .call()
        .with_context(|| format!("DELETE connection :{}", controller_port(mixed_port)))?;
    Ok(())
}

pub fn close_all(mixed_port: u16) -> Result<()> {
    let url = format!(
        "http://127.0.0.1:{}/connections",
        controller_port(mixed_port)
    );
    ureq::delete(&url)
        .timeout(FETCH_TIMEOUT)
        .set("Authorization", &format!("Bearer {CONTROLLER_SECRET}"))
        .call()
        .with_context(|| format!("DELETE connections :{}", controller_port(mixed_port)))?;
    Ok(())
}

pub fn fetch_proxies(mixed_port: u16) -> Result<Vec<LiveGroup>> {
    let url = format!("http://127.0.0.1:{}/proxies", controller_port(mixed_port));
    let body = authorized_get(&url, FETCH_TIMEOUT)
        .with_context(|| format!("GET proxies :{}", controller_port(mixed_port)))?;
    let parsed: ProxiesResponse = serde_json::from_str(&body).context("parse proxies json")?;
    let mut groups: Vec<LiveGroup> = parsed
        .proxies
        .iter()
        .filter_map(|(name, raw)| LiveGroup::from_raw(name, raw, &parsed.proxies))
        .collect();
    groups.sort_by(|a, b| a.name.cmp(&b.name));
    log::debug("controller", format!("proxies groups={}", groups.len()));
    Ok(groups)
}

pub fn fetch_live(mixed_port: u16) -> Result<LiveSnapshot> {
    let traffic = fetch(mixed_port)?;
    let groups = fetch_proxies(mixed_port)?;
    Ok(LiveSnapshot { traffic, groups })
}

pub fn ping(mixed_port: u16) -> Result<()> {
    let url = format!("http://127.0.0.1:{}/version", controller_port(mixed_port));
    authorized_get(&url, Duration::from_millis(400))
        .with_context(|| format!("GET version :{}", controller_port(mixed_port)))?;
    Ok(())
}

pub fn select_proxy(mixed_port: u16, group: &str, name: &str) -> Result<()> {
    if group.is_empty() || name.is_empty() {
        return Ok(());
    }
    let url = format!(
        "http://127.0.0.1:{}/proxies/{}",
        controller_port(mixed_port),
        encode_path_segment(group)
    );
    let body = serde_json::json!({ "name": name }).to_string();
    ureq::put(&url)
        .timeout(FETCH_TIMEOUT)
        .set("Authorization", &format!("Bearer {CONTROLLER_SECRET}"))
        .set("Content-Type", "application/json")
        .send_string(&body)
        .with_context(|| format!("PUT proxy {group} :{}", controller_port(mixed_port)))?;
    log::info("controller", format!("select {group} -> {name}"));
    Ok(())
}

pub fn test_group_delay(mixed_port: u16, group: &str) -> Result<HashMap<String, u32>> {
    let port = controller_port(mixed_port);
    let encoded = encode_path_segment(group);
    let probe = encode_path_segment("https://www.gstatic.com/generate_204");
    let url = format!("http://127.0.0.1:{port}/group/{encoded}/delay?url={probe}&timeout=5000");
    match authorized_get(&url, Duration::from_secs(8)) {
        Ok(body) => parse_delay_map(&body),
        Err(_) => {
            let url = format!("http://127.0.0.1:{port}/proxies/{encoded}/delay?url={probe}&timeout=5000");
            let body = authorized_get(&url, Duration::from_secs(8))
                .with_context(|| format!("GET delay {group} :{port}"))?;
            parse_delay_map(&body)
        }
    }
}

pub fn restore_selections(mixed_port: u16, strategy: &Strategy) {
    for group in &strategy.groups {
        if group.kind != "select" {
            continue;
        }
        let pick = group.selected.trim();
        if pick.is_empty() {
            continue;
        }
        if let Err(err) = select_proxy(mixed_port, &group.name, pick) {
            log::debug(
                "controller",
                format!("restore {} -> {}: {err:#}", group.name, pick),
            );
        }
    }
}

pub fn reload(mixed_port: u16) -> Result<()> {
    let path = paths::runtime_yaml_path()?;
    let url = format!(
        "http://127.0.0.1:{}/configs?force=true",
        controller_port(mixed_port)
    );
    let body = serde_json::json!({ "path": path.to_string_lossy() }).to_string();
    ureq::put(&url)
        .timeout(Duration::from_secs(5))
        .set("Authorization", &format!("Bearer {CONTROLLER_SECRET}"))
        .set("Content-Type", "application/json")
        .send_string(&body)
        .with_context(|| format!("PUT configs :{}", controller_port(mixed_port)))?;
    log::info("controller", "reloaded runtime.yaml");
    Ok(())
}

impl LiveGroup {
    fn from_raw(name: &str, raw: &RawProxy, all: &HashMap<String, RawProxy>) -> Option<Self> {
        if !is_group_kind(&raw.kind) {
            return None;
        }
        let members = raw
            .all
            .iter()
            .map(|member| LiveMember {
                name: member.clone(),
                delay: all.get(member).and_then(latest_delay),
            })
            .collect();
        Some(Self {
            name: name.to_string(),
            kind: normalize_group_kind(&raw.kind),
            now: raw.now.clone(),
            members,
        })
    }
}

fn is_group_kind(kind: &str) -> bool {
    matches!(
        kind.to_ascii_lowercase().as_str(),
        "selector" | "urltest" | "fallback" | "loadbalance" | "relay"
    )
}

fn normalize_group_kind(kind: &str) -> String {
    match kind.to_ascii_lowercase().as_str() {
        "urltest" => "url-test".into(),
        "fallback" => "fallback".into(),
        other => other.to_string(),
    }
}

fn latest_delay(raw: &RawProxy) -> Option<u32> {
    raw.history.last().map(|item| item.delay)
}

fn authorized_get(url: &str, timeout: Duration) -> Result<String> {
    ureq::get(url)
        .timeout(timeout)
        .set("Authorization", &format!("Bearer {CONTROLLER_SECRET}"))
        .call()
        .with_context(|| format!("GET {url}"))?
        .into_string()
        .context("read controller body")
}

fn parse_delay_map(body: &str) -> Result<HashMap<String, u32>> {
    let value: Value = serde_json::from_str(body).context("parse delay json")?;
    match value {
        Value::Object(map) => {
            if let Some(delay) = map.get("delay").and_then(Value::as_u64) {
                let mut out = HashMap::new();
                out.insert(String::new(), delay as u32);
                return Ok(out);
            }
            Ok(map
                .into_iter()
                .filter_map(|(name, delay)| delay.as_u64().map(|n| (name, n as u32)))
                .collect())
        }
        _ => anyhow::bail!("unexpected delay json"),
    }
}

fn encode_path_segment(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

impl LiveConnection {
    fn from_raw(raw: RawConnection) -> Self {
        let process = process_label(&raw.metadata.process, &raw.metadata.process_path);
        let destination = destination_label(
            &raw.metadata.host,
            &raw.metadata.destination_ip,
            &value_as_string(&raw.metadata.destination_port),
        );
        let chain = if raw.chains.is_empty() {
            "—".into()
        } else {
            raw.chains
                .iter()
                .rev()
                .cloned()
                .collect::<Vec<_>>()
                .join(" → ")
        };
        Self {
            id: raw.id,
            process,
            destination,
            network: if raw.metadata.network.is_empty() {
                "—".into()
            } else {
                raw.metadata.network.to_ascii_uppercase()
            },
            chain,
            upload: raw.upload,
            download: raw.download,
            duration: format_age(secs_since_start(&raw.start)),
        }
    }
}

fn process_label(process: &str, path: &str) -> String {
    let name = process.trim();
    if !name.is_empty() {
        return name.to_string();
    }
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "—".into())
}

fn destination_label(host: &str, ip: &str, port: &str) -> String {
    let host = host.trim();
    let ip = ip.trim();
    let port = port.trim();
    let base = if !host.is_empty() {
        host
    } else if !ip.is_empty() {
        ip
    } else {
        return "—".into();
    };
    if port.is_empty() || port == "0" {
        base.to_string()
    } else {
        format!("{base}:{port}")
    }
}

fn value_as_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

pub fn format_bytes(n: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let n = n as f64;
    if n >= GB {
        format!("{:.1} GB", n / GB)
    } else if n >= MB {
        format!("{:.1} MB", n / MB)
    } else if n >= KB {
        format!("{:.1} KB", n / KB)
    } else {
        format!("{n:.0} B")
    }
}

pub fn format_rate(bps: u64) -> String {
    format!("{}/s", format_bytes(bps))
}

fn format_age(secs: Option<u64>) -> String {
    let Some(secs) = secs else {
        return "—".into();
    };
    if secs < 60 {
        format!("{secs}秒")
    } else if secs < 3600 {
        format!("{}分{:02}秒", secs / 60, secs % 60)
    } else {
        format!("{}小时{:02}分", secs / 3600, (secs % 3600) / 60)
    }
}

fn secs_since_start(start: &str) -> Option<u64> {
    let s = start.trim();
    if s.len() < 19 {
        return None;
    }
    let year: i32 = s.get(0..4)?.parse().ok()?;
    let month: u32 = s.get(5..7)?.parse().ok()?;
    let day: u32 = s.get(8..10)?.parse().ok()?;
    let hour: u32 = s.get(11..13)?.parse().ok()?;
    let min: u32 = s.get(14..16)?.parse().ok()?;
    let sec: u32 = s.get(17..19)?.parse().ok()?;
    let unix = unix_from_civil(year, month, day, hour, min, sec)?;
    let offset = timezone_offset(s.get(19..).unwrap_or(""));
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    Some(now.saturating_sub(unix - offset).max(0) as u64)
}

fn timezone_offset(rest: &str) -> i64 {
    let Some(idx) = rest.find(['Z', 'z', '+', '-']) else {
        return 0;
    };
    let tz = &rest[idx..];
    if tz.starts_with('Z') || tz.starts_with('z') {
        return 0;
    }
    let sign: i64 = if tz.starts_with('+') { 1 } else { -1 };
    let body = tz.get(1..).unwrap_or("");
    let (hours, minutes) = if body.len() >= 5 && body.as_bytes().get(2) == Some(&b':') {
        (
            body.get(0..2).and_then(|s| s.parse().ok()).unwrap_or(0),
            body.get(3..5).and_then(|s| s.parse().ok()).unwrap_or(0),
        )
    } else if body.len() >= 4 {
        (
            body.get(0..2).and_then(|s| s.parse().ok()).unwrap_or(0),
            body.get(2..4).and_then(|s| s.parse().ok()).unwrap_or(0),
        )
    } else {
        (0, 0)
    };
    sign * (hours * 3600 + minutes * 60)
}

fn unix_from_civil(y: i32, m: u32, d: u32, hh: u32, mm: u32, ss: u32) -> Option<i64> {
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    let mut y = y as i64;
    let m = m as i64;
    let d = d as i64;
    if m <= 2 {
        y -= 1;
    }
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some(days * 86400 + i64::from(hh) * 3600 + i64::from(mm) * 60 + i64::from(ss))
}
