/// Next step a new user should take on the overview page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupNext {
    AddSubscription,
    RefreshCatalog,
    Connect,
    Ready,
}

/// Subscriptions first, then a populated catalog, then connect.
pub fn next_setup_action(
    subscription_count: usize,
    node_count: usize,
    connected: bool,
) -> SetupNext {
    if subscription_count == 0 {
        SetupNext::AddSubscription
    } else if node_count == 0 {
        SetupNext::RefreshCatalog
    } else if connected {
        SetupNext::Ready
    } else {
        SetupNext::Connect
    }
}

pub fn login_items_path_label() -> &'static str {
    "系统设置 › 通用 › 登录项与扩展"
}

pub const LOGIN_ITEMS_PREFERENCE: &str =
    "x-apple.systempreferences:com.apple.LoginItems-Settings.extension";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_strategy_asks_for_a_subscription() {
        assert_eq!(next_setup_action(0, 0, false), SetupNext::AddSubscription);
    }

    #[test]
    fn unused_subscription_asks_to_refresh() {
        assert_eq!(next_setup_action(1, 0, false), SetupNext::RefreshCatalog);
    }

    #[test]
    fn nodes_without_core_ask_to_connect() {
        assert_eq!(next_setup_action(1, 4, false), SetupNext::Connect);
    }

    #[test]
    fn connected_core_is_ready() {
        assert_eq!(next_setup_action(1, 4, true), SetupNext::Ready);
    }
}
