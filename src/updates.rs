use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub const VERSION: &str = env!("MYPROXY_VERSION");

pub fn build_badge() -> Option<&'static str> {
    if crate::backend::is_xray() {
        return Some("Xray");
    }
    match env!("MYPROXY_BUILD_CHANNEL") {
        "nightly" => Some("Nightly"),
        "dev" => Some("Dev"),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    Prod,
    Nightly,
    Xray,
}

impl Default for UpdateChannel {
    fn default() -> Self {
        if crate::backend::is_xray() || env!("MYPROXY_BUILD_CHANNEL") == "xray" {
            Self::Xray
        } else if env!("MYPROXY_BUILD_CHANNEL") == "nightly" {
            Self::Nightly
        } else {
            Self::Prod
        }
    }
}

impl UpdateChannel {
    pub fn label(self) -> &'static str {
        match self {
            Self::Prod => "正式版（Prod）",
            Self::Nightly => "Nightly",
            Self::Xray => "Xray",
        }
    }

    pub fn feed_url(self) -> &'static str {
        match self {
            Self::Prod => {
                "https://github.com/leaperone/myproxy/releases/latest/download/appcast.xml"
            }
            Self::Nightly => {
                "https://github.com/leaperone/myproxy/releases/download/nightly/appcast.xml"
            }
            Self::Xray => {
                "https://github.com/leaperone/myproxy/releases/download/xray/appcast.xml"
            }
        }
    }
}

pub fn release_url_host(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
}

pub fn allowed_release_url(url: &str) -> bool {
    if !url.starts_with("https://") {
        return false;
    }
    matches!(
        release_url_host(url),
        "github.com"
            | "release-assets.githubusercontent.com"
            | "objects.githubusercontent.com"
            | "github-releases.githubusercontent.com"
    )
}

fn resolve_location(current: &str, location: &str) -> String {
    if location.starts_with("https://") || location.starts_with("http://") {
        return location.to_string();
    }
    if location.starts_with('/') {
        if let Some(scheme_end) = current.find("://") {
            let rest = &current[scheme_end + 3..];
            let host = rest.split('/').next().unwrap_or("");
            return format!("{}://{}{}", &current[..scheme_end], host, location);
        }
    }
    location.to_string()
}

