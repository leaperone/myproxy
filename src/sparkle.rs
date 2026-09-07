use myproxy::supervisor::Supervisor;
use myproxy::updates::UpdateChannel;

#[cfg(all(target_os = "macos", feature = "sparkle"))]
extern "C" {
    fn myproxy_sparkle_init();
    fn myproxy_sparkle_check();
    fn myproxy_sparkle_set_channel(feed_url: *const std::os::raw::c_char, nightly: i32);
    fn myproxy_sparkle_set_proxy(host: *const std::os::raw::c_char, port: i32);
}

pub fn available() -> bool {
    cfg!(all(target_os = "macos", feature = "sparkle"))
}

pub fn init() {
    Supervisor::shared().set_update_proxy_hook(set_proxy);
    sync_proxy();
    #[cfg(all(target_os = "macos", feature = "sparkle"))]
    unsafe {
        myproxy_sparkle_init();
    }
}

pub fn set_channel(channel: UpdateChannel) {
    #[cfg(all(target_os = "macos", feature = "sparkle"))]
    unsafe {
        let url = std::ffi::CString::new(channel.feed_url()).expect("update feed URL");
        myproxy_sparkle_set_channel(url.as_ptr(), i32::from(channel == UpdateChannel::Nightly));
    }
    #[cfg(not(all(target_os = "macos", feature = "sparkle")))]
    let _ = channel;
}

pub fn check() {
    if !available() {
        myproxy::log::info("sparkle", "updater not linked in this build");
        return;
    }
    sync_proxy();
    #[cfg(all(target_os = "macos", feature = "sparkle"))]
    unsafe {
        myproxy_sparkle_check();
    }
    myproxy::log::info("sparkle", "check for updates");
}

fn sync_proxy() {
    set_proxy(Supervisor::shared().update_download_port());
}

fn set_proxy(port: Option<u16>) {
    static LAST: std::sync::Mutex<Option<Option<u16>>> = std::sync::Mutex::new(None);
    {
        let mut last = LAST.lock().expect("sparkle proxy");
        if *last == Some(port) {
            return;
        }
        *last = Some(port);
    }
    #[cfg(all(target_os = "macos", feature = "sparkle"))]
    {
        match port {
            Some(port) => {
                let host = std::ffi::CString::new("127.0.0.1").expect("loopback host");
                unsafe {
                    myproxy_sparkle_set_proxy(host.as_ptr(), i32::from(port));
                }
                myproxy::log::info("sparkle", format!("update downloads via 127.0.0.1:{port}"));
            }
            None => {
                unsafe {
                    myproxy_sparkle_set_proxy(std::ptr::null(), 0);
                }
                myproxy::log::debug("sparkle", "update downloads without mixed proxy");
            }
        }
    }
    #[cfg(not(all(target_os = "macos", feature = "sparkle")))]
    let _ = port;
}
