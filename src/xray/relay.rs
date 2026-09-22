use crate::controller::{LiveConnection, TrafficSnapshot, UI_CONNECTION_CAP};
use anyhow::{bail, Context, Result};
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::xray::udp;

const MAX_ACTIVE: usize = 128;
const HEADER_LIMIT: usize = 32 * 1024;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(8);

fn is_resource_exhaustion(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(libc::EMFILE | libc::ENFILE))
}

pub struct Dialed {
    pub stream: TcpStream,
    pub chain: String,
    pub rule: String,
}
/// Authorization returned by the Xray application admission broker after a
/// SOCKS credential has been checked.  The target is copied into the
/// authorization so a relay cannot silently re-route a lease.
#[derive(Clone, Debug)]
pub struct DynamicAuthorization {
    pub lease: String,
    pub host: String,
    pub port: u16,
    pub label: String,
    pub process: String,
    pub app_matcher: String,
    pub network: String,
    pub route: Arc<crate::xray::admission::AdmittedRoute>,
    pub probe: bool,
}
pub type DynamicAuthenticator = Arc<dyn Fn(&str, &str) -> Result<DynamicAuthorization> + Send + Sync>;
pub type DynamicDialer = Arc<dyn Fn(&DynamicAuthorization) -> Result<Dialed> + Send + Sync>;
pub type DynamicUdpRouter = Arc<dyn Fn(&DynamicAuthorization, &str, u16) -> Result<udp::DatagramRoute> + Send + Sync>;
pub type Dialer = Arc<dyn Fn(&str, u16) -> Result<Dialed> + Send + Sync>;
pub type UdpRouter = Arc<dyn Fn(&str, u16) -> Result<udp::DatagramRoute> + Send + Sync>;

struct Entry {
    id: String,
    started: Instant,
    source_port: u16,
    udp: AtomicBool,
    visible: AtomicBool,
    label: Mutex<(String, String)>,
    process: Mutex<String>,
    app_matcher: Mutex<String>,
    sockets: Mutex<Vec<TcpStream>>,
    closed: AtomicBool,
    finished: AtomicBool,
    up: AtomicU64,
    down: AtomicU64,
    last_activity: AtomicU64,
}
impl Entry {
    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        for socket in self.sockets.lock().expect("flow sockets").iter() {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }
    fn track(&self, socket: &TcpStream) -> Result<()> {
        let mut sockets = self.sockets.lock().expect("flow sockets");
        if self.closed.load(Ordering::Acquire) {
            let _ = socket.shutdown(Shutdown::Both);
            bail!("连接已关闭");
        }
        sockets.push(socket.try_clone()?);
        Ok(())
    }
}
struct State {
    stop: AtomicBool,
    active: AtomicUsize,
    serial: AtomicU64,
    up: AtomicU64,
    down: AtomicU64,
    rows: Mutex<VecDeque<Arc<Entry>>>,
}
struct FlowGuard {
    state: Arc<State>,
    entry: Arc<Entry>,
}
impl Drop for FlowGuard {
    fn drop(&mut self) {
        self.entry.close();
        self.entry.sockets.lock().expect("flow sockets").clear();
        self.entry.finished.store(true, Ordering::Release);
        self.state.active.fetch_sub(1, Ordering::AcqRel);
        if !self.entry.visible.load(Ordering::Acquire) {
            self.state.rows.lock().expect("flow rows").retain(|row| row.id != self.entry.id);
        }
    }
}

pub struct MixedServer {
    state: Arc<State>,
    acceptor: Option<JoinHandle<()>>,
}
impl MixedServer {
    pub fn start(listener: TcpListener, dialer: Dialer) -> Result<Self> {
        Self::start_inner(listener, dialer, None, None)
    }

    pub fn start_with_udp(listener: TcpListener, dialer: Dialer, udp_router: UdpRouter) -> Result<Self> {
        Self::start_inner(listener, dialer, None, Some(udp_router))
    }

