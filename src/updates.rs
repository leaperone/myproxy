use std::io::Read;
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub const VERSION: &str = env!("MYPROXY_VERSION");

pub fn build_badge() -> Option<&'static str> {
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
}

impl Default for UpdateChannel {
    fn default() -> Self {
        if env!("MYPROXY_BUILD_CHANNEL") == "nightly" {
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

pub struct ReleaseResponse {
    pub response: ureq::Response,
    pub host: String,
    pub hops: u8,
    pub content_length: Option<u64>,
}

/// Open a GitHub release URL. Follow redirects with a new request each hop so
/// Mixed HTTP CONNECT is not reused across github.com → release-assets.
pub fn open_release(
    url: &str,
    mixed_port: Option<u16>,
    timeout: Duration,
    method: &str,
) -> Result<ReleaseResponse, String> {
    if !allowed_release_url(url) {
        return Err("blocked update host".to_string());
    }
    let mut builder = ureq::AgentBuilder::new()
        .timeout(timeout)
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(30))
        .redirects(0);
    if let Some(port) = mixed_port {
        let proxy = ureq::Proxy::new(&format!("http://127.0.0.1:{port}"))
            .map_err(|err| err.to_string())?;
        builder = builder.proxy(proxy);
    }
    let agent = builder.build();
    let method = if method.eq_ignore_ascii_case("HEAD") {
        "HEAD"
    } else {
        "GET"
    };
    let mut current = url.to_string();
    for hop in 0..8 {
        let response = match agent.request(method, &current).call() {
            Ok(response) => response,
            Err(ureq::Error::Status(code, response)) if (300..400).contains(&code) => response,
            Err(ureq::Error::Status(code, _)) => {
                return Err(format!("http {code} from {}", release_url_host(&current)));
            }
            Err(err) => return Err(format!("{} ({})", err, release_url_host(&current))),
        };
        let status = response.status();
        crate::log::debug(
            "updates",
            format!(
                "release hop {hop} {} status={status} mixed={} method={method}",
                release_url_host(&current),
                mixed_port.is_some()
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
                return Err(format!(
                    "blocked redirect host {}",
                    release_url_host(&current)
                ));
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
            response,
        });
    }
    Err("too many redirects".to_string())
}

pub fn fetch_release_bytes(
    url: &str,
    mixed_port: Option<u16>,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    let opened = open_release(url, mixed_port, timeout, "GET")?;
    let mut body = Vec::new();
    opened
        .response
        .into_reader()
        .read_to_end(&mut body)
        .map_err(|err| err.to_string())?;
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
