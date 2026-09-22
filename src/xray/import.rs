//! Import direct proxy share links for the opt-in Xray channel.
//!
//! This is intentionally a parser boundary: unsupported query features are
//! rejected instead of being silently dropped and no error contains input
//! URLs or credentials.

use std::collections::HashMap;

use anyhow::{bail, Result};
use base64::Engine;
use serde_yaml::{Mapping, Value};

pub fn parse_links(text: &str) -> Result<Vec<Value>> {
    if text.len() > 4 * 1024 * 1024 {
        bail!("分享内容超过4MB");
    }
    let mut lines: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    if lines.is_empty() {
        bail!("没有找到代理分享链接");
    }
    if !lines.iter().any(|line| is_supported_scheme(line)) {
        if let Ok(decoded) = decode_b64(text.trim()) {
            let decoded =
                String::from_utf8(decoded).map_err(|_| anyhow::anyhow!("分享内容不是文本链接"))?;
            if decoded.trim() != text.trim() {
                return parse_links(&decoded);
            }
        }
        bail!("没有找到支持的分享链接");
    }
    let mut result = Vec::new();
    for line in lines.drain(..) {
        result.push(parse_one(&line)?);
    }
    Ok(result)
}

fn is_supported_scheme(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    ["vless://", "trojan://", "vmess://", "ss://"]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

fn parse_one(input: &str) -> Result<Value> {
    let lower = input.to_ascii_lowercase();
    if lower.starts_with("vless://") {
        parse_vless(&input[8..])
    } else if lower.starts_with("trojan://") {
        parse_trojan(&input[9..])
    } else if lower.starts_with("vmess://") {
        parse_vmess(&input[8..])
    } else if lower.starts_with("ss://") {
        parse_ss(&input[5..])
    } else if lower.starts_with("tuic://") || lower.starts_with("hysteria2://") {
        bail!("暂不支持这种节点链接。支持 VLESS、VMess、Trojan 和 Shadowsocks 链接")
    } else {
        bail!("链接格式不受支持");
    }
}

fn parse_vless(body: &str) -> Result<Value> {
    let (authority, query, fragment) = split_url(body)?;
    let (user, host, port) = parse_authority(authority, true)?;
    let mut node = base_node("vless", &host, port, &fragment)?;
    put(&mut node, "uuid", user);
    let query = query_map(query)?;
    apply_common_query(&mut node, &query, true)?;
    Ok(Value::Mapping(node))
}

fn parse_trojan(body: &str) -> Result<Value> {
    let (authority, query, fragment) = split_url(body)?;
    let (password, host, port) = parse_authority(authority, true)?;
    let mut node = base_node("trojan", &host, port, &fragment)?;
    put(&mut node, "password", password);
    let query = query_map(query)?;
    apply_common_query(&mut node, &query, false)?;
    if query
        .get("security")
        .is_some_and(|v| v != "tls" && v != "none")
    {
        bail!("Trojan 的 security 参数不受支持");
    }
    if query.get("security").is_some_and(|v| v == "none") {
        bail!("Trojan 明文传输被拒绝");
    }
    put(&mut node, "tls", "true");
    Ok(Value::Mapping(node))
}

fn parse_vmess(body: &str) -> Result<Value> {
    let encoded = body.split('#').next().unwrap_or_default();
    let decoded = decode_b64(encoded).map_err(|_| anyhow::anyhow!("VMess 分享内容不是有效配置"))?;
    let json: serde_json::Value = serde_json::from_slice(&decoded)
        .map_err(|_| anyhow::anyhow!("VMess 分享内容不是有效配置"))?;
    let object = json
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("VMess 分享内容不是对象"))?;
    let host = json_string(object, "add").ok_or_else(|| anyhow::anyhow!("VMess 缺少服务器地址"))?;
    let port = json_port(object, "port").ok_or_else(|| anyhow::anyhow!("VMess 端口无效"))?;
    let uuid = json_string(object, "id").ok_or_else(|| anyhow::anyhow!("VMess 缺少 UUID"))?;
    let mut node = base_node(
        "vmess",
        &host,
        port,
        json_string(object, "ps").as_deref().unwrap_or(""),
    )?;
    put(&mut node, "uuid", uuid);
    if let Some(cipher) = json_string(object, "scy") {
        put(&mut node, "cipher", cipher);
    }
    let network = json_string(object, "net").unwrap_or_else(|| "tcp".into());
    put(&mut node, "network", network.clone());
    if json_string(object, "tls").is_some_and(|v| v != "none" && !v.is_empty()) {
        put(&mut node, "tls", "true");
    }
    if let Some(sni) = json_string(object, "sni") {
        put(&mut node, "servername", sni);
    }
    let mut opts = Mapping::new();
    if let Some(path) = json_string(object, "path") {
        put(&mut opts, "path", path);
    }
    if let Some(host_header) = json_string(object, "host") {
        put(&mut opts, "headers", mapping_one("Host", host_header));
    }
    if network.eq_ignore_ascii_case("ws") && !opts.is_empty() {
        put(&mut node, "ws-opts", opts);
    }
    if network.eq_ignore_ascii_case("grpc") {
        if let Some(service) = json_string(object, "path") {
            put(
                &mut node,
                "grpc-opts",
                mapping_one("grpc-service-name", service),
            );
        }
    }
    if let Some(name) = json_string(object, "type") {
        if name != "none" && !name.is_empty() {
            bail!("VMess transport type 不受支持");
        }
    }
    Ok(Value::Mapping(node))
}