    /// Starts the application-owned admission relay.  Unlike the legacy
    /// listener this endpoint accepts only SOCKS5 and resolves the credential
    /// to one bounded, lease-bearing authorization before any payload dial.
    pub fn start_dynamic(
        listener: TcpListener,
        authenticator: DynamicAuthenticator,
        dialer: DynamicDialer,
        udp_router: DynamicUdpRouter,
    ) -> Result<Self> {
        if !listener.local_addr()?.ip().is_loopback() { bail!("动态入口必须绑定本机地址"); }
        listener.set_nonblocking(true)?;
        let state = Arc::new(State {
            stop: AtomicBool::new(false), active: AtomicUsize::new(0), serial: AtomicU64::new(1),
            up: AtomicU64::new(0), down: AtomicU64::new(0), rows: Mutex::new(VecDeque::new()),
        });
        let shared = state.clone();
        let acceptor = thread::Builder::new().name("myproxy-admission-relay".into()).spawn(move || {
            while !shared.stop.load(Ordering::Acquire) {
                let (client, _) = match listener.accept() {
                    Ok(value) => value,
                    Err(error) if is_resource_exhaustion(&error) => {
                        thread::sleep(Duration::from_millis(100));
                        continue;
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(_) => break,
                };
                if client.set_nonblocking(false).is_err() { continue; }
                if shared.active.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < MAX_ACTIVE).then_some(n + 1)).is_err() {
                    let _ = client.shutdown(Shutdown::Both); continue;
                }
                let entry = Arc::new(Entry {
                    id: format!("flow-{}", shared.serial.fetch_add(1, Ordering::Relaxed)), started: Instant::now(),
                    source_port: client.peer_addr().map(|p| p.port()).unwrap_or(0), udp: AtomicBool::new(false), visible: AtomicBool::new(false),
                    label: Mutex::new(("等待请求".into(), "待路由".into())), process: Mutex::new(String::new()), app_matcher: Mutex::new(String::new()), sockets: Mutex::new(vec![]), closed: AtomicBool::new(false),
                    finished: AtomicBool::new(false), up: AtomicU64::new(0), down: AtomicU64::new(0), last_activity: AtomicU64::new(0),
                });
                let guard = FlowGuard { state: shared.clone(), entry: entry.clone() };
                if entry.track(&client).is_err() { drop(guard); continue; }
                {
                    let mut rows = shared.rows.lock().expect("flow rows");
                    while rows.len() >= UI_CONNECTION_CAP {
                        if let Some(index) = rows.iter().position(|row| row.finished.load(Ordering::Acquire)) { rows.remove(index); } else { break; }
                    }
                    rows.push_back(entry.clone());
                }
                let worker_state = shared.clone();
                let worker_authenticator = authenticator.clone();
                let worker_dialer = dialer.clone();
                let worker_udp_router = udp_router.clone();
                let _ = thread::Builder::new().name("myproxy-admission-flow".into()).spawn(move || {
                    let _guard = guard;
                    if let Err(error) = serve_dynamic(client, &entry, &worker_state, &worker_authenticator, &worker_dialer, &worker_udp_router) {
                        *entry.label.lock().expect("flow label") = ("失败".into(), format!("失败：{error}"));
                    }
                });
            }
        })?;
        Ok(Self { state, acceptor: Some(acceptor) })
    }

    fn start_inner(listener: TcpListener, dialer: Dialer, credentials: Option<(String, String)>, udp_router: Option<UdpRouter>) -> Result<Self> {
        if !listener.local_addr()?.ip().is_loopback() {
            bail!("混合入口必须绑定本机地址");
        }
        listener.set_nonblocking(true)?;
        let state = Arc::new(State {
            stop: AtomicBool::new(false),
            active: AtomicUsize::new(0),
            serial: AtomicU64::new(1),
            up: AtomicU64::new(0),
            down: AtomicU64::new(0),
            rows: Mutex::new(VecDeque::new()),
        });
        let shared = state.clone();
        let acceptor = thread::Builder::new()
            .name("myproxy-mixed".into())
            .spawn(move || {
                while !shared.stop.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((client, _)) => {
                            if client.set_nonblocking(false).is_err() {
                                continue;
                            }
                            if shared
                                .active
                                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                                    (n < MAX_ACTIVE).then_some(n + 1)
                                })
                                .is_err()
                            {
                                let _ = client.shutdown(Shutdown::Both);
                                continue;
                            }
                            let entry = Arc::new(Entry {
                                id: format!(
                                    "flow-{}",
                                    shared.serial.fetch_add(1, Ordering::Relaxed)
                                ),
                                started: Instant::now(),
                                source_port: client.peer_addr().map(|peer| peer.port()).unwrap_or(0),
                                udp: AtomicBool::new(false),
                                visible: AtomicBool::new(false),
                                label: Mutex::new(("等待请求".into(), "待路由".into())),
                                process: Mutex::new(String::new()),
                                app_matcher: Mutex::new(String::new()),
                                sockets: Mutex::new(vec![]),
                                closed: AtomicBool::new(false),
                                finished: AtomicBool::new(false),
                                up: AtomicU64::new(0),
                                down: AtomicU64::new(0),
                                last_activity: AtomicU64::new(0),
                            });
                            let guard = FlowGuard {
                                state: shared.clone(),
                                entry: entry.clone(),
                            };
                            if entry.track(&client).is_err() {
                                drop(guard);
                                continue;
                            }
                            {
                                let mut rows = shared.rows.lock().expect("flow rows");
                                while rows.len() >= UI_CONNECTION_CAP {
                                    if let Some(index) = rows
                                        .iter()
                                        .position(|row| row.finished.load(Ordering::Acquire))
                                    {
                                        rows.remove(index);
                                    } else {
                                        break;
                                    }
                                }
                                rows.push_back(entry.clone());
                            }
                            let dialer = dialer.clone();
                            let worker_state = shared.clone();
                            let credentials = credentials.clone();
                            let udp_router = udp_router.clone();
                            let _ = thread::Builder::new().name("myproxy-flow".into()).spawn(
                                move || {
                                    let _guard = guard;
                                    if let Err(error) =
                                        serve(client, &dialer, &entry, &worker_state, credentials.as_ref(), udp_router.as_ref())
                                    {
                                        entry.label.lock().expect("flow label").1 =
                                            format!("失败：{error}");
                                    }
                                },
                            );
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10))
                        }
                        Err(error) if is_resource_exhaustion(&error) => {
                            // Keep the listener alive while transient process
                            // descriptor pressure subsides. Existing flows
                            // continue to drain and release their clones.
                            thread::sleep(Duration::from_millis(100));
                        }
                        Err(_) => break,
                    }
                }
            })?;
        Ok(Self {
            state,
            acceptor: Some(acceptor),
        })
    }

    pub fn start_authenticated(
        listener: TcpListener,
        dialer: Dialer,
        username: String,
        password: String,
        udp_router: UdpRouter,
    ) -> Result<Self> {
        if username.is_empty() || username.len() > 255 || password.is_empty() || password.len() > 255 {
            bail!("系统接管入口需要有效的认证信息");
        }
        Self::start_inner(listener, dialer, Some((username, password)), Some(udp_router))
    }

    pub fn snapshot(&self) -> TrafficSnapshot {
        self.snapshot_with_processes(&std::collections::HashMap::new())
    }

    pub fn snapshot_with_processes(&self, processes: &std::collections::HashMap<u16, crate::network_extension::ActivityProcess>) -> TrafficSnapshot {
        let rows = self.state.rows.lock().expect("flow rows");
        let connection_count = rows.iter().filter(|entry| entry.visible.load(Ordering::Acquire) && !entry.finished.load(Ordering::Acquire)).count();
        let connections = rows.iter()
            .rev()
            .filter(|entry| entry.visible.load(Ordering::Acquire))
            .map(|entry| {
                let (destination, chain) = entry.label.lock().expect("flow label").clone();
                LiveConnection {
                    id: entry.id.clone(),
                    process: processes.get(&entry.source_port).map(|process| process.display.clone()).filter(|value| !value.is_empty()).unwrap_or_else(|| {
                        let value = entry.process.lock().expect("flow process").clone();
                        if value.is_empty() { "本地代理".into() } else { value }
                    }),
                    app_matcher: processes.get(&entry.source_port).map(|process| process.matcher.clone()).filter(|value| !value.is_empty()).unwrap_or_else(|| entry.app_matcher.lock().expect("flow app matcher").clone()),
                    destination,
                    network: if entry.udp.load(Ordering::Relaxed) { "udp".into() } else { "tcp".into() },
                    chain,
                    upload: entry.up.load(Ordering::Relaxed),
                    download: entry.down.load(Ordering::Relaxed),
                    duration: if entry.finished.load(Ordering::Acquire) {
                        "已结束".into()
                    } else {
                        format!("{}s", entry.started.elapsed().as_secs())
                    },
                }
            })
            .collect();
        TrafficSnapshot {
            connections,
            connection_count,
            upload_total: self.state.up.load(Ordering::Relaxed),
            download_total: self.state.down.load(Ordering::Relaxed),
        }
    }
    pub fn close_one(&self, id: &str) -> Result<()> {
        let rows = self.state.rows.lock().expect("flow rows");
        rows.iter()
            .find(|entry| entry.id == id)
            .context("连接不存在")?
            .close();
        Ok(())
    }
    pub fn close_all(&self) -> Result<()> {
        for entry in self.state.rows.lock().expect("flow rows").iter() {
            entry.close();
        }
        Ok(())
    }
}
impl Drop for MixedServer {
    fn drop(&mut self) {
        self.state.stop.store(true, Ordering::Release);
        let _ = self.close_all();
        if let Some(handle) = self.acceptor.take() {
            let _ = handle.join();
        }
    }
}

