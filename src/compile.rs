use std::fs;

use anyhow::{Context, Result};

use crate::catalog::{self, Catalog};
use crate::log;
use crate::paths;
use crate::strategy::Strategy;

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
        format!("127.0.0.1:{}", strategy.mixed_port.saturating_add(107)).into(),
    );
    root.insert("secret".into(), "myproxy-local".into());

    let proxies: Vec<serde_yaml::Value> = catalog.nodes.iter().map(|n| n.raw.clone()).collect();
    root.insert("proxies".into(), serde_yaml::Value::Sequence(proxies));

    let mut groups = Vec::new();
    for group in &strategy.groups {
        let mut members = catalog::resolve_group_members(group, catalog);
        if members.is_empty() {
            log::warn("compile", format!("group {} empty, using DIRECT", group.name));
            members.push("DIRECT".into());
        }
        let mut item = serde_yaml::Mapping::new();
        item.insert("name".into(), group.name.clone().into());
        let kind = if group.kind == "url-test" {
            "url-test"
        } else {
            "select"
        };
        item.insert("type".into(), kind.into());
        if kind == "url-test" {
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
    for rule in &strategy.rules {
        let target = via_target(&rule.via, strategy);
        if !rule.keyword.is_empty() {
            rules.push(format!("DOMAIN-KEYWORD,{},{}", rule.keyword, target));
        } else if !rule.suffix.is_empty() {
            rules.push(format!(
                "DOMAIN-SUFFIX,{},{}",
                rule.suffix.trim_start_matches('.'),
                target
            ));
        } else if !rule.domain.is_empty() {
            if rule.domain.starts_with("*.") {
                rules.push(format!(
                    "DOMAIN-SUFFIX,{},{}",
                    rule.domain.trim_start_matches("*."),
                    target
                ));
            } else {
                rules.push(format!("DOMAIN,{},{}", rule.domain, target));
            }
        }
        if !rule.app.is_empty() {
            rules.push(format!("PROCESS-NAME,{},{}", rule.app, target));
        }
    }
    rules.push(format!(
        "MATCH,{}",
        default_group(strategy)
    ));
    let rules: Vec<serde_yaml::Value> = rules.into_iter().map(serde_yaml::Value::String).collect();
    root.insert("rules".into(), serde_yaml::Value::Sequence(rules));

    let yaml = serde_yaml::to_string(&serde_yaml::Value::Mapping(root))?;
    let path = paths::runtime_yaml_path()?;
    fs::write(&path, &yaml).with_context(|| format!("write {}", path.display()))?;
    log::info(
        "compile",
        format!(
            "runtime.yaml nodes={} groups={} rules={}",
            catalog.nodes.len(),
            strategy.groups.len(),
            strategy.rules.len()
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
