//! Projection of one catalog node into an Xray 26.9.9 outbound.
//!
//! This module is deliberately independent of the application router.  It
//! accepts the already parsed catalog representation and either returns a
//! complete outbound or a diagnostic which lets the caller exclude that node.

use std::collections::BTreeMap;

use anyhow::{bail, Context, Result};
use base64::Engine;
use serde_json::{json, Map, Value};
use serde_yaml::Value as YamlValue;
use uuid::Uuid;

use crate::catalog::Node;

pub fn render(node: &Node, tag: &str) -> Result<Value> {
    let tag = tag.trim();
    if tag.is_empty() || tag.chars().any(|c| c.is_control()) {
        bail!("invalid Xray outbound tag");
    }
    reject_unimplemented_features(&node.raw)?;
    let kind = required_string(&node.raw, "type")?.to_ascii_lowercase();
    let address = required_string(&node.raw, "server")?;
    let port = required_port(&node.raw)?;
    let (protocol, settings) = match kind.as_str() {
        "ss" | "shadowsocks" => ("shadowsocks", shadowsocks(&node.raw, &address, port)?),
        "vmess" => ("vmess", vmess(&node.raw, &address, port)?),
        "vless" => ("vless", vless(&node.raw, &address, port)?),
        "trojan" => ("trojan", trojan(&node.raw, &address, port)?),
        "socks" | "socks5" => ("socks", socks(&node.raw, &address, port)?),
        "http" => ("http", http(&node.raw, &address, port)?),
        "hysteria2" | "hysteria" => ("hysteria", hysteria2(&node.raw, &address, port)?),
        other => bail!("协议 {other} 尚未由 Xray 26.9.9 支持"),
    };
    let mut outbound = Map::new();
    outbound.insert("tag".into(), Value::String(tag.into()));
    outbound.insert("protocol".into(), Value::String(protocol.into()));
    outbound.insert("settings".into(), settings);
    if let Some(stream) = stream_settings(&node.raw, &kind)? {
        outbound.insert("streamSettings".into(), stream);
    }
    // The core process bypasses the system DNS proxy, so the Go resolver's UDP
    // query to the system nameserver times out. Resolve the server name first.
    use_configured_dns(&mut outbound);
    Ok(Value::Object(outbound))
}