struct Request {
    host: String,
    port: u16,
    network: String,
    handshake: Vec<u8>,
    initial: Vec<u8>,
    body_limit: Option<u64>,
    socks: bool,
    udp_associate: bool,
}

fn serve_dynamic(
    mut client: TcpStream,
    entry: &Arc<Entry>,
    state: &Arc<State>,
    authenticator: &DynamicAuthenticator,
    dialer: &DynamicDialer,
    udp_router: &DynamicUdpRouter,
) -> Result<()> {
    client.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    client.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;
    let (request, authorization) = socks_request_dynamic(&mut client, authenticator)?;
    if authorization.probe {
        if request.udp_associate {
            return udp::serve_association(client, 0, Arc::new(|_, _| bail!("readiness probe cannot forward payload")), |_,_,_,_,_| {});
        }
        client.write_all(&request.handshake)?;
        let mut byte = [0];
        let _ = client.read(&mut byte);
        bail!("admission probe cannot carry payload");
    }
    if authorization.network != request.network { bail!("协议与连接凭据不匹配"); }
    *entry.process.lock().expect("flow process") = authorization.process.clone();
    *entry.app_matcher.lock().expect("flow app matcher") = authorization.app_matcher.clone();
    if request.udp_associate {
        entry.udp.store(true, Ordering::Release);
        entry.label.lock().expect("flow label").0 = "DNS / UDP".into();
        let route = udp_router.clone();
        return udp::serve_association(client, 0, Arc::new(move |host, port| {
            if authorization.host != host || authorization.port != port { bail!("UDP target does not match admitted lease"); }
            route(&authorization, host, port)
        }), |host, port, routed, up, down| {
            entry.visible.store(true, Ordering::Release);
            let target = match routed { udp::DatagramRoute::Direct => "DIRECT", udp::DatagramRoute::Socks { label, .. } => label.as_str() };
            *entry.label.lock().expect("flow label") = (format!("{host}:{port}"), target.into());
            entry.up.fetch_add(up, Ordering::Relaxed); entry.down.fetch_add(down, Ordering::Relaxed);
            state.up.fetch_add(up, Ordering::Relaxed); state.down.fetch_add(down, Ordering::Relaxed);
        });
    }
    if authorization.host != request.host || authorization.port != request.port {
        bail!("admission target binding mismatch");
    }
    entry.visible.store(true, Ordering::Release);
    entry.label.lock().expect("flow label").0 = format!("{}:{}", request.host, request.port);
    let mut dialed = dialer(&authorization)?;
    entry.track(&dialed.stream)?;
    entry.label.lock().expect("flow label").1 = if dialed.rule.is_empty() { dialed.chain.clone() } else { format!("{} → {}", dialed.rule, dialed.chain) };
    client.write_all(&request.handshake)?;
    for socket in [&client, &dialed.stream] { socket.set_read_timeout(Some(Duration::from_secs(1)))?; socket.set_write_timeout(Some(Duration::from_secs(15)))?; }
    thread::scope(|scope| {
        let upload = scope.spawn(|| {
            let result = pump(&client, &dialed.stream, &request.initial, request.body_limit, entry, state, true);
            if result.is_err() { entry.close(); }
            result
        });
        let download = pump(&dialed.stream, &client, &[], None, entry, state, false);
        if download.is_err() { entry.close(); }
        let sent = upload.join().map_err(|_| anyhow::anyhow!("连接转发线程异常"))?;
        sent.and(download)
    })
}

