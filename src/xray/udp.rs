use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

#[derive(Clone, PartialEq, Eq)]
pub enum DatagramRoute {
    Direct,
    Socks { address: SocketAddr, username: String, password: String, label: String, fallback_direct: bool },
}

pub type UdpRouter = Arc<dyn Fn(&str, u16) -> Result<DatagramRoute> + Send + Sync>;

struct Destination {
    host: String,
    port: u16,
    route: DatagramRoute,
    requested_route: DatagramRoute,
    socket: UdpSocket,
    control: Option<TcpStream>,
    last_used: Instant,
}

pub fn serve_association(
    mut control: TcpStream,
    requested_port: u16,
    router: UdpRouter,
    mut observe: impl FnMut(&str, u16, &DatagramRoute, u64, u64),
) -> Result<()> {
    let peer_ip = control.peer_addr()?.ip();
    if !peer_ip.is_loopback() { bail!("UDP 捕获仅接受本机连接"); }
    let incoming = UdpSocket::bind(("127.0.0.1", 0))?;
    incoming.set_nonblocking(true)?;
    let port = incoming.local_addr()?.port();
    control.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, (port >> 8) as u8, port as u8])?;
    control.set_nonblocking(true)?;
    let mut peer = if requested_port > 0 { Some(SocketAddr::new(peer_ip, requested_port)) } else { None };
    let mut destinations: Vec<Destination> = Vec::new();
    let mut packet = [0u8; 65535];
    let mut last_activity = Instant::now();
    loop {
        if !control_alive(&control) || last_activity.elapsed() > Duration::from_secs(300) { return Ok(()); }
        let mut progressed = false;
        match incoming.recv_from(&mut packet) {
            Ok((size, source)) => {
                if source.ip() != peer_ip || peer.is_some_and(|expected| expected != source) { continue; }
                let Ok((host, port, offset)) = decode_target(&packet[..size]) else { continue; };
                let Ok(route) = router(&host, port) else { continue; };
                peer.get_or_insert(source);
                let index = destinations.iter().position(|target| target.host == host && target.port == port && target.requested_route == route);
                let index = if let Some(index) = index { index } else {
                    if destinations.len() >= 32 {
                        let oldest = destinations.iter().enumerate().min_by_key(|(_, item)| item.last_used).map(|(index, _)| index).unwrap();
                        destinations.swap_remove(oldest);
                    }
                    let Ok(destination) = Destination::open(host.clone(), port, route.clone()) else { continue; };
                    destinations.push(destination);
                    destinations.len() - 1
                };
                let target = &mut destinations[index];
                let bytes = if matches!(target.route, DatagramRoute::Direct) { &packet[offset..size] } else { &packet[..size] };
                if target.socket.send(bytes).is_ok() {
                    target.last_used = Instant::now();
                    last_activity = Instant::now();
                    observe(&host, port, &target.route, (size - offset) as u64, 0);
                }
                progressed = true;
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {},
            Err(error) => return Err(error.into()),
        }
        if let Some(peer) = peer {
            for target in &mut destinations {
                for _ in 0..16 {
                    match target.socket.recv(&mut packet) {
                        Ok(size) => {
                            let (frame, payload_bytes) = match target.route {
                                DatagramRoute::Direct => (encode_target(&target.host, target.port, &packet[..size])?, size),
                                DatagramRoute::Socks { .. } => {
                                    let Ok((_, _, offset)) = decode_target(&packet[..size]) else { continue; };
                                    (packet[..size].to_vec(), size - offset)
                                }
                            };
                            if incoming.send_to(&frame, peer).is_ok() {
                                target.last_used = Instant::now();
                                last_activity = Instant::now();
                                observe(&target.host, target.port, &target.route, 0, payload_bytes as u64);
                            }
                            progressed = true;
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                        Err(_) => break,
                    }
                }
            }
        }
        destinations.retain(|target| target.last_used.elapsed() < Duration::from_secs(120)
            && target.control.as_ref().is_none_or(control_alive));
        if !progressed { std::thread::sleep(Duration::from_millis(5)); }
    }
}

fn control_alive(control: &TcpStream) -> bool {
    let mut byte = [0];
    match control.peek(&mut byte) {
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => true,
        _ => false,
    }
}

impl Destination {
    fn open(host: String, port: u16, route: DatagramRoute) -> Result<Self> {
        let (socket, control) = match &route {
            DatagramRoute::Direct => {
                let address = (host.as_str(), port).to_socket_addrs()?.next().context("UDP 目标解析失败")?;
                let socket = UdpSocket::bind(if address.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" })?;
                socket.connect(address)?;
                (socket, None)
            }
            DatagramRoute::Socks { address, username, password, fallback_direct, .. } => {
                let (control, relay) = match associate(*address, username, password) {
                    Ok(session) => session,
                    Err(_) if *fallback_direct => {
                        let mut direct = Self::open(host, port, DatagramRoute::Direct)?;
                        direct.requested_route = route;
                        return Ok(direct);
                    }
                    Err(error) => return Err(error),
                };
                let socket = UdpSocket::bind(if relay.is_ipv4() { "127.0.0.1:0" } else { "[::1]:0" })?;
                socket.connect(relay)?;
                (socket, Some(control))
            }
        };
        socket.set_nonblocking(true)?;
        Ok(Self { host, port, requested_route: route.clone(), route, socket, control, last_used: Instant::now() })
    }
}

pub(crate) fn associate(address: SocketAddr, user: &str, pass: &str) -> Result<(TcpStream, SocketAddr)> {
    if !address.ip().is_loopback() || user.is_empty() || user.len() > 255 || pass.is_empty() || pass.len() > 255 {
        bail!("UDP 上游必须是带认证的本机入口");
    }
    let mut control = TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
    control.set_read_timeout(Some(Duration::from_secs(2)))?;
    control.set_write_timeout(Some(Duration::from_secs(2)))?;
    control.write_all(&[5, 1, 2])?;
    let mut reply = [0; 2];
    control.read_exact(&mut reply)?;
    if reply != [5, 2] { bail!("UDP 上游不支持认证"); }
    let mut auth = vec![1, user.len() as u8];
    auth.extend_from_slice(user.as_bytes());
    auth.push(pass.len() as u8);
    auth.extend_from_slice(pass.as_bytes());
    control.write_all(&auth)?;
    control.read_exact(&mut reply)?;
    if reply != [1, 0] { bail!("UDP 上游认证失败"); }
    control.write_all(&[5, 3, 0, 1, 0, 0, 0, 0, 0, 0])?;
    let mut header = [0; 4];
    control.read_exact(&mut header)?;
    if header[0] != 5 || header[1] != 0 || header[2] != 0 { bail!("UDP 上游拒绝会话"); }
    let (host, port) = super::relay::socks_address(&mut control, header[3])?;
    let ip: IpAddr = host.parse().context("UDP 上游必须返回本机 IP 地址")?;
    let ip = if ip.is_unspecified() { address.ip() } else { ip };
    if !ip.is_loopback() { bail!("UDP 上游地址不是本机"); }
    control.set_nonblocking(true)?;
    Ok((control, SocketAddr::new(ip, port)))
}

pub(crate) fn encode_target(host: &str, port: u16, payload: &[u8]) -> Result<Vec<u8>> {
    if port == 0 { bail!("UDP 目标端口不能为0"); }
    let mut output = vec![0, 0, 0];
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => { output.push(1); output.extend_from_slice(&ip.octets()); }
        Ok(IpAddr::V6(ip)) => { output.push(4); output.extend_from_slice(&ip.octets()); }
        Err(_) => {
            if host.is_empty() || host.len() > 253 || host.chars().any(char::is_control) { bail!("无效的 UDP 目标域名"); }
            output.extend([3, host.len() as u8]); output.extend_from_slice(host.as_bytes());
        }
    }
    output.extend_from_slice(&port.to_be_bytes()); output.extend_from_slice(payload);
    if output.len() > 65507 { bail!("UDP 数据报过大"); }
    Ok(output)
}

pub(crate) fn decode_target(packet: &[u8]) -> Result<(String, u16, usize)> {
    if packet.len() < 4 || packet[..3] != [0, 0, 0] { bail!("无效或分片的 SOCKS UDP 数据报"); }
    let mut at = 4;
    let host = match packet[3] {
        1 => {
            let bytes: [u8; 4] = packet.get(at..at + 4).context("缺少 UDP 地址")?.try_into()?;
            at += 4; std::net::Ipv4Addr::from(bytes).to_string()
        }
        4 => {
            let bytes: [u8; 16] = packet.get(at..at + 16).context("缺少 UDP 地址")?.try_into()?;
            at += 16; std::net::Ipv6Addr::from(bytes).to_string()
        }
        3 => {
            let size = *packet.get(at).context("缺少 UDP 域名长度")? as usize; at += 1;
            if size == 0 || size > 253 { bail!("无效的 UDP 域名长度"); }
            let value = std::str::from_utf8(packet.get(at..at + size).context("缺少 UDP 域名")?)?.to_string();
            if value.chars().any(char::is_control) { bail!("无效的 UDP 域名"); }
            at += size; value
        }
        _ => bail!("不支持的 UDP 地址类型"),
    };
    let bytes: [u8; 2] = packet.get(at..at + 2).context("缺少 UDP 端口")?.try_into()?;
    let port = u16::from_be_bytes(bytes);
    if port == 0 { bail!("UDP 目标端口不能为0"); }
    Ok((host, port, at + 2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xray::relay::{Dialed, Dialer, MixedServer};
    use std::net::{Shutdown, TcpListener};

    fn server(route: DatagramRoute) -> (MixedServer, SocketAddr) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let dialer: Dialer = Arc::new(|host, port| Ok(Dialed { stream: TcpStream::connect((host, port))?, chain: "DIRECT".into(), rule: "capture fixture".into() }));
        let server = MixedServer::start_authenticated(listener, dialer, "fixture".into(), "password".into(), Arc::new(move |_, _| Ok(route.clone()))).unwrap();
        (server, address)
    }

    fn round_trip(proxy: SocketAddr) {
        let echo = UdpSocket::bind("127.0.0.1:0").unwrap();
        echo.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let destination = echo.local_addr().unwrap();
        let worker = std::thread::spawn(move || {
            let mut data = [0; 1024];
            let (size, from) = echo.recv_from(&mut data).unwrap();
            assert_eq!(&data[..size], b"dns-like-payload");
            std::thread::sleep(Duration::from_millis(250));
            echo.send_to(&data[..size], from).unwrap();
        });
        let (control, relay) = associate(proxy, "fixture", "password").unwrap();
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        client.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let frame = encode_target("127.0.0.1", destination.port(), b"dns-like-payload").unwrap();
        client.send_to(&frame, relay).unwrap();
        let mut response = [0; 2048];
        let (size, from) = client.recv_from(&mut response).unwrap();
        assert_eq!(from, relay);
        let (host, port, offset) = decode_target(&response[..size]).unwrap();
        assert_eq!(host, "127.0.0.1"); assert_eq!(port, destination.port());
        assert_eq!(&response[offset..size], b"dns-like-payload");
        control.shutdown(Shutdown::Both).unwrap(); drop(control);
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if UdpSocket::bind(relay).is_ok() { break; }
            assert!(Instant::now() < deadline, "UDP association survived control shutdown");
            std::thread::sleep(Duration::from_millis(10));
        }
        worker.join().unwrap();
    }

    #[test]
    fn authenticated_capture_forwards_tcp_and_direct_udp_then_releases_association() {
        let (server, address) = server(DatagramRoute::Direct);
        let mut unauthorized = TcpStream::connect(address).unwrap();
        unauthorized.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        unauthorized.write_all(&[5,1,0]).unwrap(); let mut reply=[0;2]; unauthorized.read_exact(&mut reply).unwrap(); assert_eq!(reply,[5,255]);
        assert!(associate(address,"wrong","password").is_err());
        let echo = TcpListener::bind("127.0.0.1:0").unwrap(); let port=echo.local_addr().unwrap().port();
        let worker=std::thread::spawn(move || { let(mut socket,_)=echo.accept().unwrap(); let mut data=[0;4];socket.read_exact(&mut data).unwrap();assert_eq!(&data,b"ping");socket.write_all(b"pong").unwrap(); });
        let mut client=crate::xray::relay::dial_socks(address,"fixture","password","127.0.0.1",port).unwrap(); client.write_all(b"ping").unwrap();let mut reply=[0;4];client.read_exact(&mut reply).unwrap();assert_eq!(&reply,b"pong");drop(client);worker.join().unwrap();
        round_trip(address);
        let ledger=server.snapshot();assert!(ledger.upload_total>=20 && ledger.download_total>=20);
        assert!(ledger.connections.iter().any(|row| row.network=="udp" && row.upload==16 && row.download==16));
    }

    #[test]
    fn capture_readiness_probes_do_not_create_connection_history() {
        let (server,address)=server(DatagramRoute::Direct);
        let(control,_)=associate(address,"fixture","password").unwrap();
        assert_eq!(server.snapshot().connection_count,0);
        assert!(server.snapshot().connections.is_empty());
        drop(control);
        std::thread::sleep(Duration::from_millis(30));
        assert!(server.snapshot().connections.is_empty());
    }

    #[test]
    fn authenticated_udp_can_chain_through_an_authenticated_upstream() {
        let (_upstream, address) = server(DatagramRoute::Direct);
        let (_capture, capture) = server(DatagramRoute::Socks { address, username: "fixture".into(), password: "password".into(), label: "fixture node".into(), fallback_direct: false });
        round_trip(capture);
    }

    #[test]
    fn udp_addresses_preserve_ipv6_and_reject_fragmentation_or_invalid_targets() {
        let frame=encode_target("2001:db8::1",443,b"payload").unwrap();
        let(host,port,offset)=decode_target(&frame).unwrap();assert_eq!(host,"2001:db8::1");assert_eq!(port,443);assert_eq!(&frame[offset..],b"payload");
        let mut fragmented=frame.clone();fragmented[2]=1;assert!(decode_target(&fragmented).is_err());
        assert!(encode_target("example.com",0,b"").is_err());
        assert!(encode_target(&"x".repeat(256),53,b"").is_err());
        assert!(decode_target(&[0,0,0,3,0,0,53]).is_err());
        assert!(decode_target(&[0,0,0,1,127]).is_err());
    }
}
