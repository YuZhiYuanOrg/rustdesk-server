use std::sync::atomic::{AtomicBool, AtomicUsize};

/// Registration timeout in milliseconds
pub const REG_TIMEOUT: i32 = 30_000;

/// Timeout for checking relay servers in milliseconds
pub const CHECK_RELAY_TIMEOUT: u64 = 3_000;

/// Punch request deduplication time window in seconds
pub const PUNCH_REQ_DEDUPE_SEC: u64 = 60;

/// Rotation index for relay server selection
pub static ROTATION_RELAY_SERVER: AtomicUsize = AtomicUsize::new(0);

/// Flag to always use relay for connections
pub static ALWAYS_USE_RELAY: AtomicBool = AtomicBool::new(false);

/// Flag to require login for connections
pub static MUST_LOGIN: AtomicBool = AtomicBool::new(false);
