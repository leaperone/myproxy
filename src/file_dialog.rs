use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, Context, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileDialogChoice {
    Path(PathBuf),
    Cancelled,
}

pub fn save_strategy(default_name: &str) -> Result<FileDialogChoice> {
    native_save(default_name)
}

pub fn open_strategy() -> Result<FileDialogChoice> {
    native_open()
}

#[cfg(target_os = "macos")]
fn native_save(default_name: &str) -> Result<FileDialogChoice> {
    let script = format!(
        "try\nset theFile to choose file name with prompt \"导出 myproxy 配置\" default name {} default location (path to downloads folder)\nreturn POSIX path of theFile\non error number -128\nreturn \"CANCELLED\"\nend try",
        applescript_string(default_name)
    );
    run_osascript(&script)
}

#[cfg(target_os = "macos")]
fn native_open() -> Result<FileDialogChoice> {
    let script = "try\nset theFile to choose file with prompt \"导入 myproxy 配置\"\nreturn POSIX path of theFile\non error number -128\nreturn \"CANCELLED\"\nend try";
    run_osascript(script)
}

#[cfg(not(target_os = "macos"))]
fn native_save(_default_name: &str) -> Result<FileDialogChoice> {
    bail!("file dialogs are only available on macOS")
}

#[cfg(not(target_os = "macos"))]
fn native_open() -> Result<FileDialogChoice> {
    bail!("file dialogs are only available on macOS")
}

#[cfg(target_os = "macos")]
fn run_osascript(script: &str) -> Result<FileDialogChoice> {
    let output = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .context("osascript")?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout == "CANCELLED" {
        return Ok(FileDialogChoice::Cancelled);
    }
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("User canceled") || stderr.contains("-128") {
            return Ok(FileDialogChoice::Cancelled);
        }
        bail!("file dialog failed: {}", stderr.trim());
    }
    if stdout.is_empty() {
        return Ok(FileDialogChoice::Cancelled);
    }
    Ok(FileDialogChoice::Path(PathBuf::from(stdout)))
}

#[cfg(target_os = "macos")]
fn applescript_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
