use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::thread;

use anyhow::{bail, Result};
use serde::Serialize;
use uuid::Uuid;

use crate::compile;
use crate::log;
use crate::login_item;
use crate::strategy::Strategy;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnableRequest {
    pub revision: u64,
    pub socks_port: u16,
    pub username: String,
    pub password: String,
    pub process_rules: Vec<ProcessRule>,
    pub group_ports: Vec<GroupPort>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessRule {
    pub pattern: String,
    pub via: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GroupPort {
    pub name: String,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CaptureFace {
    socks_port: u16,
    process_rules: Vec<ProcessRule>,
    group_ports: Vec<GroupPort>,
}

struct Session {
    revision: u64,
    username: String,
    password: String,
    socks_port: u16,
    group_ports: BTreeMap<String, u16>,
    next_port: u16,
    last_face: Option<CaptureFace>,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
static LAST_ENABLE: Mutex<Option<CaptureFace>> = Mutex::new(None);
static LAST_ENABLE_REVISION: Mutex<Option<u64>> = Mutex::new(None);
static OPERATION_REVISION: AtomicU64 = AtomicU64::new(0);
static OPERATION_LOCK: Mutex<()> = Mutex::new(());

pub fn inbound_plan(strategy: &Strategy) -> EnableRequest {
    let socks_port = compile::network_extension_socks_port(strategy.mixed_port);
    let controller = compile::controller_port(strategy.mixed_port);
    let mut session = SESSION.lock().expect("ne session");
    let session = session.get_or_insert_with(|| Session {
        revision: 0,
        username: format!("ne-{}", Uuid::new_v4().simple()),
        password: Uuid::new_v4().simple().to_string(),
        socks_port,
        group_ports: BTreeMap::new(),
        next_port: next_listener_port(
            socks_port.saturating_add(1),
            socks_port,
            controller,
            &BTreeMap::new(),
        ),
        last_face: None,
    });
    if session.socks_port != socks_port {
        session.socks_port = socks_port;
        session.group_ports.clear();
        session.next_port = next_listener_port(
            socks_port.saturating_add(1),
            socks_port,
            controller,
            &BTreeMap::new(),
        );
    }

    let mut process_rules = Vec::new();
    let mut needed = Vec::new();
    for set in &strategy.rule_sets {
        let via = compile::via_target(&set.via, strategy);
        for matcher in &set.matchers {
            if matcher.kind != "app" {
                continue;
            }
            let pattern = matcher.value.trim();
            if pattern.is_empty() {
                continue;
            }
            if !matches!(via.as_str(), "DIRECT" | "REJECT")
                && !needed.iter().any(|name| name == &via)
            {
                needed.push(via.clone());
            }
            process_rules.push(ProcessRule {
                pattern: pattern.to_string(),
                via: via.clone(),
            });
        }
    }

    let mut group_ports = Vec::new();
    for name in needed {
        let port = if let Some(port) = session.group_ports.get(&name).copied() {
            port
        } else {
            let port = next_listener_port(
                session.next_port,
                session.socks_port,
                controller,
                &session.group_ports,
            );
            session.group_ports.insert(name.clone(), port);
            session.next_port = port.saturating_add(1);
            port
        };
        group_ports.push(GroupPort { name, port });
    }

    let face = CaptureFace {
        socks_port,
        process_rules: process_rules.clone(),
        group_ports: group_ports.clone(),
    };
    if session.last_face.as_ref() != Some(&face) {
        session.revision = session.revision.saturating_add(1);
        session.last_face = Some(face);
    }

    EnableRequest {
        revision: session.revision,
        socks_port,
        username: session.username.clone(),
        password: session.password.clone(),
        process_rules,
        group_ports,
    }
}

fn next_listener_port(
    start: u16,
    socks_port: u16,
    controller: u16,
    used: &BTreeMap<String, u16>,
) -> u16 {
    let mut port = start.max(1);
    loop {
        if port != socks_port && port != controller && !used.values().any(|used| *used == port) {
            return port;
        }
        let next = port.saturating_add(1);
        if next == port {
            return port;
        }
        port = next;
    }
}

pub fn enable_async(strategy: &Strategy) {
    let request = inbound_plan(strategy);
    let face = CaptureFace {
        socks_port: request.socks_port,
        process_rules: request.process_rules.clone(),
        group_ports: request.group_ports.clone(),
    };
    {
        let mut last = LAST_ENABLE.lock().expect("ne last enable");
        if last.as_ref() == Some(&face) {
            log::debug("ne", "skip unchanged capture plan");
            return;
        }
        *last = Some(face);
    }
    let revision = OPERATION_REVISION.fetch_add(1, Ordering::SeqCst) + 1;
    *LAST_ENABLE_REVISION.lock().expect("ne last enable revision") = Some(revision);
    thread::Builder::new()
        .name("myproxy-ne".into())
        .spawn(move || {
            let _operation = OPERATION_LOCK.lock().expect("ne operation");
            if OPERATION_REVISION.load(Ordering::SeqCst) != revision {
                let mut last_revision = LAST_ENABLE_REVISION.lock().expect("ne last enable revision");
                if *last_revision == Some(revision) {
                    *last_revision = None;
                    *LAST_ENABLE.lock().expect("ne last enable") = None;
                }
                return;
            }
            if let Err(err) = enable_blocking(&request) {
                if OPERATION_REVISION.load(Ordering::SeqCst) == revision {
                    *LAST_ENABLE.lock().expect("ne last enable") = None;
                    *LAST_ENABLE_REVISION.lock().expect("ne last enable revision") = None;
                }
                log::error("ne", format!("{err:#}"));
            }
        })
        .ok();
}

pub fn disable_async() {
    *LAST_ENABLE.lock().expect("ne last enable") = None;
    *LAST_ENABLE_REVISION.lock().expect("ne last enable revision") = None;
    let revision = OPERATION_REVISION.fetch_add(1, Ordering::SeqCst) + 1;
    thread::Builder::new()
        .name("myproxy-ne-stop".into())
        .spawn(move || {
            let _operation = OPERATION_LOCK.lock().expect("ne operation");
            if OPERATION_REVISION.load(Ordering::SeqCst) != revision {
                return;
            }
            if let Err(err) = disable_blocking() {
                log::error("ne", format!("{err:#}"));
            }
        })
        .ok();
}

fn enable_blocking(request: &EnableRequest) -> Result<()> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = request;
        bail!("System Extension is macOS-only");
    }
    #[cfg(target_os = "macos")]
    {
        if !login_item::is_bundled() {
            log::warn(
                "ne",
                "system extension needs the bundled .app (Contents/Library/SystemExtensions)",
            );
            return Ok(());
        }
        let json = serde_json::to_string(request).expect("ne request json");
        log::info(
            "ne",
            format!(
                "enable revision={} socks={} process_rules={} group_routes={}",
                request.revision,
                request.socks_port,
                request.process_rules.len(),
                request.group_ports.len()
            ),
        );
        for rule in &request.process_rules {
            log::info("ne", format!("capture {} via {}", rule.pattern, rule.via));
        }
        let mut error = std::ptr::null_mut();
        let rc = unsafe { ffi::myproxy_ne_enable(c_string(&json).as_ptr(), &mut error) };
        let message = take_error(error);
        match rc {
            0 => {
                log::info("ne", "system extension running");
                Ok(())
            }
            2 => {
                log::warn("ne", "system extension requires reboot");
                Ok(())
            }
            _ => bail!(
                "{}",
                message.unwrap_or_else(|| "system extension enable failed".into())
            ),
        }
    }
}

fn disable_blocking() -> Result<()> {
    #[cfg(not(target_os = "macos"))]
    {
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        let mut error = std::ptr::null_mut();
        let rc = unsafe { ffi::myproxy_ne_disable(&mut error) };
        let message = take_error(error);
        if rc == 0 {
            log::info("ne", "system extension stopped");
            Ok(())
        } else {
            bail!(
                "{}",
                message.unwrap_or_else(|| "system extension disable failed".into())
            )
        }
    }
}

#[cfg(target_os = "macos")]
fn c_string(value: &str) -> std::ffi::CString {
    std::ffi::CString::new(value.replace('\0', "")).expect("c string")
}

#[cfg(target_os = "macos")]
fn take_error(ptr: *mut std::os::raw::c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let message = unsafe { std::ffi::CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned();
    unsafe { ffi::myproxy_ne_free_string(ptr) };
    Some(message)
}

#[cfg(target_os = "macos")]
mod ffi {
    use std::os::raw::c_char;

    unsafe extern "C" {
        pub fn myproxy_ne_enable(
            json: *const c_char,
            error_out: *mut *mut c_char,
        ) -> i32;
        pub fn myproxy_ne_disable(error_out: *mut *mut c_char) -> i32;
        pub fn myproxy_ne_free_string(value: *mut c_char);
    }
}
