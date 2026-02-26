//! Relay server module
//!
//! This module provides relay server functionality for RustDesk.
//! It handles TCP and WebSocket connections, bandwidth limiting,
//! and relay pairing between peers.
//!
//! Enhanced with network traversal capabilities:
//! - QUIC protocol support for UDP-based connections
//! - Port hopping to avoid port blocking
//! - STUN/TURN support for NAT traversal

mod blacklist;
mod command;
mod config;
mod connection;
mod relay;
mod state;
mod stream;

// Network traversal modules
mod nat_traversal;
mod port_hopping;

use hbb_common::{
    log,
    tcp::listen_any,
    tokio,
    ResultType,
};
use sodiumoxide::crypto::sign;
use std::sync::atomic::Ordering;

pub use connection::io_loop;

/// Start the relay server
#[tokio::main(flavor = "multi_thread")]
pub async fn start(port: &str, key: &str) -> ResultType<()> {
    let key = get_server_sk(key);

    // Load configuration
    config::check_params();

    // Load blacklist and blocklist from files
    connection::load_lists().await;

    let port: u16 = port.parse()?;
    log::info!("Listening on tcp :{}", port);
    let port2 = port + 2;
    log::info!("Listening on websocket :{}", port2);

    // Initialize network traversal features
    let _nat_traversal = init_nat_traversal().await;
    let _port_hopping = init_port_hopping().await;

    // Log enabled features
    if config::ENABLE_QUIC.load(Ordering::SeqCst) {
        let port_quic = port + 4;
        log::info!("QUIC enabled on port :{}", port_quic);
    }

    if config::ENABLE_PORT_HOPPING.load(Ordering::SeqCst) {
        log::info!("Port hopping enabled");
    }

    if config::ENABLE_STUN.load(Ordering::SeqCst) {
        log::info!("STUN enabled for NAT traversal");
    }

    if config::ENABLE_TURN.load(Ordering::SeqCst) {
        log::info!("TURN relay enabled");
    }

    let main_task = async move {
        loop {
            log::info!("Start");
            io_loop(listen_any(port).await?, listen_any(port2).await?, &key).await;
        }
    };

    let listen_signal = crate::common::listen_signal();
    tokio::select!(
        res = main_task => res,
        res = listen_signal => res,
    )
}

/// Initialize NAT traversal capabilities
async fn init_nat_traversal() -> Option<nat_traversal::NatTraversal> {
    if !config::ENABLE_STUN.load(Ordering::SeqCst) && !config::ENABLE_TURN.load(Ordering::SeqCst) {
        return None;
    }

    let nat_config = nat_traversal::NatTraversalConfig::from_env();
    let nat = nat_traversal::NatTraversal::new(
        nat_config.stun_servers,
        nat_config.turn_servers,
    );

    // Try to discover public address if STUN is enabled
    if config::ENABLE_STUN.load(Ordering::SeqCst) {
        match nat.discover_public_address().await {
            Ok(addr) => {
                log::info!("Discovered public address via STUN: {}", addr);
            }
            Err(e) => {
                log::warn!("Failed to discover public address: {}", e);
            }
        }
    }

    Some(nat)
}

/// Initialize port hopping if enabled
async fn init_port_hopping() -> Option<std::sync::Arc<port_hopping::PortHoppingManager>> {
    if !config::ENABLE_PORT_HOPPING.load(Ordering::SeqCst) {
        return None;
    }

    let hopping_config = port_hopping::PortHoppingConfig::from_env();
    
    match port_hopping::PortHoppingManager::new(
        hopping_config.start_port,
        hopping_config.port_range,
        hopping_config.hop_interval_secs,
    ).await {
        Ok(manager) => {
            let manager = std::sync::Arc::new(manager);
            manager.clone().start_hopping().await;
            log::info!(
                "Port hopping initialized: ports {}-{}, interval {}s",
                hopping_config.start_port,
                hopping_config.start_port + hopping_config.port_range,
                hopping_config.hop_interval_secs
            );
            Some(manager)
        }
        Err(e) => {
            log::error!("Failed to initialize port hopping: {}", e);
            None
        }
    }
}

/// Process the server key
fn get_server_sk(key: &str) -> String {
    let mut key = key.to_owned();

    // Check if key is a crypto private key
    if let Ok(sk) = base64::decode(&key) {
        if sk.len() == sign::SECRETKEYBYTES {
            log::info!("The key is a crypto private key");
            key = base64::encode(&sk[(sign::SECRETKEYBYTES / 2)..]);
        }
    }

    // Generate a new key if requested
    if key == "-" || key == "_" {
        let (pk, _) = crate::common::gen_sk(300);
        key = pk;
    }

    if !key.is_empty() {
        log::info!("Key: {}", key);
    }

    key
}
