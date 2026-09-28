use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use myproxy::supervisor::Supervisor;
use myproxy::updates::{
    archive_filename, decode_asset_query, fetch_release_bytes_with_progress,
    fetch_release_to_temp_with_progress, open_release,
    release_url_host, rewrite_appcast_enclosures, ReleasePath, ReleaseProgress, UpdateChannel,
};

#[cfg(all(target_os = "macos", feature = "sparkle"))]
extern "C" {
    fn myproxy_sparkle_init();
    fn myproxy_sparkle_check();
    fn myproxy_sparkle_set_channel(feed_url: *const std::os::raw::c_char, nightly: i32);
}

#[cfg(all(target_os = "macos", feature = "sparkle"))]
#[no_mangle]
pub extern "C" fn myproxy_sparkle_mark_update_resume() {
    if let Err(error) = myproxy::xray::update_resume::mark_if_wanted() {
        myproxy::log::warn("sparkle", format!("write Xray update resume marker failed: {error:#}"));
    }
}

static FEED_PORT: AtomicU16 = AtomicU16::new(0);
static REMOTE_FEED: Mutex<Option<String>> = Mutex::new(None);
static SNAPSHOT: Mutex<UpdateTransportSnapshot> = Mutex::new(UpdateTransportSnapshot::idle());

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateTransportSnapshot {
    pub operation: u64,
    pub phase: &'static str,
    pub route: &'static str,
    pub detail: String,
    pub fallback_reason: Option<String>,
}

impl UpdateTransportSnapshot {
    const fn idle() -> Self {
        Self { operation: 0, phase: "idle", route: "none", detail: String::new(), fallback_reason: None }
    }
}

pub fn snapshot() -> UpdateTransportSnapshot {
    SNAPSHOT.lock().map(|snapshot| snapshot.clone()).unwrap_or_else(|_| UpdateTransportSnapshot::idle())
}

pub fn status() -> String {
    snapshot().detail
}

fn set_status(message: impl Into<String>) {
    let current = snapshot();
    if current.phase == "idle" {
        set_transport(current.operation, "idle", "none", message);
    }
}

fn accepts(snapshot: &UpdateTransportSnapshot, operation: u64) -> bool {
    snapshot.operation == operation && snapshot.phase != "cancelled"
}

#[cfg(all(target_os = "macos", feature = "sparkle"))]
#[no_mangle]
pub extern "C" fn myproxy_sparkle_begin_check() -> u64 {
    let mut snapshot = SNAPSHOT.lock().expect("update status");
    let operation = snapshot.operation.saturating_add(1);
    *snapshot = UpdateTransportSnapshot {
        operation, phase: "checking", route: "none",
        detail: "正在检查更新，优先使用当前代理，失败后尝试直连。".into(),
        fallback_reason: None,
    };
    operation
}

fn set_transport(operation: u64, phase: &'static str, route: &'static str, detail: impl Into<String>) {
    let detail = detail.into();
    if let Ok(mut snapshot) = SNAPSHOT.lock() {
        if !accepts(&snapshot, operation) { return; }
        if snapshot.phase != phase || snapshot.route != route {
            if phase == "fallback" || phase == "error" {
                myproxy::log::warn("updates", &detail);
            } else {
                myproxy::log::info("updates", &detail);
            }
        }
        let fallback_reason = if route == "direct-fallback" || phase == "error" {
            snapshot.fallback_reason.clone()
        } else { None };
        *snapshot = UpdateTransportSnapshot { operation, phase, route, detail, fallback_reason };
    }
}

fn note_fallback(operation: u64, reason: &str) {
    if let Ok(mut snapshot) = SNAPSHOT.lock() {
        if accepts(&snapshot, operation) { snapshot.fallback_reason = Some(reason.to_string()); }
    }
}

#[cfg(all(target_os = "macos", feature = "sparkle"))]
#[no_mangle]
pub unsafe extern "C" fn myproxy_sparkle_event(operation: u64, event: i32, value: *const std::os::raw::c_char) {
    let value = if value.is_null() { String::new() } else {
        std::ffi::CStr::from_ptr(value).to_string_lossy().chars().take(160).collect()
    };
    let current = snapshot();
    if !accepts(&current, operation) { return; }
    let (phase, message) = match event {
        1 => ("available", format!("发现新版本 {value}，可以下载更新。")),
        2 => ("no-update", "当前通道没有可用更新。".into()),
        3 => ("validating", "更新包已交给更新器，等待校验。".into()),
        4 => ("installing", "更新器正在安装已校验的更新。".into()),
        5 if current.phase != "error" => ("error", format!("更新器中止操作：{value}。请重试检查更新。")),
        6 => ("cancelled", "更新已取消，当前版本继续运行。".into()),
        _ => return,
    };
    set_transport(operation, phase, current.route, message);
}