fn percent_encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() * 2);
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(value) = u8::from_str_radix(&raw[i + 1..i + 3], 16) {
                out.push(value);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn decode_asset_query(query: &str) -> Option<String> {
    let encoded = query.strip_prefix("u=")?;
    let url = percent_decode(encoded);
    allowed_release_url(&url).then_some(url)
}

pub fn archive_filename(url: &str) -> &str {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let name = path.rsplit('/').next().unwrap_or("");
    if name.ends_with(".sparkle.zip")
        || name.ends_with(".zip")
        || name.ends_with(".delta")
        || name.ends_with(".dmg")
        || name.ends_with(".tar.xz")
        || name.ends_with(".tar.bz2")
        || name.ends_with(".tar.gz")
    {
        name
    } else {
        "update.zip"
    }
}

pub fn rewrite_appcast_enclosures(xml: &str, local_port: u16) -> String {
    let mut out = String::with_capacity(xml.len() + 64);
    let mut rest = xml;
    while let Some(idx) = rest.find("url=\"") {
        out.push_str(&rest[..idx + 5]);
        rest = &rest[idx + 5..];
        let Some(end) = rest.find('"') else {
            out.push_str(rest);
            return out;
        };
        let url = &rest[..end];
        if allowed_release_url(url) {
            out.push_str(&format!(
                "http://127.0.0.1:{local_port}/asset/{}?u={}",
                archive_filename(url),
                percent_encode(url)
            ));
        } else {
            out.push_str(url);
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleasePath {
    Proxy,
    Direct,
    DirectFallback,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseProgress {
    Attempting { path: ReleasePath },
    Fallback { path: ReleasePath, reason: String },
    Reading { path: ReleasePath, bytes: u64, total: Option<u64> },
}

#[derive(Debug)]
pub struct StagedRelease {
    pub file: File,
    pub path: ReleasePath,
    pub content_length: u64,
    pub fallback_reason: Option<String>,
    location: PathBuf,
}

impl StagedRelease {
    pub fn rewind(&mut self) -> std::io::Result<()> {
        use std::io::Seek;
        self.file.rewind()
    }
}

impl Drop for StagedRelease {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.location);
    }
}

impl ReleasePath {
    pub fn label(self) -> &'static str {
        match self {
            Self::Proxy => "当前代理",
            Self::Direct => "直连",
            Self::DirectFallback => "代理失败后直连回落",
        }
    }
}

pub struct ReleaseResponse {
    pub response: ureq::Response,
    pub host: String,
    pub hops: u8,
    pub content_length: Option<u64>,
    pub path: ReleasePath,
}

/// Open a GitHub release URL. Follow redirects with a new request each hop so
/// Mixed HTTP CONNECT is not reused across github.com → release-assets.
fn open_release_once(
    url: &str,
    mixed_port: Option<u16>,
    timeout: Duration,
    method: &str,
    path: ReleasePath,
) -> Result<ReleaseResponse, String> {
    let method = if method.eq_ignore_ascii_case("HEAD") {
        "HEAD"
    } else {
        "GET"
    };
    let mut current = url.to_string();
    for hop in 0..8 {
        // Build a new agent for every hop. In particular, a GitHub redirect to
        // release-assets must establish a new CONNECT tunnel in Mixed.
        let mut builder = ureq::AgentBuilder::new()
            .timeout(timeout)
            .timeout_connect(Duration::from_secs(15))
            .timeout_read(Duration::from_secs(30))
            .redirects(0)
            .try_proxy_from_env(false);
        if let Some(port) = mixed_port {
            let proxy = ureq::Proxy::new(&format!("http://127.0.0.1:{port}"))
                .map_err(|_| "invalid local update proxy".to_string())?;
            builder = builder.proxy(proxy);
        }
        let agent = builder.build();
        let response = match agent.request(method, &current).set("Accept-Encoding", "identity").call() {
            Ok(response) => response,
            Err(ureq::Error::Status(code, response)) if (300..400).contains(&code) => response,
            Err(ureq::Error::Status(code, _)) => {
                return Err(format!("http {code} from {}", release_url_host(&current)));
            }
            Err(ureq::Error::Transport(error)) => return Err(format!("{}: {:?} from {}", path.label(), error.kind(), release_url_host(&current))),
        };
        let status = response.status();
        crate::log::debug(
            "updates",
            format!(
                "release hop {hop} {} status={status} path={path:?} method={method}",
                release_url_host(&current),
            ),
        );
        if (300..400).contains(&status) {
            let location = response
                .header("location")
                .or_else(|| response.header("Location"))
                .unwrap_or("")
                .to_string();
            if location.is_empty() {
                return Err(format!("redirect {status} missing location"));
            }
            current = resolve_location(&current, &location);
            if !allowed_release_url(&current) {
                return Err("blocked redirect host".to_string());
            }
            continue;
        }
        if status != 200 {
            return Err(format!("http {status} from {}", release_url_host(&current)));
        }
        let content_length = response
            .header("Content-Length")
            .or_else(|| response.header("content-length"))
            .and_then(|value| value.parse().ok());
        return Ok(ReleaseResponse {
            host: release_url_host(&current).to_string(),
            hops: hop + 1,
            content_length,
            path,
            response,
        });
    }
    Err("too many redirects".to_string())
}

/// Prefer the running Mixed proxy. If any proxy request fails, retry the
/// complete redirect chain directly so a broken selected node cannot block
/// application updates. GET/HEAD requests here are idempotent.
pub fn open_release(
    url: &str,
    mixed_port: Option<u16>,
    timeout: Duration,
    method: &str,
) -> Result<ReleaseResponse, String> {
    if !allowed_release_url(url) {
        return Err("blocked update host".to_string());
    }
    let Some(port) = mixed_port else {
        return open_release_once(url, None, timeout, method, ReleasePath::Direct);
    };
    match open_release_once(url, Some(port), timeout, method, ReleasePath::Proxy) {
        Ok(response) => {
            crate::log::info(
                "updates",
                format!("release request succeeded via current proxy host={}", response.host),
            );
            Ok(response)
        }
        Err(proxy_error) => {
            crate::log::warn(
                "updates",
                format!(
                    "release request via current proxy failed: {proxy_error}; retrying direct"
                ),
            );
            match open_release_once(url, None, timeout, method, ReleasePath::DirectFallback) {
                Ok(response) => {
                    crate::log::info(
                        "updates",
                        format!(
                            "release request succeeded via direct fallback host={}",
                            response.host
                        ),
                    );
                    Ok(response)
                }
                Err(direct_error) => Err(format!(
                    "代理请求失败：{proxy_error}；直连回落也失败：{direct_error}"
                )),
            }
        }
    }
}

/// Fetch and validate a complete response before returning it. The callback
/// is invoked before each route attempt and while reading its body, allowing
/// the local feed to expose proxy/direct fallback progress.
pub fn fetch_release_bytes_with_progress<F>(
    url: &str,
    mixed_port: Option<u16>,
    timeout: Duration,
    progress: F,
) -> Result<(Vec<u8>, ReleasePath), String>
where
    F: FnMut(ReleaseProgress),
{
    let mut staged = fetch_release_to_temp_with_limit(url, mixed_port, timeout, 16 * 1024 * 1024, progress)?;
    staged.rewind().map_err(|_| "staged update rewind failed".to_string())?;
    let mut body = Vec::new();
    staged.file.read_to_end(&mut body).map_err(|_| "staged update read failed".to_string())?;
    Ok((body, staged.path))
}

pub fn fetch_release_to_temp_with_progress<F>(
    url: &str,
    mixed_port: Option<u16>,
    timeout: Duration,
    progress: F,
) -> Result<StagedRelease, String>
where
    F: FnMut(ReleaseProgress),
{
    fetch_release_to_temp_with_limit(url, mixed_port, timeout, 2 * 1024 * 1024 * 1024, progress)
}

fn fetch_release_to_temp_with_limit<F>(
    url: &str,
    mixed_port: Option<u16>,
    timeout: Duration,
    max_bytes: u64,
    progress: F,
) -> Result<StagedRelease, String>
where
    F: FnMut(ReleaseProgress),
{
    fetch_with_opener(url, mixed_port, max_bytes, progress, |url, port, path| {
        open_release_once(url, port, timeout, "GET", path)
    })
}

fn fetch_with_opener<F, O>(
    url: &str,
    mixed_port: Option<u16>,
    max_bytes: u64,
    mut progress: F,
    mut open: O,
) -> Result<StagedRelease, String>
where
    F: FnMut(ReleaseProgress),
    O: FnMut(&str, Option<u16>, ReleasePath) -> Result<ReleaseResponse, String>,
{
    if !allowed_release_url(url) { return Err("blocked update host".to_string()); }
    let attempts = if mixed_port.is_some() {
        vec![(mixed_port, ReleasePath::Proxy), (None, ReleasePath::DirectFallback)]
    } else {
        vec![(None, ReleasePath::Direct)]
    };
    let mut failures = Vec::new();
    for (attempt, (port, path)) in attempts.into_iter().enumerate() {
        if attempt > 0 {
            progress(ReleaseProgress::Fallback { path, reason: failures.last().cloned().unwrap_or_else(|| "current proxy failed".into()) });
        } else {
            progress(ReleaseProgress::Attempting { path });
        }
        let opened = match open(url, port, path) {
            Ok(opened) => opened,
            Err(error) => {
                if error.starts_with("blocked ") {
                    return Err(error);
                }
                failures.push(format!("{}: {error}", path.label()));
                continue;
            }
        };
        progress(ReleaseProgress::Reading { path, bytes: 0, total: opened.content_length });
        let expected = opened.content_length;
        if expected.unwrap_or(0) > max_bytes {
            failures.push(format!("{}: response too large", path.label()));
            continue;
        }
        let location = std::env::temp_dir().join(format!("myproxy-update-{}-{}", std::process::id(), uuid::Uuid::new_v4().simple()));
        let mut create = OpenOptions::new();
        create.read(true).write(true).create_new(true);
        #[cfg(unix)]
        { use std::os::unix::fs::OpenOptionsExt; create.mode(0o600); }
        let file = match create.open(&location) {
            Ok(file) => file,
            Err(_) => { failures.push(format!("{}: staging file unavailable", path.label())); continue; }
        };
        let mut staged = StagedRelease { file, path, content_length: 0, fallback_reason: failures.first().cloned(), location };
        let mut reader = opened.response.into_reader();
        let result = stage_body(&mut reader, &mut staged.file, expected, max_bytes, |bytes| {
            progress(ReleaseProgress::Reading { path, bytes, total: expected });
        });
        match result {
            Ok(bytes) => {
                staged.content_length = bytes;
                staged.rewind().map_err(|_| "staged update rewind failed".to_string())?;
                return Ok(staged);
            }
            Err(error) => failures.push(format!("{}: {error}", path.label())),
        }
    }
    Err(format!("update request failed ({})", failures.join("; ")))
}

fn stage_body<R: Read, F: FnMut(u64)>(reader: &mut R, file: &mut File, expected: Option<u64>, max: u64, mut progress: F) -> Result<u64, String> {
    let mut bytes = 0u64;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buf).map_err(|error| format!("response body read failed: {:?}", error.kind()))?;
        if count == 0 { break; }
        bytes = bytes.saturating_add(count as u64);
        if bytes > max { return Err("response body too large".into()); }
        file.write_all(&buf[..count]).map_err(|_| "staging file write failed")?;
        progress(bytes);
    }
    if bytes == 0 { return Err("empty update response".into()); }
    if let Some(expected) = expected {
        if bytes != expected { return Err(format!("response body truncated (received {bytes} of {expected})")); }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE_URL: &str = "https://github.com/leaperone/myproxy/releases/download/fixture/app.zip";

    fn fixture(path: ReleasePath, body: &str, length: Option<u64>) -> ReleaseResponse {
        ReleaseResponse {
            response: ureq::Response::new(200, "OK", body).unwrap(),
            host: "github.com".into(), hops: 1, content_length: length, path,
        }
    }

    fn staged_bytes(staged: &mut StagedRelease) -> Vec<u8> {
        staged.rewind().unwrap();
        let mut bytes = Vec::new();
        staged.file.read_to_end(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn proxy_success_never_attempts_direct_and_cleans_up_after_delivery() {
        let mut attempted = Vec::new();
        let mut staged = fetch_with_opener(FIXTURE_URL, Some(40999), 20, |_| {}, |_, port, path| {
            attempted.push((port, path));
            Ok(fixture(path, "archive", Some(7)))
        }).unwrap();
        assert_eq!(attempted, [(Some(40999), ReleasePath::Proxy)]);
        assert_eq!(staged_bytes(&mut staged), b"archive");
        assert_eq!(staged.fallback_reason, None);
        let location = staged.location.clone();
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&location).unwrap().permissions().mode() & 0o777, 0o600);
        }
        drop(staged);
        assert!(!location.exists());
    }

    #[test]
    fn proxy_connection_failure_retries_direct_and_preserves_reason() {
        let mut attempted = Vec::new();
        let mut events = Vec::new();
        let mut staged = fetch_with_opener(FIXTURE_URL, Some(40999), 20, |event| events.push(event), |_, port, path| {
            attempted.push((port, path));
            if port.is_some() { Err("connection refused".into()) }
            else { Ok(fixture(path, "direct", Some(6))) }
        }).unwrap();
        assert_eq!(attempted, [(Some(40999), ReleasePath::Proxy), (None, ReleasePath::DirectFallback)]);
        assert_eq!(staged.path, ReleasePath::DirectFallback);
        assert_eq!(staged_bytes(&mut staged), b"direct");
        assert_eq!(staged.fallback_reason.as_deref(), Some("当前代理: connection refused"));
        assert!(events.contains(&ReleaseProgress::Fallback { path: ReleasePath::DirectFallback, reason: "当前代理: connection refused".into() }));
    }

    #[test]
    fn truncated_proxy_body_is_discarded_before_direct_retry() {
        let mut attempted = Vec::new();
        let mut staged = fetch_with_opener(FIXTURE_URL, Some(40999), 20, |_| {}, |_, port, path| {
            attempted.push(port);
            Ok(if port.is_some() { fixture(path, "bad", Some(10)) } else { fixture(path, "complete", Some(8)) })
        }).unwrap();
        assert_eq!(attempted, [Some(40999), None]);
        assert_eq!(staged.content_length, 8);
        assert_eq!(staged_bytes(&mut staged), b"complete");
        assert!(staged.fallback_reason.as_deref().unwrap().contains("received 3 of 10"));
    }

    #[test]
    fn both_routes_failing_report_both_causes() {
        let error = fetch_with_opener(FIXTURE_URL, Some(40999), 20, |_| {}, |_, port, _| {
            Err(if port.is_some() { "proxy refused" } else { "TLS failed" }.into())
        }).unwrap_err();
        assert!(error.contains("proxy refused"));
        assert!(error.contains("TLS failed"));
    }

    #[test]
    fn direct_only_does_not_claim_proxy_failure() {
        let mut attempts = Vec::new();
        let mut staged = fetch_with_opener(FIXTURE_URL, None, 20, |_| {}, |_, port, path| {
            attempts.push((port, path));
            Ok(fixture(path, "direct", None))
        }).unwrap();
        assert_eq!(attempts, [(None, ReleasePath::Direct)]);
        assert_eq!(staged.fallback_reason, None);
        assert_eq!(staged_bytes(&mut staged), b"direct");
    }

    #[test]
    fn blocked_hosts_and_redirects_are_never_retried() {
        assert!(fetch_with_opener("http://example.com/app.zip", Some(40999), 20, |_| {}, |_, _, _| {
            panic!("blocked URL must not be requested")
        }).is_err());
        let mut attempts = 0;
        let result = fetch_with_opener(FIXTURE_URL, Some(40999), 20, |_| {}, |_, _, _| {
            attempts += 1;
            Err("blocked redirect host example.com".into())
        });
        assert!(result.is_err());
        assert_eq!(attempts, 1);
    }

    #[test]
    fn oversized_and_empty_responses_do_not_become_download_success() {
        for (body, length) in [("", None), ("", Some(0)), ("too big", None), ("size", Some(99))] {
            let result = fetch_with_opener(FIXTURE_URL, None, 4, |_| {}, |_, _, path| Ok(fixture(path, body, length)));
            assert!(result.is_err(), "body={body:?}, length={length:?}");
        }
    }

    #[test]
    fn update_channels_keep_independent_release_feeds() {
        assert_eq!(UpdateChannel::Prod.feed_url(), "https://github.com/leaperone/myproxy/releases/latest/download/appcast.xml");
        assert_eq!(UpdateChannel::Nightly.feed_url(), "https://github.com/leaperone/myproxy/releases/download/nightly/appcast.xml");
        assert_eq!(UpdateChannel::Xray.feed_url(), "https://github.com/leaperone/myproxy/releases/download/xray/appcast.xml");
        assert_eq!(serde_json::to_string(&UpdateChannel::Xray).unwrap(), "\"xray\"");
    }

    #[test]
    fn allow_github_release_hosts() {
        assert!(allowed_release_url(
            "https://github.com/leaperone/myproxy/releases/download/nightly/appcast.xml"
        ));
        assert!(allowed_release_url(
            "https://release-assets.githubusercontent.com/github-production-release-asset/1"
        ));
        assert!(!allowed_release_url("https://example.com/appcast.xml"));
        assert!(!allowed_release_url("http://github.com/x"));
    }

    #[test]
    fn rewrite_enclosure_to_loopback() {
        let xml = r#"<enclosure url="https://github.com/leaperone/myproxy/releases/download/v1/a.zip" />"#;
        let out = rewrite_appcast_enclosures(xml, 9);
        assert!(out.contains("http://127.0.0.1:9/asset/a.zip?u="));
        assert!(!out.contains("url=\"https://github.com/"));
        let query = out
            .split("asset/a.zip?")
            .nth(1)
            .unwrap()
            .trim_end_matches("\" />");
        assert_eq!(
            decode_asset_query(query).as_deref(),
            Some("https://github.com/leaperone/myproxy/releases/download/v1/a.zip")
        );
    }

    #[test]
    fn resolve_absolute_and_relative_location() {
        assert_eq!(
            resolve_location(
                "https://github.com/a/b",
                "https://release-assets.githubusercontent.com/x"
            ),
            "https://release-assets.githubusercontent.com/x"
        );
        assert_eq!(
            resolve_location("https://github.com/a/b", "/latest/download/appcast.xml"),
            "https://github.com/latest/download/appcast.xml"
        );
    }

    #[test]
    fn staging_rejects_short_and_empty_bodies() {
        let dir = std::env::temp_dir().join(format!("myproxy-test-{}", uuid::Uuid::new_v4().simple()));
        let mut file = OpenOptions::new().read(true).write(true).create(true).open(&dir).unwrap();
        assert!(stage_body(&mut &b"abc"[..], &mut file, Some(4), 100, |_| {}).is_err());
        assert!(stage_body(&mut &b""[..], &mut file, Some(1), 100, |_| {}).is_err());
        let _ = fs::remove_file(dir);
    }

    #[test]
    fn staging_rejects_size_limit_and_preserves_exact_body() {
        let dir = std::env::temp_dir().join(format!("myproxy-test-{}", uuid::Uuid::new_v4().simple()));
        let mut file = OpenOptions::new().read(true).write(true).create(true).open(&dir).unwrap();
        assert!(stage_body(&mut &b"abcd"[..], &mut file, None, 3, |_| {}).is_err());
        let _ = fs::remove_file(dir);
        let dir = std::env::temp_dir().join(format!("myproxy-test-{}", uuid::Uuid::new_v4().simple()));
        let mut file = OpenOptions::new().read(true).write(true).create_new(true).open(&dir).unwrap();
        assert_eq!(stage_body(&mut &b"abc"[..], &mut file, Some(3), 3, |_| {}).unwrap(), 3);
        use std::io::Seek;
        file.rewind().unwrap();
        let mut body = Vec::new(); file.read_to_end(&mut body).unwrap();
        assert_eq!(body, b"abc");
        let _ = fs::remove_file(dir);
    }
}
