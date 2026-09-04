use std::path::PathBuf;

use anyhow::{Result, bail};

/// `Some` when this process is `*.app/Contents/MacOS/<bin>`.
pub fn app_bundle_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let exe = exe.canonicalize().unwrap_or(exe);
    let macos = exe.parent()?;
    if macos.file_name()?.to_str()? != "MacOS" {
        return None;
    }
    let contents = macos.parent()?;
    if contents.file_name()?.to_str()? != "Contents" {
        return None;
    }
    let app = contents.parent()?;
    if app.extension().and_then(|e| e.to_str()) != Some("app") {
        return None;
    }
    Some(app.to_path_buf())
}

pub fn is_bundled() -> bool {
    app_bundle_path().is_some()
}

/// Apply `launch_at_login` via `SMAppService`. No-op when turning off outside a bundle.
pub fn sync(enable: bool) -> Result<()> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = enable;
        return Ok(());
    }
    #[cfg(target_os = "macos")]
    {
        if !is_bundled() {
            if enable {
                bail!("开机启动需要安装为 .app，例如 ~/Applications/myproxy.app");
            }
            return Ok(());
        }
        smapp_set(enable)
    }
}

#[cfg(target_os = "macos")]
fn smapp_set(enable: bool) -> Result<()> {
    use objc::runtime::{BOOL, Object};
    use objc::{class, msg_send, sel, sel_impl};

    #[link(name = "ServiceManagement", kind = "framework")]
    extern "C" {}

    const NOT_REGISTERED: isize = 0;
    const ENABLED: isize = 1;
    const REQUIRES_APPROVAL: isize = 2;
    const NOT_FOUND: isize = 3;

    unsafe {
        let service: *mut Object = msg_send![class!(SMAppService), mainAppService];
        if service.is_null() {
            bail!("SMAppService unavailable");
        }
        let status: isize = msg_send![service, status];
        if enable {
            if status == ENABLED {
                return Ok(());
            }
            let mut err: *mut Object = std::ptr::null_mut();
            let ok: BOOL = msg_send![service, registerAndReturnError: &mut err];
            if ok == 0 {
                bail!("{}", nserror_message(err));
            }
            let status: isize = msg_send![service, status];
            if status == REQUIRES_APPROVAL {
                crate::log::warn(
                    "login",
                    "login item waiting for System Settings approval",
                );
            }
        } else if status != NOT_REGISTERED && status != NOT_FOUND {
            let mut err: *mut Object = std::ptr::null_mut();
            let ok: BOOL = msg_send![service, unregisterAndReturnError: &mut err];
            if ok == 0 {
                bail!("{}", nserror_message(err));
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
unsafe fn nserror_message(err: *mut objc::runtime::Object) -> String {
    use std::ffi::CStr;

    use objc::runtime::Object;
    use objc::{msg_send, sel, sel_impl};

    if err.is_null() {
        return "unknown login item error".into();
    }
    let desc: *mut Object = msg_send![err, localizedDescription];
    if desc.is_null() {
        return "unknown login item error".into();
    }
    let utf8: *const i8 = msg_send![desc, UTF8String];
    if utf8.is_null() {
        return "unknown login item error".into();
    }
    CStr::from_ptr(utf8).to_string_lossy().into_owned()
}