fn route_name(path: ReleasePath) -> &'static str {
    match path {
        ReleasePath::Proxy => "proxy",
        ReleasePath::Direct => "direct",
        ReleasePath::DirectFallback => "direct-fallback",
    }
}

pub fn available() -> bool {
    cfg!(all(target_os = "macos", feature = "sparkle"))
}

pub fn init() {
    Supervisor::shared().set_update_proxy_hook(note_mixed_port);
    start_local_feed();
    note_mixed_port(Supervisor::shared().update_download_port());
    #[cfg(all(target_os = "macos", feature = "sparkle"))]
    unsafe {
        myproxy_sparkle_init();
    }
}

pub fn set_channel(channel: UpdateChannel) {
    let remote = channel.feed_url().to_string();
    let channel_id = match channel {
        UpdateChannel::Prod => 0,
        UpdateChannel::Nightly => 1,
        UpdateChannel::Xray => 2,
    };
    *REMOTE_FEED.lock().expect("sparkle feed") = Some(remote);
    let port = start_local_feed();
    let local = format!("http://127.0.0.1:{port}/appcast.xml");
    #[cfg(all(target_os = "macos", feature = "sparkle"))]
    unsafe {
        let url = std::ffi::CString::new(local).expect("update feed URL");
        myproxy_sparkle_set_channel(url.as_ptr(), channel_id);
    }
    #[cfg(not(all(target_os = "macos", feature = "sparkle")))]
    let _ = (local, channel_id);
}

pub fn check() {
    if !available() {
        set_status("此构建没有应用内更新器。");
        myproxy::log::info("sparkle", "updater not linked in this build");
        return;
    }
    #[cfg(all(target_os = "macos", feature = "sparkle"))]
    unsafe {
        myproxy_sparkle_check();
    }
    myproxy::log::info("sparkle", "check for updates");
}

fn note_mixed_port(port: Option<u16>) {
    static LAST: Mutex<Option<Option<u16>>> = Mutex::new(None);
    {
        let mut last = LAST.lock().expect("sparkle proxy");
        if *last == Some(port) {
            return;
        }
        *last = Some(port);
    }
    match port {
        Some(port) => {
            set_status(format!("更新下载通道：当前代理 127.0.0.1:{port}，失败后直连回落。"));
            myproxy::log::info(
                "sparkle",
                format!("update downloads via mixed http 127.0.0.1:{port}"),
            )
        }
        None => {
            set_status("更新下载通道：当前未连接，使用直连。");
            myproxy::log::debug("sparkle", "update downloads without mixed proxy")
        }
    }
}

fn start_local_feed() -> u16 {
    static ONCE: OnceLock<u16> = OnceLock::new();
    *ONCE.get_or_init(|| {
        let listener = TcpListener::bind("127.0.0.1:0").expect("sparkle local feed");
        let port = listener.local_addr().expect("sparkle local addr").port();
        FEED_PORT.store(port, Ordering::Relaxed);
        std::thread::Builder::new()
            .name("sparkle-feed".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    if let Ok(stream) = stream {
                        std::thread::spawn(move || {
                            let _ = handle_feed_conn(stream);
                        });
                    }
                }
            })
            .expect("sparkle feed thread");
        myproxy::log::info("sparkle", format!("local update feed on 127.0.0.1:{port}"));
        port
    })
}

