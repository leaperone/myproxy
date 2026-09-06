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

pub struct Supervisor {
    child: Mutex<Option<Child>>,
    running_tun: Mutex<bool>,
    running_se: Mutex<bool>,
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
        }
    }

    pub fn connect(&self, strategy: &Strategy) -> Result<()> {
        let shutting_down = OPERATION.lock().expect("supervisor operation lock");
        if *shutting_down {
            bail!("application is shutting down");
        }
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
        let ctrl_deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if controller::ping(strategy.mixed_port).is_ok() {
                break;
            }
            if Instant::now() >= ctrl_deadline {
                let _ = child.kill();
                let _ = child.wait();
                log::error("supervisor", "controller did not come up");
                bail!(
                    "controller did not come up on 127.0.0.1:{}. See {}",
                    compile::controller_port(strategy.mixed_port),
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
        let _operation = OPERATION.lock().expect("supervisor operation lock");
        self.disconnect_inner()
    }

    pub fn shutdown(&self) -> Result<()> {
        let mut shutting_down = OPERATION.lock().expect("supervisor operation lock");
        *shutting_down = true;
        self.disconnect_inner()
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