fn use_configured_dns(outbound: &mut Map<String, Value>) {
    let stream = outbound
        .entry("streamSettings".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(stream) = stream else {
        return;
    };
    let sockopt = stream
        .entry("sockopt".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(sockopt) = sockopt else {
        return;
    };
    sockopt.insert("domainStrategy".into(), "UseIPv4".into());
}

fn shadowsocks(raw: &YamlValue, address: &str, port: u16) -> Result<Value> {
    let method = required_string(raw, "cipher")?.to_ascii_lowercase();
    // These are the methods understood by Xray's Shadowsocks outbound.  A
    // typo must be isolated here instead of producing a late core error.
    let valid = [
        "aes-128-gcm",
        "aes-256-gcm",
        "chacha20-poly1305",
        "chacha20-ietf-poly1305",
        "xchacha20-ietf-poly1305",
        "2022-blake3-aes-128-gcm",
        "2022-blake3-aes-256-gcm",
        "2022-blake3-chacha20-poly1305",
    ];
    if !valid.contains(&method.as_str()) {
        bail!("不支持的 Shadowsocks 加密方式：{method}");
    }
    Ok(json!({"servers": [{"address": address, "port": port,
        "method": method, "password": required_string(raw, "password")?}]}))
}

fn vmess(raw: &YamlValue, address: &str, port: u16) -> Result<Value> {
    let id = required_uuid(raw, "uuid")?;
    let security = raw_string(raw, "cipher")
        .unwrap_or_else(|| "auto".into())
        .to_ascii_lowercase();
    if !["auto", "none", "aes-128-gcm", "chacha20-poly1305"].contains(&security.as_str()) {
        bail!("不支持的 VMess 加密方式：{security}");
    }
    let alter_id = raw_u32(raw, "alterId")
        .or_else(|| raw_u32(raw, "alter-id"))
        .unwrap_or(0);
    if alter_id != 0 {
        bail!("此内核不支持旧版 VMess alterId");
    }
    Ok(
        json!({"vnext": [{"address": address, "port": port, "users": [{
            "id": id, "alterId": alter_id, "security": security
        }]}]}),
    )
}

fn vless(raw: &YamlValue, address: &str, port: u16) -> Result<Value> {
    let id = required_uuid(raw, "uuid")?;
    let mut user = Map::new();
    user.insert("id".into(), Value::String(id));
    user.insert("encryption".into(), Value::String("none".into()));
    if let Some(flow) = raw_string(raw, "flow") {
        if !flow.is_empty() {
            user.insert("flow".into(), Value::String(flow));
        }
    }
    Ok(json!({"vnext": [{"address": address, "port": port, "users": [Value::Object(user)]}]}))
}

fn trojan(raw: &YamlValue, address: &str, port: u16) -> Result<Value> {
    if raw_value(raw, "tls").is_some_and(|v| matches!(v, YamlValue::Bool(false))) {
        bail!("Trojan 必须使用 TLS；Xray 通道不会发送明文 Trojan");
    }
    if raw_string(raw, "flow").is_some_and(|v| !v.is_empty()) {
        bail!("Trojan flow 当前未纳入 Xray 通道");
    }
    Ok(json!({"servers": [{"address": address, "port": port,
        "password": required_string(raw, "password")?}]}))
}

fn socks(raw: &YamlValue, address: &str, port: u16) -> Result<Value> {
    let mut server = json!({"address": address, "port": port});
    if let Some(user) = raw_string(raw, "username").or_else(|| raw_string(raw, "user")) {
        server["users"] = json!([{"user": user, "pass": raw_string(raw, "password").or_else(|| raw_string(raw, "pass")).unwrap_or_default()}]);
    }
    Ok(json!({"servers": [server]}))
}

fn http(raw: &YamlValue, address: &str, port: u16) -> Result<Value> {
    socks(raw, address, port)
}

fn hysteria2(_raw: &YamlValue, address: &str, port: u16) -> Result<Value> {
    Ok(json!({"version": 2, "address": address, "port": port}))
}

fn stream_settings(raw: &YamlValue, kind: &str) -> Result<Option<Value>> {
    let network = raw_string(raw, "network")
        .unwrap_or_else(|| {
            if matches!(kind, "hysteria" | "hysteria2") {
                "hysteria".into()
            } else {
                "tcp".into()
            }
        })
        .to_ascii_lowercase();
    let reality = raw_map(raw, "reality-opts");
    let is_hysteria = matches!(kind, "hysteria" | "hysteria2");
    let tls = raw_truthy(raw, "tls") || reality.is_some() || kind == "trojan" || is_hysteria;
    let pin = raw_string(raw, "pinnedPeerCertSha256")
        .or_else(|| raw_string(raw, "pinned-peer-cert-sha256"));
    let verify_name = raw_string(raw, "verifyPeerCertByName")
        .or_else(|| raw_string(raw, "verify-peer-cert-by-name"));
    let skip = raw_truthy(raw, "skip-cert-verify");
    let pin = pin.map(|value| validate_pin(&value)).transpose()?;
    if skip && pin.is_none() {
        bail!("skip-cert-verify=true 必须提供有效的 pinnedPeerCertSha256；verifyPeerCertByName 不能替代证书固定");
    }
    if !tls && matches!(network.as_str(), "tcp" | "raw") {
        return Ok(None);
    }
    let mut stream = Map::new();
    if is_hysteria {
        if reality.is_some() {
            bail!("Hysteria2 不支持 Reality");
        }
        let auth = raw_string(raw, "password")
            .or_else(|| raw_string(raw, "auth"))
            .unwrap_or_default();
        let mut settings = json!({"version":2,"auth":auth});
        if let Some(timeout) = raw_u64(raw, "udp-idle-timeout") {
            settings["udpIdleTimeout"] = timeout.into();
        }
        stream.insert("network".into(), "hysteria".into());
        stream.insert("hysteriaSettings".into(), settings);
    } else {
        let name = match network.as_str() {
            "ws" | "websocket" => "ws",
            "grpc" => "grpc",
            "xhttp" | "splithttp" => "splithttp",
            "tcp" | "raw" => "raw",
            other => bail!("传输方式 {other} 尚未映射到 Xray 26.9.9"),
        };
        stream.insert("network".into(), name.into());
        match network.as_str() {
            "ws" | "websocket" => {
                stream.insert("wsSettings".into(), ws_settings(raw));
            }
            "grpc" => {
                stream.insert("grpcSettings".into(), grpc_settings(raw));
            }
            "xhttp" | "splithttp" => {
                stream.insert("splithttpSettings".into(), xhttp_settings(raw));
            }
            _ => {}
        }
    }
    if tls {
        let mut security = Map::new();
        if let Some(name) = raw_string(raw, "servername").or_else(|| raw_string(raw, "sni")) {
            security.insert("serverName".into(), name.into());
        }
        if let Some(alpn) = raw_string_list(raw, "alpn") {
            security.insert("alpn".into(), json!(alpn));
        }
        if let Some(fp) = raw_string(raw, "client-fingerprint") {
            security.insert("fingerprint".into(), fp.into());
        }
        if let Some(pin) = pin {
            security.insert("pinnedPeerCertSha256".into(), pin.into());
        }
        if let Some(name) = verify_name {
            security.insert("verifyPeerCertByName".into(), name.into());
        }
        if let Some(reality) = reality {
            security
                .entry("fingerprint".to_string())
                .or_insert_with(|| Value::String("chrome".into()));
            let public_key =
                raw_map_string(&reality, "public-key").context("Reality 缺少 public-key")?;
            let short_id = raw_map_string(&reality, "short-id").context("Reality 缺少 short-id")?;
            let public_key_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(public_key.as_bytes())
                .context("Reality public-key 必须是 Base64URL")?;
            if public_key_bytes.len() != 32 {
                bail!("Reality public-key 必须解码为 32 字节");
            }
            if !short_id.chars().all(|c| c.is_ascii_hexdigit())
                || short_id.len() % 2 != 0
                || short_id.len() > 16
            {
                bail!("Reality short-id 必须是最多 8 字节的偶数位十六进制");
            }
            security.insert("publicKey".into(), public_key.into());
            security.insert("shortId".into(), short_id.into());
            stream.insert("security".into(), "reality".into());
            stream.insert("realitySettings".into(), Value::Object(security));
        } else {
            stream.insert("security".into(), "tls".into());
            stream.insert("tlsSettings".into(), Value::Object(security));
        }
    }
    Ok(Some(Value::Object(stream)))
}

fn ws_settings(raw: &YamlValue) -> Value {
    let opts = raw_map(raw, "ws-opts");
    let mut out = Map::new();
    out.insert(
        "path".into(),
        raw_map_string_ref(&opts, "path")
            .unwrap_or_else(|| "/".into())
            .into(),
    );
    if let Some(headers) = opts
        .as_ref()
        .and_then(|m| m.get(YamlValue::String("headers".into())))
        .and_then(YamlValue::as_mapping)
    {
        let mut map = BTreeMap::new();
        for (key, value) in headers {
            if let (Some(k), Some(v)) = (key.as_str(), value.as_str()) {
                map.insert(k.to_string(), v.to_string());
            }
        }
        out.insert("headers".into(), json!(map));
    }
    Value::Object(out)
}

fn grpc_settings(raw: &YamlValue) -> Value {
    let opts = raw_map(raw, "grpc-opts");
    json!({"serviceName": raw_map_string_ref(&opts, "grpc-service-name").unwrap_or_default()})
}

fn xhttp_settings(raw: &YamlValue) -> Value {
    let opts = raw_map(raw, "xhttp-opts").or_else(|| raw_map(raw, "splithttp-opts"));
    let mut out = Map::new();
    out.insert(
        "path".into(),
        raw_map_string_ref(&opts, "path")
            .unwrap_or_else(|| "/".into())
            .into(),
    );
    if let Some(v) = raw_map_string_ref(&opts, "host") {
        out.insert("host".into(), v.into());
    }
    if let Some(v) = raw_map_string_ref(&opts, "mode") {
        out.insert("mode".into(), v.into());
    }
    Value::Object(out)
}

fn reject_unimplemented_features(raw: &YamlValue) -> Result<()> {
    for key in [
        "plugin",
        "plugin-opts",
        "obfs",
        "obfs-opts",
        "obfs-password",
    ] {
        if raw_value(raw, key).is_some() {
            bail!("节点字段 {key} 未映射到 Xray，已拒绝以避免静默改变传输");
        }
    }
    Ok(())
}

fn validate_pin(value: &str) -> Result<String> {
    for item in value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        let hex = item.replace(':', "");
        if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("pinnedPeerCertSha256 必须是 64 位十六进制或冒号分隔的 SHA-256；Xray 26.9.9 不接受 Base64");
        }
    }
    if value.split(',').all(|item| item.trim().is_empty()) {
        bail!("pinnedPeerCertSha256 不能为空");
    }
    Ok(value.trim().into())
}

