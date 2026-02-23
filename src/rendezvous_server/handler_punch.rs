use crate::jwt;
use hbb_common::{
    log,
    rendezvous_proto::*,
    tcp::FramedStream,
    tokio,
    try_into_v4,
    udp::FramedSocket,
    AddrMangle, ResultType,
};
use std::net::SocketAddr;
use std::sync::atomic::Ordering;

use super::constants::{ALWAYS_USE_RELAY, MUST_LOGIN, PUNCH_REQ_DEDUPE_SEC, REG_TIMEOUT};
use super::key_exchange::send_to_sink;
use super::network::{get_relay_server, is_lan};
use super::types::{Data, PunchReqEntry, PUNCH_REQS, RendezvousServer};

impl RendezvousServer {
    /// Handle punch hole request (shared logic for UDP and TCP)
    #[inline]
    pub async fn handle_punch_hole_request(
        &mut self,
        addr: SocketAddr,
        ph: PunchHoleRequest,
        key: &str,
        ws: bool,
    ) -> ResultType<(RendezvousMessage, Option<SocketAddr>)> {
        let mut ph = ph;
        if !key.is_empty() && ph.licence_key != key {
            let mut msg_out = RendezvousMessage::new();
            msg_out.set_punch_hole_response(PunchHoleResponse {
                failure: punch_hole_response::Failure::LICENSE_MISMATCH.into(),
                ..Default::default()
            });
            return Ok((msg_out, None));
        }
        // if secret is not empty check token by jwt
        if MUST_LOGIN.load(Ordering::SeqCst) {
            if ph.token.is_empty() {
                let mut msg_out = RendezvousMessage::new();
                msg_out.set_punch_hole_response(PunchHoleResponse {
                    other_failure: String::from("Connection failed, please login!"),
                    ..Default::default()
                });
                return Ok((msg_out, None));
            } else if !jwt::SECRET.is_empty() {
                let token = ph.token;
                let token = jwt::verify_token(token.as_str());
                if token.is_err() {
                    let mut msg_out = RendezvousMessage::new();
                    msg_out.set_punch_hole_response(PunchHoleResponse {
                        //提示重新登录
                        other_failure: String::from(
                            "Token error, please log out and log back in!",
                        ),
                        ..Default::default()
                    });
                    return Ok((msg_out, None));
                }
            }
        }
        let id = ph.id;
        // punch hole request from A, relay to B,
        // check if in same intranet first,
        // fetch local addrs if in same intranet.
        // because punch hole won't work if in the same intranet,
        // all routers will drop such self-connections.
        if let Some(peer) = self.pm.get(&id).await {
            let (elapsed, peer_addr) = {
                let r = peer.read().await;
                (r.last_reg_time.elapsed().as_millis() as i32, r.socket_addr)
            };
            if elapsed >= REG_TIMEOUT {
                let mut msg_out = RendezvousMessage::new();
                msg_out.set_punch_hole_response(PunchHoleResponse {
                    failure: punch_hole_response::Failure::OFFLINE.into(),
                    ..Default::default()
                });
                return Ok((msg_out, None));
            }

            // record punch hole request (from addr -> peer id/peer_addr)
            {
                let from_ip = try_into_v4(addr).ip().to_string();
                let to_ip = try_into_v4(peer_addr).ip().to_string();
                let to_id_clone = id.clone();
                let mut lock = PUNCH_REQS.lock().await;
                let mut dup = false;
                for e in lock.iter().rev().take(30) {
                    // only check recent tail subset for speed
                    if e.from_ip == from_ip && e.to_id == to_id_clone {
                        if e.tm.elapsed().as_secs() < PUNCH_REQ_DEDUPE_SEC {
                            dup = true;
                        }
                        break;
                    }
                }
                if !dup {
                    lock.push(PunchReqEntry {
                        tm: std::time::Instant::now(),
                        from_ip,
                        to_ip,
                        to_id: to_id_clone,
                    });
                }
            }

            let mut msg_out = RendezvousMessage::new();
            let peer_is_lan = is_lan(&self.inner, peer_addr);
            let is_lan = is_lan(&self.inner, addr);
            let mut relay_server = get_relay_server(&self.relay_servers, addr.ip(), peer_addr.ip());
            if ALWAYS_USE_RELAY.load(Ordering::SeqCst) || (peer_is_lan ^ is_lan) {
                if peer_is_lan {
                    // https://github.com/rustdesk/rustdesk-server/issues/24
                    relay_server = self.inner.local_ip.clone()
                }
                ph.nat_type = NatType::SYMMETRIC.into(); // will force relay
            }
            let same_intranet: bool = !ws
                && (peer_is_lan && is_lan
                    || {
                        match (peer_addr, addr) {
                            (SocketAddr::V4(a), SocketAddr::V4(b)) => a.ip() == b.ip(),
                            (SocketAddr::V6(a), SocketAddr::V6(b)) => a.ip() == b.ip(),
                            _ => false,
                        }
                    });
            let socket_addr = AddrMangle::encode(addr).into();
            if same_intranet {
                log::debug!(
                    "Fetch local addr {:?} {:?} request from {:?}",
                    id,
                    peer_addr,
                    addr
                );
                msg_out.set_fetch_local_addr(FetchLocalAddr {
                    socket_addr,
                    relay_server,
                    ..Default::default()
                });
            } else {
                log::debug!(
                    "Punch hole {:?} {:?} request from {:?}",
                    id,
                    peer_addr,
                    addr
                );
                msg_out.set_punch_hole(PunchHole {
                    socket_addr,
                    nat_type: ph.nat_type,
                    relay_server,
                    ..Default::default()
                });
            }
            Ok((msg_out, Some(peer_addr)))
        } else {
            let mut msg_out = RendezvousMessage::new();
            msg_out.set_punch_hole_response(PunchHoleResponse {
                failure: punch_hole_response::Failure::ID_NOT_EXIST.into(),
                ..Default::default()
            });
            Ok((msg_out, None))
        }
    }

