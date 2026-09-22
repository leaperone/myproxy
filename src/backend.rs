use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    #[default]
    Mihomo,
    Xray,
}

impl BackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mihomo => "mihomo",
            Self::Xray => "xray",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Mihomo => "Mihomo",
            Self::Xray => "Xray",
        }
    }
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim() {
            "mihomo" => Ok(Self::Mihomo),
            "xray" => Ok(Self::Xray),
            _ => bail!("backend must be mihomo or xray"),
        }
    }
}

pub const fn is_xray() -> bool {
    cfg!(feature = "xray-channel")
}

pub fn load() -> Result<BackendKind> {
    Ok(if is_xray() {
        BackendKind::Xray
    } else {
        BackendKind::Mihomo
    })
}

pub fn save(kind: BackendKind) -> Result<()> {
    if kind != load()? {
        bail!("运行内核由发布通道决定。请安装对应通道，Xray 配置独立保存。");
    }
    Ok(())
}
