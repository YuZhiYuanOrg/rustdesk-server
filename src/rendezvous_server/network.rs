use crate::common::*;
use hbb_common::{
    bail,
    config,
    log,
    tcp::listen_any,
    tokio::{self, net::TcpListener},
    udp::FramedSocket,
    ResultType,
};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use super::constants::ROTATION_RELAY_SERVER;
use super::types::{Inner, RelayServers};

/// Create a UDP listener on the specified port
pub async fn create_udp_listener(port: i32, rmem: usize) -> ResultType<FramedSocket> {
    let addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port as _);
    if let Ok(s) = FramedSocket::new_reuse(&addr, true, rmem).await {
        log::debug!("listen on udp {:?}", s.local_addr());
        return Ok(s);
    }
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port as _);
    let s = FramedSocket::new_reuse(&addr, true, rmem).await?;
    log::debug!("listen on udp {:?}", s.local_addr());
    Ok(s)
}

/// Create a TCP listener on the specified port
pub async fn create_tcp_listener(port: i32) -> ResultType<TcpListener> {
    let s = listen_any(port as _).await?;
    log::debug!("listen on tcp {:?}", s.local_addr());
    Ok(s)
}

/// Check if an address is in the LAN based on the configured mask
pub fn is_lan(inner: &Inner, addr: SocketAddr) -> bool {
    if let Some(network) = &inner.mask {
        match addr {
            SocketAddr::V4(v4_socket_addr) => {
                return network.contains(*v4_socket_addr.ip());
            }
            SocketAddr::V6(v6_socket_addr) => {
                if let Some(v4_addr) = v6_socket_addr.ip().to_ipv4() {
                    return network.contains(v4_addr);
                }
            }
        }
    }
    false
}

/// Get a relay server using round-robin selection
pub fn get_relay_server(relay_servers: &RelayServers, _pa: IpAddr, _pb: IpAddr) -> String {
    if relay_servers.is_empty() {
        return "".to_owned();
    } else if relay_servers.len() == 1 {
        return relay_servers[0].clone();
    }
    let i = ROTATION_RELAY_SERVER.fetch_add(1, std::sync::atomic::Ordering::SeqCst) % relay_servers.len();
    relay_servers[i].clone()
}

/// Parse relay servers from a comma-separated string
pub fn parse_relay_servers(relay_servers: &str) -> RelayServers {
    get_servers(relay_servers, "relay-servers")
}

/// Check relay servers availability
pub async fn check_relay_servers(rs0: std::sync::Arc<RelayServers>, tx: super::types::Sender) {
    use hbb_common::{
        futures::future::join_all,
        tcp::FramedStream,
        tokio::sync::Mutex,
    };
    
    let mut futs = Vec::new();
    let rs = std::sync::Arc::new(Mutex::new(Vec::new()));
    for x in rs0.iter() {
        let mut host = x.to_owned();
        if !host.contains(':') {
            host = format!("{}:{}", host, config::RELAY_PORT);
        }
        let rs = rs.clone();
        let x = x.clone();
        futs.push(tokio::spawn(async move {
            if FramedStream::new(&host, None, super::constants::CHECK_RELAY_TIMEOUT)
                .await
                .is_ok()
            {
                rs.lock().await.push(x);
            }
        }));
    }
    join_all(futs).await;
    log::debug!("check_relay_servers");
    let rs = std::mem::take(&mut *rs.lock().await);
    if !rs.is_empty() {
        tx.send(super::types::Data::RelayServers(rs)).ok();
    }
}

/// Test hbbs server functionality
pub async fn test_hbbs(addr: SocketAddr) -> ResultType<()> {
    use hbb_common::{
        protobuf::Message as _,
        rendezvous_proto::*,
        tokio::time::{interval, Duration},
    };
    
    let mut addr = addr;
    if addr.ip().is_unspecified() {
        addr.set_ip(if addr.is_ipv4() {
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        } else {
            IpAddr::V6(Ipv6Addr::LOCALHOST)
        });
    }

    let mut socket = FramedSocket::new(config::Config::get_any_listen_addr(addr.is_ipv4())).await?;
    let mut msg_out = RendezvousMessage::new();
    msg_out.set_register_peer(RegisterPeer {
        id: "(:test_hbbs:)".to_owned(),
        ..Default::default()
    });
    let mut last_time_recv = std::time::Instant::now();

    let mut timer = interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = timer.tick() => {
                if last_time_recv.elapsed().as_secs() > 12 {
                    bail!("Timeout of test_hbbs");
                }
                socket.send(&msg_out, addr).await?;
            }
            Some(Ok((bytes, _))) = socket.next() => {
                if let Ok(msg_in) = RendezvousMessage::parse_from_bytes(&bytes) {
                    log::trace!("Recv {:?} of test_hbbs", msg_in);
                    last_time_recv = std::time::Instant::now();
                }
            }
        }
    }
}
