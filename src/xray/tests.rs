use super::*;
use crate::catalog::Node;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};

static TEST_LOCK: Mutex<()> = Mutex::new(());

struct Guard(PathBuf);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = disconnect();
        std::env::remove_var(crate::paths::data_dir_env());
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn isolated() -> Guard {
    let directory =
        std::env::temp_dir().join(format!("myproxy-xray-test-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&directory).unwrap();
    std::env::set_var(crate::paths::data_dir_env(), &directory);
    Guard(directory)
}

#[test]
#[cfg(feature = "xray-channel")]
fn xray_data_ignores_inherited_production_override() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let isolated = isolated();
    let production = isolated.0.join("production");
    fs::create_dir(&production).unwrap();
    let strategy = production.join("strategy.json");
    fs::write(&strategy, b"preserved production configuration").unwrap();
    let previous = std::env::var_os("MYPROXY_DATA_DIR");
    std::env::set_var("MYPROXY_DATA_DIR", &production);
    let selected = crate::paths::data_dir();
    match previous {
        Some(value) => std::env::set_var("MYPROXY_DATA_DIR", value),
        None => std::env::remove_var("MYPROXY_DATA_DIR"),
    }
    assert_eq!(selected.unwrap(), isolated.0);
    assert_eq!(fs::read(strategy).unwrap(), b"preserved production configuration");
}

fn headers(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        stream.read_exact(&mut byte).unwrap();
        bytes.push(byte[0]);
        assert!(bytes.len() < 65536);
    }
    String::from_utf8(bytes).unwrap()
}
fn proxy(label: &'static str) -> (SocketAddr, Arc<AtomicBool>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    std::thread::spawn(move || {
        while !stopped.load(Ordering::Acquire) {
            if let Ok((mut socket, _)) = listener.accept() {
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(4)))
                    .unwrap();
                std::thread::spawn(move || {
                    let request = headers(&mut socket);
                    assert!(request.starts_with("CONNECT "));
                    socket
                        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                        .unwrap();
                    let request = headers(&mut socket);
                    assert!(request.starts_with("GET /"));
                    if request.starts_with("GET /hold ") {
                        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 99\r\n\r\nX").unwrap();
                        let mut end=[0];
                        let _=socket.read(&mut end);
                        return;
                    }
                    write!(
                        socket,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        label.len(),
                        label
                    )
                    .unwrap();
                    let _ = socket.shutdown(Shutdown::Write);
                });
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    });
    (address, stop)
}
fn request(port: u16, protocol: &str) -> String {
    let mut socket = TcpStream::connect(("127.0.0.1", port)).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(8)))
        .unwrap();
    if protocol == "socks" {
        socket.write_all(&[5, 1, 0]).unwrap();
        let mut method = [0; 2];
        socket.read_exact(&mut method).unwrap();
        assert_eq!(method, [5, 0]);
        let host = b"test.invalid";
        let mut cmd = vec![5, 1, 0, 3, host.len() as u8];
        cmd.extend(host);
        cmd.extend(80u16.to_be_bytes());
        socket.write_all(&cmd).unwrap();
        let mut reply = [0; 10];
        socket.read_exact(&mut reply).unwrap();
        assert_eq!(reply[1], 0);
    } else if protocol == "connect" {
        socket
            .write_all(b"CONNECT test.invalid:80 HTTP/1.1\r\nHost: test.invalid:80\r\n\r\n")
            .unwrap();
        assert!(headers(&mut socket).starts_with("HTTP/1.1 200"));
    }
    let uri = if protocol == "http" {
        "http://test.invalid/"
    } else {
        "/"
    };
    write!(
        socket,
        "GET {uri} HTTP/1.1\r\nHost: test.invalid\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).unwrap();
    response
}
fn catalog(a: SocketAddr, b: SocketAddr) -> Catalog {
    Catalog {
        nodes: [("A", a), ("B", b)]
            .into_iter()
            .map(|(name, address)| Node {
                name: name.into(),
                subscription: "fixture".into(),
                raw: serde_yaml::from_str(&format!(
                    "name: {name}\ntype: http\nserver: 127.0.0.1\nport: {}\n",
                    address.port()
                ))
                .unwrap(),
            })
            .collect(),
        ..Catalog::default()
    }
}
fn port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[test]
fn mixed_global_rules_and_selection_use_real_xray_and_application_ledger() {
    if std::env::var_os("XRAY_BINARY").is_none() {
        return;
    }
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _guard = isolated();
    let (a, stop_a) = proxy("NODE_A");
    let (b, stop_b) = proxy("NODE_B");
    let mut strategy = default_strategy();
    strategy.mixed_port = port();
    strategy.global_selected = "A".into();
    strategy.groups = vec![Group::all_nodes("节点选择".into(), "fallback".into())];
    let catalog = catalog(a, b);
    strategy.save().unwrap();
    activate(&strategy, &catalog).unwrap();
    assert_eq!(traffic().unwrap().connection_count, 0);
    for protocol in ["http", "socks", "connect"] {
        assert!(request(strategy.mixed_port, protocol).ends_with("NODE_A"));
    }
    let identity = runtime_identity().unwrap();
    let core_pid = active().unwrap().child.lock().unwrap().id();
    let mut held=relay::dial_socks(SocketAddr::from(([127,0,0,1],strategy.mixed_port)),"","","test.invalid",80).unwrap();
    held.write_all(b"GET /hold HTTP/1.1\r\nHost: test.invalid\r\n\r\n").unwrap();
    assert!(headers(&mut held).starts_with("HTTP/1.1 200"));
    let mut initial=[0];held.read_exact(&mut initial).unwrap();assert_eq!(&initial,b"X");
    strategy.global_selected = "B".into();
    strategy.save().unwrap();
    select_proxy(&identity, GLOBAL_GROUP, "B").unwrap();
    match held.read(&mut initial) {
        Ok(0)=>{},
        Err(error) if matches!(error.kind(),std::io::ErrorKind::ConnectionReset|std::io::ErrorKind::ConnectionAborted)=>{},
        other=>panic!("old global connection still alive: {other:?}"),
    }
    assert!(request(strategy.mixed_port, "http").ends_with("NODE_B"));
    assert_eq!(
        active().unwrap().child.lock().unwrap().id(),
        core_pid,
        "selection must not restart core"
    );
    let ledger = traffic().unwrap();
    assert!(ledger.upload_total > 0 && ledger.download_total > 0);
    assert!(ledger
        .connections
        .iter()
        .any(|row| row.chain.contains("B") && row.destination.contains("test.invalid")));
    strategy.mixed_mode = InboundMode::Rule;
    strategy.rule_sets = vec![crate::strategy::RuleSet {
        unavailable_fallback: Default::default(),
        id: "test".into(),
        name: "网站走A".into(),
        via: "A".into(),
        matchers: vec![crate::strategy::Matcher {
            kind: "suffix".into(),
            value: "test.invalid".into(),
        }],
    }];
    strategy.unmatched_via = "B".into();
    strategy.save().unwrap();
    activate(&strategy, &catalog).unwrap();
    assert!(request(strategy.mixed_port, "http").ends_with("NODE_A"));
    assert_eq!(active().unwrap().child.lock().unwrap().id(),core_pid,"routing changes must not restart Xray");
    disconnect().unwrap();
    assert!(!is_running());
    activate(&strategy, &catalog).unwrap();
    assert_eq!(traffic().unwrap().connection_count, 0);
    stop_a.store(true, Ordering::Release);
    stop_b.store(true, Ordering::Release);
}

