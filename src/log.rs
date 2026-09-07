use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::paths;

const RING: usize = 200;
const MAX_FILE_BYTES: u64 = 1_500_000;
const TAIL_BYTES: u64 = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Trace => "trace",
        }
    }

    pub fn from_line(line: &str) -> Option<Self> {
        match line.split_whitespace().nth(1)? {
            "error" => Some(Self::Error),
            "warn" => Some(Self::Warn),
            "info" => Some(Self::Info),
            "debug" => Some(Self::Debug),
            "trace" => Some(Self::Trace),
            _ => None,
        }
    }

    fn to_file(self, developer: bool) -> bool {
        match self {
            Self::Error | Self::Warn | Self::Info => true,
            Self::Debug | Self::Trace => developer,
        }
    }

    fn to_stderr(self, developer: bool) -> bool {
        match self {
            Self::Error | Self::Warn => true,
            Self::Info => developer,
            Self::Debug | Self::Trace => false,
        }
    }
}

struct State {
    developer: bool,
    lines: VecDeque<String>,
    file: Option<File>,
    path: Option<PathBuf>,
    generation: u64,
}

static LOG: OnceLock<Mutex<State>> = OnceLock::new();

fn state() -> &'static Mutex<State> {
    LOG.get_or_init(|| Mutex::new(State::open()))
}

fn lock() -> std::sync::MutexGuard<'static, State> {
    state().lock().unwrap_or_else(|err| err.into_inner())
}

impl State {
    fn open() -> Self {
        let path = paths::app_log_path().ok();
        let file = path
            .as_ref()
            .and_then(|path| OpenOptions::new().create(true).append(true).open(path).ok());
        Self {
            developer: env_forced(),
            lines: VecDeque::with_capacity(RING),
            file,
            path,
            generation: 0,
        }
    }

    fn rotate_if_needed(&mut self) {
        let Some(path) = &self.path else {
            return;
        };
        let Ok(meta) = std::fs::metadata(path) else {
            return;
        };
        if meta.len() < MAX_FILE_BYTES {
            return;
        }
        self.file = None;
        let backup = path.with_extension("log.1");
        let _ = std::fs::rename(path, &backup);
        self.file = OpenOptions::new().create(true).append(true).open(path).ok();
    }
}

pub fn init() {
    let _ = state();
}

pub fn env_forced() -> bool {
    matches!(
        std::env::var("MYPROXY_DEV").ok().as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

pub fn set_developer(on: bool) {
    let on = on || env_forced();
    let mut st = lock();
    if st.developer == on {
        return;
    }
    st.developer = on;
    drop(st);
    info(
        "log",
        if on {
            "developer mode on"
        } else {
            "developer mode off"
        },
    );
}

pub fn developer() -> bool {
    lock().developer || env_forced()
}

pub fn path() -> Option<PathBuf> {
    lock().path.clone()
}

pub fn recent(limit: usize) -> Vec<String> {
    if let Some(path) = path() {
        if let Ok(lines) = tail_lines(&path, limit) {
            if !lines.is_empty() {
                return lines;
            }
        }
    }
    let st = lock();
    st.lines.iter().rev().take(limit).rev().cloned().collect()
}

pub fn generation() -> u64 {
    lock().generation
}

pub fn stamp() -> u64 {
    let gen = generation();
    let len = path()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .unwrap_or(0);
    gen.wrapping_add(len)
}

pub fn error(target: &str, msg: impl AsRef<str>) {
    emit(Level::Error, target, msg.as_ref());
}

pub fn warn(target: &str, msg: impl AsRef<str>) {
    emit(Level::Warn, target, msg.as_ref());
}

pub fn info(target: &str, msg: impl AsRef<str>) {
    emit(Level::Info, target, msg.as_ref());
}

pub fn debug(target: &str, msg: impl AsRef<str>) {
    emit(Level::Debug, target, msg.as_ref());
}

pub fn trace(target: &str, msg: impl AsRef<str>) {
    emit(Level::Trace, target, msg.as_ref());
}

fn sanitize(msg: &str) -> String {
    msg.replace(['\n', '\r'], " ")
}

fn format_line(level: Level, target: &str, msg: &str) -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() % 86_400)
        .unwrap_or(0);
    let h = ts / 3600;
    let m = (ts % 3600) / 60;
    let s = ts % 60;
    format!(
        "{h:02}:{m:02}:{s:02}Z {} {target} {}",
        level.as_str(),
        sanitize(msg)
    )
}

fn tail_lines(path: &Path, limit: usize) -> std::io::Result<Vec<String>> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let take = len.min(TAIL_BYTES);
    if take == 0 {
        return Ok(Vec::new());
    }
    file.seek(SeekFrom::End(-(take as i64)))?;
    let mut buf = String::new();
    file.read_to_string(&mut buf)?;
    let mut lines: Vec<String> = buf.lines().map(str::to_string).collect();
    if len > take && !lines.is_empty() {
        lines.remove(0);
    }
    if lines.len() > limit {
        lines.drain(0..lines.len() - limit);
    }
    Ok(lines)
}

#[cfg(unix)]
fn flock_exclusive(file: &File) {
    use std::os::unix::io::AsRawFd;
    unsafe {
        libc::flock(file.as_raw_fd(), libc::LOCK_EX);
    }
}

#[cfg(unix)]
fn flock_unlock(file: &File) {
    use std::os::unix::io::AsRawFd;
    unsafe {
        libc::flock(file.as_raw_fd(), libc::LOCK_UN);
    }
}

#[cfg(not(unix))]
fn flock_exclusive(_: &File) {}

#[cfg(not(unix))]
fn flock_unlock(_: &File) {}

fn emit(level: Level, target: &str, msg: &str) {
    let mut st = lock();
    let developer = st.developer || env_forced();
    if !level.to_file(developer) && !level.to_stderr(developer) {
        return;
    }
    let line = format_line(level, target, msg);
    if level.to_stderr(developer) {
        eprintln!("myproxy {line}");
    }
    if !level.to_file(developer) {
        return;
    }
    st.rotate_if_needed();
    if let Some(file) = &mut st.file {
        flock_exclusive(file);
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
        flock_unlock(file);
    }
    if st.lines.len() == RING {
        st.lines.pop_front();
    }
    st.lines.push_back(line);
    st.generation = st.generation.saturating_add(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_level_target_message() {
        let line = format_line(Level::Warn, "ne-host", "waiting for approval");
        assert!(line.ends_with("Z warn ne-host waiting for approval"), "{line}");
        assert_eq!(Level::from_line(&line), Some(Level::Warn));
    }

    #[test]
    fn sanitizes_newlines() {
        let line = format_line(Level::Error, "ui", "one\ntwo\rthree");
        assert!(!line.contains('\n'));
        assert!(line.ends_with("Z error ui one two three"), "{line}");
    }

    #[test]
    fn tails_last_lines() {
        let dir = std::env::temp_dir().join(format!("myproxy-log-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("myproxy.log");
        let mut file = File::create(&path).unwrap();
        writeln!(file, "00:00:00Z info a one").unwrap();
        writeln!(file, "00:00:01Z warn b two").unwrap();
        writeln!(file, "00:00:02Z error c three").unwrap();
        drop(file);
        let lines = tail_lines(&path, 2).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            lines,
            vec![
                "00:00:01Z warn b two".to_string(),
                "00:00:02Z error c three".to_string(),
            ]
        );
    }
}
