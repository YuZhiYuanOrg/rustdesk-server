use crate::peer::*;
use hbb_common::{
    log,
    protobuf::Message as _,
    rendezvous_proto::{
        register_pk_response::Result::{INVALID_ID_FORMAT, TOO_FREQUENT, UUID_MISMATCH},
        *,
    },
    sodiumoxide::crypto::sign,
};

use super::constants::REG_TIMEOUT;
use super::types::RendezvousServer;

impl RendezvousServer {
    /// Handle public key registration
    pub async fn handle_register_pk(
        &mut self,
        rk: RegisterPk,
        addr: std::net::SocketAddr,
        ws: bool,
    ) -> Result<register_pk_response::Result, register_pk_response::Result> {
        if rk.uuid.is_empty() || rk.pk.is_empty() {
            return Err(INVALID_ID_FORMAT);
        }
        let id = rk.id;
        let ip = addr.ip().to_string();
        if id.len() < 6 {
            return Err(UUID_MISMATCH);
        } else if !self.check_ip_blocker(&ip, &id).await {
            return Err(TOO_FREQUENT);
        }
        let peer = self.pm.get_or(&id).await;
        let (changed, ip_changed) = {
            let peer = peer.read().await;
            if peer.uuid.is_empty() {
                (true, false)
            } else {
                if peer.uuid == rk.uuid {
                    if peer.info.ip != ip && peer.pk != rk.pk {
                        log::warn!(
                            "Peer {} ip/pk mismatch: {}/{:?} vs {}/{:?}",
                            id,
                            ip,
                            rk.pk,
                            peer.info.ip,
                            peer.pk,
                        );
                        drop(peer);
                        return Err(UUID_MISMATCH);
                    }
                } else {
                    log::warn!(
                        "Peer {} uuid mismatch: {:?} vs {:?}",
                        id,
                        rk.uuid,
                        peer.uuid
                    );
                    drop(peer);
                    return Err(UUID_MISMATCH);
                }
                let ip_changed = peer.info.ip != ip;
                (
                    peer.uuid != rk.uuid || peer.pk != rk.pk || ip_changed,
                    ip_changed,
                )
            }
        };
        let mut req_pk = peer.read().await.reg_pk;
        if req_pk.1.elapsed().as_secs() > 6 {
            req_pk.0 = 0;
        } else if req_pk.0 > 2 {
            return Err(TOO_FREQUENT);
        }
        req_pk.0 += 1;
        req_pk.1 = std::time::Instant::now();
        peer.write().await.reg_pk = req_pk;
        if ip_changed {
            let mut lock = IP_CHANGES.lock().await;
            if let Some((tm, ips)) = lock.get_mut(&id) {
                if tm.elapsed().as_secs() > IP_CHANGE_DUR {
                    *tm = std::time::Instant::now();
                    ips.clear();
                    ips.insert(ip.clone(), 1);
                } else if let Some(v) = ips.get_mut(&ip) {
                    *v += 1;
                } else {
                    ips.insert(ip.clone(), 1);
                }
            } else {
                lock.insert(
                    id.clone(),
                    (
                        std::time::Instant::now(),
                        std::collections::HashMap::from([(ip.clone(), 1)]),
                    ),
                );
            }
        }
        if changed || ws {
            self.pm.update_pk(id, peer, addr, rk.uuid, rk.pk, ip).await;
        }
        Ok(register_pk_response::Result::OK)
    }

    /// Update peer address and check if public key request is needed
    #[inline]
    pub async fn update_addr(
        &mut self,
        id: String,
        socket_addr: std::net::SocketAddr,
    ) -> bool {
        let (request_pk, ip_change) = if let Some(old) = self.pm.get_in_memory(&id).await {
            let mut old = old.write().await;
            let ip = socket_addr.ip();
            let ip_change = if old.socket_addr.port() != 0 {
                ip != old.socket_addr.ip()
            } else {
                ip.to_string() != old.info.ip
            } && !ip.is_loopback();
            let request_pk = old.pk.is_empty() || ip_change;
            if !request_pk {
                old.socket_addr = socket_addr;
                old.last_reg_time = std::time::Instant::now();
            }
            let ip_change = if ip_change && old.reg_pk.0 <= 2 {
                Some(if old.socket_addr.port() == 0 {
                    old.info.ip.clone()
                } else {
                    old.socket_addr.to_string()
                })
            } else {
                None
            };
            (request_pk, ip_change)
        } else {
            (true, None)
        };
        if let Some(old) = ip_change {
            log::info!("IP change of {} from {} to {}", id, old, socket_addr);
        }
        request_pk
    }

    /// Check if an IP should be blocked due to excessive requests
    pub async fn check_ip_blocker(&self, ip: &str, id: &str) -> bool {
        let mut lock = IP_BLOCKER.lock().await;
        let now = std::time::Instant::now();
        if let Some(old) = lock.get_mut(ip) {
            let counter = &mut old.0;
            if counter.1.elapsed().as_secs() > IP_BLOCK_DUR {
                counter.0 = 0;
            } else if counter.0 > 30 {
                return false;
            }
            counter.0 += 1;
            counter.1 = now;

            let counter = &mut old.1;
            let is_new = counter.0.get(id).is_none();
            if counter.1.elapsed().as_secs() > DAY_SECONDS {
                counter.0.clear();
            } else if counter.0.len() > 300 {
                return !is_new;
            }
            if is_new {
                counter.0.insert(id.to_owned());
            }
            counter.1 = now;
        } else {
            lock.insert(ip.to_owned(), ((0, now), (Default::default(), now)));
        }
        true
    }

    /// Get public key for a peer, signed with server key if available
    #[inline]
    pub async fn get_pk(&mut self, version: &str, id: String) -> hbb_common::bytes::Bytes {
        if version.is_empty() || self.inner.sk.is_none() {
            hbb_common::bytes::Bytes::new()
        } else {
            match self.pm.get(&id).await {
                Some(peer) => {
                    let pk = peer.read().await.pk.clone();
                    sign::sign(
                        &hbb_common::message_proto::IdPk {
                            id,
                            pk,
                            ..Default::default()
                        }
                        .write_to_bytes()
                        .unwrap_or_default(),
                        self.inner.sk.as_ref().unwrap(),
                    )
                    .into()
                }
                _ => hbb_common::bytes::Bytes::new(),
            }
        }
    }

    /// Get peer online states
    pub async fn peers_online_state(&mut self, peers: Vec<String>) -> hbb_common::bytes::BytesMut {
        let mut states = hbb_common::bytes::BytesMut::zeroed((peers.len() + 7) / 8);
        for (i, peer_id) in peers.iter().enumerate() {
            if let Some(peer) = self.pm.get_in_memory(peer_id).await {
                let elapsed = peer.read().await.last_reg_time.elapsed().as_millis() as i32;
                // bytes index from left to right
                let states_idx = i / 8;
                let bit_idx = 7 - i % 8;
                if elapsed < REG_TIMEOUT {
                    states[states_idx] |= 0x01 << bit_idx;
                }
            }
        }
        states
    }
}
