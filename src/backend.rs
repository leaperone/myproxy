use std::fs;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::log;
use crate::paths;

const BACKEND_SCHEMA: u32 = 1;

/// The process that owns proxy listeners and outbound connections.
///
/// Mihomo remains the default. Xray is an opt-in channel stored outside
/// `strategy.json`, so upgrading an existing installation cannot silently
/// change its runtime.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    #[default]
    Mihomo,
    Xray,
}

impl BackendKind {
    pub const ALL: [Self; 2] = [Self::Mihomo, Self::Xray];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mihomo => "mihomo",
            Self::Xray => "xray",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Mihomo => "兼容核心",
            Self::Xray => "Xray 通道",
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "mihomo" | "clash" | "default" => Ok(Self::Mihomo),
            "xray" | "xray-core" => Ok(Self::Xray),
            _ => anyhow::bail!("backend must be mihomo or xray"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct BackendSettings {
    #[serde(default = "schema")]
    schema: u32,
    #[serde(default)]
    backend: BackendKind,
}

fn schema() -> u32 {
    BACKEND_SCHEMA
}

/// Read the opt-in backend selector. Missing or malformed legacy state is
/// treated as Mihomo so an existing installation keeps its old behavior.
pub fn load() -> Result<BackendKind> {
    let path = paths::backend_path()?;
    if !path.exists() {
        return Ok(BackendKind::Mihomo);
    }
    let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    let settings: BackendSettings = match serde_json::from_slice(&bytes) {
        Ok(settings) => settings,
        Err(error) => {
            log::warn(
                "backend",
                format!("ignore invalid {}; keeping Mihomo: {error}", path.display()),
            );
            return Ok(BackendKind::Mihomo);
        }
    };
    if settings.schema > BACKEND_SCHEMA {
        anyhow::bail!(
            "backend settings schema {} is newer than this build",
            settings.schema
        );
    }
    if settings.backend == BackendKind::Xray && !paths::bundled_xray().is_file() {
        log::warn(
            "backend",
            format!(
                "Xray is selected but its binary is missing at {}; keeping Mihomo",
                paths::bundled_xray().display()
            ),
        );
        return Ok(BackendKind::Mihomo);
    }
    Ok(settings.backend)
}

pub fn save(kind: BackendKind) -> Result<()> {
    let path = paths::backend_path()?;
    let settings = BackendSettings {
        schema: BACKEND_SCHEMA,
        backend: kind,
    };
    paths::atomic_write(&path, serde_json::to_string_pretty(&settings)?.as_bytes())?;
    log::info("backend", format!("selected {}", kind.as_str()));
    Ok(())
}

pub fn is_xray() -> bool {
    load().map(|kind| kind == BackendKind::Xray).unwrap_or(false)
}
