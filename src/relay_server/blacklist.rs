use std::io::Read;

use hbb_common::tokio::sync::RwLock;
use hbb_common::log;

use std::collections::HashSet;

/// Load IPs from a file into a HashSet
pub fn load_list_from_file(path: &str) -> HashSet<String> {
    let mut set = HashSet::new();
    if let Ok(mut file) = std::fs::File::open(path) {
        let mut contents = String::new();
        if file.read_to_string(&mut contents).is_ok() {
            for line in contents.split('\n') {
                if let Some(ip) = line.trim().split(' ').next() {
                    if !ip.is_empty() {
                        set.insert(ip.to_owned());
                    }
                }
            }
        }
    }
    set
}

/// Add IPs to a list (supports pipe-separated multiple IPs)
pub async fn add_to_list(list: &RwLock<HashSet<String>>, ips: &str) {
    for ip in ips.split('|') {
        list.write().await.insert(ip.to_owned());
    }
}

/// Remove IPs from a list (supports "all" to clear, or pipe-separated IPs)
pub async fn remove_from_list(list: &RwLock<HashSet<String>>, ips: &str) {
    if ips == "all" {
        list.write().await.clear();
    } else {
        for ip in ips.split('|') {
            list.write().await.remove(ip);
        }
    }
}

/// Check if an IP is in a list
pub async fn is_in_list(list: &RwLock<HashSet<String>>, ip: &str) -> bool {
    list.read().await.get(ip).is_some()
}

/// Get all IPs from a list as a sorted vector
pub async fn get_all_list(list: &RwLock<HashSet<String>>) -> Vec<String> {
    list.read().await.clone().into_iter().collect()
}

/// Log the count of items in a list
pub async fn log_list_count(list: &RwLock<HashSet<String>>, name: &str, file: &str) {
    log::info!("#{}({}): {}", name, file, list.read().await.len());
}
