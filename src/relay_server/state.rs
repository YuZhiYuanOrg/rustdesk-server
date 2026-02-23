use std::collections::{HashMap, HashSet};

use hbb_common::tokio::sync::{Mutex, RwLock};

use super::stream::StreamTrait;

/// Usage statistics: (elapsed_ms, total_bits, highest_speed, current_speed)
pub type Usage = (usize, usize, usize, usize);

lazy_static::lazy_static! {
    /// Pending peers waiting to be paired for relay
    pub static ref PEERS: Mutex<HashMap<String, Box<dyn StreamTrait>>> = Default::default();
    /// Usage statistics per connection
    pub static ref USAGE: RwLock<HashMap<String, Usage>> = Default::default();
    /// IPs that are rate-limited
    pub static ref BLACKLIST: RwLock<HashSet<String>> = Default::default();
    /// IPs that are completely blocked
    pub static ref BLOCKLIST: RwLock<HashSet<String>> = Default::default();
}
