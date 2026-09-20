//! Xray-only application admission broker.  Policy stays in the Host through
//! `Routing`; this module owns only authentication, framing, and leases.

use super::{policy, relay};
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use uuid::Uuid;

const MAX_FRAME: usize = 32 * 1024;
const MAX_PAYLOAD: usize = 16 * 1024;
const MAX_IN_FLIGHT: usize = 128;
const MAX_NONCES: usize = 8192;
const NONCE_TTL: Duration = Duration::from_secs(30);
const LEASE_TTL: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bootstrap { pub version: u8, pub activation: String, pub port: u16, pub key: String }

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowSource {
    pub process_id: Option<i32>, pub user_id: Option<u32>, pub process_start: Option<String>,
    pub executable_path: Option<String>, pub bundle_id: Option<String>, pub signing_id: Option<String>, pub team_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowRequest {
    pub version: u8, pub activation: String, pub nonce: String, pub flow_id: String,
    pub kind: String, pub network: String, pub host: String, pub hostname: Option<String>, pub port: u16,
    pub source: FlowSource,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdmissionReply {
    pub version: u8, pub activation: String, pub nonce: String, pub action: String, pub generation: u64,
    pub host: String, pub port: u16, pub rule: String, pub chain: Vec<String>,
    pub relay_port: Option<u16>, pub lease: Option<String>, pub password: Option<String>,
}

pub trait Routing: Send + Sync {
    fn decide(&self, request: &FlowRequest) -> Result<AdmittedRoute>;
    fn generation(&self) -> Result<u64>;
    fn dial(&self, route: &AdmittedRoute) -> Result<relay::Dialed>;
    fn datagram_route(&self, route: &AdmittedRoute) -> Result<super::udp::DatagramRoute>;
}
#[derive(Clone, Debug)]
pub struct AdmittedRoute { pub generation: u64, pub decision: policy::Decision, pub host: String, pub port: u16 }

struct Lease { route: Arc<AdmittedRoute>, request: FlowRequest, password: String, expires: Instant }
#[derive(Default)]
struct State {
    leases: HashMap<String, Lease>,
    nonces: VecDeque<(String, Instant)>,
    decisions: VecDeque<(Instant, crate::controller::LiveConnection)>,
}

pub struct AdmissionService {
    bootstrap: Bootstrap, relay_port: u16, probe_user: String, probe_password: String,
    state: Arc<Mutex<State>>, stop: Arc<AtomicBool>, active: Arc<AtomicUsize>,
    broker: Option<JoinHandle<()>>, relay: relay::MixedServer,
}

impl AdmissionService {
    pub fn start(routing: Arc<dyn Routing>) -> Result<Self> {
        #[cfg(not(target_os = "macos"))]
        { let _ = routing; bail!("Xray application admission requires macOS"); }
        #[cfg(target_os = "macos")]
        {
            let key = Arc::new(random_key()?);
            let activation = Uuid::new_v4().to_string();
            let broker_listener = TcpListener::bind(("127.0.0.1", 0))?;
            let relay_listener = TcpListener::bind(("127.0.0.1", 0))?;
            broker_listener.set_nonblocking(true)?;
            let broker_port = broker_listener.local_addr()?.port();
            let relay_port = relay_listener.local_addr()?.port();
            let probe_user = format!("probe-{}", Uuid::new_v4().simple());
            let probe_password = random_text()?;
            let state = Arc::new(Mutex::new(State::default()));
            let stop = Arc::new(AtomicBool::new(false)); let active = Arc::new(AtomicUsize::new(0));
            let service_state = state.clone(); let service_routing = routing.clone(); let service_stop = stop.clone(); let service_active = active.clone();
            let activation_for_thread = activation.clone();
            let broker_key = key.clone();
            let broker = thread::Builder::new().name("myproxy-admission".into()).spawn(move || {
                while !service_stop.load(Ordering::Acquire) {
                    let Ok((mut stream, _)) = broker_listener.accept() else { thread::sleep(Duration::from_millis(2)); continue; };
                    if service_active.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < MAX_IN_FLIGHT).then_some(n + 1)).is_err() { continue; }
                    let state = service_state.clone(); let routing = service_routing.clone(); let activation = activation_for_thread.clone(); let active = service_active.clone();
                    let relay_port_for_thread = relay_port;
                    let request_key = broker_key.clone();
                    let guard = ActiveGuard(active);
                    let _ = thread::Builder::new().name("myproxy-admission-request".into()).spawn(move || {
                        let _guard = guard;
                        let _ = handle_request(&mut stream, &activation, relay_port_for_thread, request_key.as_ref(), state, routing);
                    });
                }
            })?;
            let auth_state = state.clone(); let auth_routing = routing.clone(); let auth_activation = activation.clone(); let auth_probe_user = probe_user.clone(); let auth_probe_password = probe_password.clone();
            let authenticator: relay::DynamicAuthenticator = Arc::new(move |user, pass| {
                if user == auth_probe_user && pass == auth_probe_password { return Ok(relay::DynamicAuthorization { lease: user.into(), host: String::new(), port: 0, label: "probe".into(), process: "probe".into(), app_matcher: String::new(), network: "probe".into(), route: Arc::new(AdmittedRoute { generation: 0, decision: policy::Decision { allow_direct_fallback: false, route: policy::Route::Reject, chain: vec!["PROBE".into()], rule: "probe".into() }, host: String::new(), port: 0 }), probe: true }); }
                let generation = auth_routing.generation()?;
                let mut guard = auth_state.lock().expect("admission state");
                reap(&mut guard);
                let lease = guard.leases.get(user).context("admission lease missing or expired")?;
                if lease.password != pass || lease.route.generation != generation || lease.request.activation != auth_activation { bail!("admission lease authentication failed"); }
                let lease = guard.leases.remove(user).context("admission lease disappeared")?;
                let (process, app_matcher) = source_labels(&lease.request.source);
                Ok(relay::DynamicAuthorization { lease: user.into(), host: lease.route.host.clone(), port: lease.route.port, label: lease.route.decision.chain.join(" → "), process, app_matcher, network: lease.request.network.clone(), route: lease.route, probe: false })
            });
            let dial_routing = routing.clone();
            let dialer: relay::DynamicDialer = Arc::new(move |auth| {
                if dial_routing.generation()? != auth.route.generation { bail!("stale TCP route generation"); }
                dial_routing.dial(auth.route.as_ref())
            });
            let udp_routing = routing.clone();
            let udp_router: relay::DynamicUdpRouter = Arc::new(move |auth, host, port| {
                if auth.route.host != host || auth.route.port != port || auth.network != "udp" { bail!("UDP target does not match admitted route"); }
                if auth.route.decision.route == policy::Route::Direct || auth.route.decision.route == policy::Route::Reject { bail!("owned UDP cannot hand back Direct or Reject"); }
                if udp_routing.generation()? != auth.route.generation { bail!("stale UDP route generation"); }
                udp_routing.datagram_route(auth.route.as_ref())
            });
            let relay = relay::MixedServer::start_dynamic(relay_listener, authenticator, dialer, udp_router)?;
            Ok(Self { bootstrap: Bootstrap { version: 1, activation, port: broker_port, key: B64.encode(key.as_ref()) }, relay_port, probe_user, probe_password, state, stop, active, broker: Some(broker), relay })
        }
    }
    pub fn bootstrap(&self) -> &Bootstrap { &self.bootstrap }
    pub fn relay_port(&self) -> u16 { self.relay_port }
    pub fn probe_credentials(&self) -> (&str, &str) { (&self.probe_user, &self.probe_password) }
    pub fn snapshot(&self) -> crate::controller::TrafficSnapshot {
        let mut snapshot = self.relay.snapshot();
        let mut state = self.state.lock().expect("admission state");
        reap(&mut state);
        snapshot.connections.extend(state.decisions.iter().rev().map(|(_, row)| row.clone()));
        snapshot.connections.truncate(crate::controller::UI_CONNECTION_CAP);
        snapshot
    }
    pub fn close_one(&self, id: &str) -> Result<()> {
        if id.starts_with("decision-") { bail!("这条连接没有经过应用转发，无法在这里关闭"); }
        self.relay.close_one(id)
    }
    pub fn close_all(&self) -> Result<()> {
        self.state.lock().expect("admission state").leases.clear();
        self.relay.close_all()
    }
}
impl Drop for AdmissionService {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.broker.take() { let _ = handle.join(); }
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.active.load(Ordering::Acquire) > 0 && Instant::now() < deadline { thread::sleep(Duration::from_millis(5)); }
    }
}
struct ActiveGuard(Arc<AtomicUsize>); impl Drop for ActiveGuard { fn drop(&mut self) { self.0.fetch_sub(1, Ordering::AcqRel); } }

