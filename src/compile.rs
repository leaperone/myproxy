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

    if strategy.system_extension {
        insert_network_extension_listeners(&mut root, strategy);
    } else if strategy.tun {
        insert_tun_intercept(&mut root);
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
                    if !strategy.system_extension {
                        rules.push(format!("PROCESS-NAME,{},{}", matcher.value, target));
                    }
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
            "runtime.yaml nodes={} groups={} rule_sets={} compiled_rules={} tun={} se={}",
            catalog.nodes.len(),
            strategy.groups.len(),
            strategy.rule_sets.len(),
            compiled,
            strategy.tun,
            strategy.system_extension
        ),
    );
    Ok(yaml)
}

fn yaml_strings(items: &[&str]) -> serde_yaml::Value {
    serde_yaml::Value::Sequence(
        items
            .iter()
            .map(|s| serde_yaml::Value::String((*s).into()))
            .collect(),
    )
}

fn insert_tun_intercept(root: &mut serde_yaml::Mapping) {
    let mut tun = serde_yaml::Mapping::new();
    tun.insert("enable".into(), true.into());
    tun.insert("stack".into(), "system".into());
    tun.insert("auto-route".into(), true.into());
    tun.insert("strict-route".into(), true.into());
    tun.insert("auto-detect-interface".into(), true.into());
    tun.insert(
        "dns-hijack".into(),
        yaml_strings(&["any:53", "tcp://any:53"]),
    );
    root.insert("tun".into(), serde_yaml::Value::Mapping(tun));

    let mut dns = serde_yaml::Mapping::new();
    dns.insert("enable".into(), true.into());
    dns.insert("listen".into(), "127.0.0.1:1053".into());
    dns.insert("ipv6".into(), true.into());
    dns.insert("enhanced-mode".into(), "fake-ip".into());
    dns.insert("fake-ip-range".into(), "198.18.0.1/16".into());
    dns.insert(
        "fake-ip-filter".into(),
        yaml_strings(&["*.lan", "*.local", "localhost"]),
    );
    dns.insert(
        "default-nameserver".into(),
        yaml_strings(&["8.8.8.8", "1.1.1.1"]),
    );
    dns.insert("nameserver".into(), yaml_strings(&["1.1.1.1", "8.8.8.8"]));
    dns.insert(
        "proxy-server-nameserver".into(),
        yaml_strings(&["8.8.8.8", "1.1.1.1"]),
    );
    root.insert("dns".into(), serde_yaml::Value::Mapping(dns));

    let mut http = serde_yaml::Mapping::new();
    http.insert(
        "ports".into(),
        serde_yaml::Value::Sequence(vec![
            80.into(),
            serde_yaml::Value::String("8080-8880".into()),
        ]),
    );
    let mut tls = serde_yaml::Mapping::new();
    tls.insert(
        "ports".into(),
        serde_yaml::Value::Sequence(vec![443.into(), 8443.into()]),
    );
    let mut quic = serde_yaml::Mapping::new();
    quic.insert(
        "ports".into(),
        serde_yaml::Value::Sequence(vec![443.into(), 8443.into()]),
    );
    let mut sniff = serde_yaml::Mapping::new();
    sniff.insert("HTTP".into(), serde_yaml::Value::Mapping(http));
    sniff.insert("TLS".into(), serde_yaml::Value::Mapping(tls));
    sniff.insert("QUIC".into(), serde_yaml::Value::Mapping(quic));

    let mut sniffer = serde_yaml::Mapping::new();
    sniffer.insert("enable".into(), true.into());
    sniffer.insert("override-destination".into(), true.into());
    sniffer.insert("force-dns-mapping".into(), true.into());
    sniffer.insert("parse-pure-ip".into(), true.into());
    sniffer.insert("sniff".into(), serde_yaml::Value::Mapping(sniff));
    root.insert("sniffer".into(), serde_yaml::Value::Mapping(sniffer));
}

pub fn network_extension_socks_port(mixed_port: u16) -> u16 {
    let port = mixed_port.saturating_add(1);
    if port == controller_port(mixed_port) {
        port.saturating_add(1)
    } else {
        port
    }
}

