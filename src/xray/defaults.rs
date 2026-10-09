use crate::strategy::{Group, InboundMode, RoutingProfile, Strategy};

pub fn default_strategy() -> Strategy {
    let mut strategy = Strategy::default();
    strategy.mixed_port = 40808;
    strategy.mixed_mode = InboundMode::Global;
    strategy.global_selected = "节点选择".into();
    strategy.routing_profile = RoutingProfile::Group;
    strategy.unmatched_via = "节点选择".into();
    strategy.rule_sets.clear();
    let mut selector = Group::all_nodes("节点选择".into(), "select".into());
    selector.group_refs = vec!["美国优先".into(), "日本优先".into(), "香港优先".into()];
    strategy.groups = vec![selector];
    for (name, order) in [
        ("美国优先", ["美国", "日本", "香港"]),
        ("日本优先", ["日本", "香港", "美国"]),
        ("香港优先", ["香港", "美国", "日本"]),
    ] {
        let mut profile = Group::matching(name.into(), "fallback".into(), vec![], vec![]);
        profile.group_refs = order.into_iter().map(str::to_owned).collect();
        strategy.groups.push(profile);
    }
    for (name, patterns) in [
        ("美国", vec!["美国", "*|us|*", "United States", "🇺🇸"]),
        ("日本", vec!["日本", "*|jp|*", "Japan", "🇯🇵"]),
        ("香港", vec!["香港", "*|hk|*", "Hong Kong", "🇭🇰"]),
    ] {
        strategy.groups.push(Group::matching(
            name.into(), "url-test".into(), vec![],
            patterns.into_iter().map(str::to_owned).collect(),
        ));
    }
    strategy
}