fn handle_request(stream: &mut TcpStream, activation: &str, relay_port: u16, key: &[u8; 32], state: Arc<Mutex<State>>, routing: Arc<dyn Routing>) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(1)))?; stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    let payload = read_frame(stream, MAX_FRAME)?; let envelope: Envelope = serde_json::from_slice(&payload)?;
    let raw = envelope.payload.as_bytes(); if raw.len() > MAX_PAYLOAD || !verify_mac(key, raw, &envelope.mac)? { bail!("admission request authentication failed"); }
    let request: FlowRequest = serde_json::from_slice(raw)?; validate_request(&request, activation)?;
    let mut guard = state.lock().expect("admission state"); reap(&mut guard); if guard.nonces.iter().any(|(nonce, _)| nonce == &request.nonce) { bail!("admission nonce replay"); } guard.nonces.push_back((request.nonce.clone(), Instant::now())); drop(guard);
    let route = routing.decide(&request)?; if route.generation != routing.generation()? { bail!("admission generation changed"); }
    let mut reply = AdmissionReply { version: 1, activation: activation.into(), nonce: request.nonce.clone(), action: "reject".into(), generation: route.generation, host: route.host.clone(), port: route.port, rule: route.decision.rule.clone(), chain: route.decision.chain.clone(), relay_port: None, lease: None, password: None };
    if matches!(route.decision.route, policy::Route::Direct | policy::Route::Reject) {
        let (process, app_matcher) = source_labels(&request.source);
        let row = crate::controller::LiveConnection {
            id: format!("decision-{}", request.nonce), process, app_matcher,
            destination: format!("{}:{}", request.hostname.as_deref().unwrap_or(&request.host), request.port),
            network: request.network.clone(), chain: route.decision.chain.join(" → "), upload: 0, download: 0,
            duration: if route.decision.route == policy::Route::Direct { "直连 · 未经应用转发".into() } else { "已拒绝".into() },
        };
        let mut state = state.lock().expect("admission state");
        while state.decisions.len() >= 512 { state.decisions.pop_front(); }
        state.decisions.push_back((Instant::now(), row));
    }
    if route.decision.route == policy::Route::Direct { reply.action = "direct".into(); }
    else if route.decision.route != policy::Route::Reject {
        let lease_id = Uuid::new_v4().to_string(); let password = random_text()?;
        let mut guard = state.lock().expect("admission state"); if guard.leases.len() >= 4096 { bail!("admission lease capacity reached"); }
        guard.leases.insert(lease_id.clone(), Lease { route: Arc::new(route), request, password: password.clone(), expires: Instant::now() + LEASE_TTL });
        reply.action = "proxy".into(); reply.relay_port = Some(relay_port); reply.lease = Some(lease_id); reply.password = Some(password);
    }
    let body = serde_json::to_vec(&reply)?; let payload = String::from_utf8(body).context("admission response is not UTF-8")?; let signed = Envelope { mac: sign_mac(key, payload.as_bytes())?, payload };
    if reply.generation != routing.generation()? { bail!("admission generation changed before reply"); }
    write_frame(stream, &serde_json::to_vec(&signed)?)
}

