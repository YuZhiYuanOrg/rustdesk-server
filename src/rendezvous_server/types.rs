use crate::peer::*;
use hbb_common::{
    allow_err,
    bytes::Bytes,
    bytes_codec::BytesCodec,
    futures_util::{
        sink::SinkExt,
        stream::SplitSink,
    },
    log,
    protobuf::Message as _,
    rendezvous_proto::*,
    sodiumoxide::crypto::{box_::PublicKey, box_::SecretKey, sign},
    tcp::Encrypt,
    tokio::{
        net::TcpStream,
        sync::{mpsc, Mutex},
    },
    tokio_util::codec::Framed,
};
use ipnetwork::Ipv4Network;
use once_cell::sync::Lazy;
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::Arc,
    time::Instant,
};
use hbb_common::tokio::sync::Mutex as TokioMutex;

// ============================================================================
// Data enum for channel communication
// ============================================================================

#[derive(Clone, Debug)]
pub enum Data {
    Msg(Box<RendezvousMessage>, SocketAddr),
    RelayServers0(String),
    RelayServers(RelayServers),
}

// ============================================================================
// Sink types for TCP and WebSocket connections
// ============================================================================

type TcpStreamSink = SplitSink<Framed<TcpStream, BytesCodec>, Bytes>;
type WsSink = SplitSink<tokio_tungstenite::WebSocketStream<TcpStream>, tungstenite::Message>;

pub struct SafeWsSink {
    pub sink: WsSink,
    pub encrypt: Option<Encrypt>,
}

pub struct SafeTcpStreamSink {
    pub sink: TcpStreamSink,
    pub encrypt: Option<Encrypt>,
}

pub enum Sink {
    Wss(SafeWsSink),
    Tss(SafeTcpStreamSink),
}

impl Sink {
    pub async fn send(&mut self, msg: &RendezvousMessage) {
        if let Ok(mut bytes) = msg.write_to_bytes() {
            match self {
                Sink::Wss(s) => {
                    if let Some(key) = s.encrypt.as_mut() {
                        bytes = key.enc(&bytes);
                    }
                    allow_err!(s.sink.send(tungstenite::Message::Binary(bytes)).await)
                }
                Sink::Tss(s) => {
                    if let Some(key) = s.encrypt.as_mut() {
                        bytes = key.enc(&bytes);
                    }
                    allow_err!(s.sink.send(Bytes::from(bytes)).await)
                }
            }
        }
    }
}

// ============================================================================
// Channel types
// ============================================================================

pub type Sender = mpsc::UnboundedSender<Data>;
pub type Receiver = mpsc::UnboundedReceiver<Data>;

// ============================================================================
// Relay servers type
// ============================================================================

pub type RelayServers = Vec<String>;

// ============================================================================
// Punch request deduplication
// ============================================================================

#[derive(Clone)]
pub struct PunchReqEntry {
    pub tm: Instant,
    pub from_ip: String,
    pub to_ip: String,
    pub to_id: String,
}

pub static PUNCH_REQS: Lazy<TokioMutex<Vec<PunchReqEntry>>> =
    Lazy::new(|| TokioMutex::new(Vec::new()));

// ============================================================================
// Inner configuration
// ============================================================================

#[derive(Clone)]
pub struct Inner {
    pub serial: i32,
    pub version: String,
    pub software_url: String,
    pub mask: Option<Ipv4Network>,
    pub local_ip: String,
    pub sk: Option<sign::SecretKey>,
    pub secure_tcp_pk_b: PublicKey,
    pub secure_tcp_sk_b: SecretKey,
}

// ============================================================================
// RendezvousServer main structure
// ============================================================================

#[derive(Clone)]
pub struct RendezvousServer {
    pub tcp_punch: Arc<Mutex<HashMap<SocketAddr, Sink>>>,
    pub pm: PeerMap,
    pub tx: Sender,
    pub relay_servers: Arc<RelayServers>,
    pub relay_servers0: Arc<RelayServers>,
    pub rendezvous_servers: Arc<Vec<String>>,
    pub inner: Arc<Inner>,
    pub ws_map: Arc<Mutex<HashMap<SocketAddr, Sink>>>,
}

// ============================================================================
// Loop failure enum for error handling
// ============================================================================

pub enum LoopFailure {
    UdpSocket,
    Listener3,
    Listener2,
    Listener,
}
