# Network Traversal Enhancement

This document describes the network traversal enhancements added to the relay server to improve connection success rates in complex network environments.

## Features

### 1. QUIC Protocol Support

QUIC (Quick UDP Internet Connections) provides a UDP-based alternative to TCP, which can bypass certain firewall restrictions.

**Configuration:**
```bash
# Enable QUIC support (requires "quic" feature flag)
export ENABLE_QUIC=true
```

**Note:** QUIC support requires the `quic` feature to be enabled during compilation:
```bash
cargo build --features quic
```

### 2. Port Hopping

Port hopping dynamically changes the listening port to avoid port-based blocking.

**Configuration:**
```bash
# Enable port hopping
export ENABLE_PORT_HOPPING=true

# Starting port for hopping range
export PORT_HOPPING_START=21120

# Number of ports in the hopping range
export PORT_HOPPING_RANGE=100

# Interval between port hops (in seconds)
export PORT_HOPPING_INTERVAL=300
```

**How it works:**
- The server listens on multiple ports simultaneously
- Periodically opens new ports and closes old ones
- Clients can connect to any active port in the range

### 3. STUN Support

STUN (Session Traversal Utilities for NAT) helps discover the public IP address and port mapping.

**Configuration:**
```bash
# Enable STUN
export ENABLE_STUN=true

# Configure STUN servers (comma-separated)
export STUN_SERVERS="stun.l.google.com:19302,stun1.l.google.com:19302"
```

**Benefits:**
- Discovers public IP address
- Helps identify NAT type
- Aids in NAT traversal strategies

### 4. TURN Relay Support

TURN (Traversal Using Relays around NAT) provides a relay server for cases where direct connection is impossible.

**Configuration:**
```bash
# Enable TURN
export ENABLE_TURN=true

# Configure TURN servers (format: turn://username:password@host:port)
export TURN_SERVERS="turn://user:pass@turn.example.com:3478"
```

**Note:** TURN support is currently a placeholder. Full implementation requires additional development.

## Architecture

### Module Structure

```
src/relay_server/
├── stream.rs           # Stream abstraction (TCP, WebSocket, QUIC)
├── port_hopping.rs     # Port hopping implementation
├── nat_traversal.rs    # STUN/TURN support
├── config.rs           # Configuration management
├── connection.rs       # Connection handling
└── mod.rs              # Module integration
```

### Stream Abstraction

The `StreamTrait` provides a unified interface for different transport protocols:

```rust
pub trait StreamTrait: Send + Sync + 'static {
    async fn recv(&mut self) -> Option<Result<BytesMut, Error>>;
    async fn send_raw(&mut self, bytes: Bytes) -> ResultType<()>;
    fn is_ws(&self) -> bool;
    fn set_raw(&mut self);
}
```

Supported implementations:
- `FramedStream` - TCP
- `WebSocketStream` - WebSocket
- `QuicStream` - QUIC (when feature enabled)

## Usage Examples

### Basic Setup

```bash
# Start relay server with default settings
./hbbr

# Start with network traversal features
export ENABLE_STUN=true
export ENABLE_PORT_HOPPING=true
./hbbr
```

### Advanced Configuration

```bash
# Full network traversal setup
export ENABLE_QUIC=true
export ENABLE_PORT_HOPPING=true
export PORT_HOPPING_START=21120
export PORT_HOPPING_RANGE=100
export PORT_HOPPING_INTERVAL=300
export ENABLE_STUN=true
export STUN_SERVERS="stun.l.google.com:19302,stun1.l.google.com:19302"

./hbbr
```

## Implementation Status

| Feature | Status | Notes |
|---------|--------|-------|
| QUIC Protocol | ✅ Implemented | Requires feature flag |
| Port Hopping | ✅ Implemented | Production ready |
| STUN Client | ✅ Implemented | Basic functionality |
| TURN Client | ⚠️ Placeholder | Needs full implementation |
| NAT Type Detection | 📝 Planned | Future enhancement |
| ICE Framework | 📝 Planned | Future enhancement |

## Performance Considerations

### QUIC
- Lower latency than TCP in lossy networks
- Better performance through NAT
- Built-in encryption and multiplexing

### Port Hopping
- Minimal overhead
- Configurable hop interval
- Graceful port transition

### STUN
- Lightweight discovery process
- Cached results to minimize queries
- Fallback to multiple servers

## Security Considerations

1. **QUIC**: Provides built-in encryption (TLS 1.3)
2. **Port Hopping**: May trigger security alerts in strict environments
3. **STUN/TURN**: No sensitive data transmitted during discovery

## Troubleshooting

### QUIC Connection Issues
- Ensure UDP traffic is allowed through firewall
- Check if QUIC feature is enabled during compilation
- Verify port availability

### Port Hopping Not Working
- Check if ports in range are available
- Verify firewall rules allow dynamic port binding
- Review logs for binding errors

### STUN Discovery Fails
- Verify STUN server addresses are correct
- Check network connectivity to STUN servers
- Try alternative STUN servers

## Future Enhancements

1. **Full TURN Implementation**
   - Complete TURN protocol support
   - TURN credentials management
   - TURN server health monitoring

2. **NAT Type Detection**
   - Automatic NAT type identification
   - Adaptive traversal strategy selection

3. **ICE Framework**
   - Interactive Connectivity Establishment
   - Candidate gathering and prioritization
   - Connectivity checks

4. **Connection Quality Monitoring**
   - Real-time latency measurement
   - Automatic protocol switching
   - Bandwidth adaptation

## Contributing

When contributing to network traversal features:

1. Test with various network configurations
2. Document any new configuration options
3. Update this README with new features
4. Add unit tests for new functionality

## License

This project is licensed under the same license as the main RustDesk server project.
