use std::fs;
use std::fs::File;
use std::net::TcpStream;
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreHealth {
    pub wanted: bool,
    pub ready: bool,
    pub note: Option<String>,
}

impl CoreHealth {
    fn idle() -> Self {
        Self {
            wanted: false,
            ready: false,
            note: None,
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

pub struct Supervisor {
    child: Mutex<Option<Child>>,
    running_tun: Mutex<bool>,
    running_se: Mutex<bool>,
    health: Mutex<HealthWatch>,
}

// ponytail: one core per process; cross-process CLI operations need a file lock.
// The flag remains set after shutdown so queued operations cannot restart the core.
static OPERATION: Mutex<bool> = Mutex::new(false);

impl Default for Supervisor {
    fn default() -> Self {
        Self {
            child: Mutex::new(None),
            running_tun: Mutex::new(false),
            running_se: Mutex::new(false),
            health: Mutex::new(HealthWatch::default()),
        }
    }
}

impl Supervisor {
    pub fn shared() -> Arc<Self> {
        static INSTANCE: OnceLock<Arc<Supervisor>> = OnceLock::new();
        INSTANCE.get_or_init(|| Arc::new(Self::default())).clone()
    }

    pub fn adopt_running(&self, tun: bool, system_extension: bool) {
        if self.is_running() {
            *self.running_tun.lock().expect("supervisor lock") = tun;
            *self.running_se.lock().expect("supervisor lock") = system_extension;
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
        let shutting_down = OPERATION.lock().expect("supervisor operation lock");
        if *shutting_down {
            bail!("application is shutting down");
        }
        set_wanted(true);
        self.reset_health();
        self.connect_inner(strategy)
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
        compile::compile(strategy, &catalog)?;
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
        let shutting_down = OPERATION.lock().expect("supervisor operation lock");
        if *shutting_down {
            bail!("application is shutting down");
        }
        log::debug("supervisor", "apply");
        let catalog = catalog::refresh(strategy)?;
        compile::compile(strategy, &catalog)?;
        if self.is_running() {
            let was_tun = *self.running_tun.lock().expect("supervisor lock");
            let was_se = *self.running_se.lock().expect("supervisor lock");
            if was_tun != strategy.tun || was_se != strategy.system_extension {
                log::info(
                    "supervisor",
                    format!(
                        "intercept tun {was_tun}→{} se {was_se}→{}, reconnect",
                        strategy.tun, strategy.system_extension
                    ),
                );
                self.connect_inner(strategy)?;
            } else {
                controller::reload(strategy.mixed_port)?;
                controller::restore_selections(strategy.mixed_port, strategy);
                if strategy.system_extension {
                    crate::network_extension::enable_async(strategy);
                }
            }
        }
        Ok(catalog)
    }

    pub fn disconnect(&self) -> Result<()> {
        set_wanted(false);
        self.reset_health();
        let _operation = OPERATION.lock().expect("supervisor operation lock");
        self.disconnect_inner()
    }

    pub fn shutdown(&self) -> Result<()> {
        set_wanted(false);
        self.reset_health();
        let mut shutting_down = OPERATION.lock().expect("supervisor operation lock");
        *shutting_down = true;
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
        if !self.check_ready(strategy) {
            return self.note_failure(strategy, now);
        }
        self.mark_ready();
        CoreHealth {
            wanted: true,
            ready: true,
            note: None,
        }
    }

    fn disconnect_inner(&self) -> Result<()> {
        crate::network_extension::disable_async();
        let mut stopped = false;
        let child = self.child.lock().expect("supervisor lock").take();
        if let Some(mut child) = child {
            let _ = child.kill();
            let _ = child.wait();
            stopped = true;
        }
        if let Ok(path) = paths::pid_path() {
            if let Ok(pid) = fs::read_to_string(&path) {
                if let Ok(pid) = pid.trim().parse::<i32>() {
                    unsafe {
                        libc::kill(pid, libc::SIGTERM);
                    }
                    stopped = true;
                }
                let _ = fs::remove_file(path);
            }
        }
        *self.running_tun.lock().expect("supervisor lock") = false;
        *self.running_se.lock().expect("supervisor lock") = false;
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
                    _ => *slot = None,
                }
            }
        }
        if let Ok(path) = paths::pid_path() {
            if let Ok(pid) = fs::read_to_string(path) {
                if let Ok(pid) = pid.trim().parse::<i32>() {
                    return unsafe { libc::kill(pid, 0) == 0 };
                }
            }
        }
        false
    }

    fn check_ready(&self, strategy: &Strategy) -> bool {
        self.is_running()
            && mixed_listening(strategy.mixed_port)
            && controller::probe(strategy.mixed_port, compile::default_group(strategy)).is_ok()
    }

    fn wait_ready(&self, strategy: &Strategy, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self.check_ready(strategy) {
                self.mark_ready();
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn mark_ready(&self) {
        let mut watch = self.health.lock().expect("supervisor health lock");
        watch.last_check = Some(Instant::now());
        watch.fails = 0;
        watch.recoveries = 0;
        watch.last = CoreHealth {
            wanted: true,
            ready: true,
            note: None,
        };
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
        let health = if !is_wanted() {
            CoreHealth::idle()
        } else if fails < FAIL_BEFORE_RETRY {
            CoreHealth {
                wanted: true,
                ready: false,
                note: Some("核心暂时无响应，正在确认…".into()),
            }
        } else if recoveries >= MAX_RECOVERIES {
            CoreHealth {
                wanted: true,
                ready: false,
                note: Some("核心反复异常，已停止自动恢复。请手动连接。".into()),
            }
        } else if last_recover
            .map(|at| now.duration_since(at) < RECOVER_INTERVAL)
            .unwrap_or(false)
        {
            CoreHealth {
                wanted: true,
                ready: false,
                note: Some("核心异常，等待自动重试…".into()),
            }
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
        let Ok(guard) = OPERATION.try_lock() else {
            let health = CoreHealth {
                wanted: true,
                ready: false,
                note: Some("核心异常，正在等待当前操作结束…".into()),
            };
            self.store_health(health.clone());
            return health;
        };
        if *guard {
            let health = CoreHealth {
                wanted: true,
                ready: false,
                note: Some("核心异常，应用正在退出…".into()),
            };
            self.store_health(health.clone());
            return health;
        }
        {
            let mut watch = self.health.lock().expect("supervisor health lock");
            if watch.recoveries >= MAX_RECOVERIES {
                let health = CoreHealth {
                    wanted: true,
                    ready: false,
                    note: Some("核心反复异常，已停止自动恢复。请手动连接。".into()),
                };
                watch.last = health.clone();
                return health;
            }
            if watch
                .last_recover
                .map(|at| now.duration_since(at) < RECOVER_INTERVAL)
                .unwrap_or(false)
            {
                let health = CoreHealth {
                    wanted: true,
                    ready: false,
                    note: Some("核心异常，等待自动重试…".into()),
                };
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
                    if self.check_ready(strategy) {
                        drop(guard);
                        self.mark_ready();
                        return CoreHealth {
                            wanted: true,
                            ready: true,
                            note: Some("核心已重新加载。".into()),
                        };
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
            Ok(()) if self.check_ready(strategy) => {
                drop(guard);
                self.mark_ready();
                return CoreHealth {
                    wanted: true,
                    ready: true,
                    note: Some("核心已重新连接。".into()),
                };
            }
            Ok(()) => CoreHealth {
                wanted: true,
                ready: false,
                note: Some("核心异常，正在重新连接…".into()),
            },
            Err(err) => {
                log::warn("supervisor", format!("health reconnect failed: {err:#}"));
                CoreHealth {
                    wanted: true,
                    ready: false,
                    note: Some("核心重连失败，稍后会再试。".into()),
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_apply_observes_shutdown() {
        let mut operation = OPERATION.lock().unwrap();
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
        *operation = true;
        drop(operation);
        assert_eq!(queued.join().unwrap(), "application is shutting down");
        *OPERATION.lock().unwrap() = false;
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