fn handle_feed_conn(mut stream: std::net::TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(15)))?;
    stream.set_write_timeout(Some(Duration::from_secs(900)))?;
    let (method, target) = match read_http_line(&mut stream) {
        Some(pair) => pair,
        None => return Ok(()),
    };
    let path = target.split('?').next().unwrap_or("");
    let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
    let Some(operation) = query.split('&').find_map(|part| part.strip_prefix("operation=")?.parse::<u64>().ok()) else {
        return write_http(&mut stream, 400, "text/plain", b"missing update operation");
    };
    if !accepts(&snapshot(), operation) {
        return write_http(&mut stream, 409, "text/plain", b"expired update operation");
    }
    let mixed = Supervisor::shared().update_download_port();
    if path == "/appcast.xml" {
        let remote = REMOTE_FEED
            .lock()
            .ok()
            .and_then(|guard| guard.clone());
        let Some(remote) = remote else {
            return write_http(&mut stream, 503, "text/plain", b"no feed");
        };
        match fetch_release_bytes_with_progress(&remote, mixed, Duration::from_secs(30), |progress| {
            match progress {
                ReleaseProgress::Fallback { path, reason } => {
                    note_fallback(operation, &reason);
                    set_transport(operation, "fallback", route_name(path), format!("当前代理失败：{reason}。正在尝试直连。"));
                },
                ReleaseProgress::Attempting { path } => set_transport(operation,
                    "connecting",
                    route_name(path),
                    format!("正在通过{}检查更新。", path.label()),
                ),
                ReleaseProgress::Reading { path, bytes, total } => set_transport(operation,
                    "reading",
                    route_name(path),
                    format!("正在通过{}读取更新信息（{bytes}/{}）。", path.label(), total.map(|v| v.to_string()).unwrap_or_else(|| "未知".into())),
                ),
            }
        }) {
            Ok((body, path)) => {
                set_transport(operation, "validating", route_name(path), format!("已通过{}取得更新信息，正在检查版本。", path.label()));
                let xml = String::from_utf8_lossy(&body);
                let rewritten =
                    rewrite_appcast_enclosures(&xml, FEED_PORT.load(Ordering::Relaxed))
                        .replace("?u=", &format!("?operation={operation}&amp;u="));
                if method == "HEAD" {
                    return write_http_head(&mut stream, 200, "application/xml", rewritten.len());
                }
                write_http(&mut stream, 200, "application/xml", rewritten.as_bytes())
            }
            Err(err) => {
                let route = if mixed.is_some() { "代理和直连都失败" } else { "直连失败" };
                set_transport(operation, "error", "none", format!("更新检查失败：{route}。{err}"));
                myproxy::log::error(
                    "sparkle",
                    format!("feed fetch failed from {}: {err}", release_url_host(&remote)),
                );
                write_http(&mut stream, 502, "text/plain", b"feed fetch failed")
            }
        }
    } else if path == "/asset" || path.starts_with("/asset/") {
        let Some(url) = query.split('&').find_map(decode_asset_query) else {
            return write_http(&mut stream, 400, "text/plain", b"bad asset");
        };
        stream_asset(&mut stream, &method, &url, mixed, operation)
    } else {
        write_http(&mut stream, 404, "text/plain", b"not found")
    }
}