#[test]
fn occupied_mixed_port_does_not_stop_existing_session() {
    if std::env::var_os("XRAY_BINARY").is_none() {
        return;
    }
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _guard = isolated();
    let mut strategy = default_strategy();
    strategy.mixed_port = port();
    strategy.mixed_mode = InboundMode::Direct;
    strategy.save().unwrap();
    activate(&strategy, &Catalog::default()).unwrap();
    let identity = runtime_identity().unwrap();
    let occupied = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    strategy.mixed_port = occupied.local_addr().unwrap().port();
    assert!(activate(&strategy, &Catalog::default()).is_err());
    assert_eq!(runtime_identity(), Some(identity));
    assert!(status().unwrap().ready);
}

#[test]
fn disconnect_reaps_xray_while_a_background_task_retains_the_runtime() {
    if std::env::var_os("XRAY_BINARY").is_none() {
        return;
    }
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _guard = isolated();
    let mut strategy = default_strategy();
    strategy.mixed_port = port();
    strategy.mixed_mode = InboundMode::Direct;
    activate(&strategy, &Catalog::default()).unwrap();
    let retained = active().unwrap();

    disconnect().unwrap();

    assert!(retained.child.lock().unwrap().try_wait().unwrap().is_some());
    assert!(!status().unwrap().running);
    disconnect().unwrap();
}

#[test]
fn replacement_reaps_xray_while_a_background_task_retains_the_old_runtime() {
    if std::env::var_os("XRAY_BINARY").is_none() {
        return;
    }
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _guard = isolated();
    let mut strategy = default_strategy();
    strategy.mixed_port = port();
    strategy.mixed_mode = InboundMode::Direct;
    activate(&strategy, &Catalog::default()).unwrap();
    let retained = active().unwrap();
    let old_pid = retained.child.lock().unwrap().id();

    strategy.mixed_port = port();
    activate(&strategy, &Catalog::default()).unwrap();

    assert!(retained.child.lock().unwrap().try_wait().unwrap().is_some());
    assert_ne!(active().unwrap().child.lock().unwrap().id(), old_pid);
    assert!(status().unwrap().ready);
}

