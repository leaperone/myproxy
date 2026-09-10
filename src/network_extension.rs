use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::compile;
use crate::gfw;
use crate::log;
use crate::login_item;
use crate::strategy::{InboundMode, Strategy};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnableRequest {
    pub revision: u64,
    pub operation_revision: u64,
    pub socks_port: u16,
    pub username: String,
    pub password: String,
    pub process_rules: Vec<ProcessRule>,
    pub dest_rules: Vec<DestRule>,
    pub gfw_domains: Vec<String>,
    pub group_ports: Vec<GroupPort>,
    #[serde(default)]
    pub gfw_ports: Vec<GroupPort>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessRule {
    pub order: u64,
    pub pattern: String,
    pub via: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DestRule {
    pub order: u64,
    pub kind: String,
    pub value: String,
    pub via: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupPort {
    pub name: String,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CaptureFace {
    socks_port: u16,
    process_rules: Vec<ProcessRule>,
    dest_rules: Vec<DestRule>,
    gfw_domains: Vec<String>,
    group_ports: Vec<GroupPort>,
    gfw_ports: Vec<GroupPort>,
}

struct Session {
    revision: u64,
    username: String,
    password: String,
    socks_port: u16,
    group_ports: BTreeMap<String, u16>,
    next_port: u16,
    last_face: Option<CaptureFace>,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
static OPERATION_REVISION: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    Unsupported,
    Unbundled,
    Disabled,
    Requesting,
    WaitingApproval,
    RequiresReboot,
    Running,
    Stopping,
    Failed,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unsupported => "当前系统不支持",
            Self::Unbundled => "需要已签名应用",
            Self::Disabled => "已关闭",
            Self::Requesting => "正在启用",
            Self::WaitingApproval => "等待系统授权",
            Self::RequiresReboot => "需要重启系统",
            Self::Running => "正在运行",
            Self::Stopping => "正在关闭",
            Self::Failed => "接管失败",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DnsPhase {
    Unknown,
    Disabled,
    Waiting,
    Running,
    Stopping,
    Failed,
}

impl DnsPhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "状态未知",
            Self::Disabled => "已关闭",
            Self::Waiting => "等待运行报告",
            Self::Running => "正在运行",
            Self::Stopping => "正在关闭",
            Self::Failed => "DNS 不可用",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub phase: Phase,
    pub dns_phase: DnsPhase,
    #[serde(default)]
    pub observed: bool,
    pub desired_revision: u64,
    pub applied_revision: Option<u64>,
    pub message: Option<String>,
    pub dns_message: Option<String>,
}

impl RuntimeStatus {
    pub fn phase_label(&self) -> &'static str {
        if self.observed {
            self.phase.label()
        } else {
            "状态未知"
        }
    }

    pub fn dns_label(&self) -> &'static str {
        if self.observed {
            self.dns_phase.label()
        } else {
            "状态未知"
        }
    }

    pub fn is_pending(&self) -> bool {
        matches!(self.phase, Phase::Requesting | Phase::Stopping)
    }
}

fn unavailable_status() -> RuntimeStatus {
    RuntimeStatus {
        phase: if cfg!(target_os = "macos") {
            Phase::Unbundled
        } else {
            Phase::Unsupported
        },
        dns_phase: DnsPhase::Unknown,
        observed: true,
        desired_revision: 0,
        applied_revision: None,
        message: None,
        dns_message: None,
    }
}

/// Reads a bounded in-memory host snapshot. The host refreshes its existing
/// provider status channel at most once per two seconds without reconnecting.
pub fn status() -> RuntimeStatus {
    #[cfg(target_os = "macos")]
    if login_item::is_bundled() {
        let value = unsafe { ffi::myproxy_ne_status() };
        if let Some(json) = take_error(value) {
            if let Ok(status) = serde_json::from_str(&json) {
                return status;
            }
        }
        let mut status = unavailable_status();
        status.phase = Phase::Failed;
        status.message = Some("无法读取系统接管状态".into());
        return status;
    }
    unavailable_status()
}

/// CLI callers keep the host task alive until it finishes or needs the user.
/// WaitingApproval is a submitted request, never proof of an active provider.
pub fn wait_for_settle(timeout: Duration) -> RuntimeStatus {
    let deadline = Instant::now() + timeout;
    loop {
        let status = status();
        if (!status.is_pending() && status.observed) || Instant::now() >= deadline {
            return status;
        }
        wait_for_callbacks();
    }
}

fn wait_for_callbacks() {
    #[cfg(target_os = "macos")]
    unsafe {
        ffi::myproxy_ne_wait(100)
    };
    #[cfg(not(target_os = "macos"))]
    std::thread::sleep(Duration::from_millis(100));
}

/// Shutdown is complete only after both native managers acknowledge disable.
/// Unknown DNS state is not proof that macOS restored its resolver settings.
pub fn wait_disabled(timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let current = status();
        if matches!(current.phase, Phase::Unsupported | Phase::Unbundled)
            || (current.observed
                && current.phase == Phase::Disabled
                && current.dns_phase == DnsPhase::Disabled)
        {
            return Ok(());
        }
        if current.phase == Phase::Failed {
            bail!(
                "系统接管关闭失败：{}",
                current.message.as_deref().unwrap_or(current.phase.label())
            );
        }
        if Instant::now() >= deadline {
            bail!(
                "等待系统接管关闭超时：{}；DNS {}",
                current.phase.label(),
                current.dns_phase.label()
            );
        }
        wait_for_callbacks();
    }
}

/// A short-lived CLI cannot retain an authorization continuation after exit.
/// Cancel only unfinished work, then finish its cleanup before returning.
pub fn cancel_pending_for_cli() -> Result<bool> {
    let current = status();
    if !current.is_pending() && current.phase != Phase::WaitingApproval {
        return Ok(false);
    }
    disable_async()?;
    wait_disabled(Duration::from_secs(30))?;
    Ok(true)
}

pub fn prepare(strategy: &Strategy) -> Result<()> {
    if !strategy.system_extension {
        return Ok(());
    }
    prepare_request(&try_inbound_plan(strategy)?)
}

pub fn prepare_request(request: &EnableRequest) -> Result<()> {
    validate_request(request)?;
    #[cfg(not(target_os = "macos"))]
    bail!("System Extension is macOS-only");
    #[cfg(target_os = "macos")]
    {
        if !login_item::is_bundled() {
            bail!("系统接管需要包含 Network Extension 的已签名 .app");
        }
        let json = serde_json::to_string(request)?;
        let mut error = std::ptr::null_mut();
        let rc = unsafe { ffi::myproxy_ne_validate(c_string(&json).as_ptr(), &mut error) };
        let message = take_error(error);
        if rc != 0 {
            bail!(
                "{}",
                message.unwrap_or_else(|| "系统接管配置校验失败".into())
            );
        }
        Ok(())
    }
}

fn validate_request(request: &EnableRequest) -> Result<()> {
    if !request.gfw_domains.is_empty() {
        bail!("系统接管不再嵌入 GFWList 域名");
    }
    if request
        .dest_rules
        .iter()
        .any(|rule| gfw::gfw_group(&rule.via).is_some() && rule.kind == "cidr")
    {
        bail!("GFWList 无法与网段规则求交，请改用节点组或域名条件");
    }
    Ok(())
}

pub fn inbound_plan(strategy: &Strategy) -> EnableRequest {
    try_inbound_plan(strategy).expect("valid Network Extension listener plan")
}

pub fn try_inbound_plan(strategy: &Strategy) -> Result<EnableRequest> {
    let socks_port = compile::network_extension_socks_port(strategy.mixed_port);
    let controller = compile::controller_port(strategy.mixed_port);
    let mut session = SESSION.lock().expect("ne session");
    let session = session.get_or_insert_with(|| Session {
        revision: 0,
        username: format!("ne-{}", Uuid::new_v4().simple()),
        password: Uuid::new_v4().simple().to_string(),
        socks_port,
        group_ports: BTreeMap::new(),
        next_port: socks_port.checked_add(1).unwrap_or(1),
        last_face: None,
    });
    if session.socks_port != socks_port {
        session.socks_port = socks_port;
        session.group_ports.clear();
        session.next_port = socks_port.checked_add(1).unwrap_or(1);
    }

    let mut process_rules = Vec::new();
    let mut dest_rules = Vec::new();
    let mut order = 0u64;
    let mut needed = Vec::new();
    let mut gfw_needed = Vec::new();
    if strategy.extension_mode == InboundMode::Rule {
        for set in &strategy.rule_sets {
            let via = set.via.trim();
            if via.is_empty() {
                continue;
            }
            let capture_via = if let Some(group) = gfw::gfw_group(via) {
                format!("gfw:{}", compile::via_target(group, strategy))
            } else {
                compile::via_target(via, strategy)
            };
            let mut pins_inlet = false;
            for matcher in &set.matchers {
                let value = matcher.value.trim();
                if value.is_empty() {
                    continue;
                }
                match matcher.kind.as_str() {
                    "app" => {
                        pins_inlet = true;
                        push_app_patterns(&mut process_rules, order, value, &capture_via);
                    }
                    "domain" | "suffix" | "keyword" | "cidr" => {
                        if matcher.kind == "cidr" && gfw::gfw_group(&capture_via).is_some() {
                            bail!("GFWList 无法与网段规则求交，请改用节点组或域名条件");
                        }
                        pins_inlet = true;
                        dest_rules.push(DestRule {
                            order,
                            kind: matcher.kind.clone(),
                            value: value.to_string(),
                            via: capture_via.clone(),
                        });
                    }
                    _ => {}
                }
                order = order.saturating_add(1);
            }
            if !pins_inlet {
                continue;
            }
            if gfw::gfw_group(&capture_via).is_some() {
                if !gfw_needed.iter().any(|existing| existing == &capture_via) {
                    gfw_needed.push(capture_via);
                }
            } else if let Some(name) = pin_group(&capture_via, strategy) {
                if !needed.iter().any(|existing| existing == &name) {
                    needed.push(name);
                }
            }
        }
    }

    let group_ports = allocate_named_ports(
        &needed,
        session,
        socks_port,
        controller,
        strategy.mixed_port,
    )?;
    let gfw_ports = allocate_named_ports(
        &gfw_needed,
        session,
        socks_port,
        controller,
        strategy.mixed_port,
    )?;

    let face = CaptureFace {
        socks_port,
        process_rules: process_rules.clone(),
        dest_rules: dest_rules.clone(),
        gfw_domains: Vec::new(),
        group_ports: group_ports.clone(),
        gfw_ports: gfw_ports.clone(),
    };
    if session.last_face.as_ref() != Some(&face) {
        session.revision = session.revision.saturating_add(1);
        session.last_face = Some(face);
    }

    Ok(EnableRequest {
        revision: session.revision,
        operation_revision: 0,
        socks_port,
        username: session.username.clone(),
        password: session.password.clone(),
        process_rules,
        dest_rules,
        gfw_domains: Vec::new(),
        group_ports,
        gfw_ports,
    })
}

fn allocate_named_ports(
    names: &[String],
    session: &mut Session,
    socks_port: u16,
    controller: u16,
    mixed: u16,
) -> Result<Vec<GroupPort>> {
    let mut ports = Vec::new();
    for name in names {
        let port = if let Some(port) = session.group_ports.get(name).copied() {
            port
        } else {
            let port = next_listener_port(
                session.next_port,
                socks_port,
                controller,
                mixed,
                &session.group_ports,
            )?;
            session.group_ports.insert(name.clone(), port);
            session.next_port = port.checked_add(1).unwrap_or(1024);
            port
        };
        ports.push(GroupPort {
            name: name.clone(),
            port,
        });
    }
    Ok(ports)
}

fn push_app_patterns(process_rules: &mut Vec<ProcessRule>, order: u64, value: &str, via: &str) {
    let pattern = value.trim();
    if pattern.is_empty() {
        return;
    }
    if !process_rules
        .iter()
        .any(|rule| rule.pattern == pattern && rule.via == via)
    {
        process_rules.push(ProcessRule {
            order,
            pattern: pattern.to_string(),
            via: via.to_string(),
        });
    }
    if pattern.contains('*') || pattern.contains('?') {
        return;
    }
    let wildcard = format!("{pattern}*");
    if !process_rules
        .iter()
        .any(|rule| rule.pattern == wildcard && rule.via == via)
    {
        process_rules.push(ProcessRule {
            order,
            pattern: wildcard,
            via: via.to_string(),
        });
    }
}

fn pin_group(via: &str, strategy: &Strategy) -> Option<String> {
    let target = compile::via_target(via, strategy);
    if matches!(target.as_str(), "DIRECT" | "REJECT") {
        None
    } else {
        Some(target)
    }
}

fn next_listener_port(
    start: u16,
    socks_port: u16,
    controller: u16,
    mixed: u16,
    used: &BTreeMap<String, u16>,
) -> Result<u16> {
    let mut port = start.max(1024);
    for _ in 1024..=u16::MAX {
        if ![socks_port, controller, mixed, compile::DNS_LISTEN_PORT].contains(&port)
            && !used.values().any(|used| *used == port)
        {
            return Ok(port);
        }
        port = port.checked_add(1).unwrap_or(1024);
    }
    bail!("没有可用的系统接管内部监听端口")
}

pub fn enable_async(strategy: &Strategy) -> Result<()> {
    enable_request_async(&try_inbound_plan(strategy)?)
}

/// Reuse the exact candidate/restored listener credentials and capture plan.
/// Only the operation identity changes; a new process must never regenerate
/// credentials for SOCKS listeners that are already running from saved YAML.
pub fn enable_request_async(request: &EnableRequest) -> Result<()> {
    validate_request(request)?;
    #[cfg(not(target_os = "macos"))]
    bail!("System Extension is macOS-only");
    #[cfg(target_os = "macos")]
    {
        if !login_item::is_bundled() {
            bail!("系统接管需要包含 Network Extension 的已签名 .app");
        }
        let mut request = request.clone();
        request.operation_revision = OPERATION_REVISION.fetch_add(1, Ordering::SeqCst) + 1;
        let json = serde_json::to_string(&request)?;
        let mut error = std::ptr::null_mut();
        let rc = unsafe { ffi::myproxy_ne_enable(c_string(&json).as_ptr(), &mut error) };
        let message = take_error(error);
        if rc != 1 {
            bail!(
                "{}",
                message.unwrap_or_else(|| "系统接管请求提交失败".into())
            );
        }
        log::info(
            "ne",
            format!("submitted capture revision={}", request.revision),
        );
        Ok(())
    }
}

pub fn disable_async() -> Result<()> {
    let revision = OPERATION_REVISION.fetch_add(1, Ordering::SeqCst) + 1;
    #[cfg(not(target_os = "macos"))]
    {
        let _ = revision;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        if !login_item::is_bundled() {
            return Ok(());
        }
        let mut error = std::ptr::null_mut();
        let rc = unsafe { ffi::myproxy_ne_disable(revision, &mut error) };
        let message = take_error(error);
        if rc == 1 {
            log::info("ne", "submitted capture stop");
            Ok(())
        } else {
            bail!(
                "{}",
                message.unwrap_or_else(|| "关闭系统接管请求提交失败".into())
            )
        }
    }
}

#[cfg(target_os = "macos")]
fn c_string(value: &str) -> std::ffi::CString {
    std::ffi::CString::new(value.replace('\0', "")).expect("c string")
}

#[cfg(target_os = "macos")]
fn take_error(ptr: *mut std::os::raw::c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let message = unsafe { std::ffi::CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned();
    unsafe { ffi::myproxy_ne_free_string(ptr) };
    Some(message)
}

#[cfg(target_os = "macos")]
mod ffi {
    use std::os::raw::c_char;

    unsafe extern "C" {
        pub fn myproxy_ne_validate(json: *const c_char, error_out: *mut *mut c_char) -> i32;
        pub fn myproxy_ne_enable(json: *const c_char, error_out: *mut *mut c_char) -> i32;
        pub fn myproxy_ne_disable(operation_revision: u64, error_out: *mut *mut c_char) -> i32;
        pub fn myproxy_ne_status() -> *mut c_char;
        pub fn myproxy_ne_wait(milliseconds: u32);
        pub fn myproxy_ne_free_string(value: *mut c_char);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::{InboundMode, RoutingProfile, Strategy};

    #[test]
    fn rule_mode_pins_app_matchers() {
        let mut strategy = Strategy::default();
        strategy.system_extension = true;
        strategy.extension_mode = InboundMode::Rule;
        let plan = inbound_plan(&strategy);
        assert!(
            !plan.process_rules.is_empty(),
            "rule mode should keep process pins"
        );
        assert!(
            !plan.dest_rules.is_empty(),
            "rule mode should keep user domain pins"
        );
        assert!(
            !plan.group_ports.is_empty(),
            "rule mode should keep group SOCKS"
        );
        assert!(plan.gfw_domains.is_empty());
        assert!(plan.gfw_ports.is_empty());
    }

    #[test]
    fn gfw_via_requests_group_and_keeps_prefix() {
        let mut strategy = Strategy::default();
        strategy.system_extension = true;
        strategy.extension_mode = InboundMode::Rule;
        if let Some(set) = strategy.rule_sets.first_mut() {
            set.via = "gfw:Default".into();
            set.matchers = vec![crate::strategy::Matcher {
                kind: "app".into(),
                value: "Chrome".into(),
            }];
        }
        let plan = inbound_plan(&strategy);
        assert_eq!(plan.process_rules[0].via, "gfw:PROXY");
        assert!(
            plan.gfw_ports.iter().any(|port| port.name == "gfw:PROXY"),
            "process gfw via should allocate a GFW inlet: {:?}",
            plan.gfw_ports
        );
        assert!(
            plan.group_ports.is_empty(),
            "process gfw via must not pin the unwrapped group SOCKS: {:?}",
            plan.group_ports
        );
        assert!(plan.gfw_domains.is_empty());
    }

    #[test]
    fn dest_gfw_keeps_user_host_without_list() {
        let mut strategy = Strategy::default();
        strategy.system_extension = true;
        strategy.extension_mode = InboundMode::Rule;
        strategy.rule_sets = vec![crate::strategy::RuleSet {
            id: "safari".into(),
            name: "Safari".into(),
            via: "gfw:Default".into(),
            matchers: vec![crate::strategy::Matcher {
                kind: "suffix".into(),
                value: "example.com".into(),
            }],
        }];
        let plan = inbound_plan(&strategy);
        assert_eq!(
            plan.dest_rules
                .iter()
                .find(|rule| rule.value == "example.com")
                .map(|rule| rule.via.as_str()),
            Some("gfw:PROXY")
        );
        assert!(
            plan.gfw_ports.iter().any(|port| port.name == "gfw:PROXY"),
            "{:?}",
            plan.gfw_ports
        );
        assert!(plan.gfw_domains.is_empty());
    }

    #[test]
    fn rule_mode_resolves_via_and_expands_app_helpers() {
        let mut strategy = Strategy::default();
        strategy.system_extension = true;
        strategy.extension_mode = InboundMode::Rule;
        strategy.groups = vec![
            crate::strategy::Group::all_nodes("Default".into(), "select".into()),
            crate::strategy::Group::all_nodes("AI Proxy".into(), "select".into()),
        ];
        if let Some(set) = strategy.rule_sets.first_mut() {
            set.via = "default".into();
            set.matchers = vec![crate::strategy::Matcher {
                kind: "app".into(),
                value: "T3 Code (Nightly)".into(),
            }];
        }
        strategy.rule_sets.push(crate::strategy::RuleSet {
            id: "cpa".into(),
            name: "CPA".into(),
            via: "AI Proxy".into(),
            matchers: vec![crate::strategy::Matcher {
                kind: "suffix".into(),
                value: "cpa.leaper.one".into(),
            }],
        });
        let plan = inbound_plan(&strategy);
        assert!(
            plan.process_rules
                .iter()
                .any(|rule| rule.pattern == "T3 Code (Nightly)" && rule.via == "Default"),
            "resolved via should match the stored group name: {:?}",
            plan.process_rules
        );
        assert!(
            plan.process_rules
                .iter()
                .any(|rule| rule.pattern == "T3 Code (Nightly)*" && rule.via == "Default"),
            "exact app pins should also capture Electron helpers: {:?}",
            plan.process_rules
        );
        assert_eq!(
            plan.dest_rules
                .iter()
                .find(|rule| rule.value == "cpa.leaper.one")
                .map(|rule| rule.via.as_str()),
            Some("AI Proxy")
        );
        assert!(
            plan.group_ports.iter().any(|port| port.name == "Default"),
            "{:?}",
            plan.group_ports
        );
        assert!(
            plan.group_ports.iter().any(|port| port.name == "AI Proxy"),
            "user dest pins should allocate a group SOCKS: {:?}",
            plan.group_ports
        );
    }

    #[test]
    fn gfwlist_profile_keeps_user_dest_without_list() {
        let mut strategy = Strategy::default();
        strategy.system_extension = true;
        strategy.extension_mode = InboundMode::Rule;
        strategy.routing_profile = RoutingProfile::Gfwlist;
        let plan = inbound_plan(&strategy);
        assert!(
            !plan.dest_rules.is_empty(),
            "user dest pins stay in the capture plan"
        );
        assert!(plan.gfw_domains.is_empty());
    }

    #[test]
    fn non_rule_mode_clears_process_pins() {
        let mut strategy = Strategy::default();
        strategy.system_extension = true;
        strategy.extension_mode = InboundMode::Global;
        let plan = inbound_plan(&strategy);
        assert!(plan.process_rules.is_empty());
        assert!(plan.dest_rules.is_empty());
        assert!(plan.group_ports.is_empty());
        assert!(plan.gfw_ports.is_empty());
    }
}
