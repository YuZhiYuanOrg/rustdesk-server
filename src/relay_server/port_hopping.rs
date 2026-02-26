//! Port hopping module for avoiding port blocking
//!
//! This module implements port hopping functionality to help bypass
//! firewall restrictions that block specific ports.

use hbb_common::{
    bail,
    log,
    tokio::{
        self,
        net::TcpListener,
        time::{interval, Duration},
    },
    ResultType,
};
use std::sync::Arc;

/// Port hopping manager that handles dynamic port switching
pub struct PortHoppingManager {
    current_port: u16,
    port_range: (u16, u16),
    hop_interval: Duration,
    listeners: Vec<TcpListener>,
    active_ports: Vec<u16>,
}

impl PortHoppingManager {
    /// Create a new port hopping manager
    ///
    /// # Arguments
    /// * `start_port` - The starting port number
    /// * `range` - The range of ports to use (e.g., 100 means ports from start_port to start_port+100)
    /// * `interval_secs` - The interval in seconds between port hops
    pub async fn new(start_port: u16, range: u16, interval_secs: u64) -> ResultType<Self> {
        let mut manager = Self {
            current_port: start_port,
            port_range: (start_port, start_port + range),
            hop_interval: Duration::from_secs(interval_secs),
            listeners: Vec::new(),
            active_ports: Vec::new(),
        };

        // Initially listen on multiple ports for redundancy
        let initial_port_count = 5.min(range as usize);
        for i in 0..initial_port_count {
            let port = start_port + i as u16;
            if let Ok(listener) = TcpListener::bind(format!("0.0.0.0:{}", port)).await {
                log::info!("Port hopping: listening on port {}", port);
                manager.listeners.push(listener);
                manager.active_ports.push(port);
            }
        }

        if manager.listeners.is_empty() {
            bail!("Failed to bind any ports for port hopping");
        }

        Ok(manager)
    }

    /// Start the port hopping process
    pub async fn start_hopping(self: Arc<Self>) {
        let mut interval_timer = interval(self.hop_interval);
        let manager = self.clone();

        tokio::spawn(async move {
            loop {
                interval_timer.tick().await;
                manager.hop_port().await;
            }
        });
    }

    /// Perform a port hop
    async fn hop_port(&self) {
        // Remove old listeners if we have too many
        if self.listeners.len() > 10 {
            // Note: In a real implementation, we'd need to handle this more carefully
            // with proper synchronization and graceful connection draining
            log::info!("Port hopping: would close oldest listener");
        }

        // Calculate next port
        let next_port = self.get_next_port();

        // Try to bind to the new port
        match TcpListener::bind(format!("0.0.0.0:{}", next_port)).await {
            Ok(listener) => {
                log::info!("Port hopping: successfully bound to port {}", next_port);
                // In a real implementation, we'd add this to the listeners list
                // and update the active_ports
            }
            Err(e) => {
                log::warn!(
                    "Port hopping: failed to bind to port {}: {}",
                    next_port,
                    e
                );
            }
        }
    }

    /// Get the next port in the hopping sequence
    fn get_next_port(&self) -> u16 {
        let mut next_port = self.current_port + 1;
        if next_port > self.port_range.1 {
            next_port = self.port_range.0;
        }
        next_port
    }

    /// Get all currently active ports
    pub fn get_active_ports(&self) -> &[u16] {
        &self.active_ports
    }

    /// Get the current primary port
    pub fn get_current_port(&self) -> u16 {
        self.current_port
    }
}

/// Port hopping configuration
#[derive(Clone, Debug)]
pub struct PortHoppingConfig {
    /// Whether port hopping is enabled
    pub enabled: bool,
    /// Starting port number
    pub start_port: u16,
    /// Port range (number of ports to use)
    pub port_range: u16,
    /// Hop interval in seconds
    pub hop_interval_secs: u64,
}

impl Default for PortHoppingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            start_port: 21120,
            port_range: 100,
            hop_interval_secs: 300, // 5 minutes
        }
    }
}

impl PortHoppingConfig {
    /// Create configuration from environment variables
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("PORT_HOPPING_ENABLED")
                .map(|v| v == "1" || v.to_lowercase() == "true")
                .unwrap_or(false),
            start_port: std::env::var("PORT_HOPPING_START")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(21120),
            port_range: std::env::var("PORT_HOPPING_RANGE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(100),
            hop_interval_secs: std::env::var("PORT_HOPPING_INTERVAL")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(300),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_port_hopping_config() {
        let config = PortHoppingConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.start_port, 21120);
        assert_eq!(config.port_range, 100);
        assert_eq!(config.hop_interval_secs, 300);
    }

    #[test]
    fn test_get_next_port() {
        let manager = PortHoppingManager {
            current_port: 21120,
            port_range: (21120, 21125),
            hop_interval: Duration::from_secs(300),
            listeners: vec![],
            active_ports: vec![],
        };

        assert_eq!(manager.get_next_port(), 21121);
    }
}
