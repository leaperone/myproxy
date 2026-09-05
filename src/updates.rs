use serde::{Deserialize, Serialize};

pub const VERSION: &str = env!("MYPROXY_VERSION");

pub fn build_badge() -> Option<&'static str> {
    match env!("MYPROXY_BUILD_CHANNEL") {
        "nightly" => Some("Nightly"),
        "dev" => Some("Dev"),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    Prod,
    Nightly,
}

impl Default for UpdateChannel {
    fn default() -> Self {
        if env!("MYPROXY_BUILD_CHANNEL") == "nightly" {
            Self::Nightly
        } else {
            Self::Prod
        }
    }
}

impl UpdateChannel {
    pub fn label(self) -> &'static str {
        match self {
            Self::Prod => "正式版（Prod）",
            Self::Nightly => "Nightly",
        }
    }

    pub fn feed_url(self) -> &'static str {
        match self {
            Self::Prod => {
                "https://github.com/leaperone/myproxy/releases/latest/download/appcast.xml"
            }
            Self::Nightly => {
                "https://github.com/leaperone/myproxy/releases/download/nightly/appcast.xml"
            }
        }
    }
}
