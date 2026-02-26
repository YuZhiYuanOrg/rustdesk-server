//! NAT traversal module for STUN/TURN support
//!
//! This module provides NAT traversal capabilities using STUN and TURN protocols
//! to help establish connections in restrictive network environments.

use hbb_common::{
    bail,
    log,
    tokio::{
        net::UdpSocket,
        time::{timeout, Duration},
    },
    ResultType,
};
use std::net::SocketAddr;

/// STUN message types
#[derive(Debug, Clone, Copy)]
enum StunMessageType {
    BindingRequest = 0x0001,
    BindingResponse = 0x0101,
    BindingErrorResponse = 0x0111,
}

/// STUN attribute types
#[derive(Debug, Clone, Copy)]
enum StunAttributeType {
    MappedAddress = 0x0001,
    XorMappedAddress = 0x0020,
}

/// STUN client for discovering public address
pub struct StunClient {
    servers: Vec<String>,
    timeout_secs: u64,
}

impl StunClient {
    /// Create a new STUN client
    ///
    /// # Arguments
    /// * `servers` - List of STUN servers (format: "host:port")
    pub fn new(servers: Vec<String>) -> Self {
        Self {
            servers,
            timeout_secs: 5,
        }
    }

    /// Discover public IP address using STUN
    ///
    /// Tries each STUN server in order until one succeeds
    pub async fn discover_public_address(&self) -> ResultType<SocketAddr> {
        for server in &self.servers {
            match self.query_stun_server(server).await {
                Ok(addr) => {
                    log::info!("STUN: discovered public address {} via {}", addr, server);
                    return Ok(addr);
                }
                Err(e) => {
                    log::warn!("STUN: failed to query {}: {}", server, e);
                }
            }
        }

        bail!("Failed to discover public address from all STUN servers")
    }

    /// Query a specific STUN server
    async fn query_stun_server(&self, server: &str) -> ResultType<SocketAddr> {
        // Parse server address
        let server_addr: SocketAddr = server.parse()
            ?;

        // Create UDP socket
        let socket = UdpSocket::bind("0.0.0.0:0").await?;
        socket.connect(&server_addr).await?;

        // Create STUN binding request
        let request = self.create_binding_request();

        // Send request with timeout
        timeout(
            Duration::from_secs(self.timeout_secs),
            async {
                socket.send(&request).await?;

                // Receive response
                let mut response = vec![0u8; 1024];
                let len = socket.recv(&mut response).await?;
                response.truncate(len);

                // Parse response
                self.parse_binding_response(&response)
            }
        ).await?
    }

    /// Create a STUN binding request message
    fn create_binding_request(&self) -> Vec<u8> {
        let mut request = Vec::with_capacity(20);

        // Message type: Binding Request
        request.extend_from_slice(&(StunMessageType::BindingRequest as u16).to_be_bytes());

        // Message length: 0 (no attributes)
        request.extend_from_slice(&0u16.to_be_bytes());

        // Magic cookie
        request.extend_from_slice(&0x2112A442u32.to_be_bytes());

        // Transaction ID (12 bytes, random)
        // Note: In production, use a proper random number generator
        let transaction_id: [u8; 12] = [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06,
            0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C,
        ];
        request.extend_from_slice(&transaction_id);

        request
    }

    /// Parse STUN binding response to extract mapped address
    fn parse_binding_response(&self, response: &[u8]) -> ResultType<SocketAddr> {
        if response.len() < 20 {
            bail!("STUN response too short");
        }

        // Check message type
        let msg_type = u16::from_be_bytes([response[0], response[1]]);
        if msg_type != StunMessageType::BindingResponse as u16 {
            bail!("Invalid STUN response type: {}", msg_type);
        }

        // Parse attributes
        let mut offset = 20;
        while offset + 4 <= response.len() {
            let attr_type = u16::from_be_bytes([response[offset], response[offset + 1]]);
            let attr_len = u16::from_be_bytes([response[offset + 2], response[offset + 3]]) as usize;

            if attr_type == StunAttributeType::XorMappedAddress as u16 {
                return self.parse_xor_mapped_address(&response[offset + 4..offset + 4 + attr_len]);
            }

            offset += 4 + attr_len;
            // Align to 4-byte boundary
            offset = (offset + 3) & !3;
        }

        bail!("No XOR-MAPPED-ADDRESS found in STUN response")
    }

    /// Parse XOR-MAPPED-ADDRESS attribute
    fn parse_xor_mapped_address(&self, data: &[u8]) -> ResultType<SocketAddr> {
        if data.len() < 4 {
            bail!("XOR-MAPPED-ADDRESS too short");
        }

        let family = data[1];
        let port = u16::from_be_bytes([data[2] ^ 0x21, data[3] ^ 0x12]);

        match family {
            0x01 => {
                // IPv4
                if data.len() < 8 {
                    bail!("IPv4 address too short");
                }
                let mut ip_bytes = [0u8; 4];
                for (i, byte) in ip_bytes.iter_mut().enumerate() {
                    *byte = data[4 + i] ^ [0x21, 0x12, 0xA4, 0x42][i];
                }
                let ip = std::net::IpAddr::V4(std::net::Ipv4Addr::from(ip_bytes));
                Ok(SocketAddr::new(ip, port))
            }
            0x02 => {
                // IPv6
                if data.len() < 20 {
                    bail!("IPv6 address too short");
                }
                let mut ip_bytes = [0u8; 16];
                let magic_cookie = [0x21, 0x12, 0xA4, 0x42];
                for (i, byte) in ip_bytes.iter_mut().enumerate() {
                    *byte = data[4 + i] ^ magic_cookie[i % 4];
                }
                let ip = std::net::IpAddr::V6(std::net::Ipv6Addr::from(ip_bytes));
                Ok(SocketAddr::new(ip, port))
            }
            _ => bail!("Unknown address family: {}", family),
        }
    }
}

