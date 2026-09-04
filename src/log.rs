use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::paths;

const RING: usize = 200;
const MAX_FILE_BYTES: u64 = 1_500_000;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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
        let file = path.as_ref().and_then(|path| {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok()
        });
        Self {
            developer: env_forced(),
            lines: VecDeque::with_capacity(RING),
            file,
            path,
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
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok();
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
    let st = lock();
    st.lines.iter().rev().take(limit).rev().cloned().collect()
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

fn emit(level: Level, target: &str, msg: &str) {
    let mut st = lock();
    let developer = st.developer || env_forced();
    if !level.to_file(developer) && !level.to_stderr(developer) {
        return;
    }
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() % 86_400)
        .unwrap_or(0);
    let h = ts / 3600;
    let m = (ts % 3600) / 60;
    let s = ts % 60;
    let line = format!(
        "{h:02}:{m:02}:{s:02}Z {} {target} {msg}",
        level.as_str()
    );
    if level.to_stderr(developer) {
        eprintln!("myproxy {line}");
    }
    if !level.to_file(developer) {
        return;
    }
    st.rotate_if_needed();
    if let Some(file) = &mut st.file {
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
    }
    if st.lines.len() == RING {
        st.lines.pop_front();
    }
    st.lines.push_back(line);
}