fn source_labels(source: &FlowSource) -> (String, String) {
    let process = source.executable_path.as_deref().and_then(|path| std::path::Path::new(path).file_name()).and_then(|name| name.to_str()).map(str::to_owned)
        .or_else(|| source.bundle_id.clone()).or_else(|| source.signing_id.clone()).unwrap_or_else(|| "应用".into());
    let matcher = source.signing_id.as_ref().or(source.bundle_id.as_ref()).or(source.executable_path.as_ref()).cloned().unwrap_or_default();
    (process, matcher)
}

#[derive(Serialize, Deserialize)] struct Envelope { payload: String, mac: String }
fn read_frame(stream: &mut TcpStream, max: usize) -> Result<Vec<u8>> {
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut len = [0; 4]; read_exact_deadline(stream, &mut len, deadline)?;
    let size = u32::from_be_bytes(len) as usize; if size == 0 || size > max { bail!("admission frame too large"); }
    let mut body = vec![0; size]; read_exact_deadline(stream, &mut body, deadline)?; Ok(body)
}
fn read_exact_deadline(stream: &mut TcpStream, bytes: &mut [u8], deadline: Instant) -> Result<()> {
    let mut at = 0;
    while at < bytes.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() { bail!("admission frame deadline exceeded"); }
        stream.set_read_timeout(Some(remaining))?;
        match stream.read(&mut bytes[at..]) {
            Ok(0) => bail!("admission peer closed the frame"),
            Ok(size) => at += size,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
fn write_frame(stream: &mut TcpStream, body: &[u8]) -> Result<()> { if body.len() > MAX_FRAME { bail!("admission response too large"); } stream.write_all(&(body.len() as u32).to_be_bytes())?; stream.write_all(body)?; Ok(()) }
fn validate_request(request: &FlowRequest, activation: &str) -> Result<()> {
    if request.version != 1 || request.activation != activation || Uuid::parse_str(&request.nonce).is_err() || Uuid::parse_str(&request.flow_id).is_err() || request.port == 0 || request.host.is_empty() || request.host.len() > 253 || request.hostname.as_ref().is_some_and(|value| value.len() > 253) || !matches!(request.kind.as_str(), "traffic" | "dns") || !matches!(request.network.as_str(), "tcp" | "udp") { bail!("invalid admission request"); }
    for value in std::iter::once(&request.host).chain(request.hostname.as_ref()) {
        if value.bytes().any(|b| b < 0x20 || b == 0x7f) { bail!("admission host contains controls"); }
    }
    for value in [request.source.process_start.as_ref(), request.source.bundle_id.as_ref(), request.source.signing_id.as_ref(), request.source.team_id.as_ref()].into_iter().flatten() {
        if value.len() > 1024 || value.bytes().any(|b| b < 0x20 || b == 0x7f) { bail!("admission source metadata is invalid"); }
    }
    if let Some(value) = request.source.executable_path.as_ref() { if value.len() > 4096 || value.bytes().any(|b| b < 0x20 || b == 0x7f) { bail!("admission executable path is invalid"); } }
    Ok(())
}
fn reap(state: &mut State) {
    let now = Instant::now();
    state.leases.retain(|_, lease| lease.expires > now);
    while state.nonces.front().is_some_and(|(_, at)| now.duration_since(*at) > NONCE_TTL) { state.nonces.pop_front(); }
    while state.nonces.len() > MAX_NONCES { state.nonces.pop_front(); }
    while state.decisions.front().is_some_and(|(at, _)| now.duration_since(*at) > Duration::from_secs(60)) { state.decisions.pop_front(); }
}

fn random_key() -> Result<[u8; 32]> { let mut key = [0; 32]; #[cfg(target_os = "macos")] { use std::fs::File; File::open("/dev/urandom")?.read_exact(&mut key)?; Ok(key) } #[cfg(not(target_os = "macos"))] { let _ = key; bail!("macOS admission unavailable") } }
fn random_text() -> Result<String> { Ok(Uuid::new_v4().simple().to_string()) }

#[cfg(target_os = "macos")]
fn sign_mac(key: &[u8], payload: &[u8]) -> Result<String> { let mut out = [0u8; 32]; unsafe { CCHmac(2, key.as_ptr() as *const _, key.len(), payload.as_ptr() as *const _, payload.len(), out.as_mut_ptr() as *mut _); } Ok(out.iter().map(|b| format!("{b:02x}")).collect()) }
#[cfg(not(target_os = "macos"))] fn sign_mac(_: &[u8], _: &[u8]) -> Result<String> { bail!("CommonCrypto unavailable") }
fn verify_mac(key: &[u8], payload: &[u8], mac: &str) -> Result<bool> {
    let expected = sign_mac(key, payload)?;
    let expected = expected.as_bytes(); let provided = mac.as_bytes();
    let mut difference = expected.len() ^ provided.len();
    for index in 0..expected.len().max(provided.len()) { difference |= expected.get(index).copied().unwrap_or(0) as usize ^ provided.get(index).copied().unwrap_or(0) as usize; }
    Ok(difference == 0)
}
#[cfg(target_os = "macos")] #[link(name = "System", kind = "dylib")] extern "C" { fn CCHmac(algorithm: u32, key: *const std::ffi::c_void, keyLength: usize, data: *const std::ffi::c_void, dataLength: usize, macOut: *mut std::ffi::c_void); }

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "macos")]
    use crate::xray::relay::Dialed;
    #[cfg(target_os = "macos")]
    use std::net::{Shutdown, UdpSocket};
    #[cfg(target_os = "macos")]
    use std::sync::atomic::AtomicU64;
    #[cfg(target_os = "macos")]
    use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
    fn request() -> FlowRequest {
        FlowRequest { version: 1, activation: "a".into(), nonce: Uuid::new_v4().to_string(), flow_id: Uuid::new_v4().to_string(), kind: "traffic".into(), network: "tcp".into(), host: "example.com".into(), hostname: None, port: 443, source: FlowSource { process_id: None, user_id: None, process_start: None, executable_path: None, bundle_id: None, signing_id: None, team_id: None } }
    }
    #[test]
    fn request_validation_rejects_controls_and_unknown_enums() {
        let mut value = request();
        assert!(validate_request(&value, "a").is_ok());
        value.network = "icmp".into(); assert!(validate_request(&value, "a").is_err());
        value = request(); value.host = "bad\nname".into(); assert!(validate_request(&value, "a").is_err());
        value = request(); value.port = 0; assert!(validate_request(&value, "a").is_err());
    }
    #[test]
    fn stale_leases_and_nonces_are_reaped_and_bounded() {
        let mut state = State::default();
        state.nonces.push_back(("old".into(), Instant::now() - NONCE_TTL - Duration::from_secs(1)));
        for n in 0..(MAX_NONCES + 4) { state.nonces.push_back((n.to_string(), Instant::now())); }
        reap(&mut state);
        assert_eq!(state.nonces.len(), MAX_NONCES);
        assert!(!state.nonces.iter().any(|(nonce, _)| nonce == "old"));
    }
    #[test]
    fn wire_frame_size_is_bounded() {
        let body = vec![0u8; MAX_FRAME + 1];
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let client = TcpStream::connect(address).unwrap();
        let (mut server, _) = listener.accept().unwrap();
        drop(client);
        assert!(write_frame(&mut server, &body).is_err());
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn commoncrypto_hmac_sha256_known_vector() {
        let mac = sign_mac(b"key", b"The quick brown fox jumps over the lazy dog").unwrap();
        assert_eq!(mac, "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8");
        assert!(verify_mac(b"key", b"The quick brown fox jumps over the lazy dog", &mac).unwrap());
        assert!(!verify_mac(b"key", b"The quick brown fox jumps over the lazy dog", &format!("{mac}0")).unwrap());
    }

    #[cfg(target_os = "macos")]
    struct MockRouting {
        generation: AtomicU64,
        dial_calls: AtomicUsize,
        tcp_target: SocketAddr,
    }
    #[cfg(target_os = "macos")]
    impl Routing for MockRouting {
        fn decide(&self, request: &FlowRequest) -> Result<AdmittedRoute> {
            let route = if request.host == "direct.test" { policy::Route::Direct } else { policy::Route::Node("fixture-node".into()) };
            Ok(AdmittedRoute { generation: self.generation.load(Ordering::Acquire), decision: policy::Decision { allow_direct_fallback: false, route, chain: vec!["fixture-node".into()], rule: "fixture".into() }, host: request.host.clone(), port: request.port })
        }
        fn generation(&self) -> Result<u64> { Ok(self.generation.load(Ordering::Acquire)) }
        fn dial(&self, _route: &AdmittedRoute) -> Result<Dialed> { self.dial_calls.fetch_add(1, Ordering::AcqRel); Ok(Dialed { stream: TcpStream::connect(self.tcp_target)?, chain: "fixture-node".into(), rule: "fixture".into() }) }
        fn datagram_route(&self, _route: &AdmittedRoute) -> Result<crate::xray::udp::DatagramRoute> { Ok(crate::xray::udp::DatagramRoute::Direct) }
    }
    #[cfg(target_os = "macos")]
    fn fixture_request(bootstrap: &Bootstrap, host: &str, network: &str) -> FlowRequest {
        FlowRequest { version: 1, activation: bootstrap.activation.clone(), nonce: Uuid::new_v4().to_string(), flow_id: Uuid::new_v4().to_string(), kind: "traffic".into(), network: network.into(), host: host.into(), hostname: None, port: if network == "udp" { 7 } else { 443 }, source: FlowSource { process_id: Some(77), user_id: Some(501), process_start: Some("fixture".into()), executable_path: Some("/Fixture.app/Contents/MacOS/fixture".into()), bundle_id: Some("test.fixture".into()), signing_id: Some("test.fixture".into()), team_id: Some("TEAM".into()) } }
    }
    #[cfg(target_os = "macos")]
    fn broker_request(bootstrap: &Bootstrap, request: &FlowRequest) -> Result<AdmissionReply> {
        let key = B64.decode(bootstrap.key.as_bytes())?;
        let payload = serde_json::to_string(request)?;
        let envelope = Envelope { mac: sign_mac(&key, payload.as_bytes())?, payload };
        let mut stream = TcpStream::connect(("127.0.0.1", bootstrap.port))?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        write_frame(&mut stream, &serde_json::to_vec(&envelope)?)?;
        let body = read_frame(&mut stream, MAX_FRAME)?;
        let response: Envelope = serde_json::from_slice(&body)?;
        if !verify_mac(&key, response.payload.as_bytes(), &response.mac)? { bail!("fixture response MAC invalid"); }
        Ok(serde_json::from_str(&response.payload)?)
    }
    #[cfg(target_os = "macos")]
    fn socks_connect(port: u16, user: &str, password: &str, host: &str, target_port: u16) -> Result<TcpStream> {
        let mut stream = TcpStream::connect(("127.0.0.1", port))?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        stream.write_all(&[5, 1, 2])?; let mut methods = [0; 2]; stream.read_exact(&mut methods)?; assert_eq!(methods, [5, 2]);
        stream.write_all(&[1, user.len() as u8])?; stream.write_all(user.as_bytes())?; stream.write_all(&[password.len() as u8])?; stream.write_all(password.as_bytes())?;
        let mut auth = [0; 2]; stream.read_exact(&mut auth)?; if auth != [1, 0] { bail!("fixture SOCKS auth failed"); }
        stream.write_all(&[5, 1, 0, 3, host.len() as u8])?; stream.write_all(host.as_bytes())?; stream.write_all(&target_port.to_be_bytes())?;
        let mut reply = [0; 4]; stream.read_exact(&mut reply)?; if reply != [5, 0, 0, 1] { bail!("fixture SOCKS connect failed"); }
        let mut bound = [0; 6]; stream.read_exact(&mut bound)?; Ok(stream)
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn admission_broker_and_tcp_lease_are_real_and_bound() {
        let echo = TcpListener::bind(("127.0.0.1", 0)).unwrap(); let echo_addr = echo.local_addr().unwrap();
        let echo_thread = thread::spawn(move || { let (mut stream, _) = echo.accept().unwrap(); let mut data = [0; 4]; stream.read_exact(&mut data).unwrap(); stream.write_all(&data).unwrap(); let _ = stream.shutdown(Shutdown::Both); });
        let routing = Arc::new(MockRouting { generation: AtomicU64::new(9), dial_calls: AtomicUsize::new(0), tcp_target: echo_addr });
        let service = AdmissionService::start(routing.clone()).unwrap();
        let direct = broker_request(service.bootstrap(), &fixture_request(service.bootstrap(), "direct.test", "tcp")).unwrap();
        assert_eq!(direct.action, "direct"); assert!(direct.lease.is_none()); assert_eq!(routing.dial_calls.load(Ordering::Acquire), 0);
        let direct_ledger = service.snapshot();
        assert_eq!(direct_ledger.connection_count, 0);
        assert_eq!(direct_ledger.upload_total, 0);
        assert!(direct_ledger.connections.iter().any(|row| row.process == "fixture" && row.destination == "direct.test:443" && row.upload == 0 && row.download == 0));
        let request = fixture_request(service.bootstrap(), "proxy.test", "tcp"); let reply = broker_request(service.bootstrap(), &request).unwrap();
        assert_eq!(reply.action, "proxy"); let lease = reply.lease.clone().unwrap(); let password = reply.password.clone().unwrap();
        let mut stream = socks_connect(reply.relay_port.unwrap(), &lease, &password, "proxy.test", 443).unwrap(); stream.write_all(b"ping").unwrap(); let mut echoed = [0; 4]; stream.read_exact(&mut echoed).unwrap(); assert_eq!(&echoed, b"ping");
        assert!(socks_connect(reply.relay_port.unwrap(), &lease, &password, "proxy.test", 443).is_err());
        let wrong = broker_request(service.bootstrap(), &fixture_request(service.bootstrap(), "wrong.test", "tcp")).unwrap();
        assert!(socks_connect(wrong.relay_port.unwrap(), &wrong.lease.clone().unwrap(), &wrong.password.clone().unwrap(), "other.test", 443).is_err());
        let stale = broker_request(service.bootstrap(), &fixture_request(service.bootstrap(), "stale.test", "tcp")).unwrap();
        routing.generation.store(10, Ordering::Release);
        assert!(socks_connect(stale.relay_port.unwrap(), &stale.lease.unwrap(), &stale.password.unwrap(), "stale.test", 443).is_err());
        assert_eq!(routing.dial_calls.load(Ordering::Acquire), 1);
        echo_thread.join().unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn admission_udp_lease_is_consumed_bound_and_survives_pending_prune() {
        let echo = UdpSocket::bind(("127.0.0.1", 0)).unwrap(); let echo_addr = echo.local_addr().unwrap(); echo.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let echo_thread = thread::spawn(move || { let mut data = [0; 64]; let (size, peer) = echo.recv_from(&mut data).unwrap(); echo.send_to(&data[..size], peer).unwrap(); });
        let routing = Arc::new(MockRouting { generation: AtomicU64::new(3), dial_calls: AtomicUsize::new(0), tcp_target: echo_addr });
        let service = AdmissionService::start(routing.clone()).unwrap();
        let mut request = fixture_request(service.bootstrap(), &echo_addr.ip().to_string(), "udp"); request.port = echo_addr.port();
        let reply = broker_request(service.bootstrap(), &request).unwrap(); assert_eq!(reply.action, "proxy");
        let lease = reply.lease.clone().unwrap(); let password = reply.password.clone().unwrap(); let relay_port = reply.relay_port.unwrap();
        let mut control = TcpStream::connect(("127.0.0.1", relay_port)).unwrap(); control.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        control.write_all(&[5, 1, 2]).unwrap(); let mut methods = [0; 2]; control.read_exact(&mut methods).unwrap(); assert_eq!(methods, [5, 2]);
        control.write_all(&[1, lease.len() as u8]).unwrap(); control.write_all(lease.as_bytes()).unwrap(); control.write_all(&[password.len() as u8]).unwrap(); control.write_all(password.as_bytes()).unwrap(); let mut auth = [0; 2]; control.read_exact(&mut auth).unwrap(); assert_eq!(auth, [1, 0]);
        control.write_all(&[5, 3, 0, 1, 127, 0, 0, 1]).unwrap(); control.write_all(&echo_addr.port().to_be_bytes()).unwrap();
        let mut response = [0; 10]; control.read_exact(&mut response[..4]).unwrap(); assert_eq!(&response[..2], &[5, 0]); let bound_port = { let atyp = response[3]; assert_eq!(atyp, 1); control.read_exact(&mut response[4..10]).unwrap(); u16::from_be_bytes([response[8], response[9]]) };
        let client = UdpSocket::bind(("127.0.0.1", 0)).unwrap(); client.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let frame = crate::xray::udp::encode_target(&echo_addr.ip().to_string(), echo_addr.port(), b"pong").unwrap(); client.send_to(&frame, ("127.0.0.1", bound_port)).unwrap();
        let mut echoed = [0; 64]; let (size, _) = client.recv_from(&mut echoed).unwrap(); let (_, _, offset) = crate::xray::udp::decode_target(&echoed[..size]).unwrap(); assert_eq!(&echoed[offset..size], b"pong");
        { let mut state = service.state.lock().unwrap(); reap(&mut state); assert!(!state.leases.contains_key(&lease)); }
        let wrong = crate::xray::udp::encode_target("127.0.0.1", echo_addr.port().saturating_add(1), b"wrong").unwrap(); client.send_to(&wrong, ("127.0.0.1", bound_port)).unwrap(); assert!(client.recv_from(&mut echoed).is_err());
        drop(control); echo_thread.join().unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn admission_rejects_bad_mac_and_replayed_nonce() {
        let routing = Arc::new(MockRouting { generation: AtomicU64::new(1), dial_calls: AtomicUsize::new(0), tcp_target: ("127.0.0.1", 9).to_socket_addrs().unwrap().next().unwrap() });
        let service = AdmissionService::start(routing).unwrap(); let request = fixture_request(service.bootstrap(), "direct.test", "tcp"); let key = B64.decode(service.bootstrap().key.as_bytes()).unwrap(); let payload = serde_json::to_string(&request).unwrap();
        let mut bad = TcpStream::connect(("127.0.0.1", service.bootstrap().port)).unwrap(); bad.set_read_timeout(Some(Duration::from_secs(2))).unwrap(); write_frame(&mut bad, &serde_json::to_vec(&Envelope { payload: payload.clone(), mac: "00".into() }).unwrap()).unwrap(); assert!(read_frame(&mut bad, MAX_FRAME).is_err());
        assert!(broker_request(service.bootstrap(), &request).is_ok()); assert!(broker_request(service.bootstrap(), &request).is_err());
        assert!(verify_mac(&key, payload.as_bytes(), &sign_mac(&key, payload.as_bytes()).unwrap()).unwrap());
    }
}