fn parse_ss(body: &str) -> Result<Value> {
    let (without_fragment, fragment) = split_fragment(body);
    let (main, query) = split_query(without_fragment);
    let query_values = query_map(query)?;
    if let Some(key) = query_values.keys().next() {
        if matches!(
            key.as_str(),
            "plugin" | "plugin-opts" | "obfs" | "obfs-password"
        ) {
            bail!("Shadowsocks plugin/obfs 未映射到 Xray");
        }
        bail!("Shadowsocks 链接包含未支持的参数");
    }
    let decoded = decode_b64(main).ok();
    let payload = decoded
        .as_deref()
        .and_then(|v| std::str::from_utf8(v).ok())
        .unwrap_or(main);
    let at = payload
        .rfind('@')
        .ok_or_else(|| anyhow::anyhow!("Shadowsocks 分享内容格式无效"))?;
    let raw_userinfo = &payload[..at];
    let decoded_userinfo = if raw_userinfo.contains(':') {
        raw_userinfo.to_string()
    } else {
        String::from_utf8(decode_b64(raw_userinfo)?)
            .map_err(|_| anyhow::anyhow!("Shadowsocks 认证信息无效"))?
    };
    let userinfo = decoded_userinfo.as_str();
    let endpoint = payload[at + 1..].trim_end_matches('/');
    let colon = userinfo
        .find(':')
        .ok_or_else(|| anyhow::anyhow!("Shadowsocks 分享内容缺少认证信息"))?;
    let method = percent_decode(&userinfo[..colon])?;
    let password = percent_decode(&userinfo[colon + 1..])?;
    let (host, port) = parse_host_port(endpoint)?;
    let fragment = percent_decode(fragment)?;
    let mut node = base_node("ss", &host, port, &fragment)?;
    put(&mut node, "cipher", method);
    put(&mut node, "password", password);
    Ok(Value::Mapping(node))
}

fn apply_common_query(
    node: &mut Mapping,
    query: &HashMap<String, String>,
    vless: bool,
) -> Result<()> {
    for key in query.keys() {
        if matches!(
            key.as_str(),
            "allowInsecure" | "insecure" | "skip-cert-verify"
        ) {
            bail!("不安全的证书校验参数被拒绝");
        }
        if !matches!(
            key.as_str(),
            "type"
                | "security"
                | "sni"
                | "servername"
                | "alpn"
                | "fp"
                | "flow"
                | "path"
                | "host"
                | "serviceName"
                | "service-name"
                | "pbk"
                | "sid"
                | "spx"
                | "encryption"
                | "headerType"
        ) {
            bail!("链接包含未支持的参数");
        }
    }
    if query.get("encryption").is_some_and(|v| v != "none")
        || query.get("headerType").is_some_and(|v| v != "none")
    {
        bail!("链接的加密或伪装参数尚不支持");
    }
    if let Some(security) = query.get("security") {
        match security.as_str() {
            "tls" => put(node, "tls", "true"),
            "reality" => {
                let pbk = query
                    .get("pbk")
                    .ok_or_else(|| anyhow::anyhow!("Reality 链接缺少 public-key"))?;
                let sid = query.get("sid").map(String::as_str).unwrap_or("");
                let mut reality = Mapping::new();
                put(&mut reality, "public-key", pbk.clone());
                put(&mut reality, "short-id", sid);
                put(node, "reality-opts", reality);
            }
            "none" if vless => {}
            "none" => bail!("该链接要求明文传输，已拒绝"),
            _ => bail!("链接 security 参数不受支持"),
        }
    }
    if let Some(value) = query.get("sni").or_else(|| query.get("servername")) {
        put(node, "servername", value.as_str());
    }
    if let Some(value) = query.get("alpn") {
        put(
            node,
            "alpn",
            yaml_sequence(value.split(',').map(str::to_owned)),
        );
    }
    if let Some(value) = query.get("fp") {
        put(node, "client-fingerprint", value.as_str());
    }
    if let Some(value) = query.get("flow") {
        put(node, "flow", value.as_str());
    }
    let network = query.get("type").map(String::as_str).unwrap_or("tcp");
    match network {
        "tcp" | "raw" => {}
        "ws" => {
            put(node, "network", "ws");
            let mut opts = Mapping::new();
            if let Some(path) = query.get("path") {
                put(&mut opts, "path", path.as_str());
            }
            if let Some(host) = query.get("host") {
                put(&mut opts, "headers", mapping_one("Host", host.clone()));
            }
            put(node, "ws-opts", opts);
        }
        "grpc" => {
            put(node, "network", "grpc");
            let mut opts = Mapping::new();
            if let Some(service) = query
                .get("serviceName")
                .or_else(|| query.get("service-name"))
            {
                put(&mut opts, "grpc-service-name", service.as_str());
            }
            put(node, "grpc-opts", opts);
        }
        "xhttp" | "splithttp" => {
            put(node, "network", "xhttp");
            let mut opts = Mapping::new();
            if let Some(path) = query.get("path") {
                put(&mut opts, "path", path.as_str());
            }
            if let Some(host) = query.get("host") {
                put(&mut opts, "host", host.as_str());
            }
            put(node, "xhttp-opts", opts);
        }
        _ => bail!("链接 transport 参数不受支持"),
    }
    Ok(())
}

