use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use myproxy::catalog;
use myproxy::controller;
use myproxy::paths;
use myproxy::strategy::{self, InboundMode, Matcher, RoutingProfile, Strategy, GLOBAL_GROUP};
use myproxy::supervisor::Supervisor;

#[derive(Parser)]
#[command(name = "myproxyctl", version = myproxy::updates::VERSION, about = "Configure myproxy without the window")]
struct Cli {
    /// Emit one JSON value on stdout for Agent and automation use.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Capabilities,
    Log,
    Status,
    Apply,
    Connect,
    Disconnect,
    Port {
        port: u16,
    },
    Tun {
        state: String,
    },
    Extension {
        state: String,
    },
    /// Mixed inbound routing: `rule`, `proxy`, `global`, or `direct`.
    MixedMode {
        mode: Option<String>,
    },
    /// System Extension inbound routing: `rule`, `proxy`, `global`, or `direct`.
    ExtensionMode {
        mode: Option<String>,
    },
    /// Mihomo GLOBAL selector: omit to read, pass a member to switch.
    Global {
        name: Option<String>,
    },
    /// Rule-page fallback: `allowlist`, `gfwlist`, or `group` plus optional via.
    Routing {
        profile: Option<String>,
        via: Option<String>,
    },
    /// Compat for unmatched traffic: `direct` → allowlist, a group name → group.
    Unmatched {
        /// `direct`, `group` (the default group), or a group name such as `default` / `PROXY`.
        via: Option<String>,
    },
    Filter {
        #[arg(long)]
        set: Option<String>,
    },
    Subscription {
        #[command(subcommand)]
        cmd: SubCmd,
    },
    Group {
        #[command(subcommand)]
        cmd: GroupCmd,
    },
    Rule {
        #[command(subcommand)]
        cmd: RuleCmd,
    },
}

#[derive(Subcommand)]
enum SubCmd {
    List,
    Add {
        url: String,
        #[arg(long)]
        name: Option<String>,
    },
    Remove {
        id: String,
    },
}

#[derive(Subcommand)]
enum GroupCmd {
    List,
    Add {
        name: String,
        #[arg(long, default_value = "select", value_parser = ["select", "fallback", "url-test"])]
        kind: String,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        source: Vec<String>,
        #[arg(long = "contains")]
        contains: Vec<String>,
        #[arg(long = "not-contains")]
        not_contains: Vec<String>,
    },
    Set {
        name: String,
        #[arg(long, value_parser = ["select", "fallback", "url-test"])]
        kind: Option<String>,
        #[arg(long)]
        all: Option<bool>,
        #[arg(long)]
        source: Vec<String>,
        #[arg(long = "contains")]
        contains: Vec<String>,
        #[arg(long = "not-contains")]
        not_contains: Vec<String>,
    },
    Remove {
        name: String,
    },
    Include {
        group: String,
        node: String,
    },
    Exclude {
        group: String,
        node: String,
    },
}