    /// Handle punch hole sent message
    #[inline]
    pub async fn handle_hole_sent<'a>(
        &mut self,
        phs: PunchHoleSent,
        addr: SocketAddr,
        socket: Option<&'a mut FramedSocket>,
    ) -> ResultType<()> {
        // punch hole sent from B, tell A that B is ready to be connected
        let addr_a = AddrMangle::decode(&phs.socket_addr);
        log::debug!(
            "{} punch hole response to {:?} from {:?}",
            if socket.is_none() { "TCP" } else { "UDP" },
            &addr_a,
            &addr
        );
        let mut msg_out = RendezvousMessage::new();
        let mut p = PunchHoleResponse {
            socket_addr: AddrMangle::encode(addr).into(),
            pk: self.get_pk(&phs.version, phs.id).await,
            relay_server: phs.relay_server.clone(),
            ..Default::default()
        };
        if let Ok(t) = phs.nat_type.enum_value() {
            p.set_nat_type(t);
        }
        msg_out.set_punch_hole_response(p);
        if let Some(socket) = socket {
            socket.send(&msg_out, addr_a).await?;
        } else {
            self.send_to_tcp(msg_out, addr_a).await;
        }
        Ok(())
    }

    /// Handle local address message
    #[inline]
    pub async fn handle_local_addr<'a>(
        &mut self,
        la: LocalAddr,
        addr: SocketAddr,
        socket: Option<&'a mut FramedSocket>,
    ) -> ResultType<()> {
        // relay local addrs of B to A
        let addr_a = AddrMangle::decode(&la.socket_addr);
        log::debug!(
            "{} local addrs response to {:?} from {:?}",
            if socket.is_none() { "TCP" } else { "UDP" },
            &addr_a,
            &addr
        );
        let mut msg_out = RendezvousMessage::new();
        let mut p = PunchHoleResponse {
            socket_addr: la.local_addr.clone(),
            pk: self.get_pk(&la.version, la.id).await,
            relay_server: la.relay_server,
            ..Default::default()
        };
        p.set_is_local(true);
        msg_out.set_punch_hole_response(p);
        if let Some(socket) = socket {
            socket.send(&msg_out, addr_a).await?;
        } else {
            self.send_to_tcp(msg_out, addr_a).await;
        }
        Ok(())
    }

    /// Handle online request
    #[inline]
    pub async fn handle_online_request(
        &mut self,
        stream: &mut FramedStream,
        peers: Vec<String>,
    ) -> ResultType<()> {
        let states = self.peers_online_state(peers).await;

        let mut msg_out = RendezvousMessage::new();
        msg_out.set_online_response(OnlineResponse {
            states: states.into(),
            ..Default::default()
        });
        stream.send(&msg_out).await?;

        Ok(())
    }

    /// Send message to TCP sink
    #[inline]
    pub async fn send_to_tcp(&mut self, msg: RendezvousMessage, addr: SocketAddr) {
        let mut tcp = self.tcp_punch.lock().await.remove(&try_into_v4(addr));
        tokio::spawn(async move {
            send_to_sink(&mut tcp, msg).await;
        });
    }

    /// Send message to TCP sink synchronously
    #[inline]
    pub async fn send_to_tcp_sync(
        &mut self,
        msg: RendezvousMessage,
        addr: SocketAddr,
    ) -> ResultType<()> {
        let mut sink = self.tcp_punch.lock().await.remove(&try_into_v4(addr));
        send_to_sink(&mut sink, msg).await;
        Ok(())
    }

    /// Handle TCP punch hole request
    #[inline]
    pub async fn handle_tcp_punch_hole_request(
        &mut self,
        addr: SocketAddr,
        ph: PunchHoleRequest,
        key: &str,
        ws: bool,
    ) -> ResultType<()> {
        let (msg, to_addr) = self.handle_punch_hole_request(addr, ph, key, ws).await?;
        if let Some(addr) = to_addr {
            let mut sink = self.ws_map.lock().await.remove(&try_into_v4(addr));
            if let Some(s) = sink.as_mut() {
                s.send(&msg).await;
            } else {
                self.tx.send(Data::Msg(msg.into(), addr))?;
            }
        } else {
            self.send_to_tcp_sync(msg, addr).await?;
        }
        Ok(())
    }

    /// Handle UDP punch hole request
    #[inline]
    pub async fn handle_udp_punch_hole_request(
        &mut self,
        addr: SocketAddr,
        ph: PunchHoleRequest,
        key: &str,
    ) -> ResultType<()> {
        let (msg, to_addr) = self.handle_punch_hole_request(addr, ph, key, false).await?;
        self.tx.send(Data::Msg(
            msg.into(),
            match to_addr {
                Some(addr) => addr,
                None => addr,
            },
        ))?;
        Ok(())
    }
}