fn base_node(kind: &str, host: &str, port: u16, name: &str) -> Result<Mapping> {
    if host.is_empty() {
        bail!("链接缺少服务器地址");
    }
    let mut node = Mapping::new();
    put(&mut node, "name", if name.is_empty() { host } else { name });
    put(&mut node, "type", kind);
    put(&mut node, "server", host);
    put(&mut node, "port", port);
    Ok(node)
}

fn split_url(body: &str) -> Result<(&str, &str, String)> {
    let (without_fragment, fragment) = split_fragment(body);
    let (authority, query) = split_query(without_fragment);
    Ok((authority, query, percent_decode(fragment)?))
}

fn split_fragment(value: &str) -> (&str, &str) {
    value.split_once('#').map_or((value, ""), |(a, b)| (a, b))
}

fn split_query(value: &str) -> (&str, &str) {
    value.split_once('?').map_or((value, ""), |(a, b)| (a, b))
}

fn parse_authority(value: &str, user_required: bool) -> Result<(String, String, u16)> {
    let at = value
        .rfind('@')
        .ok_or_else(|| anyhow::anyhow!("链接缺少认证信息"))?;
    let user = percent_decode(&value[..at])?;
    if user_required && user.is_empty() {
        bail!("链接认证信息为空");
    }
    let (host, port) = parse_host_port(&value[at + 1..])?;
    Ok((user, host, port))
}

fn parse_host_port(value: &str) -> Result<(String, u16)> {
    if let Some(rest) = value.strip_prefix('[') {
        let end = rest
            .find(']')
            .ok_or_else(|| anyhow::anyhow!("IPv6 地址格式无效"))?;
        let host = percent_decode(&rest[..end])?;
        host.parse::<std::net::Ipv6Addr>()
            .map_err(|_| anyhow::anyhow!("IPv6 地址格式无效"))?;
        let port = rest[end + 1..]
            .strip_prefix(':')
            .ok_or_else(|| anyhow::anyhow!("链接缺少端口"))?
            .parse()
            .map_err(|_| anyhow::anyhow!("端口无效"))?;
        if port == 0 {
            bail!("端口无效");
        }
        return Ok((host.into(), port));
    }
    let (host, port) = value
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("链接缺少端口"))?;
    let port = port.parse().map_err(|_| anyhow::anyhow!("端口无效"))?;
    if host.is_empty()
        || host.contains(':')
        || host
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "/@#\\".contains(c))
        || port == 0
    {
        bail!("服务器地址或端口无效");
    }
    Ok((percent_decode(host)?, port))
}

fn query_map(query: &str) -> Result<HashMap<String, String>> {
    let mut result = HashMap::new();
    if query.is_empty() {
        return Ok(result);
    }
    for pair in query.split('&') {
        let (key, value) = pair
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("链接参数格式无效"))?;
        let key = percent_decode(key)?;
        let value = percent_decode(value)?;
        if key.is_empty() || result.insert(key, value).is_some() {
            bail!("链接参数重复或为空");
        }
    }
    Ok(result)
}

