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
        let resolvers = std::process::Command::new("/usr/sbin/scutil").arg("--dns").output()
            .context("无法读取当前系统 DNS 设置")?;
        let resolvers = system_resolvers(&String::from_utf8_lossy(&resolvers.stdout));
        let direct_dns_resolver = select_resolver(&resolvers, |ip| probe_dns(std::net::SocketAddr::new(ip,53)))
            .context("系统 DNS 服务器没有响应，系统接管未启用；本地代理继续可用")?;
        let admission = admission::AdmissionService::start(Arc::new(ApplicationRouting { direct_dns_resolver }))?;
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

fn system_resolvers(output: &str) -> Vec<std::net::IpAddr> {
    let mut resolvers = Vec::new();
    for address in output.lines().filter_map(|line| {
        let (key, value) = line.trim().split_once(" : ")?;
        if !key.starts_with("nameserver[") { return None; }
        value.trim().parse::<std::net::IpAddr>().ok()
    }) {
        if !resolvers.contains(&address) { resolvers.push(address); }
        if resolvers.len() == 4 { break; }
    }
    resolvers
}

fn select_resolver(resolvers: &[std::net::IpAddr], probe: impl Fn(std::net::IpAddr)->bool + Sync) -> Option<String> {
    std::thread::scope(|scope| {
        let handles = resolvers.iter().map(|&address| {
            let probe = &probe;
            (address, scope.spawn(move || probe(address)))
        }).collect::<Vec<_>>();
        handles.into_iter().filter_map(|(address, result)| result.join().ok().filter(|&ok| ok).map(|_| address.to_string())).next()
    })
}

fn probe_dns(address: std::net::SocketAddr) -> bool {
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};
    let result = (|| -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut stream = std::net::TcpStream::connect_timeout(&address, Duration::from_millis(700))?;
        stream.set_write_timeout(Some(Duration::from_millis(500)))?;
        let id = uuid::Uuid::new_v4();
        let mut query = id.as_bytes()[..2].to_vec();
        query.extend_from_slice(b"\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00\x07example\x03com\x00\x00\x01\x00\x01");
        stream.write_all(&(query.len() as u16).to_be_bytes())?;
        stream.write_all(&query)?;
        let mut read = |bytes: &mut [u8]| -> Result<()> {
            let mut offset = 0;
            while offset < bytes.len() {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() { bail!("DNS probe timed out"); }
                stream.set_read_timeout(Some(remaining))?;
                let size = stream.read(&mut bytes[offset..])?;
                if size == 0 { bail!("DNS probe closed"); }
                offset += size;
            }
            Ok(())
        };
        let mut length=[0;2];read(&mut length)?;
        let size = u16::from_be_bytes(length) as usize;
        if !(12..=4096).contains(&size) { bail!("Invalid DNS response size"); }
        let mut answer=vec![0;size];read(&mut answer)?;
        // Some local resolvers rewrite the transaction id while forwarding a
        // TCP probe. The probe only decides whether this resolver can answer,
        // so validate the DNS response shape and at least one answer instead
        // of rejecting a usable resolver for an id mismatch.
        if answer[2]&128==0 || answer[3]&15!=0 || answer[6..8]==[0,0] { bail!("Invalid DNS response"); }
        Ok(())
    })();
    result.is_ok()
}

struct ApplicationRouting { direct_dns_resolver: String }
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
            else if decision.route == policy::Route::Direct { self.direct_dns_resolver.clone() }
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

#[cfg(test)]
mod tests {
    #[test]
    fn xray_dns_preflight_checks_a_real_framed_answer_and_transaction_id() {
        use std::io::{Read,Write};
        for correct_id in [true,false] {
            let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address=listener.local_addr().unwrap();
            let server=std::thread::spawn(move || {
                let (mut stream,_)=listener.accept().unwrap();
                stream.set_read_timeout(Some(std::time::Duration::from_secs(3))).unwrap();
                let mut length=[0;2];stream.read_exact(&mut length).unwrap();
                let mut query=vec![0;u16::from_be_bytes(length) as usize];stream.read_exact(&mut query).unwrap();
                assert_eq!(&query[12..],b"\x07example\x03com\x00\x00\x01\x00\x01");
                if !correct_id {query[0]^=1;}
                query[2]=0x81;query[3]=0x80;query[7]=1;
                query.extend_from_slice(&[0xc0,0x0c,0,1,0,1,0,0,0,60,0,4,192,0,2,1]);
                stream.write_all(&(query.len() as u16).to_be_bytes()).unwrap();
                stream.write_all(&query).unwrap();
            });
            assert!(super::probe_dns(address));
            server.join().unwrap();
        }
    }

    #[test]
    fn xray_direct_dns_preserves_the_current_system_resolver() {
        let resolvers = super::system_resolvers("DNS configuration\nresolver #1\n  nameserver[0] : 119.29.29.29\n  nameserver[1] : 223.5.5.5\n  nameserver[0] : 119.29.29.29\n");
        assert_eq!(resolvers.iter().map(ToString::to_string).collect::<Vec<_>>(), vec!["119.29.29.29","223.5.5.5"]);
        assert_eq!(super::select_resolver(&resolvers, |ip| ip.to_string()=="223.5.5.5"), Some("223.5.5.5".into()));
        assert_eq!(super::select_resolver(&resolvers, |_| false), None);
        assert_eq!(super::system_resolvers("nameserver[0] : 2606:4700:4700::1111").len(), 1);
        assert!(super::system_resolvers("No DNS configuration available").is_empty());
    }
}
