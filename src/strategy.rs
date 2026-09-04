use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::paths;

pub const DEFAULT_EXCLUDE: &str =
    r"(?i)(流量|剩余|到期|官网|重置|过期|剩余流量|套餐到期|过期时间)";

pub const STRATEGY_SCHEMA: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Strategy {
    #[serde(default)]
    pub schema: u32,
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
    /// When true, every kept catalog node in `sources` (or all sources) is a member.
    #[serde(default)]
    pub all_nodes: bool,
    /// Subscription display names. Empty means any subscription.
    #[serde(default)]
    pub sources: Vec<String>,
    /// Case-insensitive substrings, OR'd. Ignored when `all_nodes` is true.
    #[serde(default)]
    pub name_contains: Vec<String>,
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Legacy single regex. Read on migrate, never written back.
    #[serde(default, skip_serializing)]
    pub filter: String,
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
    #[serde(default)]
    pub keyword: String,
    pub via: String,
}

impl Default for Strategy {
    fn default() -> Self {
        Self {
            schema: STRATEGY_SCHEMA,
            mixed_port: 17890,
            exclude_filter: DEFAULT_EXCLUDE.into(),
            subscriptions: Vec::new(),
            groups: vec![Group::all_nodes("PROXY".into(), "select".into())],
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
        let mut strategy: Self = serde_json::from_str(&data).context("parse strategy.json")?;
        if strategy.migrate_groups() {
            let _ = strategy.save();
        }
        Ok(strategy)
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

    pub fn add_group(&mut self, group: Group) -> &Group {
        self.groups.push(group);
        self.groups.last().expect("just pushed")
    }

    fn migrate_groups(&mut self) -> bool {
        if self.schema >= STRATEGY_SCHEMA {
            return false;
        }
        for group in &mut self.groups {
            group.migrate_legacy();
        }
        self.schema = STRATEGY_SCHEMA;
        true
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

    pub fn update_rule(&mut self, id: &str, mut next: Rule) -> bool {
        let Some(rule) = self.rules.iter_mut().find(|r| r.id == id) else {
            return false;
        };
        next.id = rule.id.clone();
        *rule = next;
        true
    }

    pub fn set_rule_via(&mut self, id: &str, via: String) -> bool {
        let via = via.trim().to_string();
        if via.is_empty() {
            return false;
        }
        let Some(rule) = self.rules.iter_mut().find(|r| r.id == id) else {
            return false;
        };
        rule.via = via;
        true
    }

    pub fn move_rule(&mut self, id: &str, delta: i32) -> bool {
        let Some(from) = self.rules.iter().position(|r| r.id == id) else {
            return false;
        };
        let to = from as i32 + delta;
        if to < 0 || to >= self.rules.len() as i32 {
            return false;
        }
        let item = self.rules.remove(from);
        self.rules.insert(to as usize, item);
        true
    }

    pub fn remove_rule(&mut self, id: &str) -> bool {
        let before = self.rules.len();
        self.rules.retain(|r| r.id != id);
        self.rules.len() != before
    }
}

impl Group {
    pub fn all_nodes(name: String, kind: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            kind,
            all_nodes: true,
            sources: Vec::new(),
            name_contains: Vec::new(),
            include: Vec::new(),
            exclude: Vec::new(),
            filter: String::new(),
        }
    }

    pub fn matching(
        name: String,
        kind: String,
        sources: Vec<String>,
        name_contains: Vec<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            kind,
            all_nodes: false,
            sources,
            name_contains,
            include: Vec::new(),
            exclude: Vec::new(),
            filter: String::new(),
        }
    }

    pub fn policy_label(&self) -> String {
        if self.all_nodes {
            if self.sources.is_empty() {
                "全部已导入节点".into()
            } else {
                format!("全部 · {}", self.sources.join(" / "))
            }
        } else {
            let mut parts = Vec::new();
            if !self.sources.is_empty() {
                parts.push(format!("来源 {}", self.sources.join(" / ")));
            }
            if !self.name_contains.is_empty() {
                parts.push(format!("名称含 {}", self.name_contains.join(" / ")));
            }
            if !self.include.is_empty() {
                parts.push(format!("钉住 {}", self.include.len()));
            }
            if parts.is_empty() {
                "无自动匹配（仅钉住）".into()
            } else {
                parts.join(" · ")
            }
        }
    }

    fn migrate_legacy(&mut self) {
        if !self.filter.trim().is_empty() {
            self.all_nodes = false;
            if self.name_contains.is_empty() {
                self.name_contains = split_legacy_filter(&self.filter);
            }
        } else if self.name_contains.is_empty()
            && self.sources.is_empty()
            && self.include.is_empty()
        {
            self.all_nodes = true;
        }
        self.filter.clear();
    }
}

pub fn parse_list(raw: &str) -> Vec<String> {
    raw.split([',', '，', '/', '|'])
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect()
}

fn split_legacy_filter(raw: &str) -> Vec<String> {
    let trimmed = raw.trim().trim_start_matches("(?i)").trim_start_matches("(?-i)");
    trimmed
        .split('|')
        .map(|part| {
            part.trim()
                .trim_start_matches("(?i)")
                .trim_matches(|c: char| c == '(' || c == ')')
                .trim()
                .to_string()
        })
        .filter(|part| !part.is_empty() && part != "?i")
        .collect()
}

impl Rule {
    pub fn new_app(app: String, via: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            app,
            domain: String::new(),
            suffix: String::new(),
            keyword: String::new(),
            via,
        }
    }

    pub fn new_domain(domain: String, via: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            app: String::new(),
            domain,
            suffix: String::new(),
            keyword: String::new(),
            via,
        }
    }

    pub fn new_suffix(suffix: String, via: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            app: String::new(),
            domain: String::new(),
            suffix,
            keyword: String::new(),
            via,
        }
    }

    pub fn new_keyword(keyword: String, via: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            app: String::new(),
            domain: String::new(),
            suffix: String::new(),
            keyword,
            via,
        }
    }

    pub fn kind_label(&self) -> &'static str {
        if !self.app.is_empty() {
            "进程"
        } else if !self.keyword.is_empty() {
            "关键字"
        } else if !self.domain.is_empty() {
            "域名"
        } else if !self.suffix.is_empty() {
            "后缀"
        } else {
            "—"
        }
    }

    pub fn match_value(&self) -> &str {
        if !self.app.is_empty() {
            &self.app
        } else if !self.keyword.is_empty() {
            &self.keyword
        } else if !self.domain.is_empty() {
            &self.domain
        } else {
            self.suffix.trim_start_matches('.')
        }
    }

    pub fn match_label(&self) -> String {
        if !self.app.is_empty() {
            format!("app:{}", self.app)
        } else if !self.keyword.is_empty() {
            format!("keyword:{}", self.keyword)
        } else if !self.domain.is_empty() {
            format!("domain:{}", self.domain)
        } else if !self.suffix.is_empty() {
            format!("*.{}", self.suffix.trim_start_matches('.'))
        } else {
            "any".into()
        }
    }

    pub fn matches_query(&self, query: &str) -> bool {
        let q = query.trim();
        if q.is_empty() {
            return true;
        }
        let lower = q.to_lowercase();
        self.kind_label().contains(q)
            || self.match_value().to_lowercase().contains(&lower)
            || self.via.to_lowercase().contains(&lower)
            || self.match_label().to_lowercase().contains(&lower)
    }
}

pub fn load_from(path: &Path) -> Result<Strategy> {
    let data = fs::read_to_string(path)?;
    let mut strategy: Strategy = serde_json::from_str(&data)?;
    strategy.migrate_groups();
    Ok(strategy)
}
