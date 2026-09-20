use std::sync::Arc;

use anyhow::{bail, Context, Result};
use crate::controller::TrafficSnapshot;
use crate::network_extension::{self, EnableRequest};
use super::{admission, policy, relay, udp};

pub struct CaptureService {
    pub request: EnableRequest,
    admission: admission::AdmissionService,
}

impl CaptureService {
    pub fn prepare() -> Result<Self> {
        let admission = admission::AdmissionService::start(Arc::new(ApplicationRouting))?;
        let (username, password) = admission.probe_credentials();
        let request = EnableRequest {
            revision: super::next_generation(), operation_revision: 0,
            socks_port: admission.relay_port(), username: username.into(), password: password.into(),
            app_admission: Some(admission.bootstrap().clone()),
            process_rules: vec![], dest_rules: vec![], qualified_rules: vec![],
            gfw_domains: vec![], group_ports: vec![], gfw_ports: vec![],
            dns_resolvers: crate::compile::DNS_NAMESERVERS.iter().map(|value| (*value).to_string()).collect(),
            capture_private_networks: true,
        };
        network_extension::prepare_request(&request)?;
        Ok(Self { request, admission })
    }

    pub fn snapshot(&self) -> TrafficSnapshot {
        let mut snapshot = self.admission.snapshot();
        for connection in &mut snapshot.connections {
            connection.id = format!("capture:{}", connection.id);
        }
        snapshot
    }

    pub fn close_one(&self, id: &str) -> Result<()> {
        self.admission.close_one(id.strip_prefix("capture:").context("无效的系统接管连接标识")?)
    }

    pub fn close_all(&self) -> Result<()> { self.admission.close_all() }
}

struct ApplicationRouting;
impl admission::Routing for ApplicationRouting {
    fn decide(&self, request: &admission::FlowRequest) -> Result<admission::AdmittedRoute> {
        let runtime = super::active()?;
        if !runtime.core_alive() { bail!("代理内核未运行"); }
        let strategy = runtime.strategy.read().expect("strategy");
        let generation = runtime.generation.load(std::sync::atomic::Ordering::Acquire);
        let applications = policy::application_identifiers(request.source.executable_path.as_deref(), request.source.bundle_id.as_deref(), request.source.signing_id.as_deref());
        let context = policy::FlowContext {
            host: &request.host, hostname: request.hostname.as_deref(), port: request.port,
            network: &request.network, user_id: request.source.user_id, applications: &applications,
        };
        let trusted_dns = request.kind == "dns"
            && matches!(request.source.signing_id.as_deref(), Some("local.harry.myproxy" | "local.harry.myproxy.xray"))
            && (request.source.team_id.as_deref() == Some("5UAHRS482C")
                || (request.source.process_id.is_none() && request.source.team_id.is_none()));
        let decision = if trusted_dns {
            policy::Decision { allow_direct_fallback: false, route: policy::Route::Direct, chain: vec!["DIRECT".into()], rule: "代理自身的域名解析".into() }
        } else {
            policy::decide_application(&strategy, &runtime.catalog, &runtime.health.read().expect("health"), &context)
        };
        let host = if request.kind == "dns" {
            if request.host.parse::<std::net::IpAddr>().is_ok() { request.host.clone() }
            else { crate::compile::DNS_NAMESERVERS[0].to_string() }
        } else {
            request.hostname.as_ref().filter(|name| !name.is_empty()).unwrap_or(&request.host).clone()
        };
        Ok(admission::AdmittedRoute { generation, decision, host, port: request.port })
    }

    fn generation(&self) -> Result<u64> {
        Ok(super::active()?.generation.load(std::sync::atomic::Ordering::Acquire))
    }

    fn dial(&self, route: &admission::AdmittedRoute) -> Result<relay::Dialed> {
        let runtime = super::active()?;
        runtime.check_admission_generation(route.generation)?;
        let policy::Route::Node(name) = &route.decision.route else { bail!("此连接未获准进入代理转发"); };
        let lane = runtime.lanes.get(name).context("所选节点已不可用")?;
        let stream = relay::dial_socks(lane.address, &lane.username, &lane.password, &route.host, route.port)?;
        runtime.check_admission_generation(route.generation)?;
        Ok(relay::Dialed { stream, chain: route.decision.chain.join(" → "), rule: route.decision.rule.clone() })
    }

    fn datagram_route(&self, route: &admission::AdmittedRoute) -> Result<udp::DatagramRoute> {
        let runtime = super::active()?;
        runtime.check_admission_generation(route.generation)?;
        let policy::Route::Node(name) = &route.decision.route else { bail!("此数据报未获准进入代理转发"); };
        let lane = runtime.lanes.get(name).context("所选节点已不可用")?;
        let target = udp::DatagramRoute::Socks { address: lane.address, username: lane.username.clone(), password: lane.password.clone(), label: route.decision.chain.join(" → "), fallback_direct: false };
        runtime.check_admission_generation(route.generation)?;
        Ok(target)
    }
}
