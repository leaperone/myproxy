//! The opt-in Xray channel.
//!
//! This module deliberately owns its configuration projection and process
//! files. The existing Mihomo runtime remains in `supervisor.rs`, with no
//! shared YAML or PID state. A missing Xray binary is a clear boundary error;
//! it never falls back to Mihomo behind the user's back.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::net::{TcpStream, ToSocketAddrs};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::backend::BackendKind;
use crate::catalog::{self, Catalog, Node};
use crate::compile;
use crate::gfw;
use crate::log;
use crate::paths;
use crate::strategy::{InboundMode, Matcher, RoutingProfile, Strategy, GLOBAL_GROUP};

const XRAY_PROBE_URL: &str = "https://www.gstatic.com/generate_204";
const XRAY_PROBE_INTERVAL: &str = "30s";
const XRAY_HTTP_PORT_OFFSET: u16 = 1;

#[derive(Clone, Debug, Serialize)]
pub struct CompileReport {
    pub json: String,
    pub warnings: Vec<String>,
    pub nodes: usize,
    pub groups: usize,
    pub current: String,
    pub socks_port: u16,
    pub http_port: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct XrayRuntimeIdentity {
    pub generation: u64,
    pub socks_port: u16,
    pub http_port: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RuntimeState {
    strategy: Strategy,
    catalog: Catalog,
    config: String,
    generation: u64,
    #[serde(default)]
    warnings: Vec<String>,
    #[serde(default)]
    current: String,
}

impl RuntimeState {
    fn identity(&self) -> XrayRuntimeIdentity {
        XrayRuntimeIdentity {
            generation: self.generation,
            socks_port: self.strategy.mixed_port,
            http_port: http_port(self.strategy.mixed_port),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct XrayStatus {
    pub backend: BackendKind,
    pub wanted: bool,
    pub running: bool,
    pub ready: bool,
    pub generation: Option<u64>,
    pub socks_port: Option<u16>,
    pub http_port: Option<u16>,
    pub current: String,
    pub warnings: Vec<String>,
    pub note: Option<String>,
}

#[derive(Clone, Debug)]
enum RouteTarget {
    Outbound(String),
    Balancer(String),
}

#[derive(Clone, Debug)]
struct NodeProjection {
    tag: String,
    value: Value,
}

#[derive(Clone, Debug)]
struct ProjectionContext {
    node_tags: HashMap<String, String>,
    group_targets: HashMap<String, RouteTarget>,
}

/// Build a complete Xray JSON document without touching the filesystem.
/// `strategy` and `catalog` have already crossed their parsing boundaries, so
/// this function only applies the Xray projection rules.
pub fn compile_config(strategy: &Strategy, catalog: &Catalog) -> Result<CompileReport> {
    strategy.validate()?;
    strategy.validate_catalog(catalog)?;
    if strategy.tun || strategy.system_extension {
        bail!(
            "Xray 通道当前只接管本地 SOCKS/HTTP 入口；请先关闭 TUN 或系统接管，再连接 Xray"
        );
    }
    if strategy.mixed_port == u16::MAX {
        bail!("Xray HTTP 入口需要占用 Mixed 端口的下一个端口");
    }

    let mut warnings = Vec::new();
    let mut outbounds = vec![
        json!({"tag": "direct", "protocol": "freedom"}),
        json!({"tag": "reject", "protocol": "blackhole", "settings": {"response": {"type": "http"}}}),
    ];
    let mut projections = Vec::new();
    let mut node_tags = HashMap::new();
    for (index, node) in catalog.nodes.iter().enumerate() {
        let tag = node_tag(index, &node.name);
        match compile_node(node, &tag) {
            Ok(value) => {
                node_tags.insert(node.name.clone(), tag.clone());
                outbounds.push(value.clone());
                projections.push(NodeProjection { tag, value });
            }
            Err(error) => warnings.push(format!(
                "节点 {} 未加入 Xray：{}",
                node.name,
                safe_error(&error)
            )),
        }
    }

    let mut balancers = Vec::new();
    let mut group_targets = HashMap::new();
    let mut observed_tags = Vec::new();
    for (index, group) in strategy.groups.iter().enumerate() {
        let members = catalog::resolve_group_members(group, catalog);
        let tags: Vec<String> = members
            .iter()
            .filter_map(|name| node_tags.get(name).cloned())
            .collect();
        if tags.len() < members.len() {
            warnings.push(format!(
                "节点组 {} 有 {} 个节点无法由 Xray 支持",
                group.name,
                members.len().saturating_sub(tags.len())
            ));
        }
        let target = if tags.is_empty() {
            RouteTarget::Outbound("reject".into())
        } else if group.kind == "fallback" || group.kind == "url-test" {
            let tag = format!("group-{}-{}", index + 1, tag_fragment(&group.name));
            let strategy_type = "leastping";
            balancers.push(json!({
                "tag": tag,
                "selector": tags,
                "strategy": {"type": strategy_type},
                "fallbackTag": "reject"
            }));
            observed_tags.extend(
                members
                    .iter()
                    .filter_map(|name| node_tags.get(name).cloned()),
            );
            RouteTarget::Balancer(tag)
        } else {
            let selected = group
                .selected
                .trim()
                .strip_prefix("node:")
                .unwrap_or(group.selected.trim());
            let selected = if !selected.is_empty() && node_tags.contains_key(selected) {
                selected.to_string()
            } else {
                members
                    .iter()
                    .find(|name| node_tags.contains_key(*name))
                    .cloned()
                    .unwrap_or_default()
            };
            if selected.is_empty() {
                RouteTarget::Outbound("reject".into())
            } else {
                RouteTarget::Outbound(node_tags[&selected].clone())
            }
        };
        group_targets.insert(group.name.to_ascii_lowercase(), target);
    }

    let mut context = ProjectionContext {
        node_tags,
        group_targets,
    };
    let global_target = if strategy.global_selected.trim().is_empty() {
        route_for_via(compile::default_group(strategy), strategy, &context)
    } else {
        route_for_via(&strategy.global_selected, strategy, &context)
    };
    context
        .group_targets
        .insert(GLOBAL_GROUP.to_ascii_lowercase(), global_target.clone());

    let mut rules = Vec::new();
    append_local_rules(&mut rules);
    if strategy.mixed_mode != InboundMode::Rule {
        rules.push(route_only_rule(inbound_target(strategy.mixed_mode, strategy, &context)));
    } else {
        for set in &strategy.rule_sets {
            let target = route_for_via(&set.via, strategy, &context);
            if let Some(gfw_group) = gfw::gfw_group(&set.via) {
                let domains = gfw::load_domains();
                if domains.is_empty() {
                    warnings.push(format!(
                        "规则 {} 需要 GFWList，缓存为空，已跳过 gfw 条件",
                        set.name
                    ));
                } else {
                    rules.push(domain_rule(
                        domains
                            .into_iter()
                            .map(|domain| format!("domain:{domain}"))
                            .collect(),
                        route_for_via(gfw_group, strategy, &context),
                    ));
                }
                continue;
            }
            for matcher in &set.matchers {
                if let Some(rule) = matcher_rule(matcher, target.clone()) {
                    rules.push(rule);
                } else {
                    warnings.push(format!(
                        "规则 {} 的匹配类型 {} 暂不支持",
                        set.name, matcher.kind
                    ));
                }
            }
        }
        if strategy.routing_profile == RoutingProfile::Gfwlist {
            let domains = gfw::load_domains();
            if domains.is_empty() {
                warnings.push("GFWList 缓存为空，未添加 GFWList 路由".into());
            } else {
                rules.push(domain_rule(
                    domains
                        .into_iter()
                        .map(|domain| format!("domain:{domain}"))
                        .collect(),
                    route_for_via(compile::default_group(strategy), strategy, &context),
                ));
            }
        }
        if strategy.routing_profile == RoutingProfile::Chinadirect {
            rules.push(field_rule(
                "ip",
                vec!["geoip:cn".into()],
                RouteTarget::Outbound("direct".into()),
            ));
        }
        let unmatched = route_for_via(
            &compile::unmatched_target(strategy),
            strategy,
            &context,
        );
        rules.push(route_only_rule(unmatched));
    }

    let inbounds = vec![
        json!({
            "tag": "myproxy-xray-socks",
            "listen": "127.0.0.1",
            "port": strategy.mixed_port,
            "protocol": "socks",
            "settings": {"auth": "noauth", "udp": true},
            "sniffing": {"enabled": true, "destOverride": ["http", "tls", "quic"]}
        }),
        json!({
            "tag": "myproxy-xray-http",
            "listen": "127.0.0.1",
            "port": http_port(strategy.mixed_port),
            "protocol": "http",
            "settings": {}
        }),
    ];
    let mut root = json!({
        "log": {"loglevel": "warning"},
        "inbounds": inbounds,
        "outbounds": outbounds,
        "routing": {
            "domainStrategy": "AsIs",
            "rules": rules,
            "balancers": balancers
        }
    });
    if !observed_tags.is_empty() {
        let mut deduped = Vec::new();
        let mut seen = HashSet::new();
        for tag in observed_tags {
            if seen.insert(tag.clone()) {
                deduped.push(tag);
            }
        }
        root["observatory"] = json!({
            "subjectSelector": deduped,
            "probeURL": XRAY_PROBE_URL,
            "probeInterval": XRAY_PROBE_INTERVAL,
            "enableConcurrency": true
        });
    }

    let current = describe_target(&global_target, &projections);
    let json = serde_json::to_string_pretty(&root)?;
    log::info(
        "xray",
        format!(
            "compiled nodes={} groups={} warnings={} socks={} http={}",
            projections.len(),
            strategy.groups.len(),
            warnings.len(),
            strategy.mixed_port,
            http_port(strategy.mixed_port)
        ),
    );
    Ok(CompileReport {
        json,
        warnings,
        nodes: projections.len(),
        groups: strategy.groups.len(),
        current,
        socks_port: strategy.mixed_port,
        http_port: http_port(strategy.mixed_port),
    })
}

fn append_local_rules(rules: &mut Vec<Value>) {
    rules.push(json!({
        "domain": ["full:localhost", "domain:local", "domain:lan", "domain:home.arpa"],
        "outboundTag": "direct"
    }));
    rules.push(json!({
        "ip": [
            "0.0.0.0/32", "10.0.0.0/8", "127.0.0.0/8", "169.254.0.0/16",
            "172.16.0.0/12", "192.168.0.0/16", "224.0.0.0/4", "255.255.255.255/32",
            "::/128", "::1/128", "fc00::/7", "fe80::/10", "ff00::/8"
        ],
        "outboundTag": "direct"
    }));
}

fn matcher_rule(matcher: &Matcher, target: RouteTarget) -> Option<Value> {
    match matcher.kind.as_str() {
        "keyword" => Some(domain_rule(
            vec![format!("keyword:{}", matcher.value.trim())],
            target,
        )),
        "suffix" => Some(domain_rule(
            vec![format!("domain:{}", matcher.value.trim().trim_start_matches('.'))],
            target,
        )),
        "domain" => {
            let value = matcher.value.trim();
            let pattern = if let Some(suffix) = value.strip_prefix("*.") {
                format!("domain:{}", suffix)
            } else {
                format!("full:{value}")
            };
            Some(domain_rule(vec![pattern], target))
        }
        "cidr" => Some(field_rule("ip", vec![matcher.value.trim().into()], target)),
        "app" => Some(field_rule("process", vec![matcher.value.trim().into()], target)),
        _ => None,
    }
}

fn domain_rule(domains: Vec<String>, target: RouteTarget) -> Value {
    field_rule("domain", domains, target)
}

fn field_rule(field: &str, values: Vec<String>, target: RouteTarget) -> Value {
    let mut rule = Map::new();
    rule.insert(field.into(), json!(values));
    match target {
        RouteTarget::Balancer(tag) => {
            rule.insert("balancerTag".into(), Value::String(tag));
        }
        RouteTarget::Outbound(tag) => {
            rule.insert("outboundTag".into(), Value::String(tag));
        }
    }
    Value::Object(rule)
}

fn route_only_rule(target: RouteTarget) -> Value {
    let mut rule = Map::new();
    rule.insert("network".into(), json!(["tcp", "udp"]));
    match target {
        RouteTarget::Balancer(tag) => {
            rule.insert("balancerTag".into(), Value::String(tag));
        }
        RouteTarget::Outbound(tag) => {
            rule.insert("outboundTag".into(), Value::String(tag));
        }
    }
    Value::Object(rule)
}

fn inbound_target(mode: InboundMode, strategy: &Strategy, context: &ProjectionContext) -> RouteTarget {
    match mode {
        InboundMode::Direct => RouteTarget::Outbound("direct".into()),
        InboundMode::Global => route_for_via(GLOBAL_GROUP, strategy, context),
        InboundMode::Proxy => route_for_via(compile::default_group(strategy), strategy, context),
        InboundMode::Rule => RouteTarget::Outbound("direct".into()),
    }
}

fn route_for_via(via: &str, strategy: &Strategy, context: &ProjectionContext) -> RouteTarget {
    let mut value = via.trim();
    if let Some(group) = gfw::gfw_group(value) {
        value = group;
    }
    if value.eq_ignore_ascii_case("direct") {
        return RouteTarget::Outbound("direct".into());
    }
    if value.eq_ignore_ascii_case("reject") {
        return RouteTarget::Outbound("reject".into());
    }
    if value.eq_ignore_ascii_case(GLOBAL_GROUP) {
        if let Some(target) = context.group_targets.get(&GLOBAL_GROUP.to_ascii_lowercase()) {
            return target.clone();
        }
        value = compile::default_group(strategy);
    }
    if value.eq_ignore_ascii_case("default") || value.eq_ignore_ascii_case("proxy") {
        value = compile::default_group(strategy);
    }
    let explicit_node = value.strip_prefix("node:").unwrap_or(value);
    if let Some(tag) = context.node_tags.get(explicit_node) {
        return RouteTarget::Outbound(tag.clone());
    }
    let group_name = value.strip_prefix("group:").unwrap_or(value);
    context
        .group_targets
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(group_name))
        .map(|(_, target)| target.clone())
        .unwrap_or_else(|| RouteTarget::Outbound("reject".into()))
}

fn describe_target(target: &RouteTarget, nodes: &[NodeProjection]) -> String {
    match target {
        RouteTarget::Balancer(tag) => format!("自动选择（{tag}）"),
        RouteTarget::Outbound(tag) if tag == "direct" => "直连".into(),
        RouteTarget::Outbound(tag) if tag == "reject" => "不可用".into(),
        RouteTarget::Outbound(tag) => nodes
            .iter()
            .find(|node| node.tag == *tag)
            .and_then(|node| node.value.get("tag"))
            .and_then(Value::as_str)
            .unwrap_or(tag)
            .to_string(),
    }
}

fn compile_node(node: &Node, tag: &str) -> Result<Value> {
    let kind = raw_string(&node.raw, "type")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let address = raw_string(&node.raw, "server").context("缺少服务器地址")?;
    let port = raw_u16(&node.raw, "port").context("缺少服务器端口")?;
    let settings = match kind.as_str() {
        "ss" | "shadowsocks" => json!({
            "servers": [{
                "address": address,
                "port": port,
                "method": raw_string(&node.raw, "cipher").context("缺少加密方式")?,
                "password": raw_string(&node.raw, "password").context("缺少密码")?
            }]
        }),
        "vmess" => json!({
            "vnext": [{
                "address": address,
                "port": port,
                "users": [{
                    "id": raw_string(&node.raw, "uuid").context("缺少 UUID")?,
                    "alterId": raw_u32(&node.raw, "alterId").or_else(|| raw_u32(&node.raw, "alter-id")).unwrap_or(0),
                    "security": raw_string(&node.raw, "cipher").unwrap_or_else(|| "auto".into())
                }]
            }]
        }),
        "vless" => {
            let mut user = Map::new();
            user.insert(
                "id".into(),
                Value::String(raw_string(&node.raw, "uuid").context("缺少 UUID")?),
            );
            user.insert("encryption".into(), Value::String("none".into()));
            if let Some(flow) = raw_string(&node.raw, "flow") {
                if !flow.is_empty() {
                    user.insert("flow".into(), Value::String(flow));
                }
            }
            json!({"vnext": [{"address": address, "port": port, "users": [Value::Object(user)]}]})
        }
        "trojan" => {
            if raw_string(&node.raw, "flow").is_some_and(|flow| !flow.is_empty()) {
                bail!("Trojan flow 已被当前 Xray 配置移除");
            }
            json!({
                "servers": [{
                    "address": address,
                    "port": port,
                    "password": raw_string(&node.raw, "password").context("缺少密码")?
                }]
            })
        }
        "socks" => {
            let mut server = json!({"address": address, "port": port});
            if let Some(user) = raw_string(&node.raw, "username").or_else(|| raw_string(&node.raw, "user")) {
                server["users"] = json!([{"user": user, "pass": raw_string(&node.raw, "password").or_else(|| raw_string(&node.raw, "pass")).unwrap_or_default()}]);
            }
            json!({"servers": [server]})
        }
        "http" => {
            let mut server = json!({"address": address, "port": port});
            if let Some(user) = raw_string(&node.raw, "username").or_else(|| raw_string(&node.raw, "user")) {
                server["users"] = json!([{"user": user, "pass": raw_string(&node.raw, "password").or_else(|| raw_string(&node.raw, "pass")).unwrap_or_default()}]);
            }
            json!({"servers": [server]})
        }
        "hysteria" | "hysteria2" | "tuic" | "wireguard" | "ssh" => {
            bail!("协议 {kind} 尚未纳入 Xray 通道")
        }
        _ => bail!("未知协议 {kind}"),
    };
    let mut outbound = Map::new();
    outbound.insert("tag".into(), Value::String(tag.into()));
    outbound.insert(
        "protocol".into(),
        Value::String(match kind.as_str() {
            "ss" | "shadowsocks" => "shadowsocks",
            "vmess" => "vmess",
            "vless" => "vless",
            "trojan" => "trojan",
            "socks" => "socks",
            "http" => "http",
            _ => unreachable!(),
        }
        .into()),
    );
    outbound.insert("settings".into(), settings);
    if let Some(stream) = stream_settings(&node.raw)? {
        outbound.insert("streamSettings".into(), stream);
    }
    Ok(Value::Object(outbound))
}

fn stream_settings(raw: &serde_yaml::Value) -> Result<Option<Value>> {
    let network = raw_string(raw, "network")
        .unwrap_or_else(|| "tcp".into())
        .to_ascii_lowercase();
    let tls = raw_truthy(raw, "tls") || raw_map(raw, "reality-opts").is_some();
    let skip = raw_truthy(raw, "skip-cert-verify");
    let pinned = raw_string(raw, "pinnedPeerCertSha256")
        .or_else(|| raw_string(raw, "pinned-peer-cert-sha256"));
    let verify_name = raw_string(raw, "verifyPeerCertByName")
        .or_else(|| raw_string(raw, "verify-peer-cert-by-name"));
    if skip && pinned.is_none() && verify_name.is_none() {
        bail!("skip-cert-verify=true 没有可用的固定证书或校验证书名；Xray 26.9.9 不再接受 allowInsecure");
    }
    if !tls && network == "tcp" {
        return Ok(None);
    }
    let mut stream = Map::new();
    stream.insert("network".into(), Value::String(network.clone()));
    let mut settings = Map::new();
    match network.as_str() {
        "ws" => {
            let opts = raw_map(raw, "ws-opts");
            let path = opts
                .as_ref()
                .and_then(|map| map.get(serde_yaml::Value::String("path".into())))
                .and_then(serde_value_string)
                .unwrap_or_else(|| "/".into());
            let mut ws = Map::new();
            ws.insert("path".into(), Value::String(path));
            if let Some(headers) = opts
                .as_ref()
                .and_then(|map| map.get(serde_yaml::Value::String("headers".into())))
                .and_then(|value| value.as_mapping())
                .and_then(yaml_string_map)
            {
                ws.insert("headers".into(), json!(headers));
            }
            settings.insert("wsSettings".into(), Value::Object(ws));
        }
        "grpc" => {
            let opts = raw_map(raw, "grpc-opts");
            let service = opts
                .as_ref()
                .and_then(|map| map.get(serde_yaml::Value::String("grpc-service-name".into())))
                .and_then(serde_value_string)
                .unwrap_or_default();
            settings.insert("grpcSettings".into(), json!({"serviceName": service}));
        }
        "h2" | "http" => {
            bail!("HTTP transport 已被 Xray 26.9.9 移除，请改用 XHTTP");
        }
        "xhttp" | "splithttp" => {
            let opts = raw_map(raw, "xhttp-opts").or_else(|| raw_map(raw, "splithttp-opts"));
            let path = opts
                .as_ref()
                .and_then(|map| map.get(serde_yaml::Value::String("path".into())))
                .and_then(serde_value_string)
                .unwrap_or_else(|| "/".into());
            let mut xhttp = Map::new();
            xhttp.insert("path".into(), Value::String(path));
            if let Some(hosts) = opts
                .as_ref()
                .and_then(|map| map.get(serde_yaml::Value::String("host".into())))
                .and_then(serde_value_string)
            {
                xhttp.insert("host".into(), Value::String(hosts));
            }
            if let Some(mode) = opts
                .as_ref()
                .and_then(|map| map.get(serde_yaml::Value::String("mode".into())))
                .and_then(serde_value_string)
            {
                xhttp.insert("mode".into(), Value::String(mode));
            }
            settings.insert("xhttpSettings".into(), Value::Object(xhttp));
        }
        "tcp" => {}
        "kcp" => bail!("mKCP transport 尚未映射到 Xray 配置"),
        _ => bail!("传输方式 {network} 尚未纳入 Xray 通道"),
    }
    if tls {
        let mut tls_settings = Map::new();
        if let Some(server_name) = raw_string(raw, "servername").or_else(|| raw_string(raw, "sni")) {
            tls_settings.insert("serverName".into(), Value::String(server_name));
        }
        if let Some(alpn) = raw_string_list(raw, "alpn") {
            tls_settings.insert("alpn".into(), json!(alpn));
        }
        if let Some(fingerprint) = raw_string(raw, "client-fingerprint") {
            tls_settings.insert("fingerprint".into(), Value::String(fingerprint));
        }
        if let Some(value) = pinned {
            tls_settings.insert("pinnedPeerCertSha256".into(), Value::String(value));
        }
        if let Some(value) = verify_name {
            tls_settings.insert("verifyPeerCertByName".into(), Value::String(value));
        }
        if let Some(reality) = raw_map(raw, "reality-opts") {
            if let Some(public_key) = reality
                .get(serde_yaml::Value::String("public-key".into()))
                .and_then(serde_value_string)
            {
                tls_settings.insert("publicKey".into(), Value::String(public_key));
            }
            if let Some(short_id) = reality
                .get(serde_yaml::Value::String("short-id".into()))
                .and_then(serde_value_string)
            {
                tls_settings.insert("shortId".into(), Value::String(short_id));
            }
            stream.insert("security".into(), Value::String("reality".into()));
            stream.insert("realitySettings".into(), Value::Object(tls_settings));
        } else {
            stream.insert("security".into(), Value::String("tls".into()));
            stream.insert("tlsSettings".into(), Value::Object(tls_settings));
        }
    }
    for (key, value) in settings {
        stream.insert(key, value);
    }
    Ok(Some(Value::Object(stream)))
}

fn raw_value<'a>(raw: &'a serde_yaml::Value, key: &str) -> Option<&'a serde_yaml::Value> {
    raw.as_mapping()?
        .get(serde_yaml::Value::String(key.to_string()))
}

fn raw_map(raw: &serde_yaml::Value, key: &str) -> Option<serde_yaml::Mapping> {
    raw_value(raw, key)?.as_mapping().cloned()
}

fn raw_string(raw: &serde_yaml::Value, key: &str) -> Option<String> {
    raw_value(raw, key).and_then(serde_value_string)
}

fn serde_value_string(value: &serde_yaml::Value) -> Option<String> {
    match value {
        serde_yaml::Value::String(value) => Some(value.clone()),
        serde_yaml::Value::Number(value) => Some(value.to_string()),
        serde_yaml::Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn raw_truthy(raw: &serde_yaml::Value, key: &str) -> bool {
    raw_value(raw, key)
        .and_then(|value| match value {
            serde_yaml::Value::Bool(value) => Some(*value),
            serde_yaml::Value::String(value) => Some(value.eq_ignore_ascii_case("true")),
            _ => None,
        })
        .unwrap_or(false)
}

fn raw_u16(raw: &serde_yaml::Value, key: &str) -> Option<u16> {
    raw_value(raw, key).and_then(|value| match value {
        serde_yaml::Value::Number(number) => number.as_u64().and_then(|value| u16::try_from(value).ok()),
        serde_yaml::Value::String(value) => value.parse().ok(),
        _ => None,
    })
}

fn raw_u32(raw: &serde_yaml::Value, key: &str) -> Option<u32> {
    raw_value(raw, key).and_then(|value| match value {
        serde_yaml::Value::Number(number) => number.as_u64().and_then(|value| u32::try_from(value).ok()),
        serde_yaml::Value::String(value) => value.parse().ok(),
        _ => None,
    })
}

fn raw_string_list(raw: &serde_yaml::Value, key: &str) -> Option<Vec<String>> {
    match raw_value(raw, key)? {
        serde_yaml::Value::Sequence(values) => Some(values.iter().filter_map(serde_value_string).collect()),
        value => serde_value_string(value).map(|value| vec![value]),
    }
}

fn yaml_string_map(map: &serde_yaml::Mapping) -> Option<HashMap<String, String>> {
    let mut result = HashMap::new();
    for (key, value) in map {
        let key = serde_value_string(key)?;
        let value = serde_value_string(value)?;
        result.insert(key, value);
    }
    Some(result)
}

fn node_tag(index: usize, name: &str) -> String {
    format!("node-{}-{}", index + 1, tag_fragment(name))
}

fn tag_fragment(value: &str) -> String {
    let mut output: String = value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect();
    output.truncate(32);
    while output.ends_with('-') {
        output.pop();
    }
    if output.is_empty() {
        "proxy".into()
    } else {
        output
    }
}

pub fn http_port(socks_port: u16) -> u16 {
    socks_port.saturating_add(XRAY_HTTP_PORT_OFFSET)
}

fn safe_error(error: &anyhow::Error) -> String {
    error
        .chain()
        .last()
        .map(|error| error.to_string())
        .unwrap_or_else(|| "配置不受支持".into())
        .replace('\n', " ")
}

pub fn apply(strategy: &Strategy, refresh: bool) -> Result<Catalog> {
    let catalog = if refresh {
        catalog::refresh(strategy)?
    } else {
        match Catalog::load() {
            Ok(catalog) if catalog.matches_strategy(strategy) => catalog,
            _ => catalog::refresh(strategy)?,
        }
    };
    apply_catalog(strategy, &catalog, wanted())?;
    Ok(catalog)
}

pub fn connect(strategy: &Strategy) -> Result<()> {
    let catalog = match Catalog::load() {
        Ok(catalog) if catalog.matches_strategy(strategy) => catalog,
        _ => catalog::refresh(strategy)?,
    };
    apply_catalog(strategy, &catalog, true)
}

fn apply_catalog(strategy: &Strategy, catalog: &Catalog, should_run: bool) -> Result<()> {
    let _operation_lock = operation_lock()?;
    let report = compile_config(strategy, catalog)?;
    let candidate = paths::xray_candidate_config_path()?;
    paths::atomic_write(&candidate, report.json.as_bytes())?;
    let validation = validate_config(&candidate);
    let _ = fs::remove_file(&candidate);
    validation?;

    let previous = load_runtime()?;
    let previous_config = fs::read(paths::xray_config_path()?).ok();
    let generation = next_generation(previous.as_ref().map_or(0, |runtime| runtime.generation));
    let runtime = RuntimeState {
        strategy: strategy.clone(),
        catalog: catalog.clone(),
        config: report.json.clone(),
        generation,
        warnings: report.warnings.clone(),
        current: report.current.clone(),
    };
    paths::atomic_write(&paths::xray_config_path()?, report.json.as_bytes())?;
    if !should_run {
        stop_process()?;
        runtime.save()?;
        set_wanted(false)?;
        return Ok(());
    }

    set_wanted(true)?;
    let result = (|| -> Result<()> {
        stop_process()?;
        start_process(&runtime)
    })();
    if let Err(error) = result {
        let rollback = (|| -> Result<()> {
            stop_process()?;
            if let Some(bytes) = previous_config {
                paths::atomic_write(&paths::xray_config_path()?, &bytes)?;
                if let Some(previous) = previous {
                    start_process(&previous)?;
                    previous.save()?;
                } else {
                    set_wanted(false)?;
                }
            } else {
                let _ = fs::remove_file(paths::xray_config_path()?);
                set_wanted(false)?;
            }
            Ok(())
        })();
        return match rollback {
            Ok(()) => Err(error).context("Xray 新配置未启动，已保留上次有效配置"),
            Err(rollback) => Err(error).context(format!("Xray 新配置未启动，回退也失败：{rollback:#}")),
        };
    }
    runtime.save()?;
    Ok(())
}

pub fn select_proxy(identity: &crate::supervisor::RuntimeIdentity, group: &str, name: &str) -> Result<()> {
    let runtime = load_runtime()?.context("没有已应用的 Xray 运行快照")?;
    let actual = runtime.identity();
    if actual.generation != identity.generation || actual.socks_port != identity.mixed_port {
        bail!("Xray 配置已改变，请刷新后重试");
    }
    let mut strategy = Strategy::load()?;
    if group == GLOBAL_GROUP {
        strategy.global_selected = name.into();
    } else if let Some(saved) = strategy.groups.iter_mut().find(|saved| saved.name == group) {
        saved.selected = name.into();
    } else {
        bail!("节点组不存在：{group}");
    }
    let catalog = runtime.catalog.clone();
    apply_catalog(&strategy, &catalog, true)
}

pub fn disconnect() -> Result<()> {
    let _operation_lock = operation_lock()?;
    set_wanted(false)?;
    stop_process()
}

pub fn is_running() -> bool {
    let Ok(pid) = read_pid() else { return false };
    pid_alive(pid) && pid_owned(pid)
}

pub fn wanted() -> bool {
    let Ok(path) = paths::xray_wanted_path() else {
        return false;
    };
    fs::read_to_string(path)
        .map(|value| value.trim() == "1")
        .unwrap_or(false)
}

pub fn sync_wanted_on_launch() -> Result<()> {
    if !is_running() && wanted() {
        set_wanted(false)?;
    }
    Ok(())
}

pub fn runtime_identity() -> Option<XrayRuntimeIdentity> {
    if !is_running() {
        return None;
    }
    load_runtime().ok().flatten().map(|runtime| runtime.identity())
}

pub fn applied_strategy() -> Option<Strategy> {
    load_runtime().ok().flatten().map(|runtime| runtime.strategy)
}

pub fn status() -> Result<XrayStatus> {
    let runtime = load_runtime()?;
    let running = is_running();
    let wanted = wanted();
    let (generation, socks_port, http_port, current, warnings) = runtime
        .as_ref()
        .map(|runtime| {
            let identity = runtime.identity();
            (
                Some(identity.generation),
                Some(identity.socks_port),
                Some(identity.http_port),
                runtime.current.clone(),
                runtime.warnings.clone(),
            )
        })
        .unwrap_or((None, None, None, String::new(), Vec::new()));
    let ready = running
        && socks_port
            .map(|port| listener_ready(port))
            .unwrap_or(false)
        && http_port
            .map(|port| listener_ready(port))
            .unwrap_or(false);
    Ok(XrayStatus {
        backend: BackendKind::Xray,
        wanted,
        running,
        ready,
        generation,
        socks_port,
        http_port,
        current,
        warnings,
        note: if !running && wanted {
            Some("Xray 已请求连接，但进程或监听未就绪".into())
        } else {
            None
        },
    })
}

fn load_runtime() -> Result<Option<RuntimeState>> {
    let path = paths::xray_runtime_state_path()?;
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("parse Xray runtime state").map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context("read Xray runtime state"),
    }
}

impl RuntimeState {
    fn save(&self) -> Result<()> {
        paths::atomic_write(
            &paths::xray_runtime_state_path()?,
            serde_json::to_string_pretty(self)?.as_bytes(),
        )
    }
}

fn validate_config(path: &Path) -> Result<()> {
    let bin = paths::bundled_xray();
    if !bin.is_file() {
        bail!(
            "Xray binary missing at {}. Run scripts/fetch-xray.sh or install it next to myproxy",
            bin.display()
        );
    }
    let output = Command::new(&bin)
        .args(["run", "-test", "-config"])
        .arg(path)
        .output()
        .context("spawn Xray config validation")?;
    if output.status.success() {
        return Ok(());
    }
    bail!("Xray 候选配置未通过内核校验；运行配置未改变")
}

fn start_process(runtime: &RuntimeState) -> Result<()> {
    let bin = paths::bundled_xray();
    if !bin.is_file() {
        bail!("Xray binary missing at {}", bin.display());
    }
    let config = paths::xray_config_path()?;
    let log_path = paths::xray_log_path()?;
    let log_file = fs::File::create(&log_path).context("create xray.log")?;
    let child = Command::new(&bin)
        .args(["run", "-config"])
        .arg(&config)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file.try_clone()?))
        .stderr(Stdio::from(log_file))
        .spawn()
        .context("spawn Xray")?;
    paths::atomic_write(&paths::xray_pid_path()?, child.id().to_string().as_bytes())?;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if !pid_alive(child.id() as i32) {
            bail!("Xray 在启动时退出，请检查 xray.log")
        }
        if listener_ready(runtime.strategy.mixed_port) && listener_ready(http_port(runtime.strategy.mixed_port)) {
            log::info("xray", format!("ready socks={} http={}", runtime.strategy.mixed_port, http_port(runtime.strategy.mixed_port)));
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    bail!("Xray 进程已启动，但 SOCKS/HTTP 入口未就绪；请检查 xray.log")
}

fn stop_process() -> Result<()> {
    let pid = read_pid().ok();
    if let Some(pid) = pid.filter(|pid| pid_alive(*pid) && pid_owned(*pid)) {
        unsafe { libc::kill(pid, libc::SIGTERM) };
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while pid_alive(pid) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        if pid_alive(pid) {
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }
    let _ = fs::remove_file(paths::xray_pid_path()?);
    Ok(())
}

fn read_pid() -> Result<i32> {
    let pid: i32 = fs::read_to_string(paths::xray_pid_path()?)?.trim().parse()?;
    Ok(pid)
}

fn pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    unsafe { libc::kill(pid, 0) == 0 }
}

fn pid_owned(pid: i32) -> bool {
    let Ok(output) = Command::new("ps").args(["-p", &pid.to_string(), "-o", "command="]).output() else {
        return false;
    };
    let command = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
    command.contains("xray") && command.contains("xray-config.json")
}

fn listener_ready(port: u16) -> bool {
    let address = ("127.0.0.1", port)
        .to_socket_addrs()
        .ok()
        .and_then(|mut addresses| addresses.next());
    address
        .map(|address| TcpStream::connect_timeout(&address, Duration::from_millis(120)).is_ok())
        .unwrap_or(false)
}

fn set_wanted(value: bool) -> Result<()> {
    paths::atomic_write(&paths::xray_wanted_path()?, if value { b"1" } else { b"0" })
}

fn next_generation(previous: u64) -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    (now.min(u64::MAX as u128) as u64).max(previous.saturating_add(1))
}

fn operation_lock() -> Result<fs::File> {
    let file = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(paths::operation_lock_path()?)?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        bail!("另一个核心操作正在进行中，请稍后重试");
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::{Group, RuleSet};

    fn fixture() -> (Strategy, Catalog) {
        let mut strategy = Strategy::default();
        strategy.mixed_port = 40808;
        strategy.groups = vec![Group::all_nodes("美国优先".into(), "url-test".into())];
        strategy.rule_sets = vec![RuleSet {
            id: "rule".into(),
            name: "GitHub".into(),
            via: "美国优先".into(),
            matchers: vec![Matcher {
                kind: "suffix".into(),
                value: "github.com".into(),
            }],
        }];
        let nodes = [
            ("us", "vmess", "us.example", 443),
            ("jp", "ss", "jp.example", 8388),
        ]
        .into_iter()
        .map(|(name, kind, server, port)| Node {
            name: name.into(),
            subscription: "fixture".into(),
            raw: serde_yaml::from_str(&format!(
                "name: {name}\ntype: {kind}\nserver: {server}\nport: {port}\nuuid: 00000000-0000-0000-0000-000000000001\ncipher: aes-128-gcm\npassword: demo\n"
            ))
            .unwrap(),
        })
        .collect();
        (strategy, Catalog { nodes, ..Catalog::default() })
    }

    #[test]
    fn compile_has_separate_socks_and_http_inbounds() {
        let (strategy, catalog) = fixture();
        let report = compile_config(&strategy, &catalog).unwrap();
        let value: Value = serde_json::from_str(&report.json).unwrap();
        let inbounds = value["inbounds"].as_array().unwrap();
        assert_eq!(inbounds[0]["port"], 40808);
        assert_eq!(inbounds[1]["port"], 40809);
        assert!(value["observatory"].is_object());
        assert!(!report.json.contains("allowInsecure"));
    }

    #[test]
    fn skip_cert_verify_without_pin_isolated_to_node() {
        let (mut strategy, mut catalog) = fixture();
        catalog.nodes[0].raw = serde_yaml::from_str(
            "name: us\ntype: vmess\nserver: us.example\nport: 443\nuuid: 00000000-0000-0000-0000-000000000001\ntls: true\nskip-cert-verify: true\n",
        )
        .unwrap();
        let report = compile_config(&strategy, &catalog).unwrap();
        assert!(report.warnings.iter().any(|warning| warning.contains("skip-cert-verify")));
        assert!(!report.json.contains("allowInsecure"));
        strategy.global_selected = "jp".into();
        assert!(compile_config(&strategy, &catalog).is_ok());
    }

    #[test]
    fn removed_http_transport_isolated_to_node() {
        let (strategy, mut catalog) = fixture();
        catalog.nodes[0].raw = serde_yaml::from_str(
            "name: us\ntype: vmess\nserver: us.example\nport: 443\nuuid: 00000000-0000-0000-0000-000000000001\nnetwork: h2\n",
        )
        .unwrap();
        let report = compile_config(&strategy, &catalog).unwrap();
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.contains("HTTP transport")));
        assert!(!report.json.contains("httpSettings"));
    }

    #[test]
    fn xhttp_uses_xray_settings_shape() {
        let (strategy, mut catalog) = fixture();
        catalog.nodes[0].raw = serde_yaml::from_str(
            "name: us\ntype: vmess\nserver: us.example\nport: 443\nuuid: 00000000-0000-0000-0000-000000000001\nnetwork: xhttp\nxhttp-opts:\n  path: /x\n  host: proxy.example\n",
        )
        .unwrap();
        let report = compile_config(&strategy, &catalog).unwrap();
        assert!(report.json.contains("xhttpSettings"));
        assert!(report.json.contains("proxy.example"));
    }

    #[test]
    fn config_passes_real_xray_schema_when_binary_is_provided() {
        let Ok(binary) = std::env::var("XRAY_BINARY") else {
            return;
        };
        let (strategy, catalog) = fixture();
        let report = compile_config(&strategy, &catalog).unwrap();
        let path = std::env::temp_dir().join(format!(
            "myproxy-xray-schema-{}.json",
            std::process::id()
        ));
        std::fs::write(&path, report.json).unwrap();
        let output = Command::new(binary)
            .args(["run", "-test", "-config"])
            .arg(&path)
            .output()
            .unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(
            output.status.success(),
            "Xray rejected the projected config: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
