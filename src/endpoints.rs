//! Local Mihomo listener layout shared by configuration, validation and control.

const CONTROLLER_OFFSET: u16 = 107;
pub const MAX_MIXED_PORT: u16 = u16::MAX - CONTROLLER_OFFSET;

pub const CONTROLLER_SECRET: &str = "myproxy-local";

/// Mirrored by `DNSProxyUpstreamResolver.mihomoDNS`, where the Network
/// Extension DNS provider relays public system lookups.
pub const DNS_LISTEN_PORT: u16 = 1053;

/// Resolvers Mihomo uses for its own lookups.
///
/// macOS hands the Network Extension DNS provider the *queried name* as the
/// flow endpoint instead of the resolver address. When a LAN name, or any
/// name while Mihomo is unavailable, has to be answered directly, the
/// provider dials this list instead of the endpoint it sees.
pub const DNS_NAMESERVERS: [&str; 2] = ["1.1.1.1", "8.8.8.8"];

pub fn controller_port(mixed_port: u16) -> u16 {
    mixed_port.saturating_add(CONTROLLER_OFFSET)
}

pub fn network_extension_socks_port(mixed_port: u16) -> u16 {
    let port = mixed_port.saturating_add(1);
    if port == controller_port(mixed_port) {
        port.saturating_add(1)
    } else {
        port
    }
}