fn socks_request_dynamic(client: &mut TcpStream, authenticator: &DynamicAuthenticator) -> Result<(Request, DynamicAuthorization)> {
    let mut greeting = [0; 2]; client.read_exact(&mut greeting)?;
    if greeting[0] != 5 || greeting[1] == 0 { bail!("无效的 SOCKS5 请求"); }
    let mut methods = vec![0; greeting[1] as usize]; client.read_exact(&mut methods)?;
    if !methods.contains(&2) { client.write_all(&[5, 255])?; bail!("动态入口需要 SOCKS 认证"); }
    client.write_all(&[5, 2])?;
    let mut header = [0; 2]; client.read_exact(&mut header)?;
    if header[0] != 1 || header[1] == 0 { client.write_all(&[1, 1])?; bail!("无效的动态 SOCKS 用户名"); }
    let mut user = vec![0; header[1] as usize]; client.read_exact(&mut user)?;
    let mut length = [0]; client.read_exact(&mut length)?;
    if length[0] == 0 { client.write_all(&[1, 1])?; bail!("无效的动态 SOCKS 密码"); }
    let mut pass = vec![0; length[0] as usize]; client.read_exact(&mut pass)?;
    let username = String::from_utf8(user).context("动态 SOCKS 用户名不是 UTF-8")?;
    let password = String::from_utf8(pass).context("动态 SOCKS 密码不是 UTF-8")?;
    let authorization = match authenticator(&username, &password) {
        Ok(authorization) => authorization,
        Err(error) => { client.write_all(&[1, 1])?; return Err(error); }
    };
    client.write_all(&[1, 0])?;
    let mut command = [0; 4]; client.read_exact(&mut command)?;
    if command[0] != 5 || command[2] != 0 || !matches!(command[1], 1 | 3) { client.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0])?; bail!("动态入口仅支持 CONNECT/UDP ASSOCIATE"); }
    let (host, port) = socks_address_with_zero(client, command[3], command[1] == 3)?;
    Ok((Request { host, port, network: if command[1] == 3 { "udp".into() } else { "tcp".into() }, initial: vec![], body_limit: None, socks: true, udp_associate: command[1] == 3, handshake: vec![5, 0, 0, 1, 0, 0, 0, 0, 0, 0] }, authorization))
}
fn serve(
    mut client: TcpStream,
    dialer: &Dialer,
    entry: &Arc<Entry>,
    state: &Arc<State>,
    auth: Option<&(String, String)>,
    udp_router: Option<&UdpRouter>,
) -> Result<()> {
    client.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    client.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;
    let mut first = [0];
    client.read_exact(&mut first)?;
    let request = if first[0] == 5 {
        socks_request(&mut client, auth)?
    } else {
        if auth.is_some() { bail!("系统接管入口只接受经过认证的 SOCKS5 请求"); }
        http_request(&mut client, first[0])?
    };
    if request.udp_associate {
        let Some(router) = udp_router else {
            client.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0])?;
            bail!("此入口没有启用 UDP");
        };
        entry.udp.store(true, Ordering::Release);
        entry.label.lock().expect("flow label").0 = "DNS / UDP".into();
        return udp::serve_association(client, request.port, router.clone(), |host, port, route, up, down| {
            entry.visible.store(true, Ordering::Release);
            let target = match route {
                udp::DatagramRoute::Direct => "DIRECT",
                udp::DatagramRoute::Socks { label, .. } => label.as_str(),
            };
            *entry.label.lock().expect("flow label") = (format!("{host}:{port}"), target.into());
            entry.up.fetch_add(up, Ordering::Relaxed);
            entry.down.fetch_add(down, Ordering::Relaxed);
            state.up.fetch_add(up, Ordering::Relaxed);
            state.down.fetch_add(down, Ordering::Relaxed);
        });
    }
    entry.visible.store(true, Ordering::Release);
    entry.label.lock().expect("flow label").0 = format!("{}:{}", request.host, request.port);
    let mut dialed = match dialer(&request.host, request.port) {
        Ok(value) => value,
        Err(error) => {
            if request.socks {
                let _ = client.write_all(&[5, 1, 0, 1, 0, 0, 0, 0, 0, 0]);
            } else {
                let _ = client.write_all(
                    b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
            return Err(error);
        }
    };
    entry.track(&dialed.stream)?;
    entry.label.lock().expect("flow label").1 = if dialed.rule.is_empty() {
        dialed.chain
    } else {
        format!("{} → {}", dialed.rule, dialed.chain)
    };
    if !request.handshake.is_empty() {
        client.write_all(&request.handshake)?;
    }
    for socket in [&client, &dialed.stream] {
        socket.set_read_timeout(Some(Duration::from_secs(1)))?;
        socket.set_write_timeout(Some(Duration::from_secs(15)))?;
    }
    thread::scope(|scope| {
        let upload = scope.spawn(|| {
            let result = pump(
                &client,
                &dialed.stream,
                &request.initial,
                request.body_limit,
                entry,
                state,
                true,
            );
            if result.is_err() {
                entry.close();
            }
            result
        });
        let download = pump(
            &dialed.stream,
            &client,
            &[],
            None,
            entry,
            state,
            false,
        );
        if download.is_err() {
            entry.close();
        }
        let sent = upload
            .join()
            .map_err(|_| anyhow::anyhow!("连接转发线程异常"))?;
        sent.and(download)
    })
}
fn pump(
    input: &TcpStream,
    output: &TcpStream,
    initial: &[u8],
    limit: Option<u64>,
    entry: &Entry,
    state: &State,
    upload: bool,
) -> Result<()> {
    let mut remaining = limit.unwrap_or(u64::MAX);
    let initial_len = initial.len().min(remaining.min(usize::MAX as u64) as usize);
    if initial_len > 0 {
        output.write_all(&initial[..initial_len])?;
        count(entry, state, initial_len, upload);
        remaining -= initial_len as u64;
    }
    let mut buffer = [0; 16 * 1024];
    while remaining > 0 {
        if state.stop.load(Ordering::Acquire) || entry.closed.load(Ordering::Acquire) {
            return Ok(());
        }
        let size = buffer.len().min(remaining.min(usize::MAX as u64) as usize);
        match input.read(&mut buffer[..size]) {
            Ok(0) => break,
            Ok(n) => {
                output.write_all(&buffer[..n])?;
                count(entry, state, n, upload);
                remaining -= n as u64;
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) =>
            {
                if entry
                    .started
                    .elapsed()
                    .as_secs()
                    .saturating_sub(entry.last_activity.load(Ordering::Relaxed))
                    >= 120
                {
                    bail!("连接空闲超时");
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
    let _ = output.shutdown(Shutdown::Write);
    Ok(())
}
fn count(entry: &Entry, state: &State, n: usize, upload: bool) {
    if upload {
        entry.up.fetch_add(n as u64, Ordering::Relaxed);
        state.up.fetch_add(n as u64, Ordering::Relaxed);
    } else {
        entry.down.fetch_add(n as u64, Ordering::Relaxed);
        state.down.fetch_add(n as u64, Ordering::Relaxed);
    }
    entry
        .last_activity
        .store(entry.started.elapsed().as_secs(), Ordering::Relaxed);
}
fn http_request(client: &mut TcpStream, first: u8) -> Result<Request> {
    let mut bytes = vec![first];
    let end = loop {
        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break end + 4;
        }
        if bytes.len() >= HEADER_LIMIT {
            bail!("请求头过长");
        }
        let mut buffer = [0; 2048];
        let n = client.read(&mut buffer)?;
        if n == 0 {
            bail!("请求头不完整");
        }
        bytes.extend_from_slice(&buffer[..n]);
    };
    if end > HEADER_LIMIT {
        bail!("请求头过长");
    }
    let head = std::str::from_utf8(&bytes[..end]).context("请求头编码无效")?;
    let mut lines = head.split("\r\n");
    let line = lines.next().unwrap();
    let fields: Vec<_> = line.split(' ').collect();
    if fields.len() != 3
        || !fields[0].bytes().all(|b| b.is_ascii_uppercase())
        || !matches!(fields[2], "HTTP/1.1" | "HTTP/1.0")
    {
        bail!("HTTP 请求行无效");
    }
    let method = fields[0];
    let target = fields[1];
    let mut parsed = Vec::new();
    let mut host_header = None;
    let mut length = None;
    let mut connection_tokens = Vec::new();
    for line in lines.take_while(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').context("HTTP 请求头无效")?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
            || value.bytes().any(|b| b < 32 && b != b'\t' || b == 127)
        {
            bail!("HTTP 请求头无效");
        }
        let lower = name.to_ascii_lowercase();
        let value = value.trim();
        match lower.as_str() {
            "host" => {
                if host_header.replace(value).is_some() {
                    bail!("重复 Host 请求头");
                }
            }
            "content-length" => {
                if length.is_some()
                    || value.is_empty()
                    || !value.bytes().all(|b| b.is_ascii_digit())
                {
                    bail!("Content-Length 无效或重复");
                }
                length = Some(value.parse::<u64>()?);
            }
            "transfer-encoding" => bail!("HTTP 分块上传暂不支持，请使用 HTTPS CONNECT"),
            "connection" => {
                connection_tokens.extend(value.split(',').map(|v| v.trim().to_ascii_lowercase()))
            }
            _ => {}
        }
        parsed.push((lower, name, value));
    }
    if connection_tokens.iter().any(|field| {
        matches!(
            field.as_str(),
            "host" | "content-length" | "transfer-encoding"
        )
    }) {
        bail!("Connection 请求头不能移除请求定界字段");
    }
    let tunnel = method == "CONNECT";
    let (host, port, path) = if tunnel {
        let (host, port) = authority(target, 443)?;
        (host, port, String::new())
    } else if let Some(rest) = target.strip_prefix("http://") {
        let split = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let (host, port) = authority(&rest[..split], 80)?;
        let suffix = &rest[split..];
        if suffix.contains('#') {
            bail!("代理请求不能包含 fragment");
        }
        let path = if suffix.starts_with('/') {
            suffix.to_string()
        } else {
            format!("/{suffix}")
        };
        (host, port, path)
    } else {
        if !target.starts_with('/') || target.contains('#') {
            bail!("代理请求必须使用 http:// 地址或 CONNECT");
        }
        let (host, port) = authority(host_header.context("缺少 Host 请求头")?, 80)?;
        (host, port, target.into())
    };
    if let Some(header) = host_header {
        let (other, other_port) = authority(header, if tunnel { port } else { 80 })?;
        if !host.eq_ignore_ascii_case(&other) || port != other_port {
            bail!("Host 与代理目标不一致");
        }
    }
    let initial = if tunnel {
        bytes[end..].to_vec()
    } else {
        let mut out = format!("{method} {path} HTTP/1.1\r\n").into_bytes();
        for (lower, name, value) in parsed {
            if lower.starts_with("proxy-")
                || matches!(
                    lower.as_str(),
                    "host" | "connection" | "keep-alive" | "upgrade" | "trailer" | "te"
                )
                || connection_tokens.contains(&lower)
            {
                continue;
            }
            out.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
        }
        let authority = if host.contains(':') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        };
        out.extend_from_slice(format!("Host: {authority}\r\nConnection: close\r\n\r\n").as_bytes());
        // Exactly one request body is sent. Pipelined subsequent requests cannot bypass routing.
        let buffered = (bytes.len() - end).min(length.unwrap_or(0).min(usize::MAX as u64) as usize);
        out.extend_from_slice(&bytes[end..end + buffered]);
        out
    };
    let body_limit = if tunnel {
        None
    } else {
        let buffered_body =
            (bytes.len() - end).min(length.unwrap_or(0).min(usize::MAX as u64) as usize) as u64;
        Some(initial.len() as u64 + length.unwrap_or(0) - buffered_body)
    };
    Ok(Request {
        host,
        port,
        network: "tcp".into(),
        initial,
        body_limit,
        socks: false,
        udp_associate: false,
        handshake: if tunnel {
            b"HTTP/1.1 200 Connection Established\r\n\r\n".to_vec()
        } else {
            vec![]
        },
    })
}
fn authority(value: &str, default_port: u16) -> Result<(String, u16)> {
    if value.is_empty()
        || value
            .bytes()
            .any(|b| b <= 32 || b >= 127 || b"/@?#\\".contains(&b))
    {
        bail!("代理目标无效");
    }
    let (host, port) = if value.starts_with('[') {
        let end = value.find(']').context("IPv6 地址无效")?;
        let host = &value[1..end];
        host.parse::<std::net::Ipv6Addr>()
            .context("IPv6 地址无效")?;
        let rest = &value[end + 1..];
        let port = if rest.is_empty() {
            default_port
        } else {
            rest.strip_prefix(':')
                .context("IPv6 端口无效")?
                .parse::<u16>()?
        };
        (host.to_string(), port)
    } else if let Some((host, port)) = value.rsplit_once(':') {
        if host.is_empty() || host.contains(':') {
            bail!("IPv6 地址必须加方括号");
        }
        (host.to_string(), port.parse::<u16>()?)
    } else {
        (value.to_string(), default_port)
    };
    if port == 0 {
        bail!("目标端口不能为0");
    }
    Ok((host, port))
}
fn socks_request(client: &mut TcpStream, credentials: Option<&(String, String)>) -> Result<Request> {
    let mut count = [0];
    client.read_exact(&mut count)?;
    let mut methods = vec![0; count[0] as usize];
    client.read_exact(&mut methods)?;
    let required = credentials.map_or(0, |_| 2);
    if !methods.contains(&required) {
        client.write_all(&[5, 255])?;
        bail!("SOCKS 认证方式不支持");
    }
    client.write_all(&[5, required])?;
    if required == 2 {
        let mut header = [0; 2];
        client.read_exact(&mut header)?;
        if header[0] != 1 || header[1] == 0 { client.write_all(&[1, 1])?; bail!("无效的 SOCKS 认证请求"); }
        let mut len = [header[1]];
        let mut user = vec![0; len[0] as usize]; client.read_exact(&mut user)?;
        client.read_exact(&mut len)?; let mut pass=vec![0;len[0] as usize]; client.read_exact(&mut pass)?;
        let (expected_user, expected_pass)=credentials.unwrap();
        if user != expected_user.as_bytes() || pass != expected_pass.as_bytes() { client.write_all(&[1,1])?; bail!("SOCKS credentials rejected"); }
        client.write_all(&[1,0])?;
    }
    let mut header = [0; 4];
    client.read_exact(&mut header)?;
    if header[0] != 5 || header[2] != 0 || !matches!(header[1], 1 | 3) {
        client.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0])?;
        bail!("此入口支持 SOCKS5 TCP CONNECT");
    }
    let (host, port) = socks_address_with_zero(client, header[3], header[1] == 3)?;
    Ok(Request {
        host,
        port,
        network: if header[1] == 3 { "udp".into() } else { "tcp".into() },
        initial: vec![],
        body_limit: None,
        socks: true,
        udp_associate: header[1] == 3,
        handshake: vec![5, 0, 0, 1, 0, 0, 0, 0, 0, 0],
    })
}
pub(crate) fn socks_address(stream: &mut TcpStream, atyp: u8) -> Result<(String, u16)> {
    socks_address_with_zero(stream, atyp, false)
}

fn socks_address_with_zero(stream: &mut TcpStream, atyp: u8, allow_zero: bool) -> Result<(String, u16)> {
    let host = match atyp {
        1 => {
            let mut ip = [0; 4];
            stream.read_exact(&mut ip)?;
            std::net::Ipv4Addr::from(ip).to_string()
        }
        4 => {
            let mut ip = [0; 16];
            stream.read_exact(&mut ip)?;
            std::net::Ipv6Addr::from(ip).to_string()
        }
        3 => {
            let mut len = [0];
            stream.read_exact(&mut len)?;
            if len[0] == 0 {
                bail!("目标域名为空");
            }
            let mut name = vec![0; len[0] as usize];
            stream.read_exact(&mut name)?;
            String::from_utf8(name)?
        }
        _ => bail!("SOCKS 地址类型不支持"),
    };
    let mut port = [0; 2];
    stream.read_exact(&mut port)?;
    let port = u16::from_be_bytes(port);
    if (port == 0 && !allow_zero) || host.chars().any(char::is_control) {
        bail!("SOCKS 目标无效");
    }
    Ok((host, port))
}
pub fn dial_socks(
    address: SocketAddr,
    username: &str,
    password: &str,
    host: &str,
    port: u16,
) -> Result<TcpStream> {
    let mut stream = TcpStream::connect_timeout(&address, HANDSHAKE_TIMEOUT)?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;
    let method = if username.is_empty() && password.is_empty() {
        0
    } else {
        2
    };
    stream.write_all(&[5, 1, method])?;
    let mut reply = [0; 2];
    stream.read_exact(&mut reply)?;
    if reply != [5, method] {
        bail!("Xray 私有入口拒绝认证方式");
    }
    if method == 2 {
        if username.len() > 255 || password.len() > 255 {
            bail!("SOCKS 凭据过长");
        }
        let mut auth = vec![1, username.len() as u8];
        auth.extend_from_slice(username.as_bytes());
        auth.push(password.len() as u8);
        auth.extend_from_slice(password.as_bytes());
        stream.write_all(&auth)?;
        stream.read_exact(&mut reply)?;
        if reply != [1, 0] {
            bail!("Xray 私有入口认证失败");
        }
    }
    let mut command = vec![5, 1, 0];
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => {
            command.push(1);
            command.extend(ip.octets());
        }
        Ok(IpAddr::V6(ip)) => {
            command.push(4);
            command.extend(ip.octets());
        }
        Err(_) => {
            if host.is_empty() || host.len() > 255 {
                bail!("目标域名无效");
            }
            command.push(3);
            command.push(host.len() as u8);
            command.extend(host.as_bytes());
        }
    }
    command.extend(port.to_be_bytes());
    stream.write_all(&command)?;
    let mut header = [0; 4];
    stream.read_exact(&mut header)?;
    if header[0] != 5 || header[1] != 0 || header[2] != 0 {
        bail!("所选节点无法连接目标");
    }
    // Xray may return an unspecified BND address, including port zero.
    let size = match header[3] {
        1 => 4,
        4 => 16,
        3 => {
            let mut n = [0];
            stream.read_exact(&mut n)?;
            n[0] as usize
        }
        _ => bail!("SOCKS 回复地址无效"),
    };
    let mut bound = vec![0; size + 2];
    stream.read_exact(&mut bound)?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resource_exhaustion_keeps_accept_loop_retriable() {
        assert!(is_resource_exhaustion(&io::Error::from_raw_os_error(libc::EMFILE)));
        assert!(is_resource_exhaustion(&io::Error::from_raw_os_error(libc::ENFILE)));
        assert!(!is_resource_exhaustion(&io::Error::from_raw_os_error(libc::ECONNABORTED)));
    }

    #[test]
    fn flow_guard_releases_tracked_socket_clones() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        let state = Arc::new(State {
            stop: AtomicBool::new(false), active: AtomicUsize::new(1), serial: AtomicU64::new(1),
            up: AtomicU64::new(0), down: AtomicU64::new(0), rows: Mutex::new(VecDeque::new()),
        });
        let entry = Arc::new(Entry {
            id: "cleanup".into(), started: Instant::now(), source_port: 0, udp: AtomicBool::new(false),
            visible: AtomicBool::new(true), label: Mutex::new((String::new(), String::new())),
            process: Mutex::new(String::new()), app_matcher: Mutex::new(String::new()), sockets: Mutex::new(vec![]),
            closed: AtomicBool::new(false), finished: AtomicBool::new(false), up: AtomicU64::new(0), down: AtomicU64::new(0), last_activity: AtomicU64::new(0),
        });
        entry.track(&client).unwrap();
        entry.track(&server).unwrap();
        assert_eq!(entry.sockets.lock().unwrap().len(), 2);
        #[cfg(unix)]
        let tracked_fd = std::os::unix::io::AsRawFd::as_raw_fd(&entry.sockets.lock().unwrap()[0]);
        {
            let _guard = FlowGuard { state, entry: entry.clone() };
        }
        assert!(entry.sockets.lock().unwrap().is_empty());
        assert!(entry.finished.load(Ordering::Acquire));
        #[cfg(unix)]
        assert_eq!(unsafe { libc::fcntl(tracked_fd, libc::F_GETFD) }, -1);
    }

    #[cfg(unix)]
    #[test]
    fn listener_survives_child_emfile_without_recreation() {
        let control = std::env::temp_dir().join(format!("myproxy-emfile-{}", std::process::id()));
        let release = control.with_extension("release");
        let done = control.with_extension("done");
        if std::env::var_os("MYPROXY_RELAY_EMFILE_CHILD").is_some() {
            unsafe {
                let limit = libc::rlimit { rlim_cur: 64, rlim_max: 64 };
                assert_eq!(libc::setrlimit(libc::RLIMIT_NOFILE, &limit), 0);
            }
            let target = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let target_address = target.local_addr().unwrap();
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let relay_address = listener.local_addr().unwrap();
            let server = MixedServer::start(listener, Arc::new(move |_, _| {
                Ok(Dialed { stream: TcpStream::connect(target_address)?, chain: "EMFILE".into(), rule: "fixture".into() })
            })).unwrap();
            std::fs::write(&control, format!("{}\n{}\n", relay_address.port(), target_address.port())).unwrap();
            let target_worker = thread::spawn(move || {
                let (mut stream, _) = target.accept().unwrap();
                let mut request = [0; 4]; stream.read_exact(&mut request).unwrap();
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK").unwrap();
            });
            let mut held = Vec::new();
            while let Ok(file) = std::fs::File::open("/dev/null") { held.push(file); }
            while !release.exists() { thread::sleep(Duration::from_millis(5)); }
            drop(held);
            target_worker.join().unwrap();
            drop(server);
            std::fs::write(done, "done").unwrap();
            return;
        }
        let _ = std::fs::remove_file(&control);
        let _ = std::fs::remove_file(&release);
        let _ = std::fs::remove_file(&done);
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "xray::relay::tests::listener_survives_child_emfile_without_recreation", "--nocapture"])
            .env("MYPROXY_RELAY_EMFILE_CHILD", "1")
            .spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !control.exists() && Instant::now() < deadline { thread::sleep(Duration::from_millis(10)); }
        assert!(control.exists(), "child did not publish relay metadata");
        let ports: Vec<u16> = std::fs::read_to_string(&control).unwrap().lines().map(|line| line.parse().unwrap()).collect();
        assert_eq!(ports.len(), 2);
        let mut client = TcpStream::connect(("127.0.0.1", ports[0])).unwrap();
        client.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        client.write_all(b"GET http://fixture.invalid/ HTTP/1.1\r\nHost: fixture.invalid\r\n\r\n").unwrap();
        client.shutdown(Shutdown::Write).unwrap();
        std::fs::write(&release, "release").unwrap();
        let mut response = String::new(); client.read_to_string(&mut response).unwrap();
        assert!(response.contains("200 OK"), "listener failed to recover after EMFILE: {response:?}");
        let deadline = Instant::now() + Duration::from_secs(3);
        while !done.exists() && Instant::now() < deadline { thread::sleep(Duration::from_millis(10)); }
        assert!(done.exists(), "child did not complete");
        let status = child.wait_with_output().unwrap();
        assert!(status.status.success(), "child failed: {}", String::from_utf8_lossy(&status.stderr));
        let _ = std::fs::remove_file(control);
        let _ = std::fs::remove_file(release);
        let _ = std::fs::remove_file(done);
    }
    #[test]
    fn ipv6_authorities_and_invalid_ports() {
        assert_eq!(
            authority("[2001:db8::1]:443", 80).unwrap(),
            ("2001:db8::1".into(), 443)
        );
        for value in [
            "[::1]garbage",
            "user@host:80",
            "host:0",
            "bad host:80",
            "[invalid]:80",
        ] {
            assert!(authority(value, 80).is_err(), "{value}");
        }
    }
    #[test]
    fn forward_request_strips_proxy_credentials_and_rejects_ambiguous_framing() {
        fn parse(request: &[u8]) -> Result<Request> {
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            client.write_all(request).unwrap();
            let (mut server, _) = listener.accept().unwrap();
            let mut first = [0];
            server.read_exact(&mut first).unwrap();
            http_request(&mut server, first[0])
        }
        let request=parse(b"POST http://test.invalid/x HTTP/1.1\r\nHost: test.invalid\r\nProxy-Authorization: secret\r\nContent-Length: 1\r\n\r\naGET http://evil.invalid/ HTTP/1.1\r\n\r\n").unwrap();
        let text = String::from_utf8(request.initial).unwrap();
        assert!(text.starts_with("POST /x"));
        assert!(!text.contains("secret"));
        assert!(!text.contains("evil"));
        assert!(text.ends_with('a'));
        for bytes in [b"GET / HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n".as_slice(),b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\nContent-Length: 0\r\n\r\n"] { assert!(parse(bytes).is_err()); }
    }
    #[test]
    fn live_bytes_and_close_are_observable_before_eof() {
        let upstream = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let target = upstream.local_addr().unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let mixed = MixedServer::start(
            listener,
            Arc::new(move |_, _| {
                Ok(Dialed {
                    stream: TcpStream::connect(target)?,
                    chain: "NODE_A".into(),
                    rule: "manual".into(),
                })
            }),
        )
        .unwrap();
        let echo = thread::spawn(move || {
            let (mut socket, _) = upstream.accept().unwrap();
            let mut buf = [0; 4];
            socket.read_exact(&mut buf).unwrap();
            socket.write_all(&buf).unwrap();
            let mut end = [0];
            assert_eq!(socket.read(&mut end).unwrap(), 0);
        });
        let mut client = TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        client
            .write_all(b"CONNECT host:443 HTTP/1.1\r\nHost: host:443\r\n\r\n")
            .unwrap();
        let mut head = vec![];
        while !head.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            client.read_exact(&mut byte).unwrap();
            head.push(byte[0]);
        }
        client.write_all(b"ping").unwrap();
        let mut buf = [0; 4];
        client.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"ping");
        let snapshot = mixed.snapshot();
        assert_eq!(snapshot.upload_total, 4);
        assert_eq!(snapshot.download_total, 4);
        assert_eq!(snapshot.connection_count, 1);
        mixed.close_one(&snapshot.connections[0].id).unwrap();
        assert_eq!(client.read(&mut buf).unwrap(), 0);
        echo.join().unwrap();
    }
    #[test]
    fn client_disconnect_releases_entry_when_upstream_closes_after_half_close() {
        let upstream = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let target = upstream.local_addr().unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let mixed = MixedServer::start(
            listener,
            Arc::new(move |_, _| {
                Ok(Dialed {
                    stream: TcpStream::connect(target)?,
                    chain: "NODE_A".into(),
                    rule: "disconnect-fixture".into(),
                })
            }),
        )
        .unwrap();
        let upstream_done = thread::spawn(move || {
            let (mut socket, _) = upstream.accept().unwrap();
            let mut request = [0; 4];
            socket.read_exact(&mut request).unwrap();
            let mut eof = [0; 1];
            assert_eq!(socket.read(&mut eof).unwrap(), 0);
            // The upstream closes after observing the client's half-close;
            // this is the safe condition in which both relay pumps can end.
        });
        let mut client = TcpStream::connect(address).unwrap();
        client.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        client.write_all(b"CONNECT host:443 HTTP/1.1\r\nHost: host:443\r\n\r\n").unwrap();
        let mut head = vec![];
        while !head.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            client.read_exact(&mut byte).unwrap();
            head.push(byte[0]);
        }
        client.write_all(b"ping").unwrap();
        client.shutdown(Shutdown::Write).unwrap();
        upstream_done.join().unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline && mixed.snapshot().connection_count != 0 {
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(mixed.snapshot().connection_count, 0);
    }
    #[test]
    fn forward_proxy_never_relays_a_second_host_on_the_same_stream() {
        let upstream = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let target = upstream.local_addr().unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let _mixed = MixedServer::start(
            listener,
            Arc::new(move |host, _| {
                assert_eq!(host, "safe.invalid");
                Ok(Dialed {
                    stream: TcpStream::connect(target)?,
                    chain: "A".into(),
                    rule: "safe".into(),
                })
            }),
        )
        .unwrap();
        let capture = thread::spawn(move || {
            let (mut socket, _) = upstream.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut request = String::new();
            socket.read_to_string(&mut request).unwrap();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK")
                .unwrap();
            request
        });
        let mut client = TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        client.write_all(b"POST http://safe.invalid/ HTTP/1.1\r\nHost: safe.invalid\r\nContent-Length: 1\r\n\r\naGET http://other.invalid/ HTTP/1.1\r\nHost: other.invalid\r\n\r\n").unwrap();
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert!(response.ends_with("OK"));
        let request = capture.join().unwrap();
        assert!(request.starts_with("POST / HTTP/1.1"));
        assert!(request.ends_with('a'));
        assert!(!request.contains("other.invalid"));
    }
}
