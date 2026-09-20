pub mod import;
pub mod geo;
mod capture;
pub mod nodes;
pub mod policy;
pub mod relay;
pub mod udp;

use std::collections::HashMap;
use std::fs;
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::strategy::{Group, InboundMode, RoutingProfile, Strategy, GLOBAL_GROUP};
use crate::supervisor::RuntimeIdentity;
use crate::{
    catalog::{self, Catalog},
    controller, paths,
};
use anyhow::{bail, Context, Result};
use policy::{NodeHealth, Route};
use relay::{Dialed, Dialer, MixedServer};
use serde::{Deserialize, Serialize};
use serde_json::json;

static OPERATION: Mutex<()> = Mutex::new(());
static GENERATION: AtomicU64 = AtomicU64::new(0);
static SERVICE: OnceLock<Mutex<Service>> = OnceLock::new();
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);
const PROBE_URL: &str = "https://www.gstatic.com/generate_204";

#[derive(Default)]
struct Service {
    runtime: Option<Arc<Runtime>>,
    entrance: Option<Arc<MixedServer>>,
    port: Option<u16>,
    capture: Option<Arc<capture::CaptureService>>,
    retired_capture: Vec<Arc<capture::CaptureService>>,
}

#[derive(Clone, PartialEq, Eq)]
enum Ingress {
    Mixed,
    Capture,
    Group(String),
}

struct Lane {
    address: SocketAddr,
    username: String,
    password: String,
}

