use std::net::SocketAddr;
use std::process::Command;

use super::policy::{HealthState, NodeHealth};

pub(super) const PRIMARY_TARGET: &str = "https://www.gstatic.com/generate_204";
pub(super) const SECONDARY_TARGET: &str = "https://cp.cloudflare.com/generate_204";

/// Probe the primary target and use the independent provider only when it
/// fails. The returned latency always belongs to the successful target.
pub(super) fn probe_targets(mut probe: impl FnMut(&str) -> Result<u32, String>) -> Result<u32, String> {
    match probe(PRIMARY_TARGET) {
        Ok(delay) => Ok(delay),
        Err(primary) => probe(SECONDARY_TARGET).map_err(|secondary| {
            format!("{}: {}; {}: {}", host(PRIMARY_TARGET), sanitize(&primary), host(SECONDARY_TARGET), sanitize(&secondary))
        }),
    }
}

pub(super) fn probe_lane(address: SocketAddr, username: &str, password: &str) -> Result<u32, String> {
    let credentials = format!("{username}:{password}");
    probe_targets(|target| {
        let started = std::time::Instant::now();
        let output = Command::new("/usr/bin/curl")
            .args([
                "-q", "--silent", "--show-error", "--output", "/dev/null", "--write-out", "%{http_code}",
                "--connect-timeout", "3", "--max-time", "5", "--noproxy", "", "--proxy",
                &format!("socks5h://{address}"), "--proxy-user", &credentials, target,
            ])
            .env_remove("ALL_PROXY")
            .env_remove("HTTPS_PROXY")
            .env_remove("HTTP_PROXY")
            .output()
            .map_err(|error| sanitize(&error.to_string()))?;
        let code = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if output.status.success() && code == "204" {
            return Ok(started.elapsed().as_millis().min(u32::MAX as u128) as u32);
        }
        let reason = if !output.status.success() {
            match output.status.code() {
                Some(5 | 6) => "proxy DNS resolution failed".into(),
                Some(7) => "proxy connection failed".into(),
                Some(28) => "probe timed out".into(),
                Some(35 | 60) => "TLS handshake failed".into(),
                Some(97) => "proxy negotiation failed".into(),
                other => format!("probe process exited with code {other:?}"),
            }
        } else {
            format!("HTTP {}", if code.is_empty() { "unknown" } else { &code })
        };
        Err(reason)
    })
}

pub(super) fn record_probe(item: &mut NodeHealth, result: Result<u32, String>, now_ms: u64) {
    item.last_check_ms = Some(now_ms);
    match result {
        Ok(delay) => {
            item.delay_ms = Some(delay);
            item.failures = 0;
            item.last_success_ms = Some(now_ms);
            item.last_error = None;
            item.state = HealthState::Healthy;
        }
        Err(error) => {
            item.delay_ms = None;
            item.failures = item.failures.saturating_add(1);
            item.last_error = Some(sanitize(&error));
            item.state = if item.failures >= 2 { HealthState::Unavailable } else { HealthState::Degraded };
        }
    }
}

fn host(target: &str) -> &str {
    target.strip_prefix("https://").and_then(|value| value.split('/').next()).unwrap_or(target)
}

fn sanitize(value: &str) -> String {
    value
        .split_whitespace()
        .filter(|part| !part.contains("://") && !part.contains('@') && !part.contains('?'))
        .take(12)
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(160)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn health() -> NodeHealth { NodeHealth { ..Default::default() } }

    #[test]
    fn primary_success_does_not_call_secondary() {
        let mut calls = Vec::new();
        let delay = probe_targets(|target| { calls.push(target.to_string()); Ok(17) }).unwrap();
        assert_eq!(delay, 17);
        assert_eq!(calls, vec![PRIMARY_TARGET]);
    }

    #[test]
    fn primary_failure_uses_secondary_latency() {
        let mut calls = Vec::new();
        let delay = probe_targets(|target| { calls.push(target.to_string()); if target == PRIMARY_TARGET { Err("timeout".into()) } else { Ok(31) } }).unwrap();
        assert_eq!(delay, 31);
        assert_eq!(calls, vec![PRIMARY_TARGET, SECONDARY_TARGET]);
    }

    #[test]
    fn all_fail_then_recovery_transitions_without_touching_destination_failures() {
        let mut item = health();
        item.destination_failures = 9;
        record_probe(&mut item, Err("first".into()), 100);
        assert_eq!(item.state, HealthState::Degraded);
        assert_eq!(item.failures, 1);
        record_probe(&mut item, Err("second".into()), 200);
        assert_eq!(item.state, HealthState::Unavailable);
        record_probe(&mut item, Ok(42), 300);
        assert_eq!(item.state, HealthState::Healthy);
        assert_eq!(item.delay_ms, Some(42));
        assert_eq!(item.destination_failures, 9);
        assert_eq!(item.last_success_ms, Some(300));
    }

    #[test]
    fn first_failure_is_not_unavailable_and_success_delay_is_exact() {
        let mut item = health();
        record_probe(&mut item, Err("primary and secondary".into()), 10);
        assert_eq!(item.state, HealthState::Degraded);
        record_probe(&mut item, Ok(7), 20);
        assert_eq!(item.delay_ms, Some(7));
        assert_eq!(item.failures, 0);
    }
}
