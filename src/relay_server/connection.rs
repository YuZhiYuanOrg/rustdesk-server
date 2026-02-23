use std::sync::atomic::Ordering;

use async_speed_limit::Limiter;
use hbb_common::{
    allow_err,
    log,
    protobuf::Message as _,
    rendezvous_proto::*,
    sleep,
    tcp::FramedStream,
    timeout,
    tokio::{
        self,
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
    },
    ResultType,
};

use super::blacklist;
use super::command;
use super::config;
use super::relay;
use super::state;
use super::stream::StreamTrait;

const BLACKLIST_FILE: &str = "blacklist.txt";
const BLOCKLIST_FILE: &str = "blocklist.txt";

/// Main I/O loop for accepting connections
pub async fn io_loop(listener: TcpListener, listener2: TcpListener, key: &str) {
    config::check_params();
    let limiter = Limiter::new(config::TOTAL_BANDWIDTH.load(Ordering::SeqCst) as f64);

    loop {
        tokio::select! {
            res = listener.accept() => {
                match res {
                    Ok((stream, addr)) => {
                        stream.set_nodelay(true).ok();
                        handle_connection(stream, addr, &limiter, key, false).await;
                    }
                    Err(err) => {
                        log::error!("listener.accept failed: {}", err);
                        break;
                    }
                }
            }
            res = listener2.accept() => {
                match res {
                    Ok((stream, addr)) => {
                        stream.set_nodelay(true).ok();
                        handle_connection(stream, addr, &limiter, key, true).await;
                    }
                    Err(err) => {
                        log::error!("listener2.accept failed: {}", err);
                        break;
                    }
                }
            }
        }
    }
}

/// Handle an incoming connection
pub async fn handle_connection(
    stream: TcpStream,
    addr: std::net::SocketAddr,
    limiter: &Limiter,
    key: &str,
    ws: bool,
) {
    let ip = hbb_common::try_into_v4(addr).ip();

    // Handle local command interface
    if !ws && ip.is_loopback() {
        let limiter = limiter.clone();
        tokio::spawn(async move {
            let mut stream = stream;
            let mut buffer = [0; 1024];
            if let Ok(Ok(n)) = timeout(1000, stream.read(&mut buffer[..])).await {
                if let Ok(data) = std::str::from_utf8(&buffer[..n]) {
                    let res = command::check_cmd(data, limiter).await;
                    stream.write(res.as_bytes()).await.ok();
                }
            }
        });
        return;
    }

    let ip = ip.to_string();
    if blacklist::is_in_list(&state::BLOCKLIST, &ip).await {
        log::info!("{} blocked", ip);
        return;
    }

    let key = key.to_owned();
    let limiter = limiter.clone();
    tokio::spawn(async move {
        allow_err!(make_pair(stream, addr, &key, limiter, ws).await);
    });
}

/// Create a relay pair from the connection
async fn make_pair(
    stream: TcpStream,
    mut addr: std::net::SocketAddr,
    key: &str,
    limiter: Limiter,
    ws: bool,
) -> ResultType<()> {
    if ws {
        use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

        let callback = |req: &Request, response: Response| {
            let headers = req.headers();
            let real_ip = headers
                .get("X-Real-IP")
                .or_else(|| headers.get("X-Forwarded-For"))
                .and_then(|header_value| header_value.to_str().ok());

            if let Some(ip) = real_ip {
                if ip.contains('.') {
                    addr = format!("{ip}:0").parse().unwrap_or(addr);
                } else {
                    addr = format!("[{ip}]:0").parse().unwrap_or(addr);
                }
            }
            Ok(response)
        };

        let ws_stream = tokio_tungstenite::accept_hdr_async(stream, callback).await?;
        make_pair_inner(ws_stream, addr, key, limiter).await;
    } else {
        make_pair_inner(FramedStream::from(stream, addr), addr, key, limiter).await;
    }
    Ok(())
}

/// Inner function to handle relay pairing
async fn make_pair_inner(
    stream: impl StreamTrait,
    addr: std::net::SocketAddr,
    key: &str,
    limiter: Limiter,
) {
    let mut stream = stream;

    if let Ok(Some(Ok(bytes))) = timeout(30_000, stream.recv()).await {
        if let Ok(msg_in) = RendezvousMessage::parse_from_bytes(&bytes) {
            if let Some(rendezvous_message::Union::RequestRelay(rf)) = msg_in.union {
                if !key.is_empty() && rf.licence_key != key {
                    log::warn!("Relay authentication failed from {} - invalid key", addr);
                    return;
                }

                if !rf.uuid.is_empty() {
                    let mut peer = state::PEERS.lock().await.remove(&rf.uuid);

                    if let Some(peer) = peer.as_mut() {
                        log::info!("Relayrequest {} from {} got paired", rf.uuid, addr);
                        let id = format!("{}:{}", addr.ip(), addr.port());
                        state::USAGE.write().await.insert(id.clone(), Default::default());

                        if !stream.is_ws() && !peer.is_ws() {
                            peer.set_raw();
                            stream.set_raw();
                            log::info!("Both are raw");
                        }

                        if let Err(err) = relay::relay(addr, &mut stream, peer, limiter, id.clone()).await
                        {
                            log::info!("Relay of {} closed: {}", addr, err);
                        } else {
                            log::info!("Relay of {} closed", addr);
                        }
                        state::USAGE.write().await.remove(&id);
                    } else {
                        log::info!("New relay request {} from {}", rf.uuid, addr);
                        state::PEERS.lock().await.insert(rf.uuid.clone(), Box::new(stream));
                        sleep(30.).await;
                        state::PEERS.lock().await.remove(&rf.uuid);
                    }
                }
            }
        }
    }
}

/// Load blacklist and blocklist from files
pub async fn load_lists() {
    let blacklist_set = blacklist::load_list_from_file(BLACKLIST_FILE);
    for ip in &blacklist_set {
        state::BLACKLIST.write().await.insert(ip.clone());
    }
    blacklist::log_list_count(&state::BLACKLIST, "blacklist", BLACKLIST_FILE).await;

    let blocklist_set = blacklist::load_list_from_file(BLOCKLIST_FILE);
    for ip in &blocklist_set {
        state::BLOCKLIST.write().await.insert(ip.clone());
    }
    blacklist::log_list_count(&state::BLOCKLIST, "blocklist", BLOCKLIST_FILE).await;
}
