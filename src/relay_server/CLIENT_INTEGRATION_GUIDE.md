# 客户端集成开发指南

本文档说明如何将新增的网络穿透功能集成到RustDesk客户端，以充分利用服务器端的增强功能。

## 目录

1. [架构概览](#架构概览)
2. [QUIC协议集成](#quic协议集成)
3. [端口跳跃支持](#端口跳跃支持)
4. [STUN/TURN集成](#stunturn集成)
5. [协议选择策略](#协议选择策略)
6. [代码示例](#代码示例)
7. [测试指南](#测试指南)

---

## 架构概览

### 客户端-服务器通信流程

```
客户端启动
    ↓
[1] 发现服务器端口（端口跳跃支持）
    ↓
[2] 选择最优协议（TCP/WebSocket/QUIC）
    ↓
[3] 建立连接
    ↓
[4] STUN发现公网地址（可选）
    ↓
[5] 中继握手
    ↓
[6] 数据传输
```

### 客户端需要实现的功能

1. **多协议支持**: TCP、WebSocket、QUIC
2. **端口发现**: 支持端口跳跃的服务器端口发现
3. **协议探测**: 自动选择最优协议
4. **NAT穿透**: STUN/TURN客户端功能
5. **连接管理**: 多连接管理和故障转移

---

## QUIC协议集成

### 1. 添加依赖

在客户端的 `Cargo.toml` 中添加：

```toml
[dependencies]
quinn = "0.10"  # 或最新版本
rustls = "0.21"
rcgen = "0.10"  # 用于生成自签名证书

[features]
default = []
quic = ["quinn"]
```

### 2. QUIC客户端实现

创建 `src/client/quic_client.rs`:

```rust
use quinn::{Endpoint, ClientConfig, Connection};
use std::net::SocketAddr;
use std::sync::Arc;

pub struct QuicClient {
    endpoint: Endpoint,
    server_addr: SocketAddr,
    server_name: String,
}

impl QuicClient {
    /// 创建QUIC客户端
    pub async fn new(server_addr: SocketAddr) -> ResultType<Self> {
        // 创建客户端端点
        let mut endpoint = Endpoint::client("[::]:0".parse()?)?;
        
        // 配置客户端（跳过证书验证，用于测试）
        let client_config = configure_client()?;
        endpoint.set_default_client_config(client_config);
        
        Ok(Self {
            endpoint,
            server_addr,
            server_name: "rustdesk-relay".to_string(),
        })
    }
    
    /// 连接到服务器
    pub async fn connect(&self) -> ResultType<Connection> {
        let connection = self.endpoint
            .connect(self.server_addr, &self.server_name)?
            .await?;
        
        log::info!("QUIC connection established to {}", self.server_addr);
        Ok(connection)
    }
    
    /// 创建双向流
    pub async fn create_stream(&self, conn: &Connection) -> ResultType<(SendStream, RecvStream)> {
        let (send, recv) = conn.open_bi().await?;
        Ok((send, recv))
    }
}

/// 配置客户端（跳过证书验证）
fn configure_client() -> ResultType<ClientConfig> {
    let crypto = rustls::ClientConfig::builder()
        .with_safe_defaults()
        .with_custom_certificate_verifier(Arc::new(SkipServerVerification));
    
    Ok(ClientConfig::new(Arc::new(crypto)))
}

/// 跳过服务器证书验证（仅用于测试）
struct SkipServerVerification;

impl rustls::client::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::Certificate,
        _intermediates: &[rustls::Certificate],
        _server_name: &rustls::ServerName,
        _scts: &mut dyn Iterator<Item = &[u8]>,
        _ocsp_response: &[u8],
        _now: std::time::SystemTime,
    ) -> Result<rustls::client::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::ServerCertVerified::assertion())
    }
}
```

### 3. QUIC流适配器

创建 `src/client/quic_stream.rs`:

```rust
use async_trait::async_trait;
use hbb_common::{
    bytes::{Bytes, BytesMut},
    tokio::io::{AsyncReadExt, AsyncWriteExt},
    ResultType,
};
use quinn::{RecvStream, SendStream};

#[async_trait]
pub trait StreamTrait: Send + Sync + 'static {
    async fn recv(&mut self) -> Option<Result<BytesMut, std::io::Error>>;
    async fn send_raw(&mut self, bytes: Bytes) -> ResultType<()>;
}

pub struct QuicStream {
    send: SendStream,
    recv: RecvStream,
}

impl QuicStream {
    pub fn new(send: SendStream, recv: RecvStream) -> Self {
        Self { send, recv }
    }
}

#[async_trait]
impl StreamTrait for QuicStream {
    async fn recv(&mut self) -> Option<Result<BytesMut, std::io::Error>> {
        let mut buf = BytesMut::with_capacity(65536);
        match self.recv.read_buf(&mut buf).await {
            Ok(0) => None,
            Ok(_) => Some(Ok(buf)),
            Err(e) => Some(Err(e)),
        }
    }
    
    async fn send_raw(&mut self, bytes: Bytes) -> ResultType<()> {
        self.send.write_all(&bytes).await?;
        self.send.flush().await?;
        Ok(())
    }
}
```

---

## 端口跳跃支持

### 1. 端口发现策略

创建 `src/client/port_discovery.rs`:

```rust
use std::net::SocketAddr;
use std::time::Duration;

pub struct PortDiscovery {
    base_port: u16,
    port_range: u16,
    current_ports: Vec<u16>,
    last_update: std::time::Instant,
}

impl PortDiscovery {
    /// 创建端口发现器
    pub fn new(base_port: u16, port_range: u16) -> Self {
        // 初始化时尝试多个可能的端口
        let current_ports = (0..5)
            .map(|i| base_port + i)
            .collect();
        
        Self {
            base_port,
            port_range,
            current_ports,
            last_update: std::time::Instant::now(),
        }
    }
    
    /// 获取当前活跃端口列表
    pub fn get_active_ports(&self) -> &[u16] {
        &self.current_ports
    }
    
    /// 尝试连接到服务器，自动发现活跃端口
    pub async fn discover_active_port(
        &mut self,
        server_ip: &str,
        timeout: Duration,
    ) -> ResultType<u16> {
        for port in &self.current_ports {
            let addr = format!("{}:{}", server_ip, port);
            
            // 尝试TCP连接
            if let Ok(_) = tokio::time::timeout(
                timeout,
                tokio::net::TcpStream::connect(&addr)
            ).await {
                log::info!("Discovered active port: {}", port);
                return Ok(*port);
            }
        }
        
        // 如果已知端口都失败，扫描整个范围
        for port in self.base_port..self.base_port + self.port_range {
            let addr = format!("{}:{}", server_ip, port);
            
            if let Ok(_) = tokio::time::timeout(
                Duration::from_millis(100),
                tokio::net::TcpStream::connect(&addr)
            ).await {
                log::info!("Discovered active port in range: {}", port);
                self.update_active_ports(port);
                return Ok(port);
            }
        }
        
        bail!("No active port found in range {}-{}", 
              self.base_port, 
              self.base_port + self.port_range)
    }
    
    /// 更新活跃端口列表
    fn update_active_ports(&mut self, new_port: u16) {
        // 保持最近发现的5个端口
        if !self.current_ports.contains(&new_port) {
            self.current_ports.push(new_port);
            if self.current_ports.len() > 5 {
                self.current_ports.remove(0);
            }
        }
        self.last_update = std::time::Instant::now();
    }
    
    /// 从服务器获取端口列表（如果有API）
    pub async fn fetch_from_server(&mut self, server_url: &str) -> ResultType<()> {
        // TODO: 实现从服务器API获取当前活跃端口
        // 例如: GET http://server:port/api/active_ports
        Ok(())
    }
}
```

### 2. 端口跳跃配置

在客户端配置中添加：

```rust
// src/client/config.rs

pub struct ClientConfig {
    // 现有配置...
    
    // 端口跳跃配置
    pub enable_port_hopping: bool,
    pub port_hopping_base: u16,
    pub port_hopping_range: u16,
    pub port_discovery_timeout: Duration,
}

impl ClientConfig {
    pub fn from_env() -> Self {
        Self {
            enable_port_hopping: std::env::var("ENABLE_PORT_HOPPING")
                .map(|v| v == "1" || v.to_lowercase() == "true")
                .unwrap_or(false),
            port_hopping_base: std::env::var("PORT_HOPPING_BASE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(21117),
            port_hopping_range: std::env::var("PORT_HOPPING_RANGE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(100),
            port_discovery_timeout: Duration::from_secs(5),
            // ... 其他配置
        }
    }
}
```

---

## STUN/TURN集成

### 1. STUN客户端

创建 `src/client/stun_client.rs`:

```rust
use std::net::{SocketAddr, UdpSocket};

pub struct StunClient {
    servers: Vec<String>,
    timeout: Duration,
}

impl StunClient {
    pub fn new(servers: Vec<String>) -> Self {
        Self {
            servers,
            timeout: Duration::from_secs(5),
        }
    }
    
    /// 发现公网地址
    pub fn discover_public_address(&self) -> ResultType<SocketAddr> {
        for server in &self.servers {
            match self.query_server(server) {
                Ok(addr) => {
                    log::info!("Discovered public address: {} via {}", addr, server);
                    return Ok(addr);
                }
                Err(e) => {
                    log::warn!("STUN query to {} failed: {}", server, e);
                }
            }
        }
        
        bail!("All STUN servers failed")
    }
    
    fn query_server(&self, server: &str) -> ResultType<SocketAddr> {
        let server_addr: SocketAddr = server.parse()?;
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.set_read_timeout(Some(self.timeout))?;
        socket.connect(&server_addr)?;
        
        // 发送STUN Binding Request
        let request = self.create_binding_request();
        socket.send(&request)?;
        
        // 接收响应
        let mut response = vec![0u8; 1024];
        let len = socket.recv(&mut response)?;
        response.truncate(len);
        
        // 解析响应
        self.parse_response(&response)
    }
    
    fn create_binding_request(&self) -> Vec<u8> {
        // 实现STUN Binding Request消息
        // 参考: RFC 5389
        vec![
            0x00, 0x01,  // Message Type: Binding Request
            0x00, 0x00,  // Message Length
            0x21, 0x12, 0xA4, 0x42,  // Magic Cookie
            // Transaction ID (12 bytes)
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06,
            0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C,
        ]
    }
    
    fn parse_response(&self, data: &[u8]) -> ResultType<SocketAddr> {
        // 解析STUN响应，提取XOR-MAPPED-ADDRESS
        // 参考: RFC 5389 Section 15.2
        // 实现细节...
        unimplemented!()
    }
}
```

### 2. NAT类型检测

创建 `src/client/nat_detection.rs`:

```rust
pub enum NatType {
    OpenInternet,      // 开放互联网
    FullCone,          // 完全锥形NAT
    RestrictedCone,    // 受限锥形NAT
    PortRestricted,    // 端口受限锥形NAT
    Symmetric,         // 对称NAT
    SymmetricUdpFirewall, // 对称UDP防火墙
    Unknown,           // 未知
}

pub struct NatDetector {
    stun_servers: Vec<String>,
}

impl NatDetector {
    pub fn new(stun_servers: Vec<String>) -> Self {
        Self { stun_servers }
    }
    
    /// 检测NAT类型
    pub async fn detect_nat_type(&self) -> ResultType<NatType> {
        // 实现RFC 3489的NAT类型检测算法
        // 1. 测试1: 从本地端口发送请求到STUN服务器
        // 2. 测试2: 从同一端口发送请求到另一个IP
        // 3. 测试3: 从同一端口发送请求到另一个端口
        // 根据响应判断NAT类型
        
        // 简化实现
        let stun_client = StunClient::new(self.stun_servers.clone());
        let public_addr = stun_client.discover_public_address()?;
        
        // TODO: 完整的NAT类型检测逻辑
        Ok(NatType::Unknown)
    }
}
```

---

## 协议选择策略

### 1. 智能协议选择器

创建 `src/client/protocol_selector.rs`:

```rust
use std::time::{Duration, Instant};

pub enum Protocol {
    Tcp,
    WebSocket,
    Quic,
}

pub struct ProtocolSelector {
    performance_history: Vec<(Protocol, Duration, bool)>,
    current_best: Protocol,
}

impl ProtocolSelector {
    pub fn new() -> Self {
        Self {
            performance_history: Vec::new(),
            current_best: Protocol::Tcp, // 默认TCP
        }
    }
    
    /// 选择最优协议
    pub fn select_protocol(&self, network_conditions: &NetworkConditions) -> Protocol {
        // 根据网络条件选择协议
        match network_conditions {
            // 高丢包率：选择QUIC
            NetworkConditions { packet_loss, .. } if *packet_loss > 0.1 => {
                Protocol::Quic
            }
            
            // 高延迟：选择QUIC
            NetworkConditions { latency, .. } if *latency > Duration::from_millis(200) => {
                Protocol::Quic
            }
            
            // 防火墙限制：尝试WebSocket
            NetworkConditions { firewall_restricted: true, .. } => {
                Protocol::WebSocket
            }
            
            // 默认：使用历史最优
            _ => self.current_best.clone(),
        }
    }
    
    /// 记录协议性能
    pub fn record_performance(&mut self, protocol: Protocol, latency: Duration, success: bool) {
        self.performance_history.push((protocol.clone(), latency, success));
        
        // 保留最近100条记录
        if self.performance_history.len() > 100 {
            self.performance_history.remove(0);
        }
        
        // 更新最优协议
        self.update_best_protocol();
    }
    
    fn update_best_protocol(&mut self) {
        // 计算每个协议的平均性能
        let mut protocol_scores: std::collections::HashMap<Protocol, f64> = 
            std::collections::HashMap::new();
        
        for (protocol, latency, success) in &self.performance_history {
            let score = if *success {
                1.0 / latency.as_secs_f64()
            } else {
                0.0
            };
            
            *protocol_scores.entry(protocol.clone()).or_insert(0.0) += score;
        }
        
        // 选择得分最高的协议
        if let Some((best, _)) = protocol_scores.iter().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()) {
            self.current_best = best.clone();
        }
    }
}

pub struct NetworkConditions {
    pub latency: Duration,
    pub packet_loss: f32,
    pub bandwidth: u64,
    pub firewall_restricted: bool,
}
```

### 2. 连接管理器

创建 `src/client/connection_manager.rs`:

```rust
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct ConnectionManager {
    connections: Arc<RwLock<Vec<Box<dyn Connection>>>>,
    protocol_selector: ProtocolSelector,
    port_discovery: PortDiscovery,
}

impl ConnectionManager {
    pub fn new(config: &ClientConfig) -> Self {
        Self {
            connections: Arc::new(RwLock::new(Vec::new())),
            protocol_selector: ProtocolSelector::new(),
            port_discovery: PortDiscovery::new(
                config.port_hopping_base,
                config.port_hopping_range,
            ),
        }
    }
    
    /// 建立最优连接
    pub async fn establish_connection(
        &mut self,
        server_ip: &str,
        config: &ClientConfig,
    ) -> ResultType<Box<dyn Connection>> {
        // 1. 发现端口
        let port = if config.enable_port_hopping {
            self.port_discovery.discover_active_port(
                server_ip,
                config.port_discovery_timeout,
            ).await?
        } else {
            config.default_port
        };
        
        // 2. 探测网络条件
        let network_conditions = self.probe_network(server_ip, port).await?;
        
        // 3. 选择协议
        let protocol = self.protocol_selector.select_protocol(&network_conditions);
        
        // 4. 建立连接
        let start = Instant::now();
        let connection = match protocol {
            Protocol::Tcp => self.connect_tcp(server_ip, port).await?,
            Protocol::WebSocket => self.connect_ws(server_ip, port).await?,
            Protocol::Quic => self.connect_quic(server_ip, port).await?,
        };
        
        let latency = start.elapsed();
        self.protocol_selector.record_performance(protocol, latency, true);
        
        Ok(connection)
    }
    
    async fn probe_network(&self, server_ip: &str, port: u16) -> ResultType<NetworkConditions> {
        // 探测网络条件
        let start = Instant::now();
        
        // 简单的ping测试
        let addr = format!("{}:{}", server_ip, port);
        let _ = tokio::net::TcpStream::connect(&addr).await?;
        
        let latency = start.elapsed();
        
        Ok(NetworkConditions {
            latency,
            packet_loss: 0.0, // TODO: 实际测量
            bandwidth: 0,     // TODO: 实际测量
            firewall_restricted: false, // TODO: 检测
        })
    }
    
    async fn connect_tcp(&self, server_ip: &str, port: u16) -> ResultType<Box<dyn Connection>> {
        let addr = format!("{}:{}", server_ip, port);
        let stream = tokio::net::TcpStream::connect(&addr).await?;
        Ok(Box::new(TcpConnection::new(stream)))
    }
    
    async fn connect_ws(&self, server_ip: &str, port: u16) -> ResultType<Box<dyn Connection>> {
        let addr = format!("ws://{}:{}", server_ip, port);
        let (stream, _) = tokio_tungstenite::connect_async(&addr).await?;
        Ok(Box::new(WebSocketConnection::new(stream)))
    }
    
    async fn connect_quic(&self, server_ip: &str, port: u16) -> ResultType<Box<dyn Connection>> {
        let addr = format!("{}:{}", server_ip, port).parse()?;
        let client = QuicClient::new(addr).await?;
        let conn = client.connect().await?;
        Ok(Box::new(QuicConnection::new(conn)))
    }
}
```

---

## 代码示例

### 完整的客户端使用示例

```rust
// src/client/relay_client.rs

use hbb_common::{log, ResultType};

pub struct RelayClient {
    config: ClientConfig,
    connection_manager: ConnectionManager,
    stun_client: Option<StunClient>,
}

impl RelayClient {
    pub fn new(config: ClientConfig) -> Self {
        let connection_manager = ConnectionManager::new(&config);
        
        let stun_client = if config.enable_stun {
            Some(StunClient::new(config.stun_servers.clone()))
        } else {
            None
        };
        
        Self {
            config,
            connection_manager,
            stun_client,
        }
    }
    
    /// 连接到中继服务器
    pub async fn connect(&mut self, server_ip: &str) -> ResultType<()> {
        // 1. 发现公网地址（可选）
        if let Some(ref stun) = self.stun_client {
            match stun.discover_public_address() {
                Ok(addr) => {
                    log::info!("My public address: {}", addr);
                }
                Err(e) => {
                    log::warn!("Failed to discover public address: {}", e);
                }
            }
        }
        
        // 2. 建立连接
        let connection = self.connection_manager
            .establish_connection(server_ip, &self.config)
            .await?;
        
        log::info!("Connected to relay server");
        
        // 3. 发送中继请求
        self.send_relay_request(connection).await?;
        
        Ok(())
    }
    
    async fn send_relay_request(&self, mut connection: Box<dyn Connection>) -> ResultType<()> {
        // 构造中继请求消息
        let mut msg = RendezvousMessage::new();
        let mut rf = RequestRelay::new();
        rf.set_uuid(uuid::Uuid::new_v4().to_string());
        rf.set_licence_key(self.config.key.clone());
        msg.set_request_relay(rf);
        
        // 发送消息
        let bytes = msg.write_to_bytes()?;
        connection.send(bytes.into()).await?;
        
        log::info!("Relay request sent");
        
        Ok(())
    }
}

// 使用示例
#[tokio::main]
async fn main() -> ResultType<()> {
    // 加载配置
    let config = ClientConfig::from_env();
    
    // 创建客户端
    let mut client = RelayClient::new(config);
    
    // 连接到服务器
    client.connect("relay.example.com").await?;
    
    // 开始中继通信
    // ...
    
    Ok(())
}
```

---

## 测试指南

### 1. 单元测试

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_port_discovery() {
        let mut discovery = PortDiscovery::new(21117, 100);
        assert!(!discovery.get_active_ports().is_empty());
    }
    
    #[test]
    fn test_protocol_selector() {
        let selector = ProtocolSelector::new();
        
        let conditions = NetworkConditions {
            latency: Duration::from_millis(300),
            packet_loss: 0.15,
            bandwidth: 0,
            firewall_restricted: false,
        };
        
        let protocol = selector.select_protocol(&conditions);
        assert!(matches!(protocol, Protocol::Quic));
    }
    
    #[tokio::test]
    async fn test_quic_client() {
        // 需要运行QUIC服务器
        // let client = QuicClient::new("127.0.0.1:21121".parse().unwrap()).await.unwrap();
        // let conn = client.connect().await.unwrap();
        // assert!(conn.close_reason().is_none());
    }
}
```

### 2. 集成测试

```bash
# 测试TCP连接
cargo test test_tcp_connection --features tcp

# 测试WebSocket连接
cargo test test_ws_connection --features websocket

# 测试QUIC连接
cargo test test_quic_connection --features quic

# 测试端口发现
cargo test test_port_discovery

# 测试STUN
cargo test test_stun_discovery
```

### 3. 端到端测试

```bash
# 1. 启动服务器（启用所有功能）
export ENABLE_QUIC=true
export ENABLE_PORT_HOPPING=true
export ENABLE_STUN=true
./hbbr

# 2. 运行客户端测试
cargo test --features quic,websocket test_e2e_relay
```

---

## 部署建议

### 1. 渐进式部署

**阶段1**: 仅启用TCP和WebSocket
```bash
# 客户端配置
export ENABLE_QUIC=false
export ENABLE_PORT_HOPPING=false
export ENABLE_STUN=true
```

**阶段2**: 启用端口跳跃
```bash
export ENABLE_PORT_HOPPING=true
export PORT_HOPPING_BASE=21117
export PORT_HOPPING_RANGE=100
```

**阶段3**: 启用QUIC
```bash
export ENABLE_QUIC=true
```

### 2. 监控指标

客户端应记录以下指标：
- 连接成功率（按协议分类）
- 平均连接延迟
- 协议切换次数
- 端口发现成功率
- STUN发现成功率

### 3. 故障转移

实现自动故障转移：
1. 主协议失败 → 尝试备用协议
2. 主端口失败 → 尝试其他端口
3. 主服务器失败 → 尝试备用服务器

---

## 常见问题

### Q1: QUIC连接失败
**A**: 检查防火墙是否允许UDP流量，确认服务器已启用QUIC

### Q2: 端口发现慢
**A**: 减小`port_discovery_timeout`，或使用服务器API获取端口列表

### Q3: STUN发现失败
**A**: 检查STUN服务器地址，尝试使用其他STUN服务器

### Q4: 协议选择不正确
**A**: 调整`ProtocolSelector`的阈值，或手动指定协议

---

## 下一步

1. 实现完整的TURN客户端
2. 添加NAT类型检测
3. 实现ICE框架
4. 添加连接质量监控
5. 实现多服务器负载均衡

---

## 参考资源

- [QUIC协议规范 (RFC 9000)](https://www.rfc-editor.org/rfc/rfc9000)
- [STUN协议 (RFC 5389)](https://www.rfc-editor.org/rfc/rfc5389)
- [TURN协议 (RFC 8656)](https://www.rfc-editor.org/rfc/rfc8656)
- [NAT类型检测 (RFC 3489)](https://www.rfc-editor.org/rfc/rfc3489)
- [Quinn文档](https://docs.rs/quinn/)
