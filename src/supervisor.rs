use std::collections::BTreeSet;
use std::fs;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::TcpStream;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use anyhow::{bail, Context, Result};

use crate::catalog::{self, Catalog};
use crate::compile;
use crate::controller;
use crate::log;
use crate::paths;
use crate::strategy::{InboundMode, Strategy, GLOBAL_GROUP};

const HEALTH_INTERVAL: Duration = Duration::from_secs(2);
const FAIL_BEFORE_RETRY: u32 = 3;
const RECOVER_INTERVAL: Duration = Duration::from_secs(8);
const MAX_RECOVERIES: u32 = 5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreHealth {
    pub wanted: bool,
    pub ready: bool,
    pub note: Option<String>,
    pub proxy_now: String,
}

impl CoreHealth {
    fn idle() -> Self {
        Self {
            wanted: false,
            ready: false,
            note: None,
            proxy_now: String::new(),
        }
    }

    fn ready(proxy_now: impl Into<String>) -> Self {
        let proxy_now = proxy_now.into();
        Self {
            wanted: true,
            ready: true,
            note: (proxy_now == "REJECT")
                .then(|| "核心已就绪；当前节点组为空，代理出口不可用".into()),
            proxy_now,
        }
    }

    fn failing(note: &str) -> Self {
        Self {
            wanted: true,
            ready: false,
            note: Some(note.into()),
            proxy_now: String::new(),
        }
    }
}

struct HealthWatch {
    last_check: Option<Instant>,
    last_recover: Option<Instant>,
    last: CoreHealth,
    fails: u32,
    recoveries: u32,
}

