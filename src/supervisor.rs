use std::fs;
use std::fs::{File, OpenOptions};
use std::net::TcpStream;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::catalog::{self, Catalog};
use crate::compile;
use crate::controller;
use crate::log;
use crate::paths;
use crate::strategy::Strategy;

const HEALTH_INTERVAL: Duration = Duration::from_secs(2);
const FAIL_BEFORE_RETRY: u32 = 3;
const RECOVER_INTERVAL: Duration = Duration::from_secs(8);
const MAX_RECOVERIES: u32 = 5;
const STARTUP_GRACE: Duration = Duration::from_secs(8);

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
        Self {
            wanted: true,
            ready: true,
            note: None,
            proxy_now: proxy_now.into(),
        }
    }

    fn recovered(proxy_now: impl Into<String>, note: &str) -> Self {
        Self {
            wanted: true,
            ready: true,
            note: Some(note.into()),
            proxy_now: proxy_now.into(),
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
    recover_after: Option<Instant>,
    last: CoreHealth,
    fails: u32,
    recoveries: u32,
}

impl Default for HealthWatch {
    fn default() -> Self {
        Self {
            last_check: None,
            last_recover: None,
            recover_after: None,
            last: CoreHealth::idle(),
            fails: 0,
            recoveries: 0,
        }
    }
}

pub struct Supervisor {
    child: Mutex<Option<Child>>,
    running_tun: Mutex<bool>,
    running_se: Mutex<bool>,
    running_mixed_port: Mutex<Option<u16>>,
    health: Mutex<HealthWatch>,
}

// The flag remains set after shutdown so queued operations cannot restart the core.
static OPERATION: Mutex<bool> = Mutex::new(false);
const OPERATION_LOCK_TIMEOUT: Duration = Duration::from_secs(5);

struct OperationGuard {
    _state: std::sync::MutexGuard<'static, bool>,
    file: File,
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
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
            Err(_) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50));
            }
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
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result == 0 {
            return Ok(OperationGuard { _state: state, file });
        }
        if Instant::now() >= deadline {
            bail!("另一个核心操作正在进行中，请稍后重试");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

impl Default for Supervisor {
    fn default() -> Self {
        Self {
            child: Mutex::new(None),
            running_tun: Mutex::new(false),
            running_se: Mutex::new(false),
            running_mixed_port: Mutex::new(None),
            health: Mutex::new(HealthWatch::default()),
        }
    }
}

impl Supervisor {
    pub fn shared() -> Arc<Self> {
        static INSTANCE: OnceLock<Arc<Supervisor>> = OnceLock::new();
        INSTANCE.get_or_init(|| Arc::new(Self::default())).clone()
    }

    pub fn adopt_running(&self, tun: bool, system_extension: bool, mixed_port: u16) {
        if pid_file_alive(Some(mixed_port)) {
            *self.running_tun.lock().expect("supervisor lock") = tun;
            *self.running_se.lock().expect("supervisor lock") = system_extension;
            *self.running_mixed_port.lock().expect("supervisor lock") = Some(mixed_port);
            set_wanted(true);
        }
    }

    pub fn sync_wanted_on_launch(&self) {
        if self.is_running() {
            set_wanted(true);
        } else {
            set_wanted(false);
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
        let mut health = self.health.lock().expect("supervisor health lock").last.clone();
        health.wanted = true;
        health
    }

    pub fn connect(&self, strategy: &Strategy) -> Result<()> {
        let operation = acquire_operation()?;
        if *operation._state {
            bail!("application is shutting down");
        }
        set_wanted(true);
        self.reset_health();
        let result = self.connect_inner(strategy);
        drop(operation);
        result
    }

    fn connect_inner(&self, strategy: &Strategy) -> Result<()> {
        log::info(
            "supervisor",
            format!(
                "connect mixed-port {} tun={} se={}",
                strategy.mixed_port, strategy.tun, strategy.system_extension
            ),
        );
        self.disconnect_inner()?;
        let catalog = catalog::refresh(strategy)?;
        self.start_with_catalog(strategy, &catalog)
    }

    fn start_with_catalog(&self, strategy: &Strategy, catalog: &Catalog) -> Result<()> {
        compile::compile(strategy, catalog)?;
        let yaml = paths::runtime_yaml_path()?;
        let bin = paths::bundled_mihomo();
        if !bin.is_file() {
            log::error("supervisor", format!("mihomo missing at {}", bin.display()));
            bail!(
                "mihomo binary missing at {}. Run scripts/fetch-mihomo.sh",
                bin.display()
            );
        }
        let status = Command::new(&bin)
            .arg("-t")
            .arg("-f")
            .arg(&yaml)
            .stdout(std::io::stderr())
            .status()
            .context("mihomo -t")?;
        if !status.success() {
            log::error("supervisor", "runtime YAML failed mihomo -t");
            bail!("runtime YAML failed mihomo -t");
        }
        if strategy.tun && !strategy.system_extension {
            ensure_tun_privileges(&bin)?;
        }

        let log_file = File::create(paths::mihomo_log_path()?).context("mihomo.log")?;
        let mut child = Command::new(&bin)
            .arg("-f")
            .arg(&yaml)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log_file.try_clone()?))
            .stderr(Stdio::from(log_file))
            .spawn()
            .context("spawn mihomo")?;
        fs::write(paths::pid_path()?, child.id().to_string())?;

        let addr = format!("127.0.0.1:{}", strategy.mixed_port);
        let wait_secs = if strategy.tun && !strategy.system_extension {
            8
        } else {
            3
        };
        let deadline = Instant::now() + Duration::from_secs(wait_secs);
        loop {
            if let Ok(Some(status)) = child.try_wait() {
                log::error("supervisor", format!("mihomo exited immediately: {status}"));
                bail!(
                    "mihomo exited immediately: {status}. See {}",
                    paths::mihomo_log_path()?.display()
                );
            }
            if TcpStream::connect_timeout(
                &addr.parse().context("mixed-port addr")?,
                Duration::from_millis(80),
            )
            .is_ok()
            {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                log::error("supervisor", format!("mixed-port {addr} did not come up"));
                bail!(
                    "mixed-port {addr} did not come up. Is the port already taken? See {}",
                    paths::mihomo_log_path()?.display()
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        *self.child.lock().expect("supervisor lock") = Some(child);
        *self.running_tun.lock().expect("supervisor lock") = strategy.tun;
        *self.running_se.lock().expect("supervisor lock") = strategy.system_extension;
        *self.running_mixed_port.lock().expect("supervisor lock") = Some(strategy.mixed_port);
        self.health
            .lock()
            .expect("supervisor health lock")
            .recover_after = Some(Instant::now() + STARTUP_GRACE);
        log::info(
            "supervisor",
            format!(
                "listening {addr}{}{}",
                if strategy.tun && !strategy.system_extension {
                    " tun"
                } else {
                    ""
                },
                if strategy.system_extension { " se" } else { "" }
            ),
        );
        if strategy.system_extension {
            crate::network_extension::enable_async(strategy);
        }
        controller::restore_selections(strategy.mixed_port, strategy);
        let _ = self.wait_ready(strategy, Duration::from_secs(2));
        Ok(())
    }

    pub fn apply(&self, strategy: &Strategy) -> Result<Catalog> {
        self.apply_inner(strategy, true)
    }

    pub fn apply_cached(&self, strategy: &Strategy) -> Result<Catalog> {
        self.apply_inner(strategy, false)
    }

    fn apply_inner(&self, strategy: &Strategy, refresh_catalog: bool) -> Result<Catalog> {
        let operation = acquire_operation()?;
        if *operation._state {
            bail!("application is shutting down");
        }
        log::debug("supervisor", "apply");
        let catalog = if refresh_catalog {
            catalog::refresh(strategy)?
        } else {
            Catalog::load().unwrap_or_default()
        };
        let running = self.is_running();
        let was_tun = *self.running_tun.lock().expect("supervisor lock");
        let was_se = *self.running_se.lock().expect("supervisor lock");
        let was_port = *self.running_mixed_port.lock().expect("supervisor lock");
        let needs_reconnect = needs_reconnect(
            running,
            is_wanted(),
            was_tun,
            was_se,
            was_port,
            strategy,
        );
        if needs_reconnect {
            if running {
                log::info(
                    "supervisor",
                    format!(
                        "apply requires reconnect: port {:?}→{} tun {was_tun}→{} se {was_se}→{}",
                        was_port,
                        strategy.mixed_port,
                        strategy.tun,
                        strategy.system_extension
                    ),
                );
            }
            self.disconnect_inner()?;
            self.start_with_catalog(strategy, &catalog)?;
        } else {
            compile::compile(strategy, &catalog)?;
            if running {
                if let Err(err) = controller::reload(strategy.mixed_port) {
                    log::warn("supervisor", format!("controller reload failed, restarting core: {err:#}"));
                    self.disconnect_inner()?;
                    self.start_with_catalog(strategy, &catalog)?;
                    return Ok(catalog);
                }
                controller::restore_selections(strategy.mixed_port, strategy);
                if strategy.system_extension {
                    crate::network_extension::enable_async(strategy);
                }
            }
        }
        drop(operation);
        Ok(catalog)
    }

    pub fn disconnect(&self) -> Result<()> {
        set_wanted(false);
        self.reset_health();
        let _operation = acquire_operation()?;
        self.disconnect_inner()
    }

    pub fn shutdown(&self) -> Result<()> {
        set_wanted(false);
        self.reset_health();
        let mut operation = acquire_operation()?;
        *operation._state = true;
        self.disconnect_inner()
    }

    pub fn observe(&self, strategy: &Strategy) -> CoreHealth {
        if !is_wanted() {
            self.reset_health();
            return CoreHealth::idle();
        }
        let now = Instant::now();
        {
            let mut watch = self.health.lock().expect("supervisor health lock");
            if let Some(last_check) = watch.last_check {
                if now.duration_since(last_check) < HEALTH_INTERVAL {
                    let mut health = watch.last.clone();
                    health.wanted = true;
                    return health;
                }
            }
            watch.last_check = Some(now);
        }
        let Some(proxy_now) = self.probe_now(strategy) else {
            return self.note_failure(strategy, now);
        };
        self.mark_ready(proxy_now.clone());
        CoreHealth::ready(proxy_now)
    }

    fn disconnect_inner(&self) -> Result<()> {
        crate::network_extension::disable_async();
        let mut stopped = false;
        let child = self.child.lock().expect("supervisor lock").take();
        let child_pid = child.as_ref().map(Child::id);
        if let Some(mut child) = child {
            let _ = child.kill();
            let _ = child.wait();
            stopped = true;
        }
        if child_pid.is_none() {
            if let Ok(path) = paths::pid_path() {
                if let Ok(pid) = fs::read_to_string(&path) {
                    if let Ok(pid) = pid.trim().parse::<i32>() {
                        let port = *self.running_mixed_port.lock().expect("supervisor lock");
                        if port.map(mixed_listening).unwrap_or(false) {
                            unsafe {
                                libc::kill(pid, libc::SIGTERM);
                            }
                            wait_for_pid_exit(pid);
                            stopped = true;
                        } else {
                            log::warn("supervisor", "ignoring stale mihomo pid without a listening port");
                        }
                    }
                }
            }
        }
        if let Ok(path) = paths::pid_path() {
            let _ = fs::remove_file(path);
        }
        *self.running_tun.lock().expect("supervisor lock") = false;
        *self.running_se.lock().expect("supervisor lock") = false;
        *self.running_mixed_port.lock().expect("supervisor lock") = None;
        if stopped {
            log::info("supervisor", "disconnect");
        }
        Ok(())
    }

    pub fn is_running(&self) -> bool {
        {
            let mut slot = self.child.lock().expect("supervisor lock");
            if let Some(child) = slot.as_mut() {
                match child.try_wait() {
                    Ok(None) => return true,
                    _ => {
                        let pid = child.id();
                        *slot = None;
                        *self.running_mixed_port.lock().expect("supervisor lock") = None;
                        if let Ok(path) = paths::pid_path() {
                            let matches = fs::read_to_string(&path)
                                .map(|value| value.trim() == pid.to_string())
                                .unwrap_or(false);
                            if matches {
                                let _ = fs::remove_file(path);
                            }
                        }
                    }
                }
            }
        }
        pid_file_alive(*self.running_mixed_port.lock().expect("supervisor lock"))
    }

    fn probe_now(&self, strategy: &Strategy) -> Option<String> {
        if !self.is_running() || !mixed_listening(strategy.mixed_port) {
            return None;
        }
        controller::probe(strategy.mixed_port, compile::default_group(strategy)).ok()
    }

    fn wait_ready(&self, strategy: &Strategy, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(proxy_now) = self.probe_now(strategy) {
                self.mark_ready(proxy_now);
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn mark_ready(&self, proxy_now: String) {
        let mut watch = self.health.lock().expect("supervisor health lock");
        watch.last_check = Some(Instant::now());
        watch.fails = 0;
        watch.recoveries = 0;
        watch.recover_after = None;
        watch.last = CoreHealth::ready(proxy_now);
    }

    fn reset_health(&self) {
        *self.health.lock().expect("supervisor health lock") = HealthWatch::default();
    }

    fn note_failure(&self, strategy: &Strategy, now: Instant) -> CoreHealth {
        let (fails, recoveries, last_recover) = {
            let mut watch = self.health.lock().expect("supervisor health lock");
            watch.last_check = Some(now);
            watch.fails = watch.fails.saturating_add(1);
            (watch.fails, watch.recoveries, watch.last_recover)
        };
        let warming_up = self
            .health
            .lock()
            .expect("supervisor health lock")
            .recover_after
            .map(|until| now < until)
            .unwrap_or(false);
        let health = if !is_wanted() {
            CoreHealth::idle()
        } else if warming_up {
            CoreHealth::failing("核心正在启动，等待就绪…")
        } else if fails < FAIL_BEFORE_RETRY {
            CoreHealth::failing("核心暂时无响应，正在确认…")
        } else if recoveries >= MAX_RECOVERIES {
            CoreHealth::failing("核心反复异常，已停止自动恢复。请手动连接。")
        } else if last_recover
            .map(|at| now.duration_since(at) < RECOVER_INTERVAL)
            .unwrap_or(false)
        {
            CoreHealth::failing("核心异常，等待自动重试…")
        } else {
            return self.recover(strategy, now);
        };
        self.store_health(health.clone());
        health
    }

    fn recover(&self, strategy: &Strategy, now: Instant) -> CoreHealth {
        if !is_wanted() {
            self.reset_health();
            return CoreHealth::idle();
        }
        let Ok(guard) = acquire_operation_with_timeout(Duration::ZERO) else {
            let health = CoreHealth::failing("核心异常，正在等待当前操作结束…");
            self.store_health(health.clone());
            return health;
        };
        if *guard._state {
            let health = CoreHealth::failing("核心异常，应用正在退出…");
            self.store_health(health.clone());
            return health;
        }
        {
            let mut watch = self.health.lock().expect("supervisor health lock");
            if watch.recoveries >= MAX_RECOVERIES {
                let health = CoreHealth::failing("核心反复异常，已停止自动恢复。请手动连接。");
                watch.last = health.clone();
                return health;
            }
            if watch
                .last_recover
                .map(|at| now.duration_since(at) < RECOVER_INTERVAL)
                .unwrap_or(false)
            {
                let health = CoreHealth::failing("核心异常，等待自动重试…");
                watch.last = health.clone();
                return health;
            }
            watch.last_recover = Some(now);
            watch.recoveries = watch.recoveries.saturating_add(1);
            watch.fails = 0;
        }
        let n = self.health.lock().expect("supervisor health lock").recoveries;
        if self.is_running() {
            log::info("supervisor", format!("health reload #{n}"));
            match controller::reload(strategy.mixed_port) {
                Ok(()) => {
                    controller::restore_selections(strategy.mixed_port, strategy);
                    if let Some(proxy_now) = self.probe_now(strategy) {
                        drop(guard);
                        self.mark_ready(proxy_now.clone());
                        return CoreHealth::recovered(proxy_now, "核心已重新加载。");
                    }
                    log::info("supervisor", "health reload still unready, reconnect");
                }
                Err(err) => {
                    log::warn("supervisor", format!("health reload failed: {err:#}"));
                }
            }
        }
        log::info("supervisor", format!("health reconnect #{n}"));
        let health = match self.connect_inner(strategy) {
            Ok(()) => {
                if let Some(proxy_now) = self.probe_now(strategy) {
                    drop(guard);
                    self.mark_ready(proxy_now.clone());
                    return CoreHealth::recovered(proxy_now, "核心已重新连接。");
                }
                CoreHealth::failing("核心异常，正在重新连接…")
            }
            Err(err) => {
                log::warn("supervisor", format!("health reconnect failed: {err:#}"));
                CoreHealth::failing("核心重连失败，稍后会再试。")
            }
        };
        drop(guard);
        self.store_health(health.clone());
        health
    }

    fn store_health(&self, health: CoreHealth) {
        let mut watch = self.health.lock().expect("supervisor health lock");
        watch.last = health;
    }
}

fn is_wanted() -> bool {
    paths::wanted_path()
        .map(|path| path.is_file())
        .unwrap_or(false)
}

fn set_wanted(on: bool) {
    let Ok(path) = paths::wanted_path() else {
        return;
    };
    if on {
        let _ = fs::write(path, "1");
    } else {
        let _ = fs::remove_file(path);
    }
}

fn mixed_listening(port: u16) -> bool {
    let Ok(addr) = format!("127.0.0.1:{port}").parse() else {
        return false;
    };
    TcpStream::connect_timeout(&addr, Duration::from_millis(80)).is_ok()
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
    if unsafe { libc::kill(pid, 0) != 0 } {
        let _ = fs::remove_file(path);
        return false;
    }
    if let Some(port) = port {
        if !mixed_listening(port) {
            let _ = fs::remove_file(path);
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