/// TURN server configuration
#[derive(Clone, Debug)]
pub struct TurnServerConfig {
    pub url: String,
    pub username: String,
    pub password: String,
}

/// TURN client for relay connections
pub struct TurnClient {
    servers: Vec<TurnServerConfig>,
}

impl TurnClient {
    /// Create a new TURN client
    pub fn new(servers: Vec<TurnServerConfig>) -> Self {
        Self { servers }
    }

    /// Create a TURN relay connection
    ///
    /// This allocates a relay address on the TURN server
    pub async fn create_relay(&self) -> ResultType<TurnRelayConnection> {
        for server in &self.servers {
            match self.allocate_relay(server).await {
                Ok(relay) => {
                    log::info!("TURN: created relay via {}", server.url);
                    return Ok(relay);
                }
                Err(e) => {
                    log::warn!("TURN: failed to allocate relay from {}: {}", server.url, e);
                }
            }
        }

        bail!("Failed to create TURN relay from all servers")
    }

    /// Allocate a relay on a specific TURN server
    async fn allocate_relay(&self, server: &TurnServerConfig) -> ResultType<TurnRelayConnection> {
        // TODO: Implement full TURN protocol
        // For now, return a placeholder
        bail!("TURN relay allocation not yet implemented")
    }
}

/// TURN relay connection
pub struct TurnRelayConnection {
    relay_addr: SocketAddr,
    server_addr: SocketAddr,
}

impl TurnRelayConnection {
    /// Get the relayed address
    pub fn relay_addr(&self) -> SocketAddr {
        self.relay_addr
    }

    /// Send data through the relay
    pub async fn send(&self, _data: &[u8]) -> ResultType<()> {
        // TODO: Implement TURN send
        bail!("TURN send not yet implemented")
    }

    /// Receive data from the relay
    pub async fn recv(&self) -> ResultType<Vec<u8>> {
        // TODO: Implement TURN recv
        bail!("TURN recv not yet implemented")
    }
}

/// NAT traversal manager that combines STUN and TURN
pub struct NatTraversal {
    stun_client: Option<StunClient>,
    turn_client: Option<TurnClient>,
}

impl NatTraversal {
    /// Create a new NAT traversal manager
    ///
    /// # Arguments
    /// * `stun_servers` - List of STUN servers
    /// * `turn_servers` - List of TURN servers with credentials
    pub fn new(stun_servers: Vec<String>, turn_servers: Vec<TurnServerConfig>) -> Self {
        Self {
            stun_client: if stun_servers.is_empty() {
                None
            } else {
                Some(StunClient::new(stun_servers))
            },
            turn_client: if turn_servers.is_empty() {
                None
            } else {
                Some(TurnClient::new(turn_servers))
            },
        }
    }

    /// Discover public address using STUN
    pub async fn discover_public_address(&self) -> ResultType<SocketAddr> {
        if let Some(stun) = &self.stun_client {
            stun.discover_public_address().await
        } else {
            bail!("No STUN servers configured")
        }
    }

    /// Create a TURN relay connection
    pub async fn create_turn_relay(&self) -> ResultType<TurnRelayConnection> {
        if let Some(turn) = &self.turn_client {
            turn.create_relay().await
        } else {
            bail!("No TURN servers configured")
        }
    }

    /// Check if STUN is available
    pub fn has_stun(&self) -> bool {
        self.stun_client.is_some()
    }

    /// Check if TURN is available
    pub fn has_turn(&self) -> bool {
        self.turn_client.is_some()
    }
}

/// NAT traversal configuration
#[derive(Clone, Debug)]
pub struct NatTraversalConfig {
    pub stun_servers: Vec<String>,
    pub turn_servers: Vec<TurnServerConfig>,
}

impl Default for NatTraversalConfig {
    fn default() -> Self {
        Self {
            stun_servers: vec![
                "stun.l.google.com:19302".to_string(),
                "stun1.l.google.com:19302".to_string(),
            ],
            turn_servers: vec![],
        }
    }
}

impl NatTraversalConfig {
    /// Create configuration from environment variables
    pub fn from_env() -> Self {
        let stun_servers = std::env::var("STUN_SERVERS")
            .unwrap_or_else(|_| "stun.l.google.com:19302,stun1.l.google.com:19302".to_string())
            .split(',')
            .map(String::from)
            .collect();

        // TODO: Parse TURN servers from environment
        let turn_servers = vec![];

        Self {
            stun_servers,
            turn_servers,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nat_traversal_config() {
        let config = NatTraversalConfig::default();
        assert!(!config.stun_servers.is_empty());
        assert!(config.turn_servers.is_empty());
    }

    #[test]
    fn test_stun_client_creation() {
        let client = StunClient::new(vec!["stun.l.google.com:19302".to_string()]);
        assert_eq!(client.servers.len(), 1);
    }
}
