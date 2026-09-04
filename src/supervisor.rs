use std::fs;
use std::fs::File;
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::catalog;
use crate::compile;
use crate::log;
use crate::paths;
use crate::strategy::Strategy;

pub struct Supervisor {
    child: Mutex<Option<Child>>,
}

impl Default for Supervisor {
    fn default() -> Self {
        Self {
            child: Mutex::new(None),
        }
    }
}

impl Supervisor {
    pub fn connect(&self, strategy: &Strategy) -> Result<()> {
        log::info("supervisor", format!("connect mixed-port {}", strategy.mixed_port));
        self.disconnect()?;
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
            .status()
            .context("mihomo -t")?;
        if !status.success() {
            log::error("supervisor", "runtime YAML failed mihomo -t");
            bail!("runtime YAML failed mihomo -t");
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
        let deadline = Instant::now() + Duration::from_secs(3);
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
        log::info("supervisor", format!("listening {addr}"));
        Ok(())
    }

    pub fn disconnect(&self) -> Result<()> {
        let mut stopped = false;
        if let Some(mut child) = self.child.lock().expect("supervisor lock").take() {
            let _ = child.kill();
            let _ = child.wait();
            stopped = true;
        }
        if let Ok(path) = paths::pid_path() {
            if let Ok(pid) = fs::read_to_string(&path) {
                if let Ok(pid) = pid.trim().parse::<i32>() {
                    unsafe { libc::kill(pid, libc::SIGTERM); }
                    stopped = true;
                }
                let _ = fs::remove_file(path);
            }
        }
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