#[test]
fn xray_authenticated_capture_udp_traverses_real_core_and_app_owned_upstream() {
    if std::env::var_os("XRAY_BINARY").is_none() { return; }
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _guard = isolated();
    let echo = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    echo.set_read_timeout(Some(Duration::from_secs(6))).unwrap();
    let target_port = echo.local_addr().unwrap().port();
    let echo_worker = std::thread::spawn(move || {
        let mut payload=[0;1024];let(size,peer)=echo.recv_from(&mut payload).unwrap();
        assert_eq!(&payload[..size], b"XRAY_UDP_PAYLOAD");
        echo.send_to(&payload[..size], peer).unwrap();
    });
    let upstream_listener=TcpListener::bind("127.0.0.1:0").unwrap();
    let upstream_port=upstream_listener.local_addr().unwrap().port();
    let direct: Dialer=Arc::new(|host,port| Ok(Dialed {stream:TcpStream::connect((host,port))?,chain:"DIRECT".into(),rule:String::new()}));
    let _upstream=MixedServer::start_authenticated(upstream_listener,direct.clone(),"fixture".into(),"password".into(),Arc::new(|_,_| Ok(udp::DatagramRoute::Direct))).unwrap();
    let catalog=Catalog {nodes:vec![Node {name:"UDP node".into(),subscription:"fixture".into(),raw:serde_yaml::from_str(&format!("name: UDP node\ntype: socks5\nserver: 127.0.0.1\nport: {upstream_port}\nusername: fixture\npassword: password\n")).unwrap()}],..Catalog::default()};
    let mut strategy=default_strategy();strategy.mixed_port=port();strategy.global_selected="UDP node".into();strategy.extension_mode=InboundMode::Global;
    activate(&strategy,&catalog).unwrap();
    let capture_listener=TcpListener::bind("127.0.0.1:0").unwrap();let capture_address=capture_listener.local_addr().unwrap();
    let _capture=MixedServer::start_authenticated(capture_listener,direct,"capture".into(),"secret".into(),Arc::new(|host,port| active()?.datagram_route(host,port,&Ingress::Capture))).unwrap();
    let (control,relay)=udp::associate(capture_address,"capture","secret").unwrap();
    let socket=std::net::UdpSocket::bind("127.0.0.1:0").unwrap();socket.set_read_timeout(Some(Duration::from_secs(6))).unwrap();
    socket.send_to(&udp::encode_target("127.0.0.1",target_port,b"XRAY_UDP_PAYLOAD").unwrap(),relay).unwrap();
    let mut response=[0;2048];let(size,_)=socket.recv_from(&mut response).unwrap();let(_,_,offset)=udp::decode_target(&response[..size]).unwrap();assert_eq!(&response[offset..size],b"XRAY_UDP_PAYLOAD");
    drop(control);echo_worker.join().unwrap();
}

#[test]
fn xray_udp_dns_preserves_queries_and_resolver_through_selected_tcp_node() {
    if std::env::var_os("XRAY_BINARY").is_none() { return; }
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _guard = isolated();
    let resolver = TcpListener::bind("127.0.0.1:0").unwrap();
    let resolver_address = resolver.local_addr().unwrap();
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = resolver.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(6))).unwrap();
        for qtype in [1u16, 28, 16] {
            let mut size = [0; 2];
            stream.read_exact(&mut size).unwrap();
            let mut query = vec![0; u16::from_be_bytes(size) as usize];
            stream.read_exact(&mut query).unwrap();
            assert_eq!(&query[12..25], b"\x07example\x03com\x00");
            assert_eq!(&query[25..27], &qtype.to_be_bytes());
            query[2] = 0x81;
            query[3] = 0x80;
            stream.write_all(&size).unwrap();
            stream.write_all(&query).unwrap();
        }
    });
    let upstream_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let upstream_port = upstream_listener.local_addr().unwrap().port();
    let dialer: Dialer = Arc::new(move |host, port| {
        assert_eq!((host, port), ("resolver.invalid", 53));
        Ok(Dialed { stream: TcpStream::connect(resolver_address)?, chain: "fixture".into(), rule: String::new() })
    });
    let _upstream = MixedServer::start_authenticated(upstream_listener, dialer, "fixture".into(), "password".into(),
        Arc::new(|_, _| bail!("this node only supports TCP"))).unwrap();
    let catalog = Catalog { nodes: vec![Node {
        name: "TCP node".into(), subscription: "fixture".into(),
        raw: serde_yaml::from_str(&format!("name: TCP node\ntype: socks5\nserver: 127.0.0.1\nport: {upstream_port}\nusername: fixture\npassword: password\n")).unwrap(),
    }], ..Catalog::default() };
    let mut strategy = default_strategy();
    strategy.mixed_port = port();
    strategy.global_selected = "TCP node".into();
    activate(&strategy, &catalog).unwrap();
    let runtime = active().unwrap();
    let lane = runtime.lanes.get("TCP node").unwrap();
    let (control, relay) = udp::associate(lane.address, &lane.username, &lane.password).unwrap();
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.set_read_timeout(Some(Duration::from_secs(6))).unwrap();
    for qtype in [1u16, 28, 16] {
        let mut query = vec![0x12, qtype as u8, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        query.extend_from_slice(b"\x07example\x03com\x00");
        query.extend_from_slice(&qtype.to_be_bytes());
        query.extend_from_slice(&[0, 1]);
        socket.send_to(&udp::encode_target("resolver.invalid", 53, &query).unwrap(), relay).unwrap();
        let mut response = [0; 2048];
        let (size, _) = socket.recv_from(&mut response).unwrap();
        let (_, response_port, offset) = udp::decode_target(&response[..size]).unwrap();
        query[2] = 0x81;
        query[3] = 0x80;
        assert_eq!(response_port, 53);
        assert_eq!(&response[offset..size], query);
    }
    drop(control);
    worker.join().unwrap();
}