fn percent_decode(value: &str) -> Result<String> {
    let mut out = Vec::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                bail!("链接转义格式无效");
            }
            let hi = hex(bytes[i + 1]).ok_or_else(|| anyhow::anyhow!("链接转义格式无效"))?;
            let lo = hex(bytes[i + 2]).ok_or_else(|| anyhow::anyhow!("链接转义格式无效"))?;
            out.push(hi * 16 + lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| anyhow::anyhow!("链接包含无效文本"))
}

fn hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn decode_b64(value: &str) -> Result<Vec<u8>> {
    let normalized = value.replace('-', "+").replace('_', "/");
    let padded = format!(
        "{}{}",
        normalized,
        "=".repeat((4 - normalized.len() % 4) % 4)
    );
    base64::engine::general_purpose::STANDARD
        .decode(padded)
        .map_err(|_| anyhow::anyhow!("链接内容不是有效 Base64"))
}

fn json_string(object: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<String> {
    object.get(key).and_then(|value| {
        value
            .as_str()
            .map(ToOwned::to_owned)
            .or_else(|| value.as_u64().map(|v| v.to_string()))
    })
}

fn json_port(object: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<u16> {
    json_string(object, key)?
        .parse()
        .ok()
        .filter(|port| *port > 0)
}

fn put(map: &mut Mapping, key: &str, value: impl Into<Value>) {
    map.insert(Value::String(key.into()), value.into());
}

fn yaml_sequence(values: impl IntoIterator<Item = String>) -> Value {
    Value::Sequence(values.into_iter().map(Value::String).collect())
}

fn mapping_one(key: &str, value: String) -> Value {
    let mut map = Mapping::new();
    put(&mut map, key, value);
    Value::Mapping(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vless_ipv6_and_escaped_fields() {
        let nodes = parse_links("vless://00000000-0000-4000-8000-000000000001@[2001:db8::1]:443?type=ws&security=tls&sni=example.com&path=%2Fchat%3Fx#Japan%20One").unwrap();
        let node = nodes[0].as_mapping().unwrap();
        assert_eq!(
            map_value(node, "server").and_then(Value::as_str),
            Some("2001:db8::1")
        );
        assert_eq!(
            map_value(node, "name").and_then(Value::as_str),
            Some("Japan One")
        );
        assert_eq!(map_value(node, "tls").and_then(Value::as_str), Some("true"));
    }

    #[test]
    fn parses_sip002_and_vmess_padding() {
        let ss_payload = base64::engine::general_purpose::STANDARD
            .encode("aes-128-gcm:pa%40ss@[2001:db8::2]:8443");
        let ss = parse_links(&format!("ss://{ss_payload}#SS")).unwrap();
        assert_eq!(ss[0]["password"], "pa@ss");
        let json = serde_json::json!({"v":"2","ps":"VM","add":"vm.example","port":"443","id":"00000000-0000-4000-8000-000000000001","net":"grpc","tls":"tls","path":"svc"});
        let encoded = base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_vec(&json).unwrap())
            .trim_end_matches('=')
            .to_owned();
        let vmess = parse_links(&format!("vmess://{encoded}")).unwrap();
        assert_eq!(vmess[0]["network"], "grpc");
        assert_eq!(vmess[0]["grpc-opts"]["grpc-service-name"], "svc");
    }

    #[test]
    fn parses_base64_whole_body() {
        let source = "trojan://secret@example.com:443?sni=example.com#fixture";
        let encoded = base64::engine::general_purpose::STANDARD.encode(source);
        assert_eq!(parse_links(&encoded).unwrap().len(), 1);
    }

    #[test]
    fn rejects_unsupported_security_transport_and_credentials_in_errors() {
        let input = "vless://secret-user@example.com:443?security=xtls";
        let error = parse_links(input).unwrap_err().to_string();
        assert!(!error.contains("secret-user"));
        assert!(!error.contains("example.com"));
        assert!(parse_links("tuic://secret@example.com:443").is_err());
        assert!(
            parse_links("ss://YWVzLTEyOC1nY206cGFzc0BleGFtcGxlLmNvbTo0NDM/?plugin=obfs").is_err()
        );
        assert!(parse_links("trojan://secret@example.com:0").is_err());
    }

    fn map_value<'a>(map: &'a Mapping, key: &str) -> Option<&'a Value> {
        map.get(&Value::String(key.into()))
    }
}
