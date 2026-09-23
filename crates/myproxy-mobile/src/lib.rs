//! Native commands delegate business decisions to the existing MyProxy policy.
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::hash::{Hash, Hasher};
use std::os::raw::c_char;
use std::sync::{Mutex, OnceLock};

use anyhow::{bail, Context, Result};
use myproxy_core::{catalog, policy, Catalog, InboundMode, Matcher, Node, NodeHealth, Route, RuleSet, Strategy, Subscription};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Platform { Ios, Android }

#[derive(Clone, Serialize, Deserialize)]
struct Document {
    #[serde(default = "schema")]
    schema: u32,
    #[serde(default)]
    revision: u64,
    strategy: Strategy,
    catalog: Catalog,
}
fn schema() -> u32 { 1 }

struct Runtime {
    document: Document,
    accepted: Catalog,
    invalid: HashMap<String, String>,
    outbounds: Vec<Value>,
    health: HashMap<String, NodeHealth>,
    platform: Platform,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
enum Command {
    Init { platform: Platform },
    Load { document: String, platform: Platform },
    Snapshot,
    Export,
    Import { text: String, name: Option<String>, #[serde(rename = "sourceId")] source_id: Option<String>, #[serde(rename = "sourceURL")] source_url: Option<String> },
    RemoveSource { id: String },
    Select { group: String, member: String },
    SetMode { mode: String },
    SetAutoConnect { enabled: bool },
    SetFallback { via: String },
    SaveRule { id: Option<String>, name: String, kind: String, value: String, via: String },
    DeleteRule { id: String },
    Health { node: String, #[serde(rename = "delayMs")] delay_ms: Option<i64>, failed: bool },
    Route { host: String, hostname: Option<String>, port: u16, network: String },
    Render,
    Connect,
    Disconnect,
    Probe,
}

static RUNTIME: OnceLock<Mutex<Option<Runtime>>> = OnceLock::new();

impl Runtime {
    fn new(platform: Platform) -> Result<Self> {
        let mut strategy = myproxy_core::defaults::default_strategy();
        strategy.extension_mode = strategy.mixed_mode;
        Self::prepare(Document { schema: 1, revision: 1, strategy, catalog: Catalog::default() }, platform, HashMap::new())
    }

    fn prepare(mut document: Document, platform: Platform, health: HashMap<String, NodeHealth>) -> Result<Self> {
        if document.schema != 1 { bail!("不支持这个配置版本") }
        if document.catalog.nodes.len() > 2048 { bail!("手机配置最多支持 2048 个节点") }
        document.strategy.extension_mode = document.strategy.mixed_mode;
        validate_policy(&document.strategy, &document.catalog, platform)?;
        let mut accepted = document.catalog.clone();
        accepted.nodes.clear();
        let mut invalid = HashMap::new();
        let mut outbounds = Vec::new();
        for node in &document.catalog.nodes {
            match myproxy_core::nodes::render(node, &tag(&node.name)) {
                Ok(value) => { accepted.nodes.push(node.clone()); outbounds.push(value); }
                Err(error) => { invalid.insert(node.name.clone(), error.to_string()); }
            }
        }
        Ok(Self { document, accepted, invalid, outbounds, health, platform })
    }

    fn snapshot(&self) -> Value {
        let strategy = &self.document.strategy;
        let live = policy::groups(strategy, &self.accepted, &self.health);
        let groups = strategy.groups.iter().map(|group| {
            let live = live.iter().find(|item| item.name == group.name);
            let members = live.map(|item| item.members.iter().map(|member| {
                let is_group = strategy.groups.iter().any(|g| g.name == member.name);
                json!({"name": member.name, "kind": if is_group { "group" } else { "node" },
                    "available": self.available(&member.name), "delayMs": member.delay})
            }).collect::<Vec<_>>()).unwrap_or_default();
            let resolved = match policy::decide_target(strategy, &self.accepted, &self.health, &group.name).route {
                Route::Node(name) => Some(name), _ => None,
            };
            json!({"id": group.id, "name": group.name, "kind": group.kind, "selected": group.selected,
                "resolved": resolved, "members": members})
        }).collect::<Vec<_>>();
        let nodes = self.document.catalog.nodes.iter().map(|node| json!({
            "name": node.name, "protocol": node.raw.get("type").and_then(|v| v.as_str()).unwrap_or("unknown"),
            "source": node.subscription, "available": self.available(&node.name),
            "error": self.invalid.get(&node.name), "delayMs": self.health.get(&node.name).and_then(|h| h.delay_ms),
        })).collect::<Vec<_>>();
        let rules = strategy.rule_sets.iter().map(|rule| {
            let simple = if rule.matchers.len() == 1 { rule.matchers.first() } else { None };
            let kind = simple.map(|m| if m.kind == "suffix" { "domain-suffix" } else { m.kind.as_str() }).unwrap_or("complex");
            let value = simple.map(|m| m.value.clone()).unwrap_or_else(|| format!("{} 项条件", rule.matchers.len()));
            json!({"id": rule.id, "name": rule.name, "kind": kind, "value": value, "via": rule.via})
        }).collect::<Vec<_>>();
        let warnings = self.document.catalog.refresh_warnings().into_iter()
            .chain(self.invalid.iter().map(|(name, error)| format!("{name}：{error}"))).collect::<Vec<_>>();
        json!({
            "revision": self.document.revision, "mode": strategy.mixed_mode.as_str(),
            "autoConnect": strategy.connect_on_launch, "selected": strategy.global_selected,
            "fallback": strategy.unmatched_via,
            "capabilities": {"dynamicDirect": self.platform == Platform::Android, "appRouting": false},
            "sources": strategy.subscriptions.iter().map(|source| json!({"id": source.id, "name": source.name,
                "url": source.url, "nodeCount": self.document.catalog.nodes.iter().filter(|n| n.subscription == source.name).count()})).collect::<Vec<_>>(),
            "nodes": nodes, "groups": groups, "rules": rules, "warnings": warnings,
            "runtime": {"phase": "disconnected", "message": null, "connectedAt": null,
                "uploadBytes": 0, "downloadBytes": 0, "connections": []},
        })
    }

    fn available(&self, target: &str) -> bool {
        match policy::decide_target(&self.document.strategy, &self.accepted, &self.health, target).route {
            Route::Node(name) => self.health.get(&name).map_or(true, |h| h.failures < 2),
            Route::Direct => self.platform == Platform::Android,
            Route::Reject => false,
        }
    }

    fn render(&self) -> Result<Value> {
        if self.accepted.nodes.is_empty() { bail!("没有可用节点，请先添加节点或订阅") }
        let config = json!({"log": {"loglevel": "none"}, "inbounds": [], "outbounds": self.outbounds});
        Ok(json!({"config": config.to_string(), "nodes": self.accepted.nodes.iter().map(|n| json!({"name": n.name, "tag": tag(&n.name)})).collect::<Vec<_>>(), "revision": self.document.revision}))
    }

    fn route(&self, host: String, hostname: Option<String>, port: u16, network: String) -> Result<Value> {
        if host.is_empty() || host.len() > 253 || port == 0 || !matches!(network.as_str(), "tcp" | "udp") { bail!("连接信息无效") }
        let context = policy::FlowContext { host: &host, hostname: hostname.as_deref(), port, network: &network, user_id: None, applications: &[] };
        let decision = policy::decide_application(&self.document.strategy, &self.accepted, &self.health, &context);
        let (action, node) = match decision.route {
            Route::Node(node) => ("proxy", Some(node)),
            Route::Direct if self.platform == Platform::Android => ("direct", None),
            _ => ("reject", None),
        };
        Ok(json!({"action": action, "tag": node.as_deref().map(tag), "node": node,
            "rule": decision.rule, "chain": decision.chain, "revision": self.document.revision}))
    }

    fn mutate(&mut self, command: Command) -> Result<Value> {
        let mut next = self.document.clone();
        match command {
            Command::Import { text, name, source_id, source_url } => import(&mut next, &text, name, source_id, source_url)?,
            Command::RemoveSource { id } => {
                let source = next.strategy.subscriptions.iter().find(|s| s.id == id).context("订阅不存在")?.name.clone();
                next.strategy.subscriptions.retain(|s| s.id != id);
                next.catalog.nodes.retain(|node| node.subscription != source);
                next.catalog.excluded.retain(|node| node.subscription != source);
                next.catalog.subscription_urls.remove(&source);
            }
            Command::Select { group, member } => {
                if group == "GLOBAL" {
                    if member.is_empty() { bail!("请选择节点或代理组") }
                    policy_target_exists(&next, &member)?;
                    next.strategy.global_selected = member;
                } else {
                    let current = next.strategy.groups.iter().find(|g| g.id == group || g.name == group).context("节点组不存在")?;
                    let members = catalog::resolve_group_members(current, &next.catalog);
                    if !member.is_empty() && !members.iter().any(|name| name == &member) { bail!("该节点不在所选节点组内") }
                    if member == current.name { bail!("节点组不能选择自身") }
                    let id = current.id.clone();
                    next.strategy.set_group_selected(&id, member);
                }
            }
            Command::SetMode { mode } => {
                next.strategy.mixed_mode = match mode.as_str() { "global" => InboundMode::Global, "rule" => InboundMode::Rule, _ => bail!("不支持的连接模式") };
            }
            Command::SetAutoConnect { enabled } => next.strategy.connect_on_launch = enabled,
            Command::SetFallback { via } => { policy_target_exists(&next, &via)?; next.strategy.unmatched_via = via; next.strategy.routing_profile = myproxy_core::RoutingProfile::Group; }
            Command::SaveRule { id, name, kind, value, via } => {
                policy_target_exists(&next, &via)?;
                let previous = id.as_ref().and_then(|id| next.strategy.rule_sets.iter().find(|r| &r.id == id)).cloned();
                let matchers = if let Some(previous) = previous.as_ref().filter(|r| r.matchers.len() != 1) {
                    if kind != "complex" { bail!("复合规则只能调整出口，请保留原有匹配条件") }
                    previous.matchers.clone()
                } else {
                    let kind: &str = match kind.as_str() { "domain-suffix" => "suffix", "domain" | "suffix" | "keyword" | "cidr" | "wildcard" => &kind, _ => bail!("这种规则暂不支持编辑") };
                    let matcher = Matcher { kind: kind.into(), value: value.trim().into() }; matcher.validate()?; vec![matcher]
                };
                let rule = RuleSet { id: id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()), name: name.trim().into(), via, matchers, unavailable_fallback: previous.and_then(|r| r.unavailable_fallback) };
                if let Some(previous) = next.strategy.rule_sets.iter_mut().find(|r| r.id == rule.id) { *previous = rule; } else { next.strategy.rule_sets.push(rule); }
            }
            Command::DeleteRule { id } => { if !next.strategy.rule_sets.iter().any(|r| r.id == id) { bail!("规则不存在") }; next.strategy.rule_sets.retain(|r| r.id != id); }
            _ => bail!("操作不属于配置修改"),
        }
        next.revision = self.document.revision.checked_add(1).context("配置版本超出范围")?;
        *self = Self::prepare(next, self.platform, self.health.clone())?;
        Ok(self.snapshot())
    }
}

fn validate_policy(strategy: &Strategy, catalog: &Catalog, platform: Platform) -> Result<()> {
    strategy.validate()?;
    strategy.validate_catalog(catalog)?;
    if !matches!(strategy.mixed_mode, InboundMode::Global | InboundMode::Rule) { bail!("手机端支持全局和规则模式，请先转换此配置") }
    if !matches!(strategy.routing_profile, myproxy_core::RoutingProfile::Group | myproxy_core::RoutingProfile::Allowlist) { bail!("此配置依赖尚未导入手机的规则数据库") }
    for rule in &strategy.rule_sets {
        if rule.matchers.iter().any(|m| matches!(m.kind.as_str(), "app" | "uid" | "geo-site" | "geo-ip")) || rule.via.starts_with("gfw:") {
            bail!("规则“{}”包含手机尚不支持的应用、用户或数据库条件", rule.name)
        }
        if platform == Platform::Ios && (rule.via.eq_ignore_ascii_case("DIRECT") || rule.unavailable_fallback == Some(myproxy_core::strategy::UnavailableFallback::Direct)) {
            bail!("规则“{}”使用动态直连，暂时不能在 iOS 启用", rule.name)
        }
    }
    if platform == Platform::Ios && (strategy.global_selected.eq_ignore_ascii_case("DIRECT") || strategy.unmatched_via.eq_ignore_ascii_case("DIRECT") || strategy.routing_profile == myproxy_core::RoutingProfile::Allowlist) {
        bail!("此配置包含 iOS 不支持的动态直连，请选择代理组作为出口")
    }
    if strategy.groups.iter().any(|g| g.kind == "relay") { bail!("手机端暂不支持串联代理组") }
    Ok(())
}

fn policy_target_exists(document: &Document, name: &str) -> Result<()> {
    if matches!(name, "DIRECT" | "REJECT") || document.strategy.groups.iter().any(|g| g.name == name) || document.catalog.nodes.iter().any(|n| n.name == name) { return Ok(()) }
    bail!("找不到所选节点或代理组")
}

fn import(document: &mut Document, text: &str, name: Option<String>, source_id: Option<String>, source_url: Option<String>) -> Result<()> {
    if text.len() > 2_000_000 { bail!("导入内容超过 2 MB") }
    if let Ok(imported) = serde_json::from_str::<Document>(text) { *document = imported; return Ok(()) }
    if let Ok(strategy) = serde_json::from_str::<Strategy>(text) { document.strategy = strategy; return Ok(()) }
    let raw_nodes = catalog::parse_subscription(text).context("未找到可导入的订阅或节点链接")?;
    if raw_nodes.is_empty() { bail!("订阅没有节点，保留原有配置") }
    let existing = source_id.as_ref().and_then(|id| document.strategy.subscriptions.iter().find(|s| &s.id == id)).cloned();
    let source = if let Some(source) = existing { source } else {
        let base = name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "导入节点".into());
        let mut label = base.trim().to_string();
        let mut number = 2;
        while document.strategy.subscriptions.iter().any(|s| s.name == label) { label = format!("{base} {number}"); number += 1; }
        Subscription { id: source_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()), name: label, url: source_url.clone().unwrap_or_default() }
    };
    let mut catalog = document.catalog.clone();
    catalog.nodes.retain(|n| n.subscription != source.name);
    catalog.excluded.retain(|n| n.subscription != source.name);
    let mut names = catalog.nodes.iter().map(|n| n.name.clone()).chain(document.strategy.groups.iter().map(|g| g.name.clone())).collect::<HashSet<_>>();
    for mut raw in raw_nodes {
        let original = raw.get("name").and_then(|v| v.as_str()).unwrap_or("导入节点").trim().replace([',', '\n', '\r'], " ");
        let mut label = if original.is_empty() { "导入节点".into() } else { original.clone() };
        let mut number = 2;
        while !names.insert(label.clone()) { label = format!("{original} · {} {number}", source.name); number += 1; }
        let mapping = raw.as_mapping_mut().context("节点格式无效")?;
        mapping.insert(serde_yaml::Value::String("name".into()), serde_yaml::Value::String(label.clone()));
        catalog.nodes.push(Node { name: label, subscription: source.name.clone(), raw });
    }
    catalog.validate()?;
    catalog.subscription_urls.insert(source.name.clone(), source.url.clone());
    if let Some(old) = document.strategy.subscriptions.iter_mut().find(|s| s.id == source.id) { *old = source; } else { document.strategy.subscriptions.push(source); }
    document.catalog = catalog;
    Ok(())
}