#[test]
fn nested_region_fallback_changes_real_xray_egress_without_core_restart() {
    if std::env::var_os("XRAY_BINARY").is_none() { return; }
    let _serial = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let _guard = isolated();
    let (a, stop_a) = proxy("REGION_US");
    let (b, stop_b) = proxy("REGION_JP");
    let mut strategy = default_strategy();
    strategy.mixed_port = port();
    strategy.global_selected = "Priority".into();
    let mut us = Group::matching("US".into(), "url-test".into(), vec![], vec![]);
    us.include = vec!["A".into()];
    let mut jp = Group::matching("JP".into(), "url-test".into(), vec![], vec![]);
    jp.include = vec!["B".into()];
    let mut priority = Group::matching("Priority".into(), "fallback".into(), vec![], vec![]);
    priority.group_refs = vec!["US".into(), "JP".into()];
    strategy.groups = vec![priority, us, jp];
    strategy.unmatched_via = "Priority".into();
    strategy.save().unwrap();
    activate(&strategy, &catalog(a, b)).unwrap();
    let runtime = active().unwrap();
    let _probe = runtime.probe_lock.lock().unwrap();
    *runtime.health.write().unwrap() = HashMap::from([
        ("A".into(), policy::NodeHealth { delay_ms: Some(80), failures: 0 }),
        ("B".into(), policy::NodeHealth { delay_ms: Some(5), failures: 0 }),
    ]);
    let pid = runtime.child.lock().unwrap().id();
    assert!(request(strategy.mixed_port, "http").ends_with("REGION_US"));
    runtime.health.write().unwrap().get_mut("A").unwrap().failures = 2;
    assert!(request(strategy.mixed_port, "socks").ends_with("REGION_JP"));
    let chains = traffic().unwrap().connections.into_iter().map(|row| row.chain).collect::<Vec<_>>();
    assert!(chains.iter().any(|chain| chain == "全局出口 → Priority → JP → B"), "{chains:?}");
    runtime.health.write().unwrap().get_mut("A").unwrap().failures = 0;
    assert!(request(strategy.mixed_port, "connect").ends_with("REGION_US"));
    assert_eq!(runtime.child.lock().unwrap().id(), pid);
    stop_a.store(true, Ordering::Release);
    stop_b.store(true, Ordering::Release);
}

#[test]
fn regional_references_compile_without_flattening_for_the_original_backend() {
    let strategy = default_strategy();
    let yaml = crate::compile::compile(&strategy, &Catalog::default()).unwrap();
    let root: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
    let groups = root.get("proxy-groups").unwrap().as_sequence().unwrap();
    let find = |name: &str| groups.iter().find(|value| value.get("name").and_then(serde_yaml::Value::as_str) == Some(name)).unwrap();
    let choices = |name| find(name).get("proxies").unwrap().as_sequence().unwrap().iter().map(|item| item.as_str().unwrap().to_string()).collect::<Vec<_>>();
    assert_eq!(choices("美国优先"), ["美国", "日本", "香港"]);
    assert_eq!(choices("日本优先"), ["日本", "香港", "美国"]);
    assert_eq!(choices("香港优先"), ["香港", "美国", "日本"]);
    assert_eq!(find("美国优先").get("type").unwrap().as_str(), Some("fallback"));
    assert_eq!(choices("美国"), ["REJECT"]);
}