fn required_uuid(raw: &YamlValue, key: &str) -> Result<String> {
    let value = required_string(raw, key)?;
    Uuid::parse_str(&value).context("UUID 格式无效")?;
    Ok(value)
}
fn required_port(raw: &YamlValue) -> Result<u16> {
    let value = raw_u64(raw, "port").context("缺少服务器端口")?;
    u16::try_from(value)
        .ok()
        .filter(|p| *p > 0)
        .context("服务器端口必须为 1-65535")
}
fn required_string(raw: &YamlValue, key: &str) -> Result<String> {
    let value = raw_string(raw, key).context(format!("缺少 {key}"))?;
    if value.trim().is_empty() {
        bail!("{key} 不能为空");
    }
    Ok(value)
}
fn raw_value<'a>(raw: &'a YamlValue, key: &str) -> Option<&'a YamlValue> {
    raw.as_mapping()?.get(YamlValue::String(key.to_string()))
}
fn raw_string(raw: &YamlValue, key: &str) -> Option<String> {
    raw_value(raw, key).and_then(|v| match v {
        YamlValue::String(s) => Some(s.clone()),
        YamlValue::Number(n) => Some(n.to_string()),
        _ => None,
    })
}
fn raw_truthy(raw: &YamlValue, key: &str) -> bool {
    raw_value(raw, key)
        .and_then(|v| match v {
            YamlValue::Bool(b) => Some(*b),
            YamlValue::String(s) => Some(s.eq_ignore_ascii_case("true")),
            _ => None,
        })
        .unwrap_or(false)
}
fn raw_u64(raw: &YamlValue, key: &str) -> Option<u64> {
    raw_value(raw, key).and_then(|v| match v {
        YamlValue::Number(n) => n.as_u64(),
        YamlValue::String(s) => s.parse().ok(),
        _ => None,
    })
}
fn raw_u32(raw: &YamlValue, key: &str) -> Option<u32> {
    raw_u64(raw, key).and_then(|v| u32::try_from(v).ok())
}
fn raw_map(raw: &YamlValue, key: &str) -> Option<serde_yaml::Mapping> {
    raw_value(raw, key)?.as_mapping().cloned()
}
fn raw_map_string(map: &serde_yaml::Mapping, key: &str) -> Option<String> {
    map.get(YamlValue::String(key.to_string()))
        .and_then(|v| match v {
            YamlValue::String(s) => Some(s.clone()),
            YamlValue::Number(n) => Some(n.to_string()),
            _ => None,
        })
}
fn raw_map_string_ref(map: &Option<serde_yaml::Mapping>, key: &str) -> Option<String> {
    map.as_ref().and_then(|m| raw_map_string(m, key))
}
fn raw_string_list(raw: &YamlValue, key: &str) -> Option<Vec<String>> {
    match raw_value(raw, key)? {
        YamlValue::Sequence(a) => Some(a.iter().filter_map(|v| raw_yaml_string(v)).collect()),
        v => raw_yaml_string(v).map(|s| vec![s]),
    }
}
fn raw_yaml_string(value: &YamlValue) -> Option<String> {
    match value {
        YamlValue::String(s) => Some(s.clone()),
        YamlValue::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn node(kind: &str) -> Node {
        Node { name: "fixture".into(), subscription: "test".into(), raw: serde_yaml::from_str(&format!("name: fixture\ntype: {kind}\nserver: example.com\nport: 443\nuuid: 00000000-0000-4000-8000-000000000001\ncipher: aes-128-gcm\npassword: fixture-password\nnetwork: tcp\ntls: true\n")).unwrap() }
    }
    #[test]
    fn dials_server_names_through_the_core_dns_client() {
        let value = render(&node("vless"), "stable-tag").unwrap();
        assert_eq!(value["streamSettings"]["sockopt"]["domainStrategy"], "UseIPv4");
    }
    #[test]
    fn renders_supported_protocols() {
        for kind in ["ss", "vmess", "vless", "trojan", "socks5", "http"] {
            let value = render(&node(kind), "stable-tag").unwrap();
            assert_eq!(value["tag"], "stable-tag");
            assert!(!value.to_string().contains("allowInsecure"));
        }
    }
    #[test]
    fn rejects_unpinned_skip_verify() {
        let mut n = node("vmess");
        n.raw["skip-cert-verify"] = true.into();
        assert!(render(&n, "tag").is_err());
    }
    #[test]
    fn validates_pin_and_rejects_plugins() {
        let mut n = node("ss");
        n.raw["skip-cert-verify"] = true.into();
        n.raw["pinnedPeerCertSha256"] = serde_yaml::Value::String("00".repeat(32));
        assert!(render(&n, "tag").is_ok());
        n.raw["plugin"] = "obfs".into();
        assert!(render(&n, "tag").is_err());
    }
    #[test]
    fn xray_binary_schema_hook_is_optional() {
        let Ok(bin) = std::env::var("XRAY_BINARY") else {
            return;
        };
        assert!(std::path::Path::new(&bin).is_file());
        for kind in [
            "ss",
            "vmess",
            "vless",
            "trojan",
            "socks5",
            "http",
            "hysteria2",
        ] {
            let mut fixture = node(kind);
            if kind == "hysteria2" {
                fixture
                    .raw
                    .as_mapping_mut()
                    .unwrap()
                    .remove(YamlValue::String("password".into()));
                fixture
                    .raw
                    .as_mapping_mut()
                    .unwrap()
                    .remove(YamlValue::String("tls".into()));
            }
            let rendered = render(&fixture, "fixture-outbound").unwrap();
            let config = json!({
                "inbounds": [{"listen":"127.0.0.1","port":19443,"protocol":"socks","settings":{"auth":"noauth"}}],
                "outbounds": [rendered, {"tag":"direct","protocol":"freedom"}],
                "routing": {"rules":[{"inboundTag":["fixture"],"outboundTag":"direct"}]}
            });
            let path =
                std::env::temp_dir().join(format!("myproxy-xray-node-{}.json", Uuid::new_v4()));
            std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
            let result = std::process::Command::new(&bin)
                .args(["run", "-test", "-config"])
                .arg(&path)
                .output();
            let _ = std::fs::remove_file(&path);
            let result = result.expect("spawn XRAY_BINARY");
            assert!(
                result.status.success(),
                "Xray rejected {kind}: {}",
                String::from_utf8_lossy(&result.stdout)
            );
        }
        for network in ["ws", "grpc", "xhttp", "reality"] {
            let mut fixture = node("trojan");
            let map = fixture.raw.as_mapping_mut().unwrap();
            match network {
                "ws" => {
                    map.insert(
                        YamlValue::String("network".into()),
                        YamlValue::String("ws".into()),
                    );
                    map.insert(
                        YamlValue::String("ws-opts".into()),
                        serde_yaml::from_str("path: /fixture\n").unwrap(),
                    );
                }
                "grpc" => {
                    map.insert(
                        YamlValue::String("network".into()),
                        YamlValue::String("grpc".into()),
                    );
                    map.insert(
                        YamlValue::String("grpc-opts".into()),
                        serde_yaml::from_str("grpc-service-name: fixture\n").unwrap(),
                    );
                }
                "xhttp" => {
                    map.insert(
                        YamlValue::String("network".into()),
                        YamlValue::String("xhttp".into()),
                    );
                    map.insert(
                        YamlValue::String("xhttp-opts".into()),
                        serde_yaml::from_str("path: /fixture\nmode: auto\n").unwrap(),
                    );
                }
                "reality" => {
                    map.remove(YamlValue::String("tls".into()));
                    map.insert(YamlValue::String("reality-opts".into()), serde_yaml::from_str("public-key: AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\nshort-id: 0123456789abcdef\n").unwrap());
                }
                _ => unreachable!(),
            }
            let rendered = render(&fixture, "fixture-transport").unwrap();
            let config = json!({
                "inbounds": [{"listen":"127.0.0.1","port":19443,"protocol":"socks","settings":{"auth":"noauth"}}],
                "outbounds": [rendered, {"tag":"direct","protocol":"freedom"}]
            });
            let path = std::env::temp_dir()
                .join(format!("myproxy-xray-transport-{}.json", Uuid::new_v4()));
            std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
            let result = std::process::Command::new(&bin)
                .args(["run", "-test", "-config"])
                .arg(&path)
                .output();
            let _ = std::fs::remove_file(&path);
            let result = result.expect("spawn XRAY_BINARY");
            assert!(
                result.status.success(),
                "Xray rejected {network}: {}",
                String::from_utf8_lossy(&result.stdout)
            );
        }
    }
}
