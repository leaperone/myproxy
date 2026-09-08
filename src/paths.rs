use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub fn data_dir() -> Result<PathBuf> {
    let dir = match std::env::var_os("MYPROXY_DATA_DIR") {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => dirs::data_dir()
            .context("no application support directory")?
            .join("myproxy"),
    };
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

pub fn runtime_state_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("runtime-state.json"))
}

pub fn candidate_yaml_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("runtime.candidate.yaml"))
}

/// Runtime files contain subscription credentials. Never publish a partial or
/// world-readable candidate, and keep the replacement on the same filesystem.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .context("create runtime candidate")?;
        file.write_all(bytes).context("write runtime candidate")?;
        file.sync_all().context("flush runtime candidate")?;
        fs::rename(&temporary, path).context("publish runtime file")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub fn ruleset_dir() -> Result<PathBuf> {
    let dir = data_dir()?.join("ruleset");
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    Ok(dir)
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
