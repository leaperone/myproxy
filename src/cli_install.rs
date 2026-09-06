use std::path::PathBuf;

use anyhow::{bail, Context, Result};

pub fn destination() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".cargo/bin/myproxyctl"))
}

fn bundled_cli() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let macos = exe.parent()?;
    let cli = macos.join("myproxyctl");
    cli.is_file().then_some(cli)
}

/// True when this process is a `.app` bundle that ships `myproxyctl`.
pub fn available() -> bool {
    bundled_cli().is_some() && crate::login_item::is_bundled()
}

pub fn is_installed() -> bool {
    let Some(source) = bundled_cli() else {
        return false;
    };
    let Some(link) = destination() else {
        return false;
    };
    std::fs::read_link(&link)
        .ok()
        .and_then(|target| {
            let target = if target.is_absolute() {
                target
            } else {
                link.parent()?.join(target)
            };
            Some(target == source)
        })
        .unwrap_or(false)
}

pub fn install() -> Result<PathBuf> {
    let source = bundled_cli().context("当前应用包没有 myproxyctl")?;
    let link = destination().context("无法确定当前用户的主目录")?;
    let parent = link.parent().context("无效的 CLI 安装路径")?;
    std::fs::create_dir_all(parent).with_context(|| format!("创建 {}", parent.display()))?;
    match std::fs::symlink_metadata(&link) {
        Ok(metadata) if metadata.file_type().is_symlink() => std::fs::remove_file(&link)?,
        Ok(_) => bail!("{} 已存在且不是符号链接，请先移走它", link.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).with_context(|| format!("检查 {}", link.display())),
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&source, &link)
        .with_context(|| format!("安装 {}", link.display()))?;
    #[cfg(not(unix))]
    bail!("当前平台不支持安装命令行工具");
    Ok(link)
}
