use std::fs;
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::log;
use crate::paths;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ServiceSnapshot {
    name: String,
    http: ProxyState,
    https: ProxyState,
    socks: ProxyState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ProxyState {
    enabled: bool,
    server: String,
    port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct RestoreFile {
    services: Vec<ServiceSnapshot>,
}

fn restore_path() -> Result<std::path::PathBuf> {
    Ok(paths::data_dir()?.join("system-proxy-restore.json"))
}

pub fn apply(mixed_port: u16) -> Result<()> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = mixed_port;
        bail!("system proxy is macOS-only");
    }
    #[cfg(target_os = "macos")]
    {
        if mixed_port == 0 {
            bail!("mixed port is required");
        }
        let services = list_enabled_services()?;
        if !restore_path()?.exists() {
            let snapshot = RestoreFile {
                services: services
                    .iter()
                    .map(|name| snapshot_service(name))
                    .collect::<Result<Vec<_>>>()?,
            };
            paths::atomic_write(
                &restore_path()?,
                serde_json::to_string_pretty(&snapshot)?.as_bytes(),
            )?;
        }
        for name in services {
            set_proxy(&name, "web", "127.0.0.1", mixed_port)?;
            set_proxy(&name, "secureweb", "127.0.0.1", mixed_port)?;
            set_proxy(&name, "socksfirewall", "127.0.0.1", mixed_port)?;
        }
        log::info("system-proxy", format!("pointed system proxy at Mixed {mixed_port}"));
        Ok(())
    }
}

pub fn restore() -> Result<bool> {
    #[cfg(not(target_os = "macos"))]
    {
        return Ok(false);
    }
    #[cfg(target_os = "macos")]
    {
        let path = restore_path()?;
        if !path.exists() {
            return Ok(false);
        }
        let data = fs::read_to_string(&path).context("read system proxy restore file")?;
        let snapshot: RestoreFile =
            serde_json::from_str(&data).context("parse system proxy restore file")?;
        for service in snapshot.services {
            restore_proxy(&service.name, "web", &service.http)?;
            restore_proxy(&service.name, "secureweb", &service.https)?;
            restore_proxy(&service.name, "socksfirewall", &service.socks)?;
        }
        let _ = fs::remove_file(&path);
        log::info("system-proxy", "restored previous system proxy");
        Ok(true)
    }
}

pub fn sync(enabled: bool, mixed_port: u16) -> Result<()> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (enabled, mixed_port);
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    if enabled {
        apply(mixed_port)
    } else {
        restore().map(|_| ())
    }
}

#[cfg(target_os = "macos")]
fn list_enabled_services() -> Result<Vec<String>> {
    let output = Command::new("networksetup")
        .arg("-listallnetworkservices")
        .output()
        .context("networksetup -listallnetworkservices")?;
    if !output.status.success() {
        bail!(
            "list network services failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .skip(1)
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('*'))
        .map(str::to_string)
        .collect())
}

#[cfg(target_os = "macos")]
fn snapshot_service(name: &str) -> Result<ServiceSnapshot> {
    Ok(ServiceSnapshot {
        name: name.to_string(),
        http: read_proxy(name, "web")?,
        https: read_proxy(name, "secureweb")?,
        socks: read_proxy(name, "socksfirewall")?,
    })
}

#[cfg(target_os = "macos")]
fn read_proxy(service: &str, kind: &str) -> Result<ProxyState> {
    let output = Command::new("networksetup")
        .arg(format!("-get{kind}proxy"))
        .arg(service)
        .output()
        .with_context(|| format!("networksetup -get{kind}proxy"))?;
    if !output.status.success() {
        return Ok(ProxyState {
            enabled: false,
            server: String::new(),
            port: 0,
        });
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut enabled = false;
    let mut server = String::new();
    let mut port = 0u16;
    for line in text.lines() {
        let (key, value) = line.split_once(':').unwrap_or((line, ""));
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        if key == "enabled" {
            enabled = value.eq_ignore_ascii_case("yes");
        } else if key == "server" {
            server = value.to_string();
        } else if key == "port" {
            port = value.parse().unwrap_or(0);
        }
    }
    Ok(ProxyState {
        enabled,
        server,
        port,
    })
}

#[cfg(target_os = "macos")]
fn set_proxy(service: &str, kind: &str, host: &str, port: u16) -> Result<()> {
    let status = Command::new("networksetup")
        .arg(format!("-set{kind}proxy"))
        .arg(service)
        .arg(host)
        .arg(port.to_string())
        .status()
        .with_context(|| format!("networksetup -set{kind}proxy"))?;
    if !status.success() {
        bail!("set {kind} proxy failed for {service}");
    }
    let status = Command::new("networksetup")
        .arg(format!("-set{kind}proxystate"))
        .arg(service)
        .arg("on")
        .status()
        .with_context(|| format!("networksetup -set{kind}proxystate"))?;
    if !status.success() {
        bail!("enable {kind} proxy failed for {service}");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn restore_proxy(service: &str, kind: &str, state: &ProxyState) -> Result<()> {
    if state.enabled && !state.server.is_empty() && state.port > 0 {
        set_proxy(service, kind, &state.server, state.port)?;
    } else {
        let _ = Command::new("networksetup")
            .arg(format!("-set{kind}proxystate"))
            .arg(service)
            .arg("off")
            .status();
    }
    Ok(())
}
