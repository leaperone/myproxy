use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use myproxy::supervisor::Supervisor;
use myproxy::updates::{
    archive_filename, decode_asset_query, fetch_release_bytes, open_release, release_url_host,
    rewrite_appcast_enclosures, UpdateChannel,
};

#[cfg(all(target_os = "macos", feature = "sparkle"))]
extern "C" {
    fn myproxy_sparkle_init();
    fn myproxy_sparkle_check();
    fn myproxy_sparkle_set_channel(feed_url: *const std::os::raw::c_char, nightly: i32);
}

static FEED_PORT: AtomicU16 = AtomicU16::new(0);
static REMOTE_FEED: Mutex<Option<(String, bool)>> = Mutex::new(None);

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
    let nightly = channel == UpdateChannel::Nightly;
    *REMOTE_FEED.lock().expect("sparkle feed") = Some((remote, nightly));
    let port = start_local_feed();
    let local = format!("http://127.0.0.1:{port}/appcast.xml");
    #[cfg(all(target_os = "macos", feature = "sparkle"))]
    unsafe {
        let url = std::ffi::CString::new(local).expect("update feed URL");
        myproxy_sparkle_set_channel(url.as_ptr(), i32::from(nightly));
    }
    #[cfg(not(all(target_os = "macos", feature = "sparkle")))]
    let _ = (local, nightly);
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
        Some(port) => myproxy::log::info(
            "sparkle",
            format!("update downloads via mixed http 127.0.0.1:{port}"),
        ),
        None => myproxy::log::debug("sparkle", "update downloads without mixed proxy"),
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
    let mixed = Supervisor::shared().update_download_port();
    if path == "/appcast.xml" {
        let remote = REMOTE_FEED
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|(url, _)| url.clone()));
        let Some(remote) = remote else {
            return write_http(&mut stream, 503, "text/plain", b"no feed");
        };
        match fetch_release_bytes(&remote, mixed, Duration::from_secs(30)) {
            Ok(body) => {
                let xml = String::from_utf8_lossy(&body);
                let rewritten =
                    rewrite_appcast_enclosures(&xml, FEED_PORT.load(Ordering::Relaxed));
                if method == "HEAD" {
                    return write_http_head(&mut stream, 200, "application/xml", rewritten.len());
                }
                write_http(&mut stream, 200, "application/xml", rewritten.as_bytes())
            }
            Err(err) => {
                myproxy::log::error(
                    "sparkle",
                    format!("feed fetch failed from {}: {err}", release_url_host(&remote)),
                );
                write_http(&mut stream, 502, "text/plain", b"feed fetch failed")
            }
        }
    } else if path == "/asset" || path.starts_with("/asset/") {
        let Some(url) = decode_asset_query(query) else {
            return write_http(&mut stream, 400, "text/plain", b"bad asset");
        };
        stream_asset(&mut stream, &method, &url, mixed)
    } else {
        write_http(&mut stream, 404, "text/plain", b"not found")
    }
}

fn stream_asset(
    stream: &mut std::net::TcpStream,
    method: &str,
    url: &str,
    mixed: Option<u16>,
) -> std::io::Result<()> {
    let started = std::time::Instant::now();
    let head = method.eq_ignore_ascii_case("HEAD");
    match open_release(url, mixed, Duration::from_secs(900), method) {
        Ok(opened) => {
            write_asset_head(stream, opened.content_length, archive_filename(url))?;
            stream.flush()?;
            if head {
                return Ok(());
            }
            let mut reader = opened.response.into_reader();
            let copied = std::io::copy(&mut reader, stream).unwrap_or(0);
            myproxy::log::info(
                "sparkle",
                format!(
                    "streamed {copied} bytes in {}ms via mixed={}",
                    started.elapsed().as_millis(),
                    mixed.is_some()
                ),
            );
            Ok(())
        }
        Err(err) => {
            myproxy::log::error(
                "sparkle",
                format!("asset fetch failed from {}: {err}", release_url_host(url)),
            );
            write_http(stream, 502, "text/plain", b"asset fetch failed")
        }
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
