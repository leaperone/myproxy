use std::fs;

use anyhow::{Context, Result};

use crate::catalog::{self, Catalog};
use crate::log;
use crate::paths;
use crate::strategy::Strategy;

pub const CONTROLLER_SECRET: &str = "myproxy-local";

pub fn controller_port(mixed_port: u16) -> u16 {
    mixed_port.saturating_add(107)
}

pub fn compile(strategy: &Strategy, catalog: &Catalog) -> Result<String> {
    let mut root = serde_yaml::Mapping::new();
    root.insert("mixed-port".into(), strategy.mixed_port.into());
    root.insert("allow-lan".into(), false.into());
    root.insert("bind-address".into(), "127.0.0.1".into());
    root.insert("mode".into(), "rule".into());
    root.insert("log-level".into(), "info".into());
    root.insert("find-process-mode".into(), "always".into());
    root.insert("ipv6".into(), true.into());
    root.insert(
        "external-controller".into(),
        format!("127.0.0.1:{}", controller_port(strategy.mixed_port)).into(),
    );
    root.insert("secret".into(), CONTROLLER_SECRET.into());

    if strategy.tun {
        let mut tun = serde_yaml::Mapping::new();
        tun.insert("enable".into(), true.into());
        tun.insert("stack".into(), "system".into());
        tun.insert("auto-route".into(), true.into());
        tun.insert("auto-detect-interface".into(), true.into());
        let hijack: Vec<serde_yaml::Value> = ["any:53", "tcp://any:53"]
            .into_iter()
            .map(|s| serde_yaml::Value::String(s.into()))
            .collect();
        tun.insert("dns-hijack".into(), serde_yaml::Value::Sequence(hijack));
        root.insert("tun".into(), serde_yaml::Value::Mapping(tun));

        let mut dns = serde_yaml::Mapping::new();
        dns.insert("enable".into(), true.into());
        dns.insert("ipv6".into(), true.into());
        dns.insert("enhanced-mode".into(), "redir-host".into());
        let nameservers: Vec<serde_yaml::Value> = ["1.1.1.1", "8.8.8.8"]
            .into_iter()
            .map(|s| serde_yaml::Value::String(s.into()))
            .collect();
        dns.insert("nameserver".into(), serde_yaml::Value::Sequence(nameservers));
        root.insert("dns".into(), serde_yaml::Value::Mapping(dns));
    }

    let proxies: Vec<serde_yaml::Value> = catalog.nodes.iter().map(|n| n.raw.clone()).collect();
    root.insert("proxies".into(), serde_yaml::Value::Sequence(proxies));

    let mut groups = Vec::new();
    for group in &strategy.groups {
        let mut members = catalog::resolve_group_members(group, catalog);
        if members.is_empty() {
            log::warn(
                "compile",
                format!("group {} empty, using DIRECT", group.name),
            );
            members.push("DIRECT".into());
        }
        let mut item = serde_yaml::Mapping::new();
        item.insert("name".into(), group.name.clone().into());
        let kind = match group.kind.as_str() {
            "url-test" => "url-test",
            "fallback" => "fallback",
            _ => "select",
        };
        item.insert("type".into(), kind.into());
        if kind == "url-test" || kind == "fallback" {
            item.insert("url".into(), "https://www.gstatic.com/generate_204".into());
            item.insert("interval".into(), 300.into());
        }
        let proxies: Vec<serde_yaml::Value> =
            members.into_iter().map(serde_yaml::Value::String).collect();
        item.insert("proxies".into(), serde_yaml::Value::Sequence(proxies));
        groups.push(serde_yaml::Value::Mapping(item));
    }
    root.insert("proxy-groups".into(), serde_yaml::Value::Sequence(groups));

    let mut rules = Vec::new();
    for set in &strategy.rule_sets {
        let target = via_target(&set.via, strategy);
        for matcher in &set.matchers {
            match matcher.kind.as_str() {
                "keyword" => {
                    rules.push(format!("DOMAIN-KEYWORD,{},{}", matcher.value, target));
                }
                "suffix" => {
                    rules.push(format!(
                        "DOMAIN-SUFFIX,{},{}",
                        matcher.value.trim_start_matches('.'),
                        target
                    ));
                }
                "domain" => {
                    if matcher.value.starts_with("*.") {
                        rules.push(format!(
                            "DOMAIN-SUFFIX,{},{}",
                            matcher.value.trim_start_matches("*."),
                            target
                        ));
                    } else {
                        rules.push(format!("DOMAIN,{},{}", matcher.value, target));
                    }
                }
                "app" => {
                    rules.push(format!("PROCESS-NAME,{},{}", matcher.value, target));
                }
                "cidr" => {
                    let value = matcher.value.trim();
                    if value.is_empty() {
                        continue;
                    }
                    let kind = if value.contains(':') {
                        "IP-CIDR6"
                    } else {
                        "IP-CIDR"
                    };
                    rules.push(format!("{kind},{value},{target},no-resolve"));
                }
                _ => {}
            }
        }
    }
    rules.push(format!("MATCH,{}", default_group(strategy)));
    let compiled = rules.len();
    let rules: Vec<serde_yaml::Value> = rules.into_iter().map(serde_yaml::Value::String).collect();
    root.insert("rules".into(), serde_yaml::Value::Sequence(rules));

    let yaml = serde_yaml::to_string(&serde_yaml::Value::Mapping(root))?;
    let path = paths::runtime_yaml_path()?;
    fs::write(&path, &yaml).with_context(|| format!("write {}", path.display()))?;
    log::info(
        "compile",
        format!(
            "runtime.yaml nodes={} groups={} rule_sets={} compiled_rules={} tun={}",
            catalog.nodes.len(),
            strategy.groups.len(),
            strategy.rule_sets.len(),
            compiled,
            strategy.tun
        ),
    );
    Ok(yaml)
}

fn via_target(via: &str, strategy: &Strategy) -> String {
    let via = via.trim();
    match via.to_ascii_lowercase().as_str() {
        "direct" => "DIRECT".into(),
        "reject" => "REJECT".into(),
        _ => {
            if let Some(name) = via.strip_prefix("node:") {
                name.to_string()
            } else if strategy.groups.iter().any(|g| g.name == via) {
                via.to_string()
            } else if via.starts_with("group:") {
                via.trim_start_matches("group:").to_string()
            } else {
                via.to_string()
            }
        }
    }
}

fn default_group(strategy: &Strategy) -> &str {
    strategy
        .groups
        .iter()
        .find(|g| g.name == "PROXY")
        .or_else(|| strategy.groups.first())
        .map(|g| g.name.as_str())
        .unwrap_or("DIRECT")
}
