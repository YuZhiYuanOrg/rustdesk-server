use hbb_common::{
    allow_err,
    bytes_codec::BytesCodec,
    futures_util::StreamExt,
    log,
    protobuf::Message as _,
    rendezvous_proto::*,
    sodiumoxide::crypto::secretbox,
    tcp::Encrypt,
    timeout,
    tokio::{
        self,
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
    },
    tokio_util::codec::Framed,
    try_into_v4,
    AddrMangle, ResultType,
};
use std::net::SocketAddr;

use super::key_exchange::{get_symetric_key_from_msg, key_exchange_phase1, send_to_sink};
use super::network::is_lan;
use super::types::{Data, RendezvousServer, Sink};

impl RendezvousServer {
    /// Handle incoming TCP messages
    #[inline]
    pub async fn handle_tcp(
        &mut self,
        bytes: &[u8],
        sink: &mut Option<Sink>,
        addr: SocketAddr,
        key: &str,
        ws: bool,
    ) -> bool {
        if let Ok(msg_in) = RendezvousMessage::parse_from_bytes(bytes) {
            match msg_in.union {
                Some(rendezvous_message::Union::RegisterPeer(rp)) => {
                    if !rp.id.is_empty() {
                        log::trace!("New peer registered: {:?} {:?}", &rp.id, &addr);
                        let request_pk = self.update_addr(rp.id, addr).await;
                        let mut msg_out = RendezvousMessage::new();
                        msg_out.set_register_peer_response(RegisterPeerResponse {
                            request_pk,
                            ..Default::default()
                        });
                        send_to_sink(sink, msg_out).await;
                        if self.inner.serial > rp.serial {
                            let mut msg_out = RendezvousMessage::new();
                            msg_out.set_configure_update(ConfigUpdate {
                                serial: self.inner.serial,
                                rendezvous_servers: (*self.rendezvous_servers).clone(),
                                ..Default::default()
                            });
                            send_to_sink(sink, msg_out).await;
                        }
                    }
                }
                Some(rendezvous_message::Union::PunchHoleRequest(ph)) => {
                    // there maybe several attempt, so sink can be none
                    if let Some(sink) = sink.take() {
                        self.tcp_punch.lock().await.insert(try_into_v4(addr), sink);
                    }
                    allow_err!(self.handle_tcp_punch_hole_request(addr, ph, key, ws).await);
                    return true;
                }
                Some(rendezvous_message::Union::RequestRelay(mut rf)) => {
                    // there maybe several attempt, so sink can be none
                    if let Some(sink) = sink.take() {
                        self.tcp_punch.lock().await.insert(try_into_v4(addr), sink);
                    }
                    if let Some(peer) = self.pm.get_in_memory(&rf.id).await {
                        let mut msg_out = RendezvousMessage::new();
                        rf.socket_addr = AddrMangle::encode(addr).into();
                        msg_out.set_request_relay(rf);
                        let peer_addr = peer.read().await.socket_addr;
                        self.tx.send(Data::Msg(msg_out.into(), peer_addr)).ok();
                    }
                    return true;
                }
                Some(rendezvous_message::Union::RelayResponse(mut rr)) => {
                    let addr_b = AddrMangle::decode(&rr.socket_addr);
                    rr.socket_addr = Default::default();
                    let id = rr.id();
                    if !id.is_empty() {
                        let pk = self.get_pk(&rr.version, id.to_owned()).await;
                        rr.set_pk(pk);
                    }
                    let mut msg_out = RendezvousMessage::new();
                    if !rr.relay_server.is_empty() {
                        if is_lan(&self.inner, addr_b) {
                            // https://github.com/rustdesk/rustdesk-server/issues/24
                            rr.relay_server = self.inner.local_ip.clone();
                        } else if rr.relay_server == self.inner.local_ip {
                            rr.relay_server = super::network::get_relay_server(
                                &self.relay_servers,
                                addr.ip(),
                                addr_b.ip(),
                            );
                        }
                    }
                    msg_out.set_relay_response(rr);
                    allow_err!(self.send_to_tcp_sync(msg_out, addr_b).await);
                }
                Some(rendezvous_message::Union::PunchHoleSent(phs)) => {
                    allow_err!(self.handle_hole_sent(phs, addr, None).await);
                }
                Some(rendezvous_message::Union::LocalAddr(la)) => {
                    allow_err!(self.handle_local_addr(la, addr, None).await);
                }
                Some(rendezvous_message::Union::TestNatRequest(tar)) => {
                    let mut msg_out = RendezvousMessage::new();
                    let mut res = TestNatResponse {
                        port: addr.port() as _,
                        ..Default::default()
                    };
                    if self.inner.serial > tar.serial {
                        let mut cu = ConfigUpdate::new();
                        cu.serial = self.inner.serial;
                        cu.rendezvous_servers = (*self.rendezvous_servers).clone();
                        res.cu = hbb_common::protobuf::MessageField::from_option(Some(cu));
                    }
                    msg_out.set_test_nat_response(res);
                    send_to_sink(sink, msg_out).await;
                }
                Some(rendezvous_message::Union::RegisterPk(rk)) => {
                    let response = self.handle_register_pk(rk, addr, ws).await;
                    match response {
                        Err(err) => {
                            let mut msg_out = RendezvousMessage::new();
                            msg_out.set_register_pk_response(RegisterPkResponse {
                                result: err.into(),
                                ..Default::default()
                            });
                            send_to_sink(sink, msg_out).await;
                            return false;
                        }
                        Ok(res) => {
                            let mut msg_out = RendezvousMessage::new();
                            msg_out.set_register_pk_response(RegisterPkResponse {
                                result: res.into(),
                                ..Default::default()
                            });
                            send_to_sink(sink, msg_out).await;
                            if ws {
                                if let Some(sink) = sink.take() {
                                    self.ws_map.lock().await.insert(try_into_v4(addr), sink);
                                }
                            }
                            return true;
                        }
                    }
                }
                Some(rendezvous_message::Union::KeyExchange(ex)) => {
                    log::trace!("KeyExchange {:?} <- bytes: {:?}", addr, hbb_common::sodiumoxide::hex::encode(&bytes));
                    if ex.keys.len() != 2 {
                        log::error!("Handshake failed: invalid phase 2 key exchange message");
                        return false;
                    }
                    log::trace!("KeyExchange their_pk: {:?}", hbb_common::sodiumoxide::hex::encode(&ex.keys[0]));
                    log::trace!("KeyExchange box: {:?}", hbb_common::sodiumoxide::hex::encode(&ex.keys[1]));
                    let their_pk: [u8; 32] = ex.keys[0].to_vec().try_into().unwrap();
                    let cryptobox: [u8; 48] = ex.keys[1].to_vec().try_into().unwrap();
                    let symetric_key = get_symetric_key_from_msg(
                        self.inner.secure_tcp_sk_b.0,
                        their_pk,
                        &cryptobox,
                    );
                    log::debug!("KeyExchange symetric key: {:?}", hbb_common::sodiumoxide::hex::encode(&symetric_key));
                    let key = secretbox::Key::from_slice(&symetric_key);
                    match key {
                        Some(key) => {
                            if let Some(sink) = sink.as_mut() {
                                match sink {
                                    Sink::Wss(s) => s.encrypt = Some(Encrypt::new(key)),
                                    Sink::Tss(s) => s.encrypt = Some(Encrypt::new(key)),
                                }
                            }
                            log::debug!("KeyExchange symetric key set");
                            return true;
                        }
                        None => {
                            log::error!("KeyExchange symetric key NOT set");
                            return false;
                        }
                    }
                }
                Some(rendezvous_message::Union::OnlineRequest(or)) => {
                    let states = self.peers_online_state(or.peers).await;
                    let mut msg_out = RendezvousMessage::new();
                    msg_out.set_online_response(OnlineResponse {
                        states: states.into(),
                        ..Default::default()
                    });
                    send_to_sink(sink, msg_out).await;
                }
                _ => {}
            }
        }
        false
    }

