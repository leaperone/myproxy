use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use myproxy::catalog;
use myproxy::compile;
use myproxy::paths;
use myproxy::strategy::{self, Matcher, Strategy};
use myproxy::supervisor::Supervisor;

#[derive(Parser)]
#[command(name = "myproxyctl", about = "Configure myproxy without the window")]
struct Cli {
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
    Add { url: String, #[arg(long)] name: Option<String> },
    Remove { id: String },
}

#[derive(Subcommand)]
enum GroupCmd {
    List,
    Add {
        name: String,
        #[arg(long, default_value = "select")]
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
        #[arg(long)]
        all: Option<bool>,
        #[arg(long)]
        source: Vec<String>,
        #[arg(long = "contains")]
        contains: Vec<String>,
        #[arg(long = "not-contains")]
        not_contains: Vec<String>,
    },
    Remove { name: String },
    Include { group: String, node: String },
    Exclude { group: String, node: String },
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
    Remove { id: String },
}

fn main() -> Result<()> {
    myproxy::log::init();
    let cli = Cli::parse();
    match cli.command {
        Commands::Capabilities => {
            println!(
                "status apply connect disconnect port tun filter subscription group rule log"
            );
        }
        Commands::Log => {
            let path = paths::app_log_path()?;
            if !path.exists() {
                println!("no log yet: {}", path.display());
                return Ok(());
            }
            let text = std::fs::read_to_string(&path)?;
            for line in text.lines().rev().take(80).collect::<Vec<_>>().into_iter().rev() {
                println!("{line}");
            }
        }
        Commands::Status => {
            let strategy = Strategy::load()?;
            let catalog = catalog::Catalog::load()?;
            println!(
                "mixed-port {}  tun {}  subs {}  nodes {}  excluded {}  groups {}  rules {}",
                strategy.mixed_port,
                if strategy.tun { "on" } else { "off" },
                strategy.subscriptions.len(),
                catalog.nodes.len(),
                catalog.excluded.len(),
                strategy.groups.len(),
                strategy.rule_sets.len()
            );
            println!("strategy {}", paths::strategy_path()?.display());
        }
        Commands::Apply => {
            myproxy::log::debug("ctl", "apply");
            let strategy = Strategy::load()?;
            let catalog = catalog::refresh(&strategy)?;
            compile::compile(&strategy, &catalog)?;
            println!(
                "applied {} nodes, {} excluded → {}",
                catalog.nodes.len(),
                catalog.excluded.len(),
                paths::runtime_yaml_path()?.display()
            );
        }
        Commands::Connect => {
            myproxy::log::debug("ctl", "connect");
            let strategy = Strategy::load()?;
            Supervisor::default().connect(&strategy)?;
            println!(
                "connected mixed-port {}{}",
                strategy.mixed_port,
                if strategy.tun { " tun" } else { "" }
            );
        }
        Commands::Disconnect => {
            myproxy::log::debug("ctl", "disconnect");
            Supervisor::default().disconnect()?;
            println!("disconnected");
        }
        Commands::Port { port } => {
            let mut strategy = Strategy::load()?;
            strategy.mixed_port = port;
            strategy.save()?;
            println!("mixed-port {port}");
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
            println!("tun {}", if on { "on" } else { "off" });
        }
        Commands::Filter { set } => {
            let mut strategy = Strategy::load()?;
            if let Some(value) = set {
                strategy.exclude_filter = value;
                strategy.save()?;
            }
            println!("{}", strategy.exclude_filter);
        }
        Commands::Subscription { cmd } => match cmd {
            SubCmd::List => {
                for sub in Strategy::load()?.subscriptions {
                    println!("{}\t{}\t{}", sub.id, sub.name, sub.url);
                }
            }
            SubCmd::Add { url, name } => {
                let mut strategy = Strategy::load()?;
                let name = name.unwrap_or_else(|| infer_name(&url));
                let added = strategy.add_subscription(name.clone(), url);
                let id = added.id.clone();
                strategy.save()?;
                println!("added {id} {name}");
            }
            SubCmd::Remove { id } => {
                let mut strategy = Strategy::load()?;
                if !strategy.remove_subscription(&id) {
                    bail!("subscription not found");
                }
                strategy.save()?;
            }
        },
        Commands::Group { cmd } => match cmd {
            GroupCmd::List => {
                let strategy = Strategy::load()?;
                let catalog = catalog::Catalog::load()?;
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
            GroupCmd::Add {
                name,
                kind,
                all,
                source,
                contains,
                not_contains,
            } => {
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
                println!("added group {name}");
            }
            GroupCmd::Set {
                name,
                all,
                source,
                contains,
                not_contains,
            } => {
                let mut strategy = Strategy::load()?;
                {
                    let some = strategy
                        .group_mut(&name)
                        .ok_or_else(|| anyhow::anyhow!("no group"))?;
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
                println!("updated group {name}");
            }
            GroupCmd::Remove { name } => {
                let mut strategy = Strategy::load()?;
                if !strategy.remove_group(&name) {
                    bail!("group not found");
                }
                strategy.save()?;
            }
            GroupCmd::Include { group, node } => {
                let mut strategy = Strategy::load()?;
                {
                    let some = strategy
                        .group_mut(&group)
                        .ok_or_else(|| anyhow::anyhow!("no group"))?;
                    if !some.include.contains(&node) {
                        some.include.push(node);
                    }
                }
                strategy.save()?;
            }
            GroupCmd::Exclude { group, node } => {
                let mut strategy = Strategy::load()?;
                {
                    let some = strategy
                        .group_mut(&group)
                        .ok_or_else(|| anyhow::anyhow!("no group"))?;
                    some.include.retain(|n| n != &node);
                    if !some.exclude.contains(&node) {
                        some.exclude.push(node);
                    }
                }
                strategy.save()?;
            }
        },
        Commands::Rule { cmd } => match cmd {
            RuleCmd::List => {
                for set in Strategy::load()?.rule_sets {
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
                println!("{}\t{}\t{} matchers", set.name, set.via, set.matchers.len());
                strategy.save()?;
            }
            RuleCmd::Remove { id } => {
                let mut strategy = Strategy::load()?;
                if !strategy.remove_rule(&id) {
                    bail!("rule not found");
                }
                strategy.save()?;
            }
        },
    }
    Ok(())
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