struct Runtime {
    strategy: RwLock<Strategy>,
    catalog: Catalog,
    source_nodes: Vec<crate::catalog::Node>,
    health: RwLock<HashMap<String, NodeHealth>>,
    lanes: HashMap<String, Lane>,
    child: Mutex<Child>,
    stopped: AtomicBool,
    generation: AtomicU64,
    warnings: Vec<String>,
    directory: PathBuf,
    probe_lock: Mutex<()>,
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        let child = self
            .child
            .get_mut()
            .unwrap_or_else(|error| error.into_inner());
        let _ = child.kill();
        let _ = child.wait();
        // Only this session's private candidate directory is removed.
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct XrayStatus {
    pub wanted: bool,
    pub running: bool,
    pub ready: bool,
    pub generation: Option<u64>,
    pub mixed_port: Option<u16>,
    pub current: String,
    pub warnings: Vec<String>,
    pub note: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SavedRuntime {
    strategy: Strategy,
    generation: u64,
    core_pid: u32,
}

fn service() -> &'static Mutex<Service> {
    SERVICE.get_or_init(Mutex::default)
}
fn active() -> Result<Arc<Runtime>> {
    service()
        .lock()
        .expect("Xray service")
        .runtime
        .clone()
        .context("尚未连接 Xray")
}

pub fn default_strategy() -> Strategy {
    let mut strategy = Strategy::default();
    strategy.mixed_port = 40808;
    strategy.mixed_mode = InboundMode::Global;
    strategy.global_selected = "节点选择".into();
    strategy.routing_profile = RoutingProfile::Group;
    strategy.unmatched_via = "节点选择".into();
    strategy.rule_sets.clear();
    strategy.groups = vec![Group::all_nodes("节点选择".into(), "select".into())];
    for (name, patterns) in [
        ("美国优先", vec!["美国", "US", "United States", "🇺🇸"]),
        ("日本优先", vec!["日本", "JP", "Japan", "🇯🇵"]),
        ("香港优先", vec!["香港", "HK", "Hong Kong", "🇭🇰"]),
    ] {
        strategy.groups.push(Group::matching(
            name.into(),
            "fallback".into(),
            vec![],
            patterns.into_iter().map(str::to_owned).collect(),
        ));
    }
    strategy
}

pub fn apply(strategy: &Strategy, refresh: bool) -> Result<Catalog> {
    let _operation = OPERATION.lock().expect("Xray operation");
    if SHUTTING_DOWN.load(Ordering::Acquire) { bail!("应用正在退出"); }
    validate_intent(strategy)?;
    let catalog = current_catalog(strategy, refresh)?;
    if wanted() {
        let capture = prepare_capture(strategy)?;
        activate(strategy, &catalog)?;
        sync_capture(strategy, capture)?;
    } else {
        let candidate = prepare(strategy, &catalog)?;
        save_applied(&candidate)?;
        // A disconnected apply validates a candidate but leaves no process running.
        drop(candidate);
    }
    Ok(catalog)
}

pub fn connect(strategy: &Strategy) -> Result<()> {
    let _operation = OPERATION.lock().expect("Xray operation");
    if SHUTTING_DOWN.load(Ordering::Acquire) { bail!("应用正在退出"); }
    validate_intent(strategy)?;
    let catalog = current_catalog(strategy, false)?;
    let capture = prepare_capture(strategy)?;
    activate(strategy, &catalog)?;
    sync_capture(strategy, capture)
}

fn current_catalog(strategy: &Strategy, refresh: bool) -> Result<Catalog> {
    if !refresh {
        if let Ok(catalog) = Catalog::load() {
            if catalog.matches_strategy(strategy) {
                return Ok(catalog);
            }
        }
    }
    catalog::refresh(strategy)
}

fn validate_intent(strategy: &Strategy) -> Result<()> {
    strategy.validate()?;
    geo::validate_rules(strategy)?;
    if strategy.tun {
        bail!("Xray 通道使用系统接管，请关闭 TUN 后再连接。");
    }
    if strategy.mixed_mode == InboundMode::Rule {
        if matches!(
            strategy.routing_profile,
            RoutingProfile::Chinadirect | RoutingProfile::Gfwlist
        ) || strategy
            .rule_sets
            .iter()
            .any(|set| crate::gfw::gfw_group(&set.via).is_some())
        {
            bail!("请将地区和域名规则集添加到规则列表，并选择未匹配走向。");
        }
    }
    Ok(())
}

fn release_capture() -> Result<()> {
    if cfg!(test) { return Ok(()); }
    crate::network_extension::disable_async()?;
    crate::network_extension::wait_disabled(Duration::from_secs(30))
        .context("系统接管或 DNS 尚未关闭，保留代理入口以维持网络")
}

fn prepare_capture(strategy: &Strategy) -> Result<Option<Arc<capture::CaptureService>>> {
    if strategy.system_extension {
        let previous = service().lock().expect("Xray service").capture.clone();
        Ok(Some(Arc::new(capture::CaptureService::prepare(strategy, previous.as_deref())?)))
    } else {
        let has_capture = service().lock().expect("Xray service").capture.is_some();
        if has_capture { release_capture()?; }
        Ok(None)
    }
}

fn sync_capture(strategy: &Strategy, candidate: Option<Arc<capture::CaptureService>>) -> Result<()> {
    if let Some(candidate) = candidate {
        let request = candidate.request.clone();
        {
            let mut state = service().lock().expect("Xray service");
            if let Some(previous) = state.capture.replace(candidate) {
                state.retired_capture.push(previous);
            }
        }
        crate::network_extension::enable_request_async(&request)?;
    } else {
        let mut state = service().lock().expect("Xray service");
        state.capture = None;
        state.retired_capture.clear();
    }
    crate::system_proxy::sync(strategy.system_proxy, strategy.mixed_port)?;
    Ok(())
}

fn activate(strategy: &Strategy, catalog: &Catalog) -> Result<()> {
    if let Ok(runtime) = active() {
        let previous = runtime.strategy.read().expect("strategy").clone();
        if runtime.core_alive() && previous.mixed_port == strategy.mixed_port
            && serde_json::to_value(&runtime.source_nodes)? == serde_json::to_value(&catalog.nodes)? {
            strategy.validate_catalog(catalog)?;
            if previous != *strategy {
                let generation = next_generation();
                save_snapshot(&runtime, strategy, generation)?;
                *runtime.strategy.write().expect("strategy") = strategy.clone();
                runtime.generation.store(generation, Ordering::Release);
                close_all()?;
            }
            return Ok(());
        }
    }
    let candidate = prepare(strategy, catalog)?;
    let same_port = service().lock().expect("Xray service").port == Some(strategy.mixed_port);
    let entrance = if same_port {
        None
    } else {
        let listener =
            TcpListener::bind(("127.0.0.1", strategy.mixed_port)).with_context(|| {
                format!(
                    "混合端口 {} 已被占用，现有代理保持运行",
                    strategy.mixed_port
                )
            })?;
        let dialer: Dialer = Arc::new(|host, port| active()?.dial(host, port));
        Some(Arc::new(MixedServer::start(listener, dialer)?))
    };
    save_applied(&candidate)?;
    let (previous, previous_entrance) = {
        let mut state = service().lock().expect("Xray service");
        let old = state.runtime.replace(candidate.clone());
        let old_entrance = entrance.and_then(|entrance| state.entrance.replace(entrance));
        state.port = Some(strategy.mixed_port);
        (old, old_entrance)
    };
    if let Some(previous) = previous {
        close_all()?;
        previous.stop()?;
    }
    drop(previous_entrance);
    if !cfg!(test) {
        start_probes(&candidate);
    }
    Ok(())
}

fn prepare(strategy: &Strategy, catalog: &Catalog) -> Result<Arc<Runtime>> {
    strategy.validate_catalog(catalog)?;
    let directory = paths::data_dir()?.join(format!("xray-session-{}", uuid::Uuid::new_v4()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(&directory)?;
    }
    #[cfg(not(unix))]
    fs::create_dir(&directory)?;
    let result = prepare_in(strategy, catalog, directory.clone());
    if result.is_err() {
        let _ = fs::remove_dir_all(&directory);
    }
    result
}

fn prepare_in(strategy: &Strategy, catalog: &Catalog, directory: PathBuf) -> Result<Arc<Runtime>> {
    let mut accepted = catalog.clone();
    accepted.nodes.clear();
    let mut inbounds = Vec::new();
    let mut outbounds = vec![json!({"tag":"reject", "protocol":"blackhole"})];
    let mut rules = Vec::new();
    let mut lanes = HashMap::new();
    let mut warnings = Vec::new();
    let mut reserved_ports = Vec::new();
    let binary = xray_binary()?;
    for (index, node) in catalog.nodes.iter().enumerate() {
        let tag = format!("node-{index}");
        let outbound = match nodes::render(node, &tag) {
            Ok(outbound) => outbound,
            Err(error) => {
                warnings.push(format!("{}：{error}", node.name));
                continue;
            }
        };
        let check = directory.join("node-check.json");
        paths::atomic_write(
            &check,
            &serde_json::to_vec(&json!({"outbounds":[outbound.clone()]}))?,
        )?;
        let validation = Command::new(&binary)
            .args(["run", "-test", "-config"])
            .arg(&check)
            .output()?;
        if !validation.status.success() {
            warnings.push(format!("{}：当前内核不接受此节点配置", node.name));
            continue;
        }
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let address = listener.local_addr()?;
        let username = uuid::Uuid::new_v4().simple().to_string();
        let password = uuid::Uuid::new_v4().simple().to_string();
        let inbound_tag = format!("private-{index}");
        inbounds.push(json!({"tag":inbound_tag,"listen":"127.0.0.1","port":address.port(),"protocol":"socks",
            "settings":{"auth":"password","accounts":[{"user":username,"pass":password}],"udp":true}}));
        rules.push(json!({"inboundTag":[inbound_tag],"outboundTag":tag}));
        outbounds.push(outbound);
        lanes.insert(
            node.name.clone(),
            Lane {
                address,
                username,
                password,
            },
        );
        accepted.nodes.push(node.clone());
        reserved_ports.push(listener);
    }
    // The only routing in Xray binds each private entrance to a single outbound.
    // Public routing and group selection are performed before reaching Xray.
    let config = json!({"log":{"loglevel":"warning"},"inbounds":inbounds,"outbounds":outbounds,
        "routing":{"domainStrategy":"AsIs","rules":rules}});
    let path = directory.join("core.json");
    paths::atomic_write(&path, &serde_json::to_vec(&config)?)?;
    let binary = xray_binary()?;
    let validation = Command::new(&binary)
        .args(["run", "-test", "-config"])
        .arg(&path)
        .output()?;
    if !validation.status.success() {
        // Core diagnostics may contain credentials. Keep them in the private session file.
        paths::atomic_write(
            &directory.join("validation.log"),
            &[validation.stdout, validation.stderr].concat(),
        )?;
        bail!("节点配置未通过 Xray 内核验证，现有连接未改变");
    }
    drop(reserved_ports);
    paths::atomic_write(&directory.join("core.log"), b"")?;
    let log = fs::OpenOptions::new()
        .append(true)
        .open(directory.join("core.log"))?;
    let child = Command::new(binary)
        .args(["run", "-config"])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .spawn()?;
    let runtime = Arc::new(Runtime {
        strategy: RwLock::new(strategy.clone()),
        catalog: accepted,
        source_nodes: catalog.nodes.clone(),
        health: RwLock::new(HashMap::new()),
        lanes,
        child: Mutex::new(child),
        stopped: AtomicBool::new(false),
        generation: AtomicU64::new(next_generation()),
        warnings,
        directory,
        probe_lock: Mutex::new(()),
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if !runtime.core_alive() {
            bail!("Xray 在启动时退出，现有配置未改变");
        }
        if runtime.lanes.values().all(|lane| {
            std::net::TcpStream::connect_timeout(&lane.address, Duration::from_millis(100)).is_ok()
        }) {
            break;
        }
        if Instant::now() >= deadline {
            bail!("Xray 私有入口未就绪，现有配置未改变");
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    Ok(runtime)
}

pub fn xray_binary() -> Result<PathBuf> {
    let binary = std::env::var_os("XRAY_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(paths::bundled_xray);
    if !binary.is_file() {
        bail!("找不到 Xray 内核，请重新下载完整的 MyProxy Xray 通道应用");
    }
    Ok(binary)
}

fn dial_direct(host: &str, port: u16) -> Result<std::net::TcpStream> {
    use std::net::ToSocketAddrs;
    for address in (host, port).to_socket_addrs().context("域名解析失败")?.take(4) {
        if let Ok(stream) = std::net::TcpStream::connect_timeout(&address, Duration::from_secs(5)) {
            return Ok(stream);
        }
    }
    bail!("直连失败")
}

impl Runtime {
    fn stop(&self) -> Result<()> {
        self.stopped.store(true, Ordering::Release);
        let mut child = self.child.lock().expect("Xray child");
        if child.try_wait()?.is_none() {
            child.kill().context("stop owned Xray process")?;
            child.wait().context("wait for owned Xray process to exit")?;
        }
        Ok(())
    }

    fn core_alive(&self) -> bool {
        !self.stopped.load(Ordering::Acquire)
            && matches!(self.child.lock().expect("Xray child").try_wait(), Ok(None))
    }
    fn dial(&self, host: &str, port: u16) -> Result<Dialed> {
        self.dial_for(host, port, &Ingress::Mixed)
    }

    fn decision(&self, host: &str, port: u16, network: &str, ingress: &Ingress) -> policy::Decision {
        let strategy = self.strategy.read().expect("strategy");
        let health = self.health.read().expect("health");
        match ingress {
            Ingress::Mixed => policy::decide_network(&strategy, &self.catalog, &health, host, port, None, network),
            Ingress::Capture => policy::decide_capture(&strategy, &self.catalog, &health, host, port, network),
            Ingress::Group(name) => {
                if strategy.extension_mode != InboundMode::Rule {
                    return policy::decide_capture(&strategy, &self.catalog, &health, host, port, network);
                }
                if let Some(rule_id) = name.strip_prefix("@rule:") {
                    policy::decide_captured_rule(&strategy, &self.catalog, &health, rule_id, host, network)
                } else {
                    policy::decide_target(&strategy, &self.catalog, &health, name)
                }
            },
        }
    }

    fn datagram_route(&self, host: &str, port: u16, ingress: &Ingress) -> Result<udp::DatagramRoute> {
        if !self.core_alive() { bail!("Xray 内核已退出"); }
        let decision = self.decision(host, port, "udp", ingress);
        match decision.route {
            Route::Direct => Ok(udp::DatagramRoute::Direct),
            Route::Reject => bail!("UDP 请求被规则拒绝，或所选节点不可用"),
            Route::Node(name) => {
                let lane = self.lanes.get(&name).context("所选 UDP 节点不可用")?;
                Ok(udp::DatagramRoute::Socks { address: lane.address, username: lane.username.clone(), password: lane.password.clone(), label: decision.chain.join(" → "), fallback_direct: decision.allow_direct_fallback })
            }
        }
    }

    fn dial_for(&self, host: &str, port: u16, ingress: &Ingress) -> Result<Dialed> {
        if !self.core_alive() {
            bail!("Xray 内核已退出，请重新连接");
        }
        let mut decision = self.decision(host, port, "tcp", ingress);
        let stream = match decision.route {
            Route::Reject => bail!("规则拒绝连接，或所选组中没有可用节点"),
            Route::Direct => dial_direct(host, port)?,
            Route::Node(name) => {
                let lane = self.lanes.get(&name).context("所选节点不可用")?;
                match relay::dial_socks(lane.address, &lane.username, &lane.password, host, port) {
                    Ok(stream) => stream,
                    Err(error) => {
                        let mut health = self.health.write().expect("health");
                        let item = health.entry(name).or_default();
                        item.failures = item.failures.saturating_add(1);
                        item.delay_ms = None;
                        drop(health);
                        if decision.allow_direct_fallback {
                            decision.chain.push("DIRECT".into());
                            dial_direct(host, port)?
                        } else {
                            return Err(error).context("代理节点连接失败");
                        }
                    }
                }
            }
        };
        Ok(Dialed {
            stream,
            chain: decision.chain.join(" → "),
            rule: decision.rule,
        })
    }
    fn probe(&self) {
        let Ok(_lock) = self.probe_lock.try_lock() else {
            return;
        };
        let lanes: Vec<_> = self.lanes.iter().collect();
        // Eight workers bound background HTTP probes, including large subscriptions.
        std::thread::scope(|scope| {
            for chunk in lanes.chunks(lanes.len().div_ceil(8).max(1)) {
                scope.spawn(move || {
                    for (name, lane) in chunk {
                        if self.stopped.load(Ordering::Acquire) {
                            return;
                        }
                        let credentials = format!("{}:{}", lane.username, lane.password);
                        let start = Instant::now();
                        let output = Command::new("/usr/bin/curl")
                            .args([
                                "-q",
                                "--silent",
                                "--output",
                                "/dev/null",
                                "--write-out",
                                "%{http_code}",
                                "--connect-timeout",
                                "2",
                                "--max-time",
                                "3",
                                "--noproxy",
                                "",
                                "--proxy",
                                &format!("socks5h://{}", lane.address),
                                "--proxy-user",
                                &credentials,
                                PROBE_URL,
                            ])
                            .env_remove("ALL_PROXY")
                            .env_remove("HTTPS_PROXY")
                            .env_remove("HTTP_PROXY")
                            .output();
                        let alive = output
                            .is_ok_and(|result| result.status.success() && result.stdout == b"204");
                        let mut health = self.health.write().expect("health");
                        let item = health.entry((*name).clone()).or_default();
                        if alive {
                            item.delay_ms =
                                Some(start.elapsed().as_millis().min(u32::MAX as u128) as u32);
                            item.failures = 0;
                        } else {
                            item.delay_ms = None;
                            item.failures = item.failures.saturating_add(1);
                        }
                    }
                });
            }
        });
    }
}

fn start_probes(runtime: &Arc<Runtime>) {
    let weak = Arc::downgrade(runtime);
    std::thread::spawn(move || loop {
        {
            let Some(runtime) = weak.upgrade() else {
                return;
            };
            if runtime.stopped.load(Ordering::Acquire) {
                return;
            }
            runtime.probe();
        }
        for _ in 0..30 {
            std::thread::sleep(Duration::from_secs(1));
            if weak
                .upgrade()
                .is_none_or(|runtime| runtime.stopped.load(Ordering::Acquire))
            {
                return;
            }
        }
    });
}

pub fn select_proxy(identity: &RuntimeIdentity, group: &str, name: &str) -> Result<()> {
    let _operation = OPERATION.lock().expect("Xray operation");
    let runtime = active()?;
    if runtime.generation.load(Ordering::Acquire) != identity.generation {
        bail!("运行配置已变化，请刷新后重试");
    }
    let mut strategy = runtime.strategy.read().expect("strategy").clone();
    let candidates = policy::groups(
        &strategy,
        &runtime.catalog,
        &runtime.health.read().expect("health"),
    );
    let live = candidates
        .iter()
        .find(|live| live.name == group)
        .context("节点组不存在")?;
    if !name.is_empty() && !live.members.iter().any(|member| member.name == name) {
        bail!("所选节点不是该组的可用成员");
    }
    let saved = Strategy::load()?;
    let saved_name = if group == GLOBAL_GROUP {
        saved.global_selected.as_str()
    } else {
        saved
            .groups
            .iter()
            .find(|item| item.name == group)
            .context("节点组已被删除")?
            .selected
            .as_str()
    };
    if saved_name != name {
        bail!("保存的节点选择已变化，请重新选择");
    }
    if group == GLOBAL_GROUP {
        strategy.global_selected = name.into();
    } else {
        strategy
            .groups
            .iter_mut()
            .find(|item| item.name == group)
            .context("节点组不存在")?
            .selected = name.into();
    }
    let generation = next_generation();
    save_snapshot(&runtime, &strategy, generation)?;
    *runtime.strategy.write().expect("strategy") = strategy;
    runtime.generation.store(generation, Ordering::Release);
    close_all()?;
    Ok(())
}

pub fn disconnect() -> Result<()> {
    let _operation = OPERATION.lock().expect("Xray operation");
    release_capture()?;
    crate::system_proxy::restore()?;
    let previous = std::mem::take(&mut *service().lock().expect("Xray service"));
    if let Some(runtime) = &previous.runtime {
        runtime.stop()?;
    }
    if let Some(entrance) = &previous.entrance {
        entrance.close_all()?;
    }
    drop(previous);
    Ok(())
}

pub fn shutdown() -> Result<()> {
    SHUTTING_DOWN.store(true, Ordering::Release);
    let result = disconnect();
    if result.is_err() { SHUTTING_DOWN.store(false, Ordering::Release); }
    result
}

pub fn is_running() -> bool {
    active().is_ok_and(|runtime| runtime.core_alive())
}
pub fn wanted() -> bool {
    service().lock().expect("Xray service").runtime.is_some()
}
pub fn sync_wanted_on_launch() -> Result<()> {
    Ok(())
}
pub fn runtime_identity() -> Option<RuntimeIdentity> {
    let runtime = active().ok()?;
    if !runtime.core_alive() {
        return None;
    }
    let port = runtime.strategy.read().ok()?.mixed_port;
    Some(RuntimeIdentity {
        generation: runtime.generation.load(Ordering::Acquire),
        mixed_port: port,
    })
}
pub fn applied_strategy() -> Option<Strategy> {
    let runtime = active().ok()?;
    let strategy = runtime.strategy.read().ok()?.clone();
    Some(strategy)
}
pub fn status() -> Result<XrayStatus> {
    let capture_status = crate::network_extension::status();
    let mut state = service().lock().expect("Xray service");
    if state.capture.as_ref().is_some_and(|capture|
        capture_status.observed
            && capture_status.phase == crate::network_extension::Phase::Running
            && capture_status.dns_phase == crate::network_extension::DnsPhase::Running
            && capture_status.applied_revision == Some(capture.request.revision)) {
        state.retired_capture.clear();
    }
    let Some(runtime) = &state.runtime else {
        return Ok(XrayStatus {
            wanted: false,
            running: false,
            ready: false,
            generation: None,
            mixed_port: None,
            current: String::new(),
            warnings: vec![],
            note: None,
        });
    };
    let running = runtime.core_alive();
    let strategy = runtime.strategy.read().expect("strategy");
    let decision = policy::decide(
        &strategy,
        &runtime.catalog,
        &runtime.health.read().expect("health"),
        "example.invalid",
        443,
        None,
    );
    let current = decision.chain.last().cloned().unwrap_or_default();
    Ok(XrayStatus {
        wanted: true,
        running,
        ready: running && state.entrance.is_some(),
        generation: Some(runtime.generation.load(Ordering::Acquire)),
        mixed_port: Some(strategy.mixed_port),
        current,
        warnings: runtime.warnings.clone(),
        note: (!running).then(|| "Xray 已退出，请重新连接".into()),
    })
}
pub fn groups() -> Result<Vec<controller::LiveGroup>> {
    let runtime = active()?;
    let strategy = runtime.strategy.read().expect("strategy");
    let health = runtime.health.read().expect("health");
    Ok(policy::groups(&strategy, &runtime.catalog, &health))
}
pub fn traffic() -> Result<controller::TrafficSnapshot> {
    let (entrance, capture) = {
        let state = service().lock().expect("Xray service");
        (state.entrance.clone().context("尚未连接")?, state.capture.clone())
    };
    let mut snapshot = entrance.snapshot();
    if let Some(capture) = capture {
        let extra = capture.snapshot();
        snapshot.connection_count += extra.connection_count;
        snapshot.upload_total = snapshot.upload_total.saturating_add(extra.upload_total);
        snapshot.download_total = snapshot.download_total.saturating_add(extra.download_total);
        snapshot.connections.extend(extra.connections);
        snapshot.connections.truncate(controller::UI_CONNECTION_CAP);
    }
    Ok(snapshot)
}
pub fn close_one(id: &str) -> Result<()> {
    if id.starts_with("capture:") {
        let capture = service().lock().expect("Xray service").capture.clone().context("系统接管入口未启动")?;
        return capture.close_one(id);
    }
    let entrance = service()
        .lock()
        .expect("Xray service")
        .entrance
        .clone()
        .context("尚未连接")?;
    entrance.close_one(id)
}
pub fn close_all() -> Result<()> {
    let captures = {
        let state = service().lock().expect("Xray service");
        state.capture.iter().chain(state.retired_capture.iter()).cloned().collect::<Vec<_>>()
    };
    for capture in captures { capture.close_all()?; }
    let entrance = service()
        .lock()
        .expect("Xray service")
        .entrance
        .clone()
        .context("尚未连接")?;
    entrance.close_all()
}
pub fn test_group_delay(group: &str) -> Result<HashMap<String, u32>> {
    let runtime = active()?;
    runtime.probe();
    Ok(groups()?
        .into_iter()
        .find(|item| item.name == group)
        .context("节点组不存在")?
        .members
        .into_iter()
        .filter_map(|member| member.delay.map(|delay| (member.name, delay)))
        .collect())
}
fn save_applied(runtime: &Runtime) -> Result<()> {
    save_snapshot(
        runtime,
        &runtime.strategy.read().expect("strategy"),
        runtime.generation.load(Ordering::Acquire),
    )
}
fn save_snapshot(runtime: &Runtime, strategy: &Strategy, generation: u64) -> Result<()> {
    paths::atomic_write(
        &paths::xray_runtime_state_path()?,
        &serde_json::to_vec(&SavedRuntime {
            strategy: strategy.clone(),
            generation,
            core_pid: runtime.child.lock().expect("child").id(),
        })?,
    )
}
fn next_generation() -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(u64::MAX as u128) as u64;
    GENERATION
        .fetch_max(now, Ordering::AcqRel)
        .max(now)
        .saturating_add(1)
}

#[cfg(test)]
mod tests;
