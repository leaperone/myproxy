//! Ephemeral Xray-only handoff for a Sparkle relaunch.
//!
//! The marker is intentionally separate from the saved strategy and runtime
//! state. It records only that a connected Xray session should be restored
//! after an updater relaunch; ordinary quit and manual disconnect never write
//! it.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const VERSION: u8 = 1;
const TTL: Duration = Duration::from_secs(10 * 60);
const FILE_NAME: &str = "xray-update-resume.json";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Marker {
    version: u8,
    wanted: bool,
    created_unix: u64,
    expires_unix: u64,
}

fn marker_path(root: &Path) -> PathBuf { root.join(FILE_NAME) }

fn now_unix() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

fn write_marker_at(root: &Path, now: u64) -> Result<()> {
    fs::create_dir_all(root).with_context(|| format!("create {}", root.display()))?;
    let marker = Marker { version: VERSION, wanted: true, created_unix: now, expires_unix: now.saturating_add(TTL.as_secs()) };
    let path = marker_path(root);
    let bytes = serde_json::to_vec(&marker)?;
    crate::paths::atomic_write(&path, &bytes)?;
    Ok(())
}

fn consume_at(root: &Path, now: u64) -> Result<bool> {
    let path = marker_path(root);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    fs::remove_file(&path)?;
    let marker: Marker = match serde_json::from_slice(&bytes) {
        Ok(marker) => marker,
        Err(_) => return Ok(false),
    };
    Ok(marker.version == VERSION && marker.wanted && marker.created_unix <= now && now <= marker.expires_unix)
}

/// Called by the Sparkle relaunch callback. This is a no-op for non-Xray
/// builds and while Xray is not currently connected.
pub fn mark_if_wanted() -> Result<()> {
    if !crate::backend::is_xray() { return Ok(()); }
    if !crate::xray::wanted() { return cancel(); }
    write_marker_at(&crate::paths::data_dir()?, now_unix()?)
}

pub fn cancel() -> Result<()> {
    match fs::remove_file(marker_path(&crate::paths::data_dir()?)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Consumes the one-shot relaunch marker. A malformed or expired marker is
/// removed and treated as absent.
pub fn consume_if_fresh() -> Result<bool> {
    if !crate::backend::is_xray() { return Ok(false); }
    consume_at(&crate::paths::data_dir()?, now_unix()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_is_consumed_once() {
        let root = std::env::temp_dir().join(format!("myproxy-update-resume-{}", uuid::Uuid::new_v4()));
        write_marker_at(&root, 100).unwrap();
        assert!(consume_at(&root, 101).unwrap());
        assert!(!consume_at(&root, 101).unwrap());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn disconnected_or_expired_marker_is_not_resumed() {
        let root = std::env::temp_dir().join(format!("myproxy-update-resume-{}", uuid::Uuid::new_v4()));
        write_marker_at(&root, 100).unwrap();
        let bytes = fs::read(marker_path(&root)).unwrap();
        let mut marker: Marker = serde_json::from_slice(&bytes).unwrap();
        marker.wanted = false;
        fs::write(marker_path(&root), serde_json::to_vec(&marker).unwrap()).unwrap();
        assert!(!consume_at(&root, 101).unwrap());
        write_marker_at(&root, 100).unwrap();
        assert!(!consume_at(&root, 100 + TTL.as_secs() + 1).unwrap());
        assert!(!marker_path(&root).exists());
        let _ = fs::remove_dir_all(root);
    }
}