#[derive(Subcommand)]
enum RuleCmd {
    List,
    Add {
        #[arg(long)]
        name: Option<String>,
        #[arg(long, default_value = "")]
        app: String,
        #[arg(long, default_value = "")]
        domain: String,
        #[arg(long, default_value = "")]
        suffix: String,
        #[arg(long, default_value = "")]
        keyword: String,
        #[arg(long, default_value = "")]
        cidr: String,
        #[arg(long)]
        via: String,
    },
    Remove {
        id: String,
    },
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let json = cli.json;
    myproxy::log::init();
    match run(cli) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            if json {
                eprintln!("{}", serde_json::json!({"error": format!("{error:#}")}));
            } else {
                eprintln!("Error: {error:#}");
            }
            std::process::ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let json = cli.json;
    match cli.command {
        Commands::Capabilities => {
            let commands = [
                "status",
                "apply",
                "connect",
                "disconnect",
                "port",
                "tun",
                "extension",
                "mixed-mode",
                "extension-mode",
                "global",
                "routing",
                "unmatched",
                "filter",
                "subscription",
                "group",
                "rule",
                "log",
            ];
            emit(
                json,
                serde_json::json!({"commands": commands, "json": true, "version": myproxy::updates::VERSION}),
                commands.join(" "),
            );
        }
        Commands::Log => {
            let path = paths::app_log_path()?;
            if !path.exists() {
                emit(
                    json,
                    serde_json::json!({"lines": [], "path": path.display().to_string()}),
                    format!("no log yet: {}", path.display()),
                );
                return Ok(());
            }
            let text = std::fs::read_to_string(&path)?;
            let lines: Vec<_> = text
                .lines()
                .rev()
                .take(80)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            if json {
                println!(
                    "{}",
                    serde_json::json!({"lines": lines, "path": path.display().to_string()})
                );
            } else {
                for line in lines {
                    println!("{line}");
                }
            }
        }
        Commands::Status => {
            let strategy = Strategy::load()?;
            let catalog = catalog::Catalog::load()?;
            let unmatched = myproxy::compile::unmatched_target(&strategy);
            let status = serde_json::json!({
                "mixed_port": strategy.mixed_port,
                "mixed_mode": strategy.mixed_mode.as_str(),
                "global": strategy.global_selected.as_str(),
                "tun": strategy.tun,
                "extension": strategy.system_extension,
                "extension_mode": strategy.extension_mode.as_str(),
                "routing": strategy.routing_profile.as_str(),
                "unmatched": unmatched,
                "unmatched_via": strategy.unmatched_via,
                "subscriptions": strategy.subscriptions.len(),
                "nodes": catalog.nodes.len(),
                "excluded": catalog.excluded.len(),
                "groups": strategy.groups.len(),
                "rules": strategy.rule_sets.len(),
                "strategy": paths::strategy_path()?.display().to_string(),
            });
            emit(
                json,
                status,
                format!(
                    "mixed-port {}  mixed-mode {}  global {}  tun {}  extension {}  extension-mode {}  routing {}  unmatched {}  subs {}  nodes {}  excluded {}  groups {}  rules {}",
                    strategy.mixed_port,
                    strategy.mixed_mode.as_str(),
                    if strategy.global_selected.is_empty() {
                        "—"
                    } else {
                        strategy.global_selected.as_str()
                    },
                    if strategy.tun { "on" } else { "off" },
                    if strategy.system_extension {
                        "on"
                    } else {
                        "off"
                    },
                    strategy.extension_mode.as_str(),
                    strategy.routing_profile.as_str(),
                    unmatched,
                    strategy.subscriptions.len(),
                    catalog.nodes.len(),
                    catalog.excluded.len(),
                    strategy.groups.len(),
                    strategy.rule_sets.len()
                ),
            );
            if !json {
                println!("strategy {}", paths::strategy_path()?.display());
            }
        }
        Commands::Apply => {
            myproxy::log::debug("ctl", "apply");
            let strategy = Strategy::load()?;
            let supervisor = Supervisor::default();
            supervisor.adopt_running(strategy.tun, strategy.system_extension, strategy.mixed_port);
            let catalog = supervisor.apply(&strategy)?;
            emit(
                json,
                serde_json::json!({
                    "status": "applied",
                    "nodes": catalog.nodes.len(),
                    "excluded": catalog.excluded.len(),
                    "runtime_yaml": paths::runtime_yaml_path()?.display().to_string(),
                }),
                format!(
                    "applied {} nodes, {} excluded → {}",
                    catalog.nodes.len(),
                    catalog.excluded.len(),
                    paths::runtime_yaml_path()?.display()
                ),
            );
        }
        Commands::Connect => {
            myproxy::log::debug("ctl", "connect");
            let strategy = Strategy::load()?;
            Supervisor::default().connect(&strategy)?;
            emit(
                json,
                serde_json::json!({
                    "status": "connected",
                    "mixed_port": strategy.mixed_port,
                    "transport": if strategy.system_extension { "extension" } else if strategy.tun { "tun" } else { "mixed" },
                }),
                format!(
                    "connected mixed-port {}{}",
                    strategy.mixed_port,
                    if strategy.system_extension {
                        " se"
                    } else if strategy.tun {
                        " tun"
                    } else {
                        ""
                    }
                ),
            );
        }
        Commands::Disconnect => {
            myproxy::log::debug("ctl", "disconnect");
            Supervisor::default().disconnect()?;
            emit(
                json,
                serde_json::json!({"status": "disconnected"}),
                "disconnected",
            );
        }
        Commands::Port { port } => {
            let mut strategy = Strategy::load()?;
            strategy.mixed_port = port;
            strategy.save()?;
            emit(
                json,
                serde_json::json!({"mixed_port": port}),
                format!("mixed-port {port}"),
            );
        }
        Commands::Tun { state } => {
            let on = match state.as_str() {
                "on" | "true" | "1" => true,
                "off" | "false" | "0" => false,
                _ => bail!("tun on|off"),
            };
            let mut strategy = Strategy::load()?;
            strategy.tun = on;
            strategy.save()?;
            emit(
                json,
                serde_json::json!({"tun": on}),
                format!("tun {}", if on { "on" } else { "off" }),
            );
        }
        Commands::Extension { state } => {
            let on = match state.as_str() {
                "on" | "true" | "1" => true,
                "off" | "false" | "0" => false,
                _ => bail!("extension on|off"),
            };
            let mut strategy = Strategy::load()?;
            strategy.system_extension = on;
            if on {
                strategy.tun = false;
            }
            strategy.save()?;
            emit(
                json,
                serde_json::json!({"extension": on, "tun": strategy.tun}),
                format!("extension {}", if on { "on" } else { "off" }),
            );
        }
        Commands::MixedMode { mode } => {
            let mut strategy = Strategy::load()?;
            if let Some(mode) = mode {
                strategy.mixed_mode = InboundMode::parse(&mode)?;
                if strategy.mixed_mode == InboundMode::Global {
                    strategy.ensure_global_selected();
                }
                strategy.save()?;
            }
            emit(
                json,
                serde_json::json!({
                    "mixed_mode": strategy.mixed_mode.as_str(),
                    "global": strategy.global_selected.as_str(),
                }),
                format!("mixed-mode {}", strategy.mixed_mode.as_str()),
            );
        }
        Commands::ExtensionMode { mode } => {
            let mut strategy = Strategy::load()?;
            if let Some(mode) = mode {
                strategy.extension_mode = InboundMode::parse(&mode)?;
                if strategy.extension_mode == InboundMode::Global {
                    strategy.ensure_global_selected();
                }
                strategy.save()?;
            }
            emit(
                json,
                serde_json::json!({
                    "extension_mode": strategy.extension_mode.as_str(),
                    "global": strategy.global_selected.as_str(),
                }),
                format!("extension-mode {}", strategy.extension_mode.as_str()),
            );
        }
        Commands::Global { name } => {
            let mut strategy = Strategy::load()?;
            let mut live = false;
            if let Some(name) = name {
                let name = name.trim();
                if name.is_empty() {
                    bail!("global <node>");
                }
                strategy.set_global_selected(name.to_string());
                strategy.save()?;
                live = controller::select_proxy(strategy.mixed_port, GLOBAL_GROUP, name).is_ok();
            }
            emit(
                json,
                serde_json::json!({
                    "global": strategy.global_selected.as_str(),
                    "live": live,
                    "mixed_mode": strategy.mixed_mode.as_str(),
                    "extension_mode": strategy.extension_mode.as_str(),
                }),
                if strategy.global_selected.is_empty() {
                    "global —".into()
                } else if live {
                    format!("global {} (live)", strategy.global_selected)
                } else {
                    format!("global {}", strategy.global_selected)
                },
            );
        }
        Commands::Routing { profile, via } => {
            let mut strategy = Strategy::load()?;
            if let Some(profile) = profile {
                let profile = RoutingProfile::parse(&profile)?;
                strategy.set_routing_profile(profile);
                if profile == RoutingProfile::Group {
                    if let Some(via) = via.as_deref() {
                        let via = via.trim();
                        if via.is_empty() {
                            bail!("routing group <via>");
                        }
                        strategy.unmatched_via = if via.eq_ignore_ascii_case("group") {
                            myproxy::compile::default_group(&strategy).to_string()
                        } else {
                            via.to_string()
                        };
                    }
                }
                strategy.save()?;
            }
            let unmatched = myproxy::compile::unmatched_target(&strategy);
            emit(
                json,
                serde_json::json!({
                    "routing": strategy.routing_profile.as_str(),
                    "unmatched": unmatched,
                    "unmatched_via": strategy.unmatched_via,
                }),
                format!(
                    "routing {} unmatched {}",
                    strategy.routing_profile.as_str(),
                    unmatched
                ),
            );
        }
        Commands::Unmatched { via } => {
            let mut strategy = Strategy::load()?;
            if let Some(via) = via {
                let via = via.trim();
                if via.is_empty() {
                    bail!("unmatched direct|<group>");
                }
                if via.eq_ignore_ascii_case("direct") {
                    strategy.set_routing_profile(RoutingProfile::Allowlist);
                    strategy.unmatched_via = "DIRECT".into();
                } else if via.eq_ignore_ascii_case("group") {
                    strategy.set_routing_profile(RoutingProfile::Group);
                    strategy.unmatched_via = myproxy::compile::default_group(&strategy).to_string();
                } else if via.eq_ignore_ascii_case("gfwlist") || via.eq_ignore_ascii_case("gfw") {
                    strategy.set_routing_profile(RoutingProfile::Gfwlist);
                } else {
                    strategy.set_routing_profile(RoutingProfile::Group);
                    strategy.unmatched_via = via.to_string();
                }
                strategy.save()?;
            }
            let unmatched = myproxy::compile::unmatched_target(&strategy);
            emit(
                json,
                serde_json::json!({
                    "routing": strategy.routing_profile.as_str(),
                    "unmatched": unmatched,
                    "unmatched_via": strategy.unmatched_via,
                    "direct": myproxy::compile::unmatched_is_direct(&strategy),
                }),
                format!(
                    "routing {} unmatched {}",
                    strategy.routing_profile.as_str(),
                    unmatched
                ),
            );
        }
        Commands::Filter { set } => {
            let mut strategy = Strategy::load()?;
            if let Some(value) = set {
                strategy.exclude_filter = value;
                strategy.save()?;
            }
            emit(
                json,
                serde_json::json!({"exclude_filter": strategy.exclude_filter}),
                strategy.exclude_filter,
            );
        }
        Commands::Subscription { cmd } => match cmd {
            SubCmd::List => {
                let subscriptions = Strategy::load()?.subscriptions;
                if json {
                    println!("{}", serde_json::json!({"subscriptions": subscriptions}));
                } else {
                    for sub in subscriptions {
                        println!("{}\t{}\t{}", sub.id, sub.name, sub.url);
                    }
                }
            }
            SubCmd::Add { url, name } => {
                let mut strategy = Strategy::load()?;
                let name = name.unwrap_or_else(|| infer_name(&url));
                let added = strategy.add_subscription(name.clone(), url);
                let id = added.id.clone();
                strategy.save()?;
                emit(
                    json,
                    serde_json::json!({"status": "added", "id": id, "name": name}),
                    format!("added {id} {name}"),
                );
            }
            SubCmd::Remove { id } => {
                let mut strategy = Strategy::load()?;
                if !strategy.remove_subscription(&id) {
                    bail!("subscription not found");
                }
                strategy.save()?;
                emit(
                    json,
                    serde_json::json!({"status": "removed", "id": id}),
                    format!("removed {id}"),
                );
            }
        },
        Commands::Group { cmd } => match cmd {
            GroupCmd::List => {
                let strategy = Strategy::load()?;
                let catalog = catalog::Catalog::load()?;
                if json {
                    let groups = strategy
                        .groups
                        .iter()
                        .map(|group| {
                            let mut value = serde_json::to_value(group)?;
                            value["members"] =
                                serde_json::json!(catalog::resolve_group_members(group, &catalog));
                            Ok(value)
                        })
                        .collect::<Result<Vec<_>>>()?;
                    println!("{}", serde_json::json!({"groups": groups}));
                } else {
                    for group in &strategy.groups {
                        let members = catalog::resolve_group_members(group, &catalog);
                        println!(
                            "{}\t{}\t{}\tmembers={}",
                            group.name,
                            group.kind,
                            group.policy_label(),
                            members.len()
                        );
                    }
                }
            }
            GroupCmd::Add {
                name,
                kind,
                all,
                source,
                contains,
                not_contains,
            } => {
                let kind = strategy::Group::parse_kind(&kind)?;
                let mut strategy = Strategy::load()?;
                let mut group = if all {
                    let mut group = strategy::Group::all_nodes(name.clone(), kind);
                    group.sources = source;
                    group
                } else {
                    strategy::Group::matching(name.clone(), kind, source, contains)
                };
                group.name_excludes = not_contains;
                strategy.add_group(group);
                strategy.save()?;
                emit(
                    json,
                    serde_json::json!({"status": "added", "name": name}),
                    format!("added group {name}"),
                );
            }
            GroupCmd::Set {
                name,
                kind,
                all,
                source,
                contains,
                not_contains,
            } => {
                let kind = kind
                    .as_deref()
                    .map(strategy::Group::parse_kind)
                    .transpose()?;
                let mut strategy = Strategy::load()?;
                {
                    let some = strategy
                        .group_mut(&name)
                        .ok_or_else(|| anyhow::anyhow!("no group"))?;
                    if let Some(kind) = kind {
                        some.kind = kind;
                    }
                    if let Some(all) = all {
                        some.all_nodes = all;
                    }
                    if !source.is_empty() {
                        some.sources = source;
                    }
                    if !contains.is_empty() {
                        some.name_contains = contains;
                        some.all_nodes = false;
                    }
                    if !not_contains.is_empty() {
                        some.name_excludes = not_contains;
                    }
                }
                strategy.save()?;
                emit(
                    json,
                    serde_json::json!({"status": "updated", "name": name}),
                    format!("updated group {name}"),
                );
            }
            GroupCmd::Remove { name } => {
                let mut strategy = Strategy::load()?;
                if !strategy.remove_group(&name) {
                    bail!("group not found");
                }
                strategy.save()?;
                emit(
                    json,
                    serde_json::json!({"status": "removed", "name": name}),
                    format!("removed group {name}"),
                );
            }
            GroupCmd::Include { group, node } => {
                let mut strategy = Strategy::load()?;
                {
                    let some = strategy
                        .group_mut(&group)
                        .ok_or_else(|| anyhow::anyhow!("no group"))?;
                    if !some.include.contains(&node) {
                        some.include.push(node.clone());
                    }
                }
                strategy.save()?;
                emit(
                    json,
                    serde_json::json!({"status": "included", "group": group, "node": node}),
                    format!("included {node} in {group}"),
                );
            }
            GroupCmd::Exclude { group, node } => {
                let mut strategy = Strategy::load()?;
                {
                    let some = strategy
                        .group_mut(&group)
                        .ok_or_else(|| anyhow::anyhow!("no group"))?;
                    some.include.retain(|n| n != &node);
                    if !some.exclude.contains(&node) {
                        some.exclude.push(node.clone());
                    }
                }
                strategy.save()?;
                emit(
                    json,
                    serde_json::json!({"status": "excluded", "group": group, "node": node}),
                    format!("excluded {node} from {group}"),
                );
            }
        },
        Commands::Rule { cmd } => match cmd {
            RuleCmd::List => {
                let rule_sets = Strategy::load()?.rule_sets;
                if json {
                    println!("{}", serde_json::json!({"rules": rule_sets}));
                } else {
                    for set in rule_sets {
                        println!(
                            "{}\t{}\t{} matchers\t{}",
                            set.name,
                            set.via,
                            set.matchers.len(),
                            set.id
                        );
                        for matcher in set.matchers {
                            println!("  {}\t{}", matcher.kind_label(), matcher.display_value());
                        }
                    }
                }
            }
            RuleCmd::Add {
                name,
                app,
                domain,
                suffix,
                keyword,
                cidr,
                via,
            } => {
                if app.is_empty()
                    && domain.is_empty()
                    && suffix.is_empty()
                    && keyword.is_empty()
                    && cidr.is_empty()
                {
                    bail!("need --app, --domain, --suffix, --keyword, or --cidr");
                }
                let matcher = if !app.is_empty() {
                    Matcher::app(app)
                } else if !keyword.is_empty() {
                    Matcher::keyword(keyword)
                } else if !cidr.is_empty() {
                    Matcher::cidr(cidr)
                } else if !domain.is_empty() {
                    Matcher::domain(domain)
                } else {
                    Matcher::suffix(suffix)
                };
                let name = name
                    .filter(|n| !n.trim().is_empty())
                    .unwrap_or_else(|| matcher.display_value());
                let mut strategy = Strategy::load()?;
                let set = strategy.add_matcher(name, matcher, via);
                let value = serde_json::json!({"status": "added", "id": set.id, "name": set.name, "via": set.via, "matchers": set.matchers.len()});
                let message = format!("{}\t{}\t{} matchers", set.name, set.via, set.matchers.len());
                strategy.save()?;
                emit(json, value, message);
            }
            RuleCmd::Remove { id } => {
                let mut strategy = Strategy::load()?;
                if !strategy.remove_rule(&id) {
                    bail!("rule not found");
                }
                strategy.save()?;
                emit(
                    json,
                    serde_json::json!({"status": "removed", "id": id}),
                    format!("removed rule {id}"),
                );
            }
        },
    }
    Ok(())
}

fn emit(json: bool, value: serde_json::Value, human: impl std::fmt::Display) {
    if json {
        println!("{value}");
    } else {
        println!("{human}");
    }
}

fn infer_name(url: &str) -> String {
    url.split('/')
        .filter(|s| !s.is_empty())
        .last()
        .unwrap_or("subscription")
        .chars()
        .take(24)
        .collect()
}
