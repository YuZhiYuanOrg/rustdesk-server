use crate::common::test_if_valid_server;
use hbb_common::{
    allow_err,
    bytes::BytesMut,
    log,
    protobuf::Message as _,
    rendezvous_proto::*,
    tokio,
    try_into_v4,
    udp::FramedSocket,
    ResultType,
};
use std::net::SocketAddr;

use super::types::RendezvousServer;

impl RendezvousServer {
    /// Handle incoming UDP messages
    #[inline]
    pub async fn handle_udp(
        &mut self,
        bytes: &BytesMut,
        addr: SocketAddr,
        socket: &mut FramedSocket,
        key: &str,
    ) -> ResultType<()> {
        if let Ok(msg_in) = RendezvousMessage::parse_from_bytes(bytes) {
            match msg_in.union {
                Some(rendezvous_message::Union::RegisterPeer(rp)) => {
                    // B registered
                    if !rp.id.is_empty() {
                        log::trace!("New peer registered: {:?} {:?}", &rp.id, &addr);
                        let request_pk = self.update_addr(rp.id, addr).await;
                        let mut msg_out = RendezvousMessage::new();
                        msg_out.set_register_peer_response(RegisterPeerResponse {
                            request_pk,
                            ..Default::default()
                        });
                        socket.send(&msg_out, addr).await?;
                        if self.inner.serial > rp.serial {
                            let mut msg_out = RendezvousMessage::new();
                            msg_out.set_configure_update(ConfigUpdate {
                                serial: self.inner.serial,
                                rendezvous_servers: (*self.rendezvous_servers).clone(),
                                ..Default::default()
                            });
                            socket.send(&msg_out, addr).await?;
                        }
                    }
                }
                Some(rendezvous_message::Union::RegisterPk(rk)) => {
                    let response = self.handle_register_pk(rk, addr, false).await;
                    match response {
                        Err(err) => {
                            let mut msg_out = RendezvousMessage::new();
                            msg_out.set_register_pk_response(RegisterPkResponse {
                                result: err.into(),
                                ..Default::default()
                            });
                            socket.send(&msg_out, addr).await?;
                        }
                        Ok(res) => {
                            let mut msg_out = RendezvousMessage::new();
                            msg_out.set_register_pk_response(RegisterPkResponse {
                                result: res.into(),
                                ..Default::default()
                            });
                            socket.send(&msg_out, addr).await?;
                        }
                    }
                }
                Some(rendezvous_message::Union::PunchHoleRequest(ph)) => {
                    if self.pm.is_in_memory(&ph.id).await {
                        self.handle_udp_punch_hole_request(addr, ph, key).await?;
                    } else {
                        // not in memory, fetch from db with spawn in case blocking me
                        let mut me = self.clone();
                        let key = key.to_owned();
                        tokio::spawn(async move {
                            allow_err!(me.handle_udp_punch_hole_request(addr, ph, &key).await);
                        });
                    }
                }
                Some(rendezvous_message::Union::PunchHoleSent(phs)) => {
                    self.handle_hole_sent(phs, addr, Some(socket)).await?;
                }
                Some(rendezvous_message::Union::LocalAddr(la)) => {
                    self.handle_local_addr(la, addr, Some(socket)).await?;
                }
                Some(rendezvous_message::Union::ConfigureUpdate(mut cu)) => {
                    if try_into_v4(addr).ip().is_loopback() && cu.serial > self.inner.serial {
                        let mut inner: super::types::Inner = (*self.inner).clone();
                        inner.serial = cu.serial;
                        self.inner = std::sync::Arc::new(inner);
                        self.rendezvous_servers = std::sync::Arc::new(
                            cu.rendezvous_servers
                                .drain(..)
                                .filter(|x| {
                                    !x.is_empty()
                                        && test_if_valid_server(x, "rendezvous-server").is_ok()
                                })
                                .collect(),
                        );
                        log::info!(
                            "configure updated: serial={} rendezvous-servers={:?}",
                            self.inner.serial,
                            self.rendezvous_servers
                        );
                    }
                }
                Some(rendezvous_message::Union::SoftwareUpdate(su)) => {
                    if !self.inner.version.is_empty() && su.url != self.inner.version {
                        let mut msg_out = RendezvousMessage::new();
                        msg_out.set_software_update(SoftwareUpdate {
                            url: self.inner.software_url.clone(),
                            ..Default::default()
                        });
                        socket.send(&msg_out, addr).await?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// Send register pk response via UDP
#[inline]
pub async fn send_rk_res(
    socket: &mut FramedSocket,
    addr: SocketAddr,
    res: register_pk_response::Result,
) -> ResultType<()> {
    let mut msg_out = RendezvousMessage::new();
    msg_out.set_register_pk_response(RegisterPkResponse {
        result: res.into(),
        ..Default::default()
    });
    socket.send(&msg_out, addr).await
}
