use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use myproxy::catalog;
use myproxy::compile;
use myproxy::paths;
use myproxy::strategy::{Rule, Strategy};
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
    Status,
    Apply,
    Connect,
    Disconnect,
    Port {
        port: u16,
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
        #[arg(long, default_value = "")]
        filter: String,
    },
    Remove { name: String },
    Include { group: String, node: String },
    Exclude { group: String, node: String },
}

#[derive(Subcommand)]
enum RuleCmd {
    List,
    Add {
        #[arg(long, default_value = "")]
        app: String,
        #[arg(long, default_value = "")]
        domain: String,
        #[arg(long, default_value = "")]
        suffix: String,
        #[arg(long)]
        via: String,
    },
    Remove { id: String },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Capabilities => {
            println!(
                "status apply connect disconnect port filter subscription group rule"
            );
        }
        Commands::Status => {
            let strategy = Strategy::load()?;
            let catalog = catalog::Catalog::load()?;
            println!(
                "mixed-port {}  subs {}  nodes {}  excluded {}  groups {}  rules {}",
                strategy.mixed_port,
                strategy.subscriptions.len(),
                catalog.nodes.len(),
                catalog.excluded.len(),
                strategy.groups.len(),
                strategy.rules.len()
            );
            println!("strategy {}", paths::strategy_path()?.display());
        }
        Commands::Apply => {
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
            let strategy = Strategy::load()?;
            Supervisor::default().connect(&strategy)?;
            println!("connected mixed-port {}", strategy.mixed_port);
        }
        Commands::Disconnect => {
            Supervisor::default().disconnect()?;
            println!("disconnected");
        }
        Commands::Port { port } => {
            let mut strategy = Strategy::load()?;
            strategy.mixed_port = port;
            strategy.save()?;
            println!("mixed-port {port}");
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
                        "{}\t{}\tfilter={}\tmembers={}",
                        group.name,
                        group.kind,
                        group.filter,
                        members.len()
                    );
                }
            }
            GroupCmd::Add { name, kind, filter } => {
                let mut strategy = Strategy::load()?;
                strategy.add_group(name.clone(), kind, filter);
                strategy.save()?;
                println!("added group {name}");
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
                for rule in Strategy::load()?.rules {
                    println!("{}\t{}\t{}", rule.id, rule.match_label(), rule.via);
                }
            }
            RuleCmd::Add {
                app,
                domain,
                suffix,
                via,
            } => {
                if app.is_empty() && domain.is_empty() && suffix.is_empty() {
                    bail!("need --app, --domain, or --suffix");
                }
                let mut strategy = Strategy::load()?;
                let rule = if !app.is_empty() {
                    Rule::new_app(app, via)
                } else if !domain.is_empty() {
                    Rule::new_domain(domain, via)
                } else {
                    Rule::new_suffix(suffix, via)
                };
                println!("{}\t{}", rule.id, rule.match_label());
                strategy.add_rule(rule);
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