impl Default for HealthWatch {
    fn default() -> Self {
        Self {
            last_check: None,
            last_recover: None,
            last: CoreHealth::idle(),
            fails: 0,
            recoveries: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeIdentity {
    pub generation: u64,
    pub mixed_port: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum OperationState {
    Idle,
    Connecting,
    Connected,
    Applying,
    Disconnecting,
    Error,
}

impl OperationState {
    pub fn is_busy(self) -> bool {
        matches!(
            self,
            Self::Connecting | Self::Applying | Self::Disconnecting
        )
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct RuntimeConfig {
    strategy: Strategy,
    catalog: Catalog,
    yaml: String,
    #[serde(default)]
    extension_request: Option<crate::network_extension::EnableRequest>,
    generation: u64,
}

impl RuntimeConfig {
    fn load() -> Result<Option<Self>> {
        let path = paths::runtime_state_path()?;
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .context("read last applied runtime")
                .map(Some),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err).context("read last applied runtime"),
        }
    }

    fn validate_extension_snapshot(&self) -> Result<()> {
        if self.strategy.system_extension && self.extension_request.is_none() {
            bail!("运行快照缺少系统接管凭据，请先断开再连接以建立完整快照");
        }
        Ok(())
    }

    fn save(&self) -> Result<()> {
        paths::atomic_write(&paths::runtime_state_path()?, &serde_json::to_vec(self)?)
    }

    fn identity(&self) -> RuntimeIdentity {
        RuntimeIdentity {
            generation: self.generation,
            mixed_port: self.strategy.mixed_port,
        }
    }
}

pub struct Supervisor {
    child: Mutex<Option<Child>>,
    running_tun: Mutex<bool>,
    running_se: Mutex<bool>,
    running_mixed_port: Mutex<Option<u16>>,
    update_proxy_hook: Mutex<Option<fn(Option<u16>)>>,
    health: Mutex<HealthWatch>,
}

// The shutdown flag remains set so queued operations cannot restart the core.
static OPERATION: Mutex<bool> = Mutex::new(false);
static OPERATION_STATE: AtomicU8 = AtomicU8::new(OperationState::Idle as u8);
const OPERATION_LOCK_TIMEOUT: Duration = Duration::from_secs(5);

struct OperationGuard {
    _state: std::sync::MutexGuard<'static, bool>,
    file: File,
}

impl OperationGuard {
    fn show(&mut self, state: OperationState) -> Result<()> {
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&serde_json::to_vec(&state)?)?;
        OPERATION_STATE.store(state as u8, Ordering::Release);
        Ok(())
    }
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
        if OPERATION_STATE.load(Ordering::Acquire) != OperationState::Error as u8 {
            OPERATION_STATE.store(OperationState::Idle as u8, Ordering::Release);
        }
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

fn acquire_operation() -> Result<OperationGuard> {
    acquire_operation_with_timeout(OPERATION_LOCK_TIMEOUT)
}

fn acquire_operation_with_timeout(timeout: Duration) -> Result<OperationGuard> {
    let deadline = Instant::now() + timeout;
    let state = loop {
        match OPERATION.try_lock() {
            Ok(state) => break state,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => bail!("另一个核心操作正在进行中，请稍后重试"),
        }
    };
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(paths::operation_lock_path()?)
        .context("open operation lock")?;
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(OperationGuard {
                _state: state,
                file,
            });
        }
        if Instant::now() >= deadline {
            bail!("另一个核心操作正在进行中，请稍后重试");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn operation_in_progress() -> Option<OperationState> {
    let local = OPERATION_STATE.load(Ordering::Acquire);
    for state in [
        OperationState::Connecting,
        OperationState::Applying,
        OperationState::Disconnecting,
    ] {
        if local == state as u8 {
            return Some(state);
        }
    }
    // flock is also held by myproxyctl. Reading its small status file lets the
    // GUI disable controls without introducing a second command queue.
    let mut file = OpenOptions::new()
        .read(true)
        .open(paths::operation_lock_path().ok()?)
        .ok()?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } == 0 {
        unsafe {
            libc::flock(file.as_raw_fd(), libc::LOCK_UN);
        }
        return None;
    }
    let mut bytes = Vec::new();
    let _ = file.read_to_end(&mut bytes);
    Some(serde_json::from_slice(&bytes).unwrap_or(OperationState::Applying))
}

impl Default for Supervisor {
    fn default() -> Self {
        Self {
            child: Mutex::new(None),
            running_tun: Mutex::new(false),
            running_se: Mutex::new(false),
            running_mixed_port: Mutex::new(None),
            update_proxy_hook: Mutex::new(None),
            health: Mutex::new(HealthWatch::default()),
        }
    }
}

impl Supervisor {
    pub fn shared() -> Arc<Self> {
        static INSTANCE: OnceLock<Arc<Supervisor>> = OnceLock::new();
        INSTANCE.get_or_init(|| Arc::new(Self::default())).clone()
    }

    pub fn set_update_proxy_hook(&self, hook: fn(Option<u16>)) {
        *self.update_proxy_hook.lock().expect("update proxy hook") = Some(hook);
    }

    pub fn update_download_port(&self) -> Option<u16> {
        if !self.is_running() {
            return None;
        }
        RuntimeConfig::load()
            .ok()
            .flatten()
            .map(|runtime| runtime.strategy.mixed_port)
            .or(*self.running_mixed_port.lock().expect("supervisor lock"))
    }

    fn remember_mixed_port(&self, port: Option<u16>) {
        *self.running_mixed_port.lock().expect("supervisor lock") = port;
        if let Some(hook) = *self.update_proxy_hook.lock().expect("update proxy hook") {
            hook(port);
        }
    }

    pub fn runtime_identity(&self) -> Option<RuntimeIdentity> {
        if self.is_busy() || !self.is_running() {
            return None;
        }
        let runtime = RuntimeConfig::load().ok().flatten()?;
        if !pid_file_alive(Some(runtime.strategy.mixed_port)) {
            return None;
        }
        Some(runtime.identity())
    }

    pub fn applied_strategy(&self) -> Option<Strategy> {
        RuntimeConfig::load()
            .ok()
            .flatten()
            .map(|runtime| runtime.strategy)
    }

    pub fn operation_state(&self) -> OperationState {
        if let Some(state) = operation_in_progress() {
            return state;
        }
        if OPERATION_STATE.load(Ordering::Acquire) == OperationState::Error as u8 {
            return OperationState::Error;
        }
        if self.is_running() {
            OperationState::Connected
        } else {
            OperationState::Idle
        }
    }

    pub fn is_busy(&self) -> bool {
        operation_in_progress().is_some()
    }

    pub fn adopt_running(&self, _tun: bool, _system_extension: bool, _mixed_port: u16) {
        let Ok(Some(runtime)) = RuntimeConfig::load() else {
            return;
        };
        if pid_file_alive(Some(runtime.strategy.mixed_port)) {
            *self.running_tun.lock().expect("supervisor lock") = runtime.strategy.tun;
            *self.running_se.lock().expect("supervisor lock") = runtime.strategy.system_extension;
            self.remember_mixed_port(Some(runtime.strategy.mixed_port));
        }
    }

    pub fn sync_wanted_on_launch(&self) {
        // A disable request may be waiting for another process's operation.
        // Never overwrite that request while adopting its still-running core.
        if !self.is_running() && !self.is_busy() {
            let _ = set_wanted(false);
            self.reset_health();
        }
    }

    pub fn wanted(&self) -> bool {
        is_wanted()
    }

    pub fn last_health(&self) -> CoreHealth {
        if !is_wanted() {
            return CoreHealth::idle();
        }
        let mut health = self
            .health
            .lock()
            .expect("supervisor health lock")
            .last
            .clone();
        health.wanted = true;
        health
    }

    pub fn connect(&self, strategy: &Strategy) -> Result<()> {
        let cancellation = cancellation_revision()?;
        let mut operation = acquire_operation()?;
        if *operation._state {
            bail!("application is shutting down");
        }
        operation.show(OperationState::Connecting)?;
        let result = (|| {
            strategy.validate()?;
            let candidate = self.prepare(strategy, false)?;
            if cancellation_revision()? != cancellation {
                bail!("连接操作已取消");
            }
            set_wanted(true)?;
            if cancellation_revision()? != cancellation {
                set_wanted(false)?;
                bail!("连接操作已取消");
            }
            self.reset_health();
            self.activate(candidate, true)
        })();
        let result = self.finish_cancelled(result);
        self.record_result(&result);
        result
    }

    pub fn apply(&self, strategy: &Strategy) -> Result<Catalog> {
        self.apply_inner(strategy, true)
    }
    pub fn apply_cached(&self, strategy: &Strategy) -> Result<Catalog> {
        self.apply_inner(strategy, false)
    }

    fn apply_inner(&self, strategy: &Strategy, refresh: bool) -> Result<Catalog> {
        let mut operation = acquire_operation()?;
        if *operation._state {
            bail!("application is shutting down");
        }
        operation.show(OperationState::Applying)?;
        let result = (|| {
            strategy.validate()?;
            let candidate = self.prepare(strategy, refresh)?;
            let catalog = candidate.catalog.clone();
            self.activate(candidate, is_wanted())?;
            Ok(catalog)
        })();
        let result = self.finish_cancelled(result);
        self.record_result(&result);
        result
    }

    fn prepare(&self, strategy: &Strategy, refresh: bool) -> Result<RuntimeConfig> {
        let catalog = if refresh {
            catalog::refresh(strategy)?
        } else {
            match Catalog::load() {
                Ok(cached) if cached.matches_strategy(strategy) => cached,
                _ => catalog::refresh(strategy)?,
            }
        };
        if refresh
            && strategy.system_extension
            && strategy
                .rule_sets
                .iter()
                .any(|set| crate::gfw::gfw_group(&set.via).is_some())
        {
            crate::gfw::refresh_domains()?;
        }
        let extension_request = if strategy.system_extension {
            let request = crate::network_extension::try_inbound_plan(strategy)?;
            crate::network_extension::prepare_request(&request)?;
            Some(request)
        } else {
            None
        };
        let yaml =
            compile::compile_with_inbound_plan(strategy, &catalog, extension_request.as_ref())?;
        let path = paths::candidate_yaml_path()?;
        paths::atomic_write(&path, yaml.as_bytes())?;
        let validation = validate_runtime_config(&path);
        let _ = fs::remove_file(&path);
        validation?;
        if strategy.tun {
            ensure_tun_privileges(&paths::bundled_mihomo())?;
        }
        Ok(RuntimeConfig {
            strategy: strategy.clone(),
            catalog,
            yaml,
            extension_request,
            generation: next_generation(0),
        })
    }

    fn activate(&self, mut candidate: RuntimeConfig, should_run: bool) -> Result<()> {
        let previous = RuntimeConfig::load()?;
        let previous_yaml = fs::read(paths::runtime_yaml_path()?).ok();
        let running = self.is_running();
        if running && previous.is_none() {
            bail!("当前核心没有已验证的运行快照；请先断开，再连接，以建立可回退配置");
        }
        if running {
            if let Some(previous) = &previous {
                previous.validate_extension_snapshot()?;
            }
        }
        if should_run && !is_wanted() {
            bail!("连接操作已取消");
        }
        candidate.generation = next_generation(previous.as_ref().map_or(0, |old| old.generation));
        paths::atomic_write(&paths::runtime_yaml_path()?, candidate.yaml.as_bytes())?;
        let result = (|| -> Result<()> {
            let health = if should_run {
                let reconnect = previous.as_ref().map_or(true, |old| {
                    needs_reconnect(
                        running,
                        true,
                        old.strategy.tun,
                        old.strategy.system_extension,
                        Some(old.strategy.mixed_port),
                        &candidate.strategy,
                    )
                });
                if reconnect {
                    self.disconnect_inner(None)?;
                    Some(self.start_runtime(&mut candidate)?)
                } else {
                    controller::reload(candidate.strategy.mixed_port)?;
                    Some(self.finish_runtime(&mut candidate)?)
                }
            } else {
                if running {
                    bail!("断开操作已请求，请等待核心停止后再应用");
                }
                None
            };
            if should_run && !is_wanted() {
                bail!("连接操作已取消");
            }
            candidate.save()?;
            if let Some(health) = health {
                self.mark_ready(health.proxy_now.clone());
                self.store_health(health);
            }
            Ok(())
        })();
        if let Err(error) = result {
            // Restore exactly what was applied, never the newly saved draft or
            // the refresh cache, which may already contain different nodes.
            let rollback = (|| -> Result<()> {
                if let Some(mut previous) = previous {
                    paths::atomic_write(&paths::runtime_yaml_path()?, previous.yaml.as_bytes())?;
                    if running && is_wanted() {
                        self.disconnect_inner(None)?;
                        let health = self.start_runtime(&mut previous)?;
                        previous.generation = next_generation(candidate.generation);
                        previous.save()?;
                        self.mark_ready(health.proxy_now.clone());
                        self.store_health(health);
                    } else if should_run {
                        self.disconnect_inner(None)?;
                    }
                } else {
                    self.disconnect_inner(None)?;
                    match previous_yaml {
                        Some(bytes) => paths::atomic_write(&paths::runtime_yaml_path()?, &bytes)?,
                        None => {
                            let _ = fs::remove_file(paths::runtime_yaml_path()?);
                        }
                    }
                    set_wanted(false)?;
                }
                Ok(())
            })();
            return match rollback {
                Ok(()) => Err(error).context("新配置未应用；已保留上次有效配置"),
                Err(rollback) => {
                    Err(error).context(format!("新配置未应用，回退也失败：{rollback:#}"))
                }
            };
        }
        Ok(())
    }

    fn start_runtime(&self, runtime: &mut RuntimeConfig) -> Result<CoreHealth> {
        runtime.validate_extension_snapshot()?;
        let strategy = &runtime.strategy;
        if !is_wanted() {
            bail!("连接操作已取消");
        }
        let bin = paths::bundled_mihomo();
        let log_file = File::create(paths::mihomo_log_path()?).context("mihomo.log")?;
        let mut child = mihomo_command(&bin)?
            .arg("-f")
            .arg(paths::runtime_yaml_path()?)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log_file.try_clone()?))
            .stderr(Stdio::from(log_file))
            .spawn()
            .context("spawn mihomo")?;
        if let Err(error) =
            paths::atomic_write(&paths::pid_path()?, child.id().to_string().as_bytes())
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        *self.child.lock().expect("supervisor lock") = Some(child);
        *self.running_tun.lock().expect("supervisor lock") = strategy.tun;
        *self.running_se.lock().expect("supervisor lock") = strategy.system_extension;
        self.remember_mixed_port(Some(strategy.mixed_port));
        let deadline = Instant::now() + Duration::from_secs(if strategy.tun { 8 } else { 3 });
        loop {
            if !is_wanted() {
                bail!("连接操作已取消");
            }
            if !self.is_running() {
                bail!("mihomo 在启动时退出，请检查内核日志");
            }
            if mixed_listening(strategy.mixed_port)
                && controller::ready(strategy.mixed_port).is_ok()
            {
                break;
            }
            if Instant::now() >= deadline {
                bail!("Mihomo 监听或控制器未就绪，请检查端口占用及内核日志");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        self.finish_runtime(runtime)
    }

    fn finish_runtime(&self, runtime: &mut RuntimeConfig) -> Result<CoreHealth> {
        runtime.validate_extension_snapshot()?;
        let strategy = &mut runtime.strategy;
        if !is_wanted() {
            bail!("连接操作已取消");
        }
        let notes = controller::restore_selections(strategy.mixed_port, strategy)?;
        let groups = controller::fetch_proxies(strategy.mixed_port)?;
        for saved in strategy
            .groups
            .iter_mut()
            .filter(|group| group.kind == "select")
        {
            if let Some(live) = groups.iter().find(|live| live.name == saved.name) {
                saved.selected = if live.now == "REJECT" {
                    String::new()
                } else {
                    live.now.clone()
                };
            }
        }
        if !strategy.global_selected.is_empty()
            || strategy.mixed_mode == InboundMode::Global
            || (strategy.system_extension && strategy.extension_mode == InboundMode::Global)
        {
            strategy.global_selected = groups
                .iter()
                .find(|live| live.name == GLOBAL_GROUP)
                .context("GLOBAL disappeared after restoring selections")?
                .now
                .clone();
        }
        let now = self.probe_strategy(strategy)?;
        if strategy.system_extension {
            let request = runtime
                .extension_request
                .as_ref()
                .context("missing applied Network Extension request")?;
            crate::network_extension::enable_request_async(request)?;
        }
        if !is_wanted() {
            bail!("连接操作已取消");
        }
        let mut health = CoreHealth::ready(now);
        if !notes.is_empty() {
            health.note = Some(notes.join("；"));
        }
        Ok(health)
    }

    pub fn disconnect(&self) -> Result<()> {
        cancel_pending_connect()?;
        set_wanted(false)?;
        self.reset_health();
        let mut operation = acquire_operation()?;
        operation.show(OperationState::Disconnecting)?;
        let result = self.disconnect_inner(None);
        self.record_result(&result);
        result
    }

    pub fn shutdown(&self) -> Result<()> {
        cancel_pending_connect()?;
        set_wanted(false)?;
        self.reset_health();
        let mut operation = acquire_operation()?;
        operation.show(OperationState::Disconnecting)?;
        *operation._state = true;
        let result = self
            .disconnect_inner(None)
            .and_then(|()| crate::network_extension::wait_disabled(Duration::from_secs(30)));
        if result.is_err() {
            *operation._state = false;
        }
        self.record_result(&result);
        result
    }

    fn disconnect_inner(&self, mixed_port: Option<u16>) -> Result<()> {
        let extension_result = crate::network_extension::disable_async();
        let port = mixed_port
            .or(*self.running_mixed_port.lock().expect("supervisor lock"))
            .or_else(|| {
                RuntimeConfig::load()
                    .ok()
                    .flatten()
                    .map(|runtime| runtime.strategy.mixed_port)
            });
        let child = self.child.lock().expect("supervisor lock").take();
        let child_pid = child.as_ref().map(|child| child.id() as i32);
        if let Some(mut child) = child {
            let _ = child.kill();
            let _ = child.wait();
        }
        let stopped = reclaim_owned_mihomo(port, child_pid);
        if let Ok(path) = paths::pid_path() {
            let _ = fs::remove_file(path);
        }
        *self.running_tun.lock().expect("supervisor lock") = false;
        *self.running_se.lock().expect("supervisor lock") = false;
        self.remember_mixed_port(None);
        if stopped || child_pid.is_some() {
            log::info("supervisor", "disconnect");
        }
        extension_result
    }

    pub fn is_running(&self) -> bool {
        {
            let mut slot = self.child.lock().expect("supervisor lock");
            if let Some(child) = slot.as_mut() {
                if matches!(child.try_wait(), Ok(None)) {
                    return true;
                }
                let pid = child.id();
                *slot = None;
                if let Ok(path) = paths::pid_path() {
                    if fs::read_to_string(&path)
                        .map(|value| value.trim() == pid.to_string())
                        .unwrap_or(false)
                    {
                        let _ = fs::remove_file(path);
                    }
                }
            }
        }
        // A CLI reload/restart can change the real port while this object lives.
        pid_file_alive(None)
    }

    fn current_for_control(&self, identity: RuntimeIdentity) -> Result<RuntimeConfig> {
        if !is_wanted() || !self.is_running() {
            bail!("核心已断开");
        }
        let runtime = RuntimeConfig::load()?.context("没有已应用的核心配置")?;
        if runtime.identity() != identity {
            bail!("核心配置已改变，请刷新后重试");
        }
        Ok(runtime)
    }

    pub fn select_proxy(&self, identity: RuntimeIdentity, group: &str, name: &str) -> Result<()> {
        let mut operation = acquire_operation_with_timeout(Duration::ZERO)?;
        if *operation._state {
            bail!("application is shutting down");
        }
        operation.show(OperationState::Applying)?;
        let mut runtime = self.current_for_control(identity)?;
        if saved_selection(group)? != name {
            bail!("节点选择已被更新，请使用最新选择重试");
        }
        let previous = controller::probe(identity.mixed_port, group)?;
        if let Err(error) = controller::select_proxy(identity.mixed_port, group, name) {
            return match controller::select_proxy(identity.mixed_port, group, &previous) {
                Ok(()) => Err(error).context("节点切换失败，已恢复原选择"),
                Err(rollback) => Err(error).context(format!(
                    "节点切换结果无法确认，恢复原选择也失败：{rollback:#}"
                )),
            };
        }
        if group == GLOBAL_GROUP {
            runtime.strategy.global_selected = name.into();
        } else if let Some(saved) = runtime
            .strategy
            .groups
            .iter_mut()
            .find(|saved| saved.name == group)
        {
            saved.selected = name.into();
        }
        runtime.generation = next_generation(runtime.generation);
        if let Err(error) = runtime.save() {
            return match controller::select_proxy(identity.mixed_port, group, &previous) {
                Ok(()) => Err(error).context("记录运行选择失败，已恢复原节点"),
                Err(rollback) => {
                    Err(error).context(format!("记录运行选择失败且无法恢复原节点：{rollback:#}"))
                }
            };
        }
        if saved_selection(group)? != name {
            bail!("节点切换期间选择被再次修改；最新保存的选择仍待应用");
        }
        Ok(())
    }

    pub fn close_one(&self, identity: RuntimeIdentity, id: &str) -> Result<()> {
        let mut operation = acquire_operation_with_timeout(Duration::ZERO)?;
        if *operation._state {
            bail!("application is shutting down");
        }
        operation.show(OperationState::Applying)?;
        self.current_for_control(identity)?;
        controller::close_one(identity.mixed_port, id)
    }

    pub fn close_all(&self, identity: RuntimeIdentity) -> Result<()> {
        let mut operation = acquire_operation_with_timeout(Duration::ZERO)?;
        if *operation._state {
            bail!("application is shutting down");
        }
        operation.show(OperationState::Applying)?;
        self.current_for_control(identity)?;
        controller::close_all(identity.mixed_port)
    }

    fn probe_strategy(&self, strategy: &Strategy) -> Result<String> {
        if !self.is_running() || !mixed_listening(strategy.mixed_port) {
            bail!("核心或 Mixed 监听不可用");
        }
        let target = match strategy.mixed_mode {
            InboundMode::Direct => "DIRECT",
            InboundMode::Global => GLOBAL_GROUP,
            _ => compile::default_group(strategy),
        };
        controller::probe(strategy.mixed_port, target)
    }

    pub fn observe(&self, _draft: &Strategy) -> CoreHealth {
        if !is_wanted() {
            self.reset_health();
            return CoreHealth::idle();
        }
        if self.is_busy() {
            return self.last_health();
        }
        let runtime = match RuntimeConfig::load() {
            Ok(Some(runtime)) => runtime,
            _ => {
                let health = CoreHealth::failing("没有已应用的运行快照，请断开后重新连接");
                self.store_health(health.clone());
                return health;
            }
        };
        let now = Instant::now();
        {
            let mut watch = self.health.lock().expect("supervisor health lock");
            if watch
                .last_check
                .map(|last| now.duration_since(last) < HEALTH_INTERVAL)
                .unwrap_or(false)
            {
                return watch.last.clone();
            }
            watch.last_check = Some(now);
        }
        let probe = self.probe_strategy(&runtime.strategy);
        if self.is_busy()
            || RuntimeConfig::load()
                .ok()
                .flatten()
                .map(|current| current.generation)
                != Some(runtime.generation)
        {
            return self.last_health();
        }
        match probe {
            Ok(proxy_now) => {
                self.mark_ready(proxy_now.clone());
                CoreHealth::ready(proxy_now)
            }
            Err(error) => self.note_failure(runtime, now, &format!("{error:#}")),
        }
    }

    fn mark_ready(&self, proxy_now: String) {
        let mut watch = self.health.lock().expect("supervisor health lock");
        watch.last_check = Some(Instant::now());
        watch.fails = 0;
        watch.recoveries = 0;
        watch.last = CoreHealth::ready(proxy_now);
    }

    fn reset_health(&self) {
        *self.health.lock().expect("supervisor health lock") = HealthWatch::default();
    }

    fn note_failure(&self, runtime: RuntimeConfig, now: Instant, reason: &str) -> CoreHealth {
        let (fails, recoveries, last_recover) = {
            let mut watch = self.health.lock().expect("supervisor health lock");
            watch.last_check = Some(now);
            watch.fails = watch.fails.saturating_add(1);
            (watch.fails, watch.recoveries, watch.last_recover)
        };
        let note = if recoveries >= MAX_RECOVERIES {
            "已停止自动恢复，请手动连接"
        } else if fails < FAIL_BEFORE_RETRY {
            "正在确认"
        } else if last_recover
            .map(|at| now.duration_since(at) < RECOVER_INTERVAL)
            .unwrap_or(false)
        {
            "等待自动重试"
        } else {
            return self.recover(runtime, now);
        };
        let health = CoreHealth::failing(&format!("{reason}；{note}"));
        self.store_health(health.clone());
        health
    }

    fn recover(&self, mut runtime: RuntimeConfig, now: Instant) -> CoreHealth {
        let Ok(mut operation) = acquire_operation_with_timeout(Duration::ZERO) else {
            return self.last_health();
        };
        if *operation._state || !is_wanted() {
            return CoreHealth::idle();
        }
        // Another process may have completed an apply between probe and lock.
        if RuntimeConfig::load()
            .ok()
            .flatten()
            .map(|current| current.generation)
            != Some(runtime.generation)
        {
            return self.last_health();
        }
        if operation.show(OperationState::Connecting).is_err() {
            return self.last_health();
        }
        {
            let mut watch = self.health.lock().expect("supervisor health lock");
            watch.last_recover = Some(now);
            watch.recoveries = watch.recoveries.saturating_add(1);
            watch.fails = 0;
        }
        let result = (|| -> Result<CoreHealth> {
            runtime.validate_extension_snapshot()?;
            paths::atomic_write(&paths::runtime_yaml_path()?, runtime.yaml.as_bytes())?;
            self.disconnect_inner(None)?;
            let health = self.start_runtime(&mut runtime)?;
            runtime.generation = next_generation(runtime.generation);
            runtime.save()?;
            Ok(health)
        })();
        let result = self.finish_cancelled(result);
        let health = match result {
            Ok(mut health) => {
                self.mark_ready(health.proxy_now.clone());
                health.note = Some(match health.note {
                    Some(note) => format!("已使用上次有效配置恢复核心；{note}"),
                    None => "已使用上次有效配置恢复核心".into(),
                });
                health
            }
            Err(error) => {
                OPERATION_STATE.store(OperationState::Error as u8, Ordering::Release);
                CoreHealth::failing(&format!("核心恢复失败：{error:#}"))
            }
        };
        self.store_health(health.clone());
        health
    }

    fn finish_cancelled<T>(&self, result: Result<T>) -> Result<T> {
        if !is_wanted() {
            if let Err(cleanup) = self.disconnect_inner(None) {
                return match result {
                    Ok(_) => Err(cleanup).context("断开核心失败"),
                    Err(error) => Err(error).context(format!("取消后的清理失败：{cleanup:#}")),
                };
            }
        }
        result
    }

    fn record_result<T>(&self, result: &Result<T>) {
        if let Err(error) = result {
            OPERATION_STATE.store(OperationState::Error as u8, Ordering::Release);
            let mut health = self.last_health();
            health.note = Some(format!("{error:#}"));
            self.store_health(health);
        }
    }

    fn store_health(&self, health: CoreHealth) {
        self.health.lock().expect("supervisor health lock").last = health;
    }
}

fn next_generation(previous: u64) -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    (now.min(u64::MAX as u128) as u64).max(previous.saturating_add(1))
}

fn saved_selection(group: &str) -> Result<String> {
    let bytes = fs::read(paths::strategy_path()?).context("read saved proxy selection")?;
    let draft: Strategy = serde_json::from_slice(&bytes).context("read saved proxy selection")?;
    if group == GLOBAL_GROUP {
        return Ok(draft.global_selected);
    }
    draft
        .groups
        .into_iter()
        .find(|saved| saved.name == group)
        .map(|saved| saved.selected)
        .context("所选节点组已被删除或重命名")
}

fn validate_runtime_config(yaml: &Path) -> Result<()> {
    let bin = paths::bundled_mihomo();
    if !bin.is_file() {
        bail!(
            "mihomo binary missing at {}. Run scripts/fetch-mihomo.sh",
            bin.display()
        );
    }
    // Mihomo's parser can echo credential-bearing proxy entries on failure.
    // Keep raw diagnostics private and report only a safe location/category.
    let output = mihomo_command(&bin)?
        .arg("-t")
        .arg("-f")
        .arg(yaml)
        .output()
        .context("mihomo -t")?;
    if !output.status.success() {
        let detail = mihomo_validation_detail(&output);
        if detail.is_empty() {
            bail!("候选配置未通过 Mihomo 校验；运行配置未改变");
        }
        bail!("候选配置未通过 Mihomo 校验：{detail}；运行配置未改变");
    }
    Ok(())
}

fn mihomo_validation_detail(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let text = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    let line = text
        .lines()
        .find(|line| line.contains("level=error"))
        .or_else(|| text.lines().rev().find(|line| !line.trim().is_empty()))
        .unwrap_or_default();
    let message = line
        .split_once("msg=\"")
        .map(|(_, value)| value.trim_end_matches('"'))
        .unwrap_or(line)
        .replace("\\\"", "\"")
        .to_ascii_lowercase();
    let location = if let Some(caps) = regex::Regex::new(r"proxy group\[(\d+)\]")
        .expect("static mihomo location pattern")
        .captures(&message)
    {
        format!("代理组 #{}", &caps[1])
    } else if let Some(caps) = regex::Regex::new(r"proxy (\d+):")
        .expect("static mihomo location pattern")
        .captures(&message)
    {
        format!("节点 #{}", &caps[1])
    } else if let Some(caps) = regex::Regex::new(r"rules\[(\d+)\]")
        .expect("static mihomo location pattern")
        .captures(&message)
    {
        format!("规则 #{}", &caps[1])
    } else if let Some(caps) = regex::Regex::new(r"yaml: line (\d+)")
        .expect("static mihomo location pattern")
        .captures(&message)
    {
        format!("YAML 第 {} 行", &caps[1])
    } else {
        "候选配置".into()
    };
    let reason = if message.contains("unsupport proxy type") {
        "节点类型不受当前 Mihomo 支持"
    } else if message.contains("not found") {
        "引用了不存在的节点或节点组"
    } else if message.contains("cannot parse") || message.contains("invalid syntax") {
        "字段格式错误"
    } else if message.contains("yaml:") || message.contains("did not find expected") {
        "YAML 结构错误"
    } else if message.contains("missing") || message.contains("required") {
        "缺少必要字段"
    } else {
        "Mihomo 拒绝了配置"
    };
    format!("{location}{reason}")
}

fn cancellation_revision() -> Result<Vec<u8>> {
    match fs::read(paths::data_dir()?.join("core.cancel")) {
        Ok(bytes) => Ok(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error).context("read pending cancellation"),
    }
}

fn cancel_pending_connect() -> Result<()> {
    paths::atomic_write(
        &paths::data_dir()?.join("core.cancel"),
        uuid::Uuid::new_v4().to_string().as_bytes(),
    )
}

fn is_wanted() -> bool {
    paths::wanted_path()
        .map(|path| path.is_file())
        .unwrap_or(false)
}

fn set_wanted(on: bool) -> Result<()> {
    let path = paths::wanted_path()?;
    if on {
        paths::atomic_write(&path, b"1")?;
    } else if let Err(error) = fs::remove_file(path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            return Err(error).context("clear core connection intent");
        }
    }
    Ok(())
}

fn mixed_listening(port: u16) -> bool {
    let Ok(addr) = format!("127.0.0.1:{port}").parse() else {
        return false;
    };
    TcpStream::connect_timeout(&addr, Duration::from_millis(80)).is_ok()
}

fn owned_listen_ports(mixed_port: u16) -> [u16; 4] {
    [
        mixed_port,
        compile::network_extension_socks_port(mixed_port),
        compile::controller_port(mixed_port),
        compile::DNS_LISTEN_PORT,
    ]
}

fn reclaim_owned_mihomo(mixed_port: Option<u16>, already_stopped: Option<i32>) -> bool {
    let mut pids = BTreeSet::new();
    if let Some(pid) = read_pid_file() {
        pids.insert(pid);
    }
    pids.extend(pids_named_mihomo());
    if let Some(port) = mixed_port {
        for owned in owned_listen_ports(port) {
            pids.extend(pids_listening_tcp(owned));
            if owned == compile::DNS_LISTEN_PORT {
                pids.extend(pids_bound_udp(owned));
            }
        }
    }
    let mut stopped = already_stopped.is_some();
    for pid in pids {
        if already_stopped == Some(pid) || !is_our_mihomo_pid(pid) {
            continue;
        }
        log::info("supervisor", format!("stopping leftover mihomo pid {pid}"));
        terminate_pid(pid);
        stopped = true;
    }
    if let Some(port) = mixed_port {
        wait_for_port_free(port);
    }
    stopped
}

fn read_pid_file() -> Option<i32> {
    let path = paths::pid_path().ok()?;
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn terminate_pid(pid: i32) {
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
    wait_for_pid_exit(pid);
    if unsafe { libc::kill(pid, 0) } == 0 {
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
        wait_for_pid_exit(pid);
    }
}

fn wait_for_port_free(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while mixed_listening(port) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn parse_pid_lines(bytes: &[u8]) -> Vec<i32> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .filter(|pid| *pid > 1)
        .collect()
}

fn pids_from_lsof(args: &[&str]) -> Vec<i32> {
    let Ok(output) = Command::new("lsof").args(args).output() else {
        return Vec::new();
    };
    parse_pid_lines(&output.stdout)
}

fn pids_listening_tcp(port: u16) -> Vec<i32> {
    let spec = format!("-iTCP:{port}");
    pids_from_lsof(&["-nP", &spec, "-sTCP:LISTEN", "-t"])
}

fn pids_bound_udp(port: u16) -> Vec<i32> {
    let spec = format!("-iUDP:{port}");
    pids_from_lsof(&["-nP", &spec, "-t"])
}

fn pids_named_mihomo() -> Vec<i32> {
    let Ok(output) = Command::new("pgrep").arg("-x").arg("mihomo").output() else {
        return Vec::new();
    };
    parse_pid_lines(&output.stdout)
}

fn is_our_mihomo_exe(exe: &Path) -> bool {
    if exe.file_name().and_then(|name| name.to_str()) != Some("mihomo") {
        return false;
    }
    let bundled = paths::bundled_mihomo();
    if exe == bundled {
        return true;
    }
    if let (Ok(exe), Ok(bundled)) = (fs::canonicalize(exe), fs::canonicalize(&bundled)) {
        if exe == bundled {
            return true;
        }
    }
    exe.components()
        .any(|component| component.as_os_str() == "myproxy.app")
}

fn is_our_mihomo_pid(pid: i32) -> bool {
    process_exe(pid)
        .map(|exe| is_our_mihomo_exe(&exe))
        .unwrap_or(false)
}

fn process_exe(pid: i32) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            fn proc_pidpath(pid: i32, buffer: *mut libc::c_void, buffersize: u32) -> i32;
        }
        let mut buf = [0u8; 4096];
        let n = unsafe { proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
        if n <= 0 {
            return None;
        }
        let path = std::str::from_utf8(&buf[..n as usize]).ok()?;
        Some(PathBuf::from(path))
    }
    #[cfg(target_os = "linux")]
    {
        fs::read_link(format!("/proc/{pid}/exe")).ok()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = pid;
        None
    }
}

fn needs_reconnect(
    running: bool,
    wanted: bool,
    running_tun: bool,
    running_se: bool,
    running_port: Option<u16>,
    strategy: &Strategy,
) -> bool {
    (running
        && (running_tun != strategy.tun
            || running_se != strategy.system_extension
            || running_port != Some(strategy.mixed_port)))
        || (!running && wanted)
}

fn wait_for_pid_exit(pid: i32) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while unsafe { libc::kill(pid, 0) == 0 } && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn pid_file_alive(port: Option<u16>) -> bool {
    let Ok(path) = paths::pid_path() else {
        return false;
    };
    let Ok(raw_pid) = fs::read_to_string(&path) else {
        return false;
    };
    let Ok(pid) = raw_pid.trim().parse::<i32>() else {
        return false;
    };
    if !is_our_mihomo_pid(pid) || unsafe { libc::kill(pid, 0) != 0 } {
        let _ = fs::remove_file(path);
        return false;
    }
    if let Some(port) = port {
        if !mixed_listening(port) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_apply_observes_shutdown() {
        *OPERATION.lock().unwrap() = true;
        let (ready, started) = std::sync::mpsc::channel();
        let queued = std::thread::spawn(move || {
            // A regressed guard still fails before any disk or core operations.
            let strategy = Strategy {
                exclude_filter: "[".into(),
                ..Strategy::default()
            };
            ready.send(()).unwrap();
            Supervisor::default()
                .apply(&strategy)
                .unwrap_err()
                .to_string()
        });
        started.recv().unwrap();
        assert_eq!(queued.join().unwrap(), "application is shutting down");
        *OPERATION.lock().unwrap() = false;
    }

    #[test]
    fn port_change_requires_reconnect() {
        let strategy = Strategy {
            mixed_port: 7891,
            ..Strategy::default()
        };
        assert!(needs_reconnect(
            true,
            true,
            strategy.tun,
            strategy.system_extension,
            Some(7890),
            &strategy,
        ));
    }

    #[test]
    fn mixed_port_notifies_update_proxy_hook() {
        use std::sync::atomic::{AtomicU16, Ordering};
        static PORT: AtomicU16 = AtomicU16::new(1);
        fn hook(port: Option<u16>) {
            PORT.store(port.unwrap_or(0), Ordering::SeqCst);
        }
        let supervisor = Supervisor::default();
        supervisor.set_update_proxy_hook(hook);
        supervisor.remember_mixed_port(Some(7891));
        assert_eq!(PORT.load(Ordering::SeqCst), 7891);
        supervisor.remember_mixed_port(None);
        assert_eq!(PORT.load(Ordering::SeqCst), 0);
        assert_eq!(supervisor.update_download_port(), None);
    }

    #[test]
    fn wanted_but_missing_core_requires_reconnect() {
        let strategy = Strategy::default();
        assert!(needs_reconnect(
            false,
            true,
            strategy.tun,
            strategy.system_extension,
            None,
            &strategy,
        ));
        assert!(!needs_reconnect(
            false,
            false,
            strategy.tun,
            strategy.system_extension,
            None,
            &strategy,
        ));
    }

    #[test]
    fn bundled_app_mihomo_is_ours() {
        assert!(is_our_mihomo_exe(Path::new(
            "/Applications/myproxy.app/Contents/MacOS/mihomo"
        )));
        assert!(!is_our_mihomo_exe(Path::new("/opt/homebrew/bin/mihomo")));
        assert!(!is_our_mihomo_exe(Path::new(
            "/Applications/myproxy.app/Contents/MacOS/myproxy"
        )));
    }

    #[test]
    fn owned_listen_ports_cover_mixed_controller_ne_and_dns() {
        let ports = owned_listen_ports(7891);
        assert_eq!(ports[0], 7891);
        assert_eq!(ports[1], compile::network_extension_socks_port(7891));
        assert_eq!(ports[2], compile::controller_port(7891));
        assert_eq!(ports[3], compile::DNS_LISTEN_PORT);
    }
}

fn ensure_tun_privileges(bin: &Path) -> Result<()> {
    if mihomo_has_tun_privs(bin) {
        log::info("supervisor", "mihomo already setuid root");
        return Ok(());
    }
    #[cfg(not(target_os = "macos"))]
    {
        bail!("TUN needs a setuid-root mihomo binary at {}", bin.display());
    }
    #[cfg(target_os = "macos")]
    {
        let quoted = sh_single_quote(bin);
        let shell = format!("chown root:admin {quoted} && chmod 4755 {quoted}");
        log::info("supervisor", "asking for administrator to setuid mihomo");
        let status = Command::new("osascript")
            .arg("-e")
            .arg(format!(
                "do shell script {} with administrator privileges",
                applescript_string(&shell)
            ))
            .stdout(std::io::stderr())
            .status()
            .context("osascript")?;
        if !status.success() || !mihomo_has_tun_privs(bin) {
            bail!(
                "TUN needs an administrator password once to mark {} setuid root",
                bin.display()
            );
        }
        log::info("supervisor", "mihomo is setuid root");
        Ok(())
    }
}

fn mihomo_command(bin: &Path) -> Result<Command> {
    let mut command = Command::new(bin);
    command.arg("-d").arg(paths::data_dir()?);
    Ok(command)
}

fn mihomo_has_tun_privs(bin: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(bin)
            .map(|meta| meta.uid() == 0 && meta.mode() & 0o4000 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = bin;
        false
    }
}

fn sh_single_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn applescript_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