fn stream_asset(
    stream: &mut std::net::TcpStream,
    method: &str,
    url: &str,
    mixed: Option<u16>,
    operation: u64,
) -> std::io::Result<()> {
    let started = std::time::Instant::now();
    let head = method.eq_ignore_ascii_case("HEAD");
    if head {
        return match open_release(url, mixed, Duration::from_secs(900), method) {
            Ok(opened) => {
                write_asset_head(stream, opened.content_length, archive_filename(url))
            }
            Err(err) => {
                let route = if mixed.is_some() { "代理和直连都失败" } else { "直连失败" };
                set_transport(operation, "error", "none", format!("更新包下载失败：{route}。{err}"));
                myproxy::log::error("sparkle", format!("asset fetch failed from {}: {err}", release_url_host(url)));
                write_http(stream, 502, "text/plain", b"asset fetch failed")
            }
        };
    }
    match fetch_release_to_temp_with_progress(url, mixed, Duration::from_secs(900), |progress| {
        match progress {
            ReleaseProgress::Fallback { path, reason } => {
                note_fallback(operation, &reason);
                set_transport(operation, "fallback", route_name(path), format!("当前代理失败：{reason}。正在直连重试下载。"));
            },
            ReleaseProgress::Attempting { path } => set_transport(operation,
                "connecting",
                route_name(path),
                format!("正在通过{}下载更新包。", path.label()),
            ),
            ReleaseProgress::Reading { path, bytes, total } => set_transport(operation,
                "reading",
                route_name(path),
                format!("正在通过{}读取更新包（{bytes}/{}）。", path.label(), total.map(|v| v.to_string()).unwrap_or_else(|| "未知".into())),
            ),
        }
    }) {
        Ok(mut staged) => {
            staged.rewind()?;
            let staged_path = staged.path;
            let staged_length = staged.content_length;
            write_asset_head(stream, Some(staged.content_length), archive_filename(url))?;
            stream.flush()?;
            if let Err(error) = std::io::copy(&mut staged.file, stream) {
                set_transport(operation, "error", route_name(staged_path), "向更新器传输更新包中断，请重试检查更新。");
                return Err(error);
            }
            myproxy::log::info(
                "sparkle",
                format!(
                    "streamed {} bytes in {}ms via {}",
                    staged_length,
                    started.elapsed().as_millis(),
                    staged_path.label()
                ),
            );
            set_transport(operation, "validating", route_name(staged_path), format!("更新包已通过{}下载，等待签名校验。", staged_path.label()));
            Ok(())
        }
        Err(err) => {
            let route = if mixed.is_some() { "代理和直连都失败" } else { "直连失败" };
            set_transport(operation, "error", "none", format!("更新包下载失败：{route}。{err}"));
            myproxy::log::error(
                "sparkle",
                format!("asset fetch failed from {}: {err}", release_url_host(url)),
            );
            write_http(stream, 502, "text/plain", b"asset fetch failed")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_transfers_cannot_overwrite_a_retry_or_a_cancellation() {
        *SNAPSHOT.lock().unwrap() = UpdateTransportSnapshot {
            operation: 22, phase: "reading", route: "proxy", detail: "current download".into(), fallback_reason: None,
        };
        set_transport(21, "error", "none", "old error");
        assert_eq!(snapshot().detail, "current download");
        set_transport(22, "cancelled", "proxy", "cancelled");
        set_transport(22, "validating", "proxy", "old completion");
        assert_eq!(snapshot().detail, "cancelled");
        *SNAPSHOT.lock().unwrap() = UpdateTransportSnapshot {
            operation: 23, phase: "checking", route: "none", detail: "new check".into(), fallback_reason: None,
        };
        set_transport(22, "error", "none", "late abort");
        assert_eq!(snapshot().detail, "new check");
        *SNAPSHOT.lock().unwrap() = UpdateTransportSnapshot::idle();
    }

    #[test]
    fn update_operation_is_not_forwarded_to_the_remote_asset_url() {
        let remote = "https://github.com/leaperone/myproxy/releases/download/test/app.zip";
        let xml = format!("<enclosure url=\"{remote}\" />");
        let local = rewrite_appcast_enclosures(&xml, 8090)
            .replace("?u=", "?operation=23&amp;u=");
        let decoded = local.replace("&amp;", "&");
        let query = decoded.split('?').nth(1).unwrap().split('"').next().unwrap();
        assert_eq!(query.split('&').find_map(decode_asset_query).as_deref(), Some(remote));
        assert!(decoded.contains("/asset/app.zip?operation=23&u="));
    }
}

fn read_http_line(stream: &mut std::net::TcpStream) -> Option<(String, String)> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 2048];
    loop {
        let n = stream.read(&mut tmp).ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.windows(4).any(|window| window == b"\r\n\r\n") || buf.len() > 32 * 1024 {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf);
    let mut parts = text.lines().next()?.split_whitespace();
    Some((parts.next()?.to_string(), parts.next()?.to_string()))
}

fn write_http(
    stream: &mut std::net::TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    write_http_head(stream, status, content_type, body.len())?;
    stream.write_all(body)
}

fn write_http_head(
    stream: &mut std::net::TcpStream,
    status: u16,
    content_type: &str,
    len: usize,
) -> std::io::Result<()> {
    let header = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: {content_type}\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n",
        if status == 200 { "OK" } else { "ERROR" }
    );
    stream.write_all(header.as_bytes())
}

fn write_asset_head(
    stream: &mut std::net::TcpStream,
    content_length: Option<u64>,
    filename: &str,
) -> std::io::Result<()> {
    let extra = match content_length {
        Some(len) => format!("Content-Length: {len}\r\n"),
        None => String::new(),
    };
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=\"{filename}\"\r\n{extra}Connection: close\r\n\r\n"
    );
    stream.write_all(header.as_bytes())
}
