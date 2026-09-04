use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::paths;

pub const DEFAULT_EXCLUDE: &str =
    r"(?i)(流量|剩余|到期|官网|重置|过期|剩余流量|套餐到期|过期时间)";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Strategy {
    pub mixed_port: u16,
    pub exclude_filter: String,
    #[serde(default)]
    pub subscriptions: Vec<Subscription>,
    #[serde(default)]
    pub groups: Vec<Group>,
    #[serde(default)]
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    pub id: String,
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub name: String,
    #[serde(default = "default_select")]
    pub kind: String,
    #[serde(default)]
    pub filter: String,
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
}

fn default_select() -> String {
    "select".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    #[serde(default)]
    pub app: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub suffix: String,
    pub via: String,
}

impl Default for Strategy {
    fn default() -> Self {
        Self {
            mixed_port: 17890,
            exclude_filter: DEFAULT_EXCLUDE.into(),
            subscriptions: Vec::new(),
            groups: vec![Group {
                id: Uuid::new_v4().to_string(),
                name: "PROXY".into(),
                kind: "select".into(),
                filter: String::new(),
                include: Vec::new(),
                exclude: Vec::new(),
            }],
            rules: Vec::new(),
        }
    }
}

impl Strategy {
    pub fn load() -> Result<Self> {
        let path = paths::strategy_path()?;
        if !path.exists() {
            let strategy = Self::default();
            strategy.save()?;
            return Ok(strategy);
        }
        let data = fs::read_to_string(&path)
            .with_context(|| format!("read {}", path.display()))?;
        serde_json::from_str(&data).context("parse strategy.json")
    }

    pub fn save(&self) -> Result<()> {
        let path = paths::strategy_path()?;
        let tmp = path.with_extension("json.tmp");
        let data = serde_json::to_string_pretty(self)?;
        fs::write(&tmp, data)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn add_subscription(&mut self, name: String, url: String) -> &Subscription {
        self.subscriptions.push(Subscription {
            id: Uuid::new_v4().to_string(),
            name,
            url,
        });
        self.subscriptions.last().expect("just pushed")
    }

    pub fn remove_subscription(&mut self, id_or_name: &str) -> bool {
        let before = self.subscriptions.len();
        self.subscriptions
            .retain(|s| s.id != id_or_name && s.name != id_or_name);
        self.subscriptions.len() != before
    }

    pub fn add_group(&mut self, name: String, kind: String, filter: String) -> &Group {
        self.groups.push(Group {
            id: Uuid::new_v4().to_string(),
            name,
            kind,
            filter,
            include: Vec::new(),
            exclude: Vec::new(),
        });
        self.groups.last().expect("just pushed")
    }

    pub fn group_mut(&mut self, name: &str) -> Option<&mut Group> {
        self.groups
            .iter_mut()
            .find(|g| g.name == name || g.id == name)
    }

    pub fn remove_group(&mut self, name: &str) -> bool {
        let before = self.groups.len();
        self.groups.retain(|g| g.name != name && g.id != name);
        self.groups.len() != before
    }

    pub fn add_rule(&mut self, rule: Rule) -> &Rule {
        self.rules.push(rule);
        self.rules.last().expect("just pushed")
    }

    pub fn remove_rule(&mut self, id: &str) -> bool {
        let before = self.rules.len();
        self.rules.retain(|r| r.id != id);
        self.rules.len() != before
    }
}

impl Rule {
    pub fn new_app(app: String, via: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            app,
            domain: String::new(),
            suffix: String::new(),
            via,
        }
    }

    pub fn new_domain(domain: String, via: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            app: String::new(),
            domain,
            suffix: String::new(),
            via,
        }
    }

    pub fn new_suffix(suffix: String, via: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            app: String::new(),
            domain: String::new(),
            suffix,
            via,
        }
    }

    pub fn match_label(&self) -> String {
        if !self.app.is_empty() {
            format!("app:{}", self.app)
        } else if !self.domain.is_empty() {
            format!("domain:{}", self.domain)
        } else if !self.suffix.is_empty() {
            format!("*.{}", self.suffix.trim_start_matches('.'))
        } else {
            "any".into()
        }
    }
}

pub fn load_from(path: &Path) -> Result<Strategy> {
    let data = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&data)?)
}
