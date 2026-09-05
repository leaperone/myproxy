use myproxy::updates::UpdateChannel;

#[cfg(all(target_os = "macos", feature = "sparkle"))]
extern "C" {
    fn myproxy_sparkle_init();
    fn myproxy_sparkle_check();
    fn myproxy_sparkle_set_channel(feed_url: *const std::os::raw::c_char, nightly: i32);
}

pub fn available() -> bool {
    cfg!(all(target_os = "macos", feature = "sparkle"))
}

pub fn init() {
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
    #[cfg(all(target_os = "macos", feature = "sparkle"))]
    unsafe {
        myproxy_sparkle_check();
    }
    myproxy::log::info("sparkle", "check for updates");
}
