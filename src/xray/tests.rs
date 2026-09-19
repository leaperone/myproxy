use super::*;
use crate::catalog::Node;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};

static TEST_LOCK: Mutex<()> = Mutex::new(());

struct Guard(PathBuf);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = disconnect();
        std::env::remove_var("MYPROXY_DATA_DIR");
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn isolated() -> Guard {
    let directory =
        std::env::temp_dir().join(format!("myproxy-xray-test-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&directory).unwrap();
    std::env::set_var("MYPROXY_DATA_DIR", &directory);
    Guard(directory)
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