fn tag(name: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut hasher);
    format!("node-{:016x}", hasher.finish())
}

fn dispatch(request: &str) -> Result<Value> {
    if request.len() > 8*1024*1024 { bail!("请求过大") }
    let command: Command = serde_json::from_str(request).context("操作参数无效")?;
    let mut state = RUNTIME.get_or_init(|| Mutex::new(None)).lock().map_err(|_| anyhow::anyhow!("核心状态不可用，请重新打开应用"))?;
    match command {
        Command::Init { platform } => {
            if state.is_none() { *state = Some(Runtime::new(platform)?); }
            let runtime = state.as_ref().context("核心尚未初始化")?;
            if runtime.platform != platform { bail!("平台配置不一致") }
            Ok(runtime.snapshot())
        }
        Command::Load { document, platform } => {
            if document.len() > 8*1024*1024 { bail!("配置过大") }
            let document: Document = serde_json::from_str(&document).context("配置文档格式无效")?;
            let health = state.as_ref().map(|s| s.health.clone()).unwrap_or_default();
            *state = Some(Runtime::prepare(document, platform, health)?);
            Ok(state.as_ref().context("核心尚未初始化")?.snapshot())
        }
        command => {
            let runtime = state.as_mut().context("核心尚未初始化")?;
            match command {
                Command::Snapshot => Ok(runtime.snapshot()),
                Command::Export => Ok(Value::String(serde_json::to_string(&runtime.document)?)),
                Command::Render => runtime.render(),
                Command::Route { host, hostname, port, network } => runtime.route(host, hostname, port, network),
                Command::Health { node, delay_ms, failed } => {
                    if runtime.accepted.nodes.iter().any(|n| n.name == node) {
                        let health = runtime.health.entry(node).or_default();
                        health.delay_ms = delay_ms.and_then(|v| u32::try_from(v).ok());
                        health.failures = if failed { health.failures.saturating_add(1) } else { 0 };
                    }
                    Ok(Value::Null)
                }
                Command::Connect | Command::Disconnect | Command::Probe => bail!("连接操作需要通过系统 VPN 模块执行"),
                command => runtime.mutate(command),
            }
        }
    }
}

pub fn call_json(request: &str) -> String {
    match std::panic::catch_unwind(|| dispatch(request)) {
        Ok(Ok(data)) => json!({"ok": true, "data": data}).to_string(),
        Ok(Err(error)) => json!({"ok": false, "error": {"code": "invalid_config", "message": error.to_string()}}).to_string(),
        Err(_) => json!({"ok": false, "error": {"code": "internal_error", "message": "核心处理失败，请重新打开应用"}}).to_string(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn myproxy_mobile_call(request: *const c_char) -> *mut c_char {
    let input = if request.is_null() { "" } else { CStr::from_ptr(request).to_str().unwrap_or("") };
    CString::new(call_json(input)).expect("JSON escapes NUL").into_raw()
}

#[no_mangle]
pub unsafe extern "C" fn myproxy_mobile_free(response: *mut c_char) {
    if !response.is_null() { drop(CString::from_raw(response)); }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_one_leaper_myproxy_core_NativeCore_call(mut env: jni::JNIEnv<'_>, _: jni::objects::JClass<'_>, request: jni::objects::JString<'_>) -> jni::sys::jstring {
    let input = env.get_string(&request).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    env.new_string(call_json(&input)).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut())
}
