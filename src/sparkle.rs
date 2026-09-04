#[cfg(all(target_os = "macos", feature = "sparkle"))]
extern "C" {
    fn myproxy_sparkle_init();
    fn myproxy_sparkle_check();
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
