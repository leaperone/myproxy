use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

pub fn data_dir() -> Result<PathBuf> {
    let dir = dirs::data_dir()
        .context("no application support directory")?
        .join("myproxy");
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    Ok(dir)
}

pub fn strategy_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("strategy.json"))
}

pub fn catalog_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("catalog.json"))
}

pub fn runtime_yaml_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("runtime.yaml"))
}

pub fn pid_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("mihomo.pid"))
}

pub fn wanted_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("core.wanted"))
}

pub fn operation_lock_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("operation.lock"))
}

pub fn mihomo_log_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("mihomo.log"))
}

pub fn app_log_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("myproxy.log"))
}

pub fn onboard_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("onboard.json"))
}

pub fn bundled_mihomo() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    let exe = fs::canonicalize(&exe).unwrap_or(exe);
    let next_to_exe = exe.parent().map(|p| p.join("mihomo")).unwrap_or_default();
    if next_to_exe.is_file() {
        return next_to_exe;
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/mihomo/mihomo")
}
