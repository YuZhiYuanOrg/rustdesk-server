//! Relay server module
//!
//! This module provides relay server functionality for RustDesk.
//! It handles TCP and WebSocket connections, bandwidth limiting,
//! and relay pairing between peers.

mod blacklist;
mod command;
mod config;
mod connection;
mod relay;
mod state;
mod stream;

use hbb_common::{
    log,
    tcp::listen_any,
    tokio,
    ResultType,
};
use sodiumoxide::crypto::sign;

pub use connection::io_loop;

/// Start the relay server
#[tokio::main(flavor = "multi_thread")]
pub async fn start(port: &str, key: &str) -> ResultType<()> {
    let key = get_server_sk(key);

    // Load blacklist and blocklist from files
    connection::load_lists().await;

    let port: u16 = port.parse()?;
    log::info!("Listening on tcp :{}", port);
    let port2 = port + 2;
    log::info!("Listening on websocket :{}", port2);

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