    /// Handle listener2 (NAT test and online request)
    pub async fn handle_listener2(&self, stream: TcpStream, addr: SocketAddr) {
        let mut rs = self.clone();
        let ip = try_into_v4(addr).ip();
        if ip.is_loopback() {
            tokio::spawn(async move {
                let mut stream = stream;
                let mut buffer = [0; 1024];
                if let Ok(Ok(n)) = timeout(1000, stream.read(&mut buffer[..])).await {
                    if let Ok(data) = std::str::from_utf8(&buffer[..n]) {
                        let res = rs.check_cmd(data).await;
                        stream.write(res.as_bytes()).await.ok();
                    }
                }
            });
            return;
        }
        let stream = hbb_common::tcp::FramedStream::from(stream, addr);
        tokio::spawn(async move {
            let mut stream = stream;
            if let Some(Ok(bytes)) = stream.next_timeout(30_000).await {
                if let Ok(msg_in) = RendezvousMessage::parse_from_bytes(&bytes) {
                    match msg_in.union {
                        Some(rendezvous_message::Union::TestNatRequest(_)) => {
                            let mut msg_out = RendezvousMessage::new();
                            msg_out.set_test_nat_response(TestNatResponse {
                                port: addr.port() as _,
                                ..Default::default()
                            });
                            stream.send(&msg_out).await.ok();
                        }
                        Some(rendezvous_message::Union::OnlineRequest(or)) => {
                            allow_err!(rs.handle_online_request(&mut stream, or.peers).await);
                        }
                        _ => {}
                    }
                }
            }
        });
    }