fn insert_network_extension_listeners(root: &mut serde_yaml::Mapping, strategy: &Strategy) {
    let plan = crate::network_extension::inbound_plan(strategy);
    let mut listeners = Vec::new();
    push_socks_pair(
        &mut listeners,
        "myproxy-network-extension-socks",
        plan.socks_port,
        None,
        &plan.username,
        &plan.password,
    );
    for group in &plan.group_ports {
        let suffix = group
            .name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect::<String>();
        push_socks_pair(
            &mut listeners,
            &format!("myproxy-network-extension-socks-route-{suffix}"),
            group.port,
            Some(group.name.as_str()),
            &plan.username,
            &plan.password,
        );
    }
    root.insert("listeners".into(), serde_yaml::Value::Sequence(listeners));
}

fn push_socks_pair(
    listeners: &mut Vec<serde_yaml::Value>,
    name_prefix: &str,
    port: u16,
    outbound: Option<&str>,
    username: &str,
    password: &str,
) {
    for (suffix, host) in [("ipv4", "127.0.0.1"), ("ipv6", "::1")] {
        let mut item = serde_yaml::Mapping::new();
        item.insert(
            "name".into(),
            format!("{name_prefix}-{suffix}").into(),
        );
        item.insert("type".into(), "socks".into());
        item.insert("listen".into(), host.into());
        item.insert("port".into(), port.into());
        item.insert("udp".into(), true.into());
        if let Some(proxy) = outbound {
            item.insert("proxy".into(), proxy.into());
        }
        let mut user = serde_yaml::Mapping::new();
        user.insert("username".into(), username.into());
        user.insert("password".into(), password.into());
        item.insert(
            "users".into(),
            serde_yaml::Value::Sequence(vec![serde_yaml::Value::Mapping(user)]),
        );
        listeners.push(serde_yaml::Value::Mapping(item));
    }
}

pub fn via_target(via: &str, strategy: &Strategy) -> String {
    let via = via.trim();
    match via.to_ascii_lowercase().as_str() {
        "direct" => "DIRECT".into(),
        "reject" => "REJECT".into(),
        _ => {
            if let Some(name) = via.strip_prefix("node:") {
                name.to_string()
            } else if let Some(name) = via.strip_prefix("group:") {
                resolve_group_target(name, strategy)
            } else {
                resolve_group_target(via, strategy)
            }
        }
    }
}

fn resolve_group_target(via: &str, strategy: &Strategy) -> String {
    if let Some(group) = strategy
        .groups
        .iter()
        .find(|group| group.name.eq_ignore_ascii_case(via))
    {
        return group.name.clone();
    }
    if via.eq_ignore_ascii_case("default") || via.eq_ignore_ascii_case("proxy") {
        return default_group(strategy).to_string();
    }
    via.to_string()
}

pub fn default_group(strategy: &Strategy) -> &str {
    strategy
        .groups
        .iter()
        .find(|group| group.name == "PROXY" || group.name.eq_ignore_ascii_case("default"))
        .or_else(|| strategy.groups.first())
        .map(|group| group.name.as_str())
        .unwrap_or("DIRECT")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::{Group, Strategy};

    fn strategy_with_groups(names: &[&str]) -> Strategy {
        let mut strategy = Strategy::default();
        strategy.groups = names
            .iter()
            .map(|name| Group::all_nodes((*name).into(), "select".into()))
            .collect();
        strategy
    }

    #[test]
    fn via_target_resolves_default_to_stored_group_name() {
        let strategy = strategy_with_groups(&["Default", "Telegram"]);
        assert_eq!(via_target("default", &strategy), "Default");
        assert_eq!(via_target("Default", &strategy), "Default");
        assert_eq!(via_target("PROXY", &strategy), "Default");
        assert_eq!(via_target("proxy", &strategy), "Default");
        assert_eq!(via_target("group:default", &strategy), "Default");
    }

    #[test]
    fn via_target_resolves_proxy_alias_when_group_is_proxy() {
        let strategy = strategy_with_groups(&["PROXY", "Telegram"]);
        assert_eq!(via_target("default", &strategy), "PROXY");
        assert_eq!(via_target("PROXY", &strategy), "PROXY");
    }

    #[test]
    fn via_target_preserves_direct_reject_and_named_groups() {
        let strategy = strategy_with_groups(&["Default", "AI Proxy"]);
        assert_eq!(via_target("direct", &strategy), "DIRECT");
        assert_eq!(via_target("REJECT", &strategy), "REJECT");
        assert_eq!(via_target("ai proxy", &strategy), "AI Proxy");
        assert_eq!(via_target("node:Some Node", &strategy), "Some Node");
    }

    #[test]
    fn default_group_prefers_proxy_or_default_name() {
        let strategy = strategy_with_groups(&["AI Proxy", "Default"]);
        assert_eq!(default_group(&strategy), "Default");
    }
}
