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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessRule {
    pub pattern: String,
    pub via: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GroupPort {
    pub name: String,
    pub port: u16,
}

struct Session {
    revision: u64,
    username: String,
    password: String,
}

static SESSION: Mutex<Option<Session>> = Mutex::new(None);

pub fn inbound_plan(strategy: &Strategy) -> EnableRequest {
    let mut session = SESSION.lock().expect("ne session");
    let next = match session.as_mut() {
        Some(existing) => {
            existing.revision = existing.revision.saturating_add(1);
            existing.revision
        }
        None => {
            *session = Some(Session {
                revision: 1,
                username: format!("ne-{}", Uuid::new_v4().simple()),
                password: Uuid::new_v4().simple().to_string(),
            });
            1
        }
    };
    let creds = session.as_ref().expect("session");
    let socks_port = compile::network_extension_socks_port(strategy.mixed_port);
    let mut process_rules = Vec::new();
    let mut group_ports = Vec::new();
    let mut next_port = socks_port.saturating_add(1);
    let controller = compile::controller_port(strategy.mixed_port);
    for set in &strategy.rule_sets {
        let via = compile::via_target(&set.via, strategy);
        let needs_group = !matches!(via.as_str(), "DIRECT" | "REJECT");
        if needs_group && !group_ports.iter().any(|g: &GroupPort| g.name == via) {
            if next_port == controller {
                next_port = next_port.saturating_add(1);
            }
            group_ports.push(GroupPort {
                name: via.clone(),
                port: next_port,
            });
            next_port = next_port.saturating_add(1);
        }
        for matcher in &set.matchers {
            if matcher.kind != "app" {
                continue;
            }
            let pattern = matcher.value.trim();
            if pattern.is_empty() {
                continue;
            }
            process_rules.push(ProcessRule {
                pattern: pattern.to_string(),
                via: via.clone(),
            });
        }
    }
    EnableRequest {
        revision: next,
        socks_port,
        username: creds.username.clone(),
        password: creds.password.clone(),
        process_rules,
        group_ports,
    }
}

pub fn enable_async(strategy: &Strategy) {
    let request = inbound_plan(strategy);
    thread::Builder::new()
        .name("myproxy-ne".into())
        .spawn(move || {
            if let Err(err) = enable_blocking(&request) {
                log::error("ne", format!("{err:#}"));
            }
        })
        .ok();
}

pub fn disable_async() {
    thread::Builder::new()
        .name("myproxy-ne-stop".into())
        .spawn(|| {
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