    /// Handle main TCP listener
    pub async fn handle_listener(&self, stream: TcpStream, addr: SocketAddr, key: &str, ws: bool) {
        log::debug!("Tcp connection from {:?}, ws: {}", addr, ws);
        let mut rs = self.clone();
        let key = key.to_owned();
        tokio::spawn(async move {
            allow_err!(rs.handle_listener_inner(stream, addr, &key, ws).await);
        });
    }

    /// Inner handler for TCP connections
    #[inline]
    pub async fn handle_listener_inner(
        &mut self,
        stream: TcpStream,
        mut addr: SocketAddr,
        key: &str,
        ws: bool,
    ) -> ResultType<()> {
        let mut sink;
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
            let (a, mut b) = ws_stream.split();
            sink = Some(Sink::Wss(super::types::SafeWsSink {
                sink: a,
                encrypt: None,
            }));
            while let Ok(Some(Ok(msg))) = timeout(30_000, b.next()).await {
                if let tungstenite::Message::Binary(bytes) = msg {
                    if !self.handle_tcp(&bytes, &mut sink, addr, key, ws).await {
                        break;
                    }
                }
            }
        } else {
            let (a, mut b) = Framed::new(stream, BytesCodec::new()).split();
            sink = Some(Sink::Tss(super::types::SafeTcpStreamSink {
                sink: a,
                encrypt: None,
            }));
            // Avoid key exchange if answering on nat helper port
            if !key.is_empty() {
                key_exchange_phase1(&self.inner, addr, &mut sink).await;
            }
            while let Ok(Some(Ok(mut bytes))) = timeout(30_000, b.next()).await {
                if let Some(Sink::Tss(s)) = sink.as_mut() {
                    if let Some(key) = s.encrypt.as_mut() {
                        if let Err(err) = key.dec(&mut bytes) {
                            log::error!("dec tcp data from {:?} err: {:?}", addr, err);
                            break;
                        }
                    }
                }
                if !self.handle_tcp(&bytes, &mut sink, addr, key, ws).await {
                    break;
                }
            }
        }
        if sink.is_none() {
            self.tcp_punch.lock().await.remove(&try_into_v4(addr));
        }
        log::debug!("Tcp connection from {:?} closed", addr);
        Ok(())
    }
}
