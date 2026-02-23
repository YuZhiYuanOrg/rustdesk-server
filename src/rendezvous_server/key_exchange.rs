use hbb_common::{
    bytes::Bytes,
    log,
    protobuf::Message as _,
    rendezvous_proto::*,
    sodiumoxide::crypto::{box_, sign},
    sodiumoxide::hex,
};

use super::types::{Inner, Sink};

/// Get server secret key from the provided key string
pub fn get_server_sk(key: &str) -> (String, Option<sign::SecretKey>) {
    let mut out_sk = None;
    let mut key = key.to_owned();
    if let Ok(sk) = base64::decode(&key) {
        if sk.len() == sign::SECRETKEYBYTES {
            log::info!("The key is a crypto private key");
            key = base64::encode(&sk[(sign::SECRETKEYBYTES / 2)..]);
            let mut tmp = [0u8; sign::SECRETKEYBYTES];
            tmp[..].copy_from_slice(&sk);
            out_sk = Some(sign::SecretKey(tmp));
        }
    }

    if key.is_empty() || key == "-" || key == "_" {
        let (pk, sk) = crate::common::gen_sk(0);
        out_sk = sk;
        if !key.is_empty() {
            key = pk;
        }
    }

    if !key.is_empty() {
        log::info!("Key: {}", key);
    }
    (key, out_sk)
}

/// Perform key exchange phase 1 - send our public key signed with server key
pub async fn key_exchange_phase1(
    inner: &Inner,
    addr: std::net::SocketAddr,
    sink: &mut Option<Sink>,
) {
    let mut msg_out = RendezvousMessage::new();
    log::debug!(
        "KeyExchange phase 1: send our pk for this tcp connection in a message signed with our server key"
    );
    let sk = &inner.sk;
    match sk {
        Some(sk) => {
            let our_pk_b = inner.secure_tcp_pk_b.clone();
            let sm = sign::sign(&our_pk_b.0, &sk);

            let bytes_sm = Bytes::from(sm);
            msg_out.set_key_exchange(KeyExchange {
                keys: vec![bytes_sm],
                ..Default::default()
            });
            log::trace!(
                "KeyExchange {:?} -> bytes: {:?}",
                addr,
                hex::encode(Bytes::from(msg_out.write_to_bytes().unwrap()))
            );
            send_to_sink(sink, msg_out).await;
        }
        None => {}
    }
}

/// Extract symmetric key from the key exchange message
pub fn get_symetric_key_from_msg(
    our_sk_b: [u8; 32],
    their_pk_b: [u8; 32],
    sealed_value: &[u8; 48],
) -> [u8; 32] {
    let their_pk_b = box_::PublicKey(their_pk_b);
    let nonce = box_::Nonce([0u8; box_::NONCEBYTES]);
    let sk = box_::SecretKey(our_sk_b);
    let key = box_::open(sealed_value, &nonce, &their_pk_b, &sk);
    match key {
        Ok(key) => {
            let mut key_array = [0u8; 32];
            key_array.copy_from_slice(&key);
            key_array
        }
        Err(e) => panic!("Error while opening the seal key{:?}", e),
    }
}

/// Send a message to the sink
pub async fn send_to_sink(sink: &mut Option<Sink>, msg: RendezvousMessage) {
    if let Some(sink) = sink.as_mut() {
        sink.send(&msg).await;
    }
}
