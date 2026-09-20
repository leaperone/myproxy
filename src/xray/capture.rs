use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::Arc;

use anyhow::{Context, Result};

use crate::controller::TrafficSnapshot;
use crate::network_extension::{self, EnableRequest};
use crate::strategy::Strategy;

use super::relay::{Dialer, MixedServer, UdpRouter};
use super::Ingress;

pub struct CaptureService {
    pub request: EnableRequest,
    listeners: HashMap<u16, Arc<MixedServer>>,
    policies: HashMap<u16, Ingress>,
}

impl CaptureService {
    pub fn prepare(strategy: &Strategy, previous: Option<&Self>) -> Result<Self> {
        let request = network_extension::try_inbound_plan(strategy)?;
        network_extension::prepare_request(&request)?;
        let mut required = vec![(request.socks_port, Ingress::Capture)];
        required.extend(request.group_ports.iter().map(|group| (group.port, Ingress::Group(group.name.clone()))));
        let mut listeners = HashMap::new();
        let mut policies = HashMap::new();
        for (port, ingress) in required {
            policies.insert(port, ingress.clone());
            if let Some(existing) = previous.and_then(|old| old.listeners.get(&port)) {
                if previous.is_some_and(|old| old.request.username == request.username && old.request.password == request.password && old.policies.get(&port) == Some(&ingress)) {
                    listeners.insert(port, existing.clone());
                    continue;
                }
            }
            let listener = TcpListener::bind(("127.0.0.1", port)).with_context(|| format!("系统接管入口 {port} 被占用"))?;
            let tcp_ingress = ingress.clone();
            let dialer: Dialer = Arc::new(move |host, port| super::active()?.dial_for(host, port, &tcp_ingress));
            let udp_router: UdpRouter = Arc::new(move |host, port| super::active()?.datagram_route(host, port, &ingress));
            let server = MixedServer::start_authenticated(listener, dialer, request.username.clone(), request.password.clone(), udp_router)?;
            listeners.insert(port, Arc::new(server));
        }
        Ok(Self { request, listeners, policies })
    }

    pub fn snapshot(&self) -> TrafficSnapshot {
        let mut snapshot = TrafficSnapshot { connections: vec![], connection_count: 0, upload_total: 0, download_total: 0 };
        let processes = network_extension::activity_process_by_port(true);
        for (port, listener) in &self.listeners {
            let mut part = listener.snapshot_with_processes(&processes);
            for connection in &mut part.connections {
                connection.id = format!("capture:{port}:{}", connection.id);
                if connection.app_matcher.is_empty() { connection.process = "系统接管".into(); }
            }
            snapshot.connection_count += part.connection_count;
            snapshot.upload_total = snapshot.upload_total.saturating_add(part.upload_total);
            snapshot.download_total = snapshot.download_total.saturating_add(part.download_total);
            snapshot.connections.extend(part.connections);
        }
        snapshot
    }

    pub fn close_one(&self, id: &str) -> Result<()> {
        let (_, rest) = id.split_once(':').context("无效的系统接管连接标识")?;
        let (port, local_id) = rest.split_once(':').context("无效的系统接管连接标识")?;
        self.listeners.get(&port.parse::<u16>()?).context("系统接管入口已关闭")?.close_one(local_id)
    }

    pub fn close_all(&self) -> Result<()> {
        for listener in self.listeners.values() { listener.close_all()?; }
        Ok(())
    }

}
