//! End-to-end DNS bootstrap coverage. This test deliberately uses only local
//! listeners and a temporary Xray configuration; it never reads application
//! state or contacts a public resolver.

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::thread;
use std::time::{Duration, Instant};

fn tcp_dns(listener: TcpListener, answer: Option<IpAddr>, queries: Arc<std::sync::Mutex<usize>>, stop: Arc<AtomicBool>) {
    listener.set_nonblocking(true).unwrap();
    while !stop.load(Ordering::Acquire) {
        let Ok((mut stream, _)) = listener.accept() else { thread::sleep(Duration::from_millis(5)); continue };
        let queries = queries.clone();
        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        stream.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
        thread::spawn(move || {
            let mut frame = [0u8; 2];
            if stream.read_exact(&mut frame).is_err() { return; }
            let size = u16::from_be_bytes(frame) as usize;
            if !(12..=4096).contains(&size) { return; }
            let mut query = vec![0u8; size];
            if stream.read_exact(&mut query).is_err() { return; }
            *queries.lock().unwrap() += 1;
            let mut response = query.clone();
            response[2] = 0x81;
            response[3] = if answer.is_some() { 0x80 } else { 0x82 };
            if let Some(IpAddr::V4(ip)) = answer {
                response[6] = 0;
                response[7] = 1;
                response.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 30, 0, 4]);
                response.extend_from_slice(&ip.octets());
            }
            let _ = stream.write_all(&(response.len() as u16).to_be_bytes());
            let _ = stream.write_all(&response);
        });
    }
}

struct ChildGuard(Child, std::path::PathBuf);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
        let _ = std::fs::remove_dir_all(&self.1);
    }
}

fn free_port() -> u16 { TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port() }

fn socks_connect(mut stream: TcpStream, host: &str, port: u16) -> std::io::Result<TcpStream> {
    stream.write_all(&[5, 1, 0])?;
    let mut hello = [0; 2]; stream.read_exact(&mut hello)?;
    assert_eq!(hello, [5, 0]);
    let host = host.as_bytes();
    stream.write_all(&[5, 1, 0, 3, host.len() as u8])?;
    stream.write_all(host)?;
    stream.write_all(&port.to_be_bytes())?;
    let mut reply = [0; 10]; stream.read_exact(&mut reply)?;
    assert_eq!(reply[1], 0, "SOCKS connect failed: {:?}", reply);
    Ok(stream)
}

#[test]
#[cfg(feature = "xray-channel")]
fn xray_dns_falls_back_between_tagged_tcp_resolvers_without_os_dns() {
    let first = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let second = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let first_addr = first.local_addr().unwrap();
    let second_addr = second.local_addr().unwrap();
    let first_queries = Arc::new(std::sync::Mutex::new(0));
    let second_queries = Arc::new(std::sync::Mutex::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let a = { let q = first_queries.clone(); let s = stop.clone(); thread::spawn(move || tcp_dns(first, None, q, s)) };
    let b = { let q = second_queries.clone(); let s = stop.clone(); thread::spawn(move || tcp_dns(second, Some("127.0.0.1".parse().unwrap()), q, s)) };
    let target = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let target_addr = target.local_addr().unwrap();
    target.set_nonblocking(true).unwrap();
    let target_stop = stop.clone();
    let target_thread = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !target_stop.load(Ordering::Acquire) && Instant::now() < deadline {
            if let Ok((mut socket, _)) = target.accept() { socket.write_all(b"dns-ok").unwrap(); return; }
            thread::sleep(Duration::from_millis(10));
        }
    });

    let dns = super::proxy_server_dns_with_resolvers(&[
        IpAddr::V4("127.0.0.1".parse().unwrap()),
        IpAddr::V4("127.0.0.1".parse().unwrap()),
    ]);
    let mut dns = dns;
    dns["servers"][0] = serde_json::json!(format!("tcp://127.0.0.1:{}", first_addr.port()));
    dns["servers"][1] = serde_json::json!(format!("tcp://127.0.0.1:{}", second_addr.port()));
    let mut rule = super::proxy_server_dns_rule_with_resolvers(&[
        IpAddr::V4(first_addr.ip().to_string().parse().unwrap()),
        IpAddr::V4(second_addr.ip().to_string().parse().unwrap()),
    ]);
    rule["port"] = serde_json::json!(format!("{},{}", first_addr.port(), second_addr.port()));
    let dir = std::env::temp_dir().join(format!("myproxy-xray-dns-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    let config = serde_json::json!({
        "log": {"loglevel":"error"}, "dns": dns,
        "inbounds": [{"tag":"socks", "listen":"127.0.0.1", "port": free_port(), "protocol":"socks", "settings": {"udp":false}}],
        "outbounds": [{"tag":"reject", "protocol":"blackhole"}, {"tag":"dns-direct", "protocol":"freedom"}, {"tag":"direct", "protocol":"freedom", "settings":{"domainStrategy":"UseIP"}}],
        "routing": {"domainStrategy":"AsIs", "rules":[rule, {"inboundTag":["socks"],"outboundTag":"direct"}]}
    });
    let path = dir.join("config.json");
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    let binary = super::xray_binary().unwrap();
    let child = Command::new(binary).args(["run", "-config"]).arg(&path).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    let guard = ChildGuard(child, dir.clone());
    let _ = guard.0.id();
    let deadline = Instant::now() + Duration::from_secs(5);
    let socks_port = config["inbounds"][0]["port"].as_u64().unwrap() as u16;
    let stream = loop {
        if Instant::now() > deadline { panic!("Xray SOCKS listener did not start"); }
        if let Ok(stream) = TcpStream::connect(SocketAddr::from(([127, 0, 0, 1], socks_port))) { break stream; }
        thread::sleep(Duration::from_millis(20));
    };
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut connected = socks_connect(stream, "dns-fixture.invalid", target_addr.port()).unwrap();
    let mut bytes = [0; 6]; connected.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, b"dns-ok");
    assert!(*first_queries.lock().unwrap() > 0, "primary resolver was not attempted");
    assert!(*second_queries.lock().unwrap() > 0, "secondary resolver fallback was not attempted");
    drop(connected);
    stop.store(true, Ordering::Release);
    let _ = a.join(); let _ = b.join(); let _ = target_thread.join();
}
