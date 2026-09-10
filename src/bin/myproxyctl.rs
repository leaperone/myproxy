use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use myproxy::catalog;
use myproxy::controller;
use myproxy::network_extension::{self, DnsPhase, Phase, RuntimeStatus};
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
    /// Write the current strategy JSON. Omit the path to use Downloads.
    Export {
        path: Option<std::path::PathBuf>,
    },
    /// Replace the live strategy from a JSON file after writing a backup.
    Import {
        path: std::path::PathBuf,
    },
}

#[derive(Subcommand)]
enum SubCmd {
    List,
    /// Refresh subscriptions explicitly and apply the resulting catalog.
    Refresh,
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
        /// Rename the group and update every group reference.
        #[arg(long)]
        rename: Option<String>,
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
                "export",
                "import",
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
            let supervisor = adopted_supervisor(&strategy);
            let runtime = supervisor.runtime_identity();
            let controller_ready = runtime
                .as_ref()
                .is_some_and(|identity| controller::ready(identity.mixed_port).is_ok());
            let extension = network_extension::wait_for_settle(Duration::from_secs(5));
            let unmatched = myproxy::compile::unmatched_target(&strategy);
            emit(json, serde_json::json!({
                "mixed_port": strategy.mixed_port,
                "mixed_mode": strategy.mixed_mode.as_str(),
                "global": strategy.global_selected,
                "tun": strategy.tun,
                "extension": strategy.system_extension,
                "extension_mode": strategy.extension_mode.as_str(),
                "routing": strategy.routing_profile.as_str(),
                "unmatched": unmatched,
                "unmatched_via": strategy.unmatched_via,
                "subscriptions": strategy.subscriptions.len(),
                "nodes": catalog.nodes.len(),
                "excluded": catalog.excluded.len(),
                "refresh_warnings": catalog.refresh_warnings(),
                "groups": strategy.groups.len(),
                "rules": strategy.rule_sets.len(),
                "operation": supervisor.operation_state(),
                "runtime": runtime.as_ref().map(|identity| serde_json::json!({
                    "generation": identity.generation,
                    "mixed_port": identity.mixed_port,
                    "controller_ready": controller_ready,
                })),
                "extension_runtime": extension,
                "strategy": paths::strategy_path()?.display().to_string(),
            }), format!(
                "saved mixed-port {}  mixed-mode {}  tun {}  extension {}  routing {}  unmatched {}\nruntime {}  controller {}  extension {}  DNS {}\nsubs {}  nodes {}  excluded {}  groups {}  rules {}",
                strategy.mixed_port, strategy.mixed_mode.as_str(), strategy.tun,
                strategy.system_extension, strategy.routing_profile.as_str(), unmatched,
                runtime.as_ref().map(|identity| format!("mixed-port {} (generation {})", identity.mixed_port, identity.generation))
                    .unwrap_or_else(|| "disconnected / unverified".into()),
                if controller_ready { "ready" } else { "unavailable" },
                if extension.observed { extension.phase_label() } else { "unknown" },
                if extension.observed { extension.dns_label() } else { "unknown" },
                strategy.subscriptions.len(), catalog.nodes.len(), catalog.excluded.len(),
                strategy.groups.len(), strategy.rule_sets.len()
            ));
            if !json {
                for warning in catalog.refresh_warnings() {
                    println!("{warning}");
                }
                if let Some(message) = extension.message {
                    println!("{message}");
                }
                if let Some(message) = extension.dns_message {
                    println!("{message}");
                }
                println!("strategy {}", paths::strategy_path()?.display());
            }
        }
        Commands::Apply => {
            let strategy = Strategy::load()?;
            let supervisor = adopted_supervisor(&strategy);
            let catalog = supervisor.apply_cached(&strategy)?;
            report_applied(json, &supervisor, &catalog)?;
        }
        Commands::Connect => {
            let strategy = Strategy::load()?;
            let supervisor = adopted_supervisor(&strategy);
            supervisor.connect(&strategy)?;
            let (extension, cancelled) = settle_extension()?;
            let runtime = supervisor.runtime_identity();
            emit(
                json,
                serde_json::json!({
                    "status": "core_ready",
                    "mixed_port": runtime.as_ref().map(|identity| identity.mixed_port),
                    "extension_runtime": extension,
                    "extension_request_cancelled": cancelled,
                }),
                format!(
                    "Mihomo ready; Mixed :{}; extension {}; DNS {}{}",
                    runtime
                        .as_ref()
                        .map(|identity| identity.mixed_port)
                        .unwrap_or(strategy.mixed_port),
                    extension.phase_label(),
                    extension.dns_label(),
                    if cancelled {
                        "; pending enable cancelled; open the signed .app to enable System Extension"
                    } else {
                        ""
                    }
                ),
            );
            extension_outcome(&extension, cancelled)?;
        }
        Commands::Disconnect => {
            let strategy = Strategy::load().unwrap_or_default();
            let supervisor = adopted_supervisor(&strategy);
            supervisor.disconnect()?;
            let (extension, cancelled) = settle_extension()?;
            let complete = extension.observed
                && matches!(
                    extension.phase,
                    Phase::Disabled | Phase::Unsupported | Phase::Unbundled
                );
            emit(
                json,
                serde_json::json!({
                    "status": if complete { "disconnected" } else { "core_stopped" },
                    "extension_runtime": extension,
                    "extension_request_cancelled": cancelled,
                }),
                format!(
                    "Mihomo stopped; extension {}; DNS {}",
                    extension.phase_label(),
                    extension.dns_label()
                ),
            );
            if !complete {
                bail!("core stopped, but System Extension shutdown is not confirmed");
            }
        }
        Commands::Port { port } => {
            let mut strategy = Strategy::load()?;
            strategy.mixed_port = port;
            strategy.save()?;
            emit(
                json,
                serde_json::json!({"mixed_port": port, "status": "saved", "applied": false}),
                format!("saved mixed-port {port}; apply/connect to activate"),
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
            if on {
                strategy.system_extension = false;
            }
            strategy.save()?;
            emit(
                json,
                serde_json::json!({"tun": on, "extension": strategy.system_extension, "status": "saved", "applied": false}),
                format!(
                    "saved tun {}; apply/connect to activate",
                    if on { "on" } else { "off" }
                ),
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
                serde_json::json!({"extension": on, "tun": strategy.tun, "status": "saved", "applied": false}),
                format!(
                    "saved extension {}; apply/connect to activate",
                    if on { "on" } else { "off" }
                ),
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
            let supervisor = adopted_supervisor(&strategy);
            let runtime = supervisor.runtime_identity();
            let mut live = None;
            if let Some(name) = name {
                let name = name.trim();
                if name.is_empty() {
                    bail!("global <node>");
                }
                if supervisor.is_busy() {
                    bail!("another core operation is in progress");
                }
                if let Some(identity) = runtime.as_ref() {
                    let groups = controller::fetch_proxies(identity.mixed_port)?;
                    let global = groups
                        .iter()
                        .find(|group| group.name == GLOBAL_GROUP)
                        .context("GLOBAL selector missing from the running core")?;
                    if !global.members.iter().any(|member| member.name == name) {
                        bail!("{name} is not a member of the running GLOBAL selector");
                    }
                } else {
                    let catalog = catalog::Catalog::load()?;
                    if !matches!(name, "DIRECT" | "REJECT")
                        && !strategy.groups.iter().any(|group| group.name == name)
                        && !catalog.nodes.iter().any(|node| {
                            node.name == name
                                && strategy
                                    .subscriptions
                                    .iter()
                                    .any(|sub| sub.name == node.subscription)
                        })
                    {
                        bail!("GLOBAL member does not exist: {name}");
                    }
                }
                strategy.set_global_selected(name.to_string());
                strategy.save()?;
                if let Some(identity) = runtime {
                    supervisor
                        .select_proxy(identity, GLOBAL_GROUP, name)
                        .context("GLOBAL selection saved but not applied")?;
                    live = Some(name.to_string());
                }
            } else if let Some(identity) = runtime {
                let groups = controller::fetch_proxies(identity.mixed_port)?;
                let global = groups
                    .into_iter()
                    .find(|group| group.name == GLOBAL_GROUP)
                    .context("GLOBAL selector missing from the running core")?;
                live = Some(global.now);
            }
            emit(
                json,
                serde_json::json!({
                    "global": strategy.global_selected,
                    "live": live.is_some(),
                    "runtime_global": live,
                    "mixed_mode": strategy.mixed_mode.as_str(),
                    "extension_mode": strategy.extension_mode.as_str(),
                }),
                format!(
                    "global saved {}; runtime {}",
                    if strategy.global_selected.is_empty() {
                        "—"
                    } else {
                        &strategy.global_selected
                    },
                    live.as_deref().unwrap_or("disconnected")
                ),
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
            SubCmd::Refresh => {
                let strategy = Strategy::load()?;
                let supervisor = adopted_supervisor(&strategy);
                let catalog = supervisor.apply(&strategy)?;
                report_applied(json, &supervisor, &catalog)?;
            }
            SubCmd::List => {
                let subscriptions: Vec<_> = Strategy::load()?
                    .subscriptions
                    .into_iter()
                    .map(|sub| serde_json::json!({"id": sub.id, "name": sub.name}))
                    .collect();
                if json {
                    println!("{}", serde_json::json!({"subscriptions": subscriptions}));
                } else {
                    for sub in subscriptions {
                        println!(
                            "{}\t{}",
                            sub["id"].as_str().unwrap_or_default(),
                            sub["name"].as_str().unwrap_or_default()
                        );
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
                rename,
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
                let mut group = strategy
                    .groups
                    .iter()
                    .find(|group| group.name == name || group.id == name)
                    .cloned()
                    .context("group not found")?;
                let id = group.id.clone();
                if let Some(rename) = rename {
                    group.name = rename.trim().to_string();
                }
                if let Some(kind) = kind {
                    group.kind = kind;
                }
                if let Some(all) = all {
                    group.all_nodes = all;
                }
                if !source.is_empty() {
                    group.sources = source;
                }
                if !contains.is_empty() {
                    group.name_contains = contains;
                    group.all_nodes = false;
                }
                if !not_contains.is_empty() {
                    group.name_excludes = not_contains;
                }
                let name = group.name.clone();
                strategy.update_group(&id, group)?;
                strategy.save()?;
                emit(
                    json,
                    serde_json::json!({"status": "updated", "name": name}),
                    format!("updated group {name}"),
                );
            }
            GroupCmd::Remove { name } => {
                let mut strategy = Strategy::load()?;
                strategy.remove_group_checked(&name)?;
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
                let mut matchers = Vec::new();
                for (kind, raw) in [
                    ("app", app),
                    ("domain", domain),
                    ("suffix", suffix),
                    ("keyword", keyword),
                    ("cidr", cidr),
                ] {
                    for value in strategy::parse_matcher_list(&raw) {
                        let matcher = if kind == "suffix" {
                            Matcher::suffix(value)
                        } else {
                            Matcher {
                                kind: kind.into(),
                                value,
                            }
                        };
                        matcher.validate()?;
                        if !matchers
                            .iter()
                            .any(|existing: &Matcher| existing.same_as(&matcher))
                        {
                            matchers.push(matcher);
                        }
                    }
                }
                let first = matchers
                    .first()
                    .context("need --app, --domain, --suffix, --keyword, or --cidr")?;
                let name = name
                    .filter(|name| !name.trim().is_empty())
                    .map(|name| name.trim().to_string())
                    .unwrap_or_else(|| first.display_value());
                let mut strategy = Strategy::load()?;
                if strategy
                    .rule_sets
                    .iter()
                    .any(|set| set.name.eq_ignore_ascii_case(&name))
                {
                    bail!("duplicate rule name: {name}; edit the existing rule instead");
                }
                let set = strategy.add_rule_set(strategy::RuleSet {
                    id: uuid::Uuid::new_v4().to_string(),
                    name,
                    via: via.trim().to_string(),
                    matchers,
                });
                let value = serde_json::json!({"status": "added", "id": set.id, "name": set.name, "via": set.via, "matchers": set.matchers.len(), "applied": false});
                let message = format!(
                    "saved {}\t{}\t{} matchers; apply/connect to activate",
                    set.name,
                    set.via,
                    set.matchers.len()
                );
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
        Commands::Export { path } => {
            let strategy = Strategy::load()?;
            let path = match path {
                Some(path) => path,
                None => strategy::default_export_path()?,
            };
            strategy::export_to(&strategy, &path)?;
            emit(
                json,
                serde_json::json!({
                    "path": path.display().to_string(),
                    "schema": strategy.schema,
                    "subscriptions": strategy.subscriptions.len(),
                    "groups": strategy.groups.len(),
                    "rules": strategy.rule_sets.len(),
                }),
                format!(
                    "exported {} subscriptions, {} groups, {} rules to {}",
                    strategy.subscriptions.len(),
                    strategy.groups.len(),
                    strategy.rule_sets.len(),
                    path.display()
                ),
            );
        }
        Commands::Import { path } => {
            let outcome = strategy::import_from(&path)?;
            emit(
                json,
                serde_json::json!({
                    "path": path.display().to_string(),
                    "backup": outcome.backup.as_ref().map(|backup| backup.display().to_string()),
                    "schema": outcome.strategy.schema,
                    "subscriptions": outcome.strategy.subscriptions.len(),
                    "groups": outcome.strategy.groups.len(),
                    "rules": outcome.strategy.rule_sets.len(),
                    "applied": false,
                }),
                format!(
                    "imported {} subscriptions, {} groups, {} rules{}; apply/connect to activate",
                    outcome.strategy.subscriptions.len(),
                    outcome.strategy.groups.len(),
                    outcome.strategy.rule_sets.len(),
                    outcome
                        .backup
                        .as_ref()
                        .map(|backup| format!("; backup {}", backup.display()))
                        .unwrap_or_default()
                ),
            );
        }
    }
    Ok(())
}

fn adopted_supervisor(strategy: &Strategy) -> std::sync::Arc<Supervisor> {
    let supervisor = Supervisor::shared();
    supervisor.adopt_running(strategy.tun, strategy.system_extension, strategy.mixed_port);
    supervisor
}

fn settle_extension() -> Result<(RuntimeStatus, bool)> {
    let status = network_extension::wait_for_settle(Duration::from_secs(30));
    if status.is_pending() || status.phase == Phase::WaitingApproval {
        let enabling = matches!(status.phase, Phase::Requesting | Phase::WaitingApproval);
        let cancelled = network_extension::cancel_pending_for_cli()?;
        Ok((network_extension::status(), enabling && cancelled))
    } else {
        Ok((status, false))
    }
}

fn extension_outcome(status: &RuntimeStatus, cancelled: bool) -> Result<()> {
    if !status.observed {
        bail!("Mihomo operation completed, but System Extension/DNS status is not confirmed");
    }
    if cancelled || status.phase == Phase::RequiresReboot {
        bail!(
            "Mihomo is ready, but System Extension is not active; finish setup in the signed .app"
        );
    }
    if status.phase == Phase::Failed || status.dns_phase == DnsPhase::Failed {
        bail!(
            "Mihomo is ready, but System Extension/DNS failed: {}",
            status
                .message
                .as_deref()
                .or(status.dns_message.as_deref())
                .unwrap_or("see extension_runtime")
        );
    }
    Ok(())
}

fn report_applied(json: bool, supervisor: &Supervisor, catalog: &catalog::Catalog) -> Result<()> {
    let (extension, cancelled) = settle_extension()?;
    let running = supervisor.runtime_identity().is_some();
    emit(
        json,
        serde_json::json!({
            "status": if running { "core_applied" } else { "prepared" },
            "nodes": catalog.nodes.len(), "excluded": catalog.excluded.len(),
            "refresh_warnings": catalog.refresh_warnings(),
            "runtime_yaml": paths::runtime_yaml_path()?.display().to_string(),
            "extension_runtime": extension, "extension_request_cancelled": cancelled,
        }),
        format!(
            "{}: {} nodes, {} excluded; extension {}; DNS {}{}",
            if running {
                "core applied"
            } else {
                "prepared; core disconnected"
            },
            catalog.nodes.len(),
            catalog.excluded.len(),
            extension.phase_label(),
            extension.dns_label(),
            if cancelled {
                "; pending enable cancelled; complete setup in the signed .app"
            } else {
                ""
            }
        ),
    );
    if !json {
        for warning in catalog.refresh_warnings() {
            println!("{warning}");
        }
    }
    extension_outcome(&extension, cancelled)
}

fn emit(json: bool, value: serde_json::Value, human: impl std::fmt::Display) {
    if json {
        println!("{value}");
    } else {
        println!("{human}");
    }
}

fn infer_name(_url: &str) -> String {
    // Subscription URLs often end in access tokens; never turn those into names/logs.
    format!(
        "subscription-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    )
}
