use std::sync::atomic::{AtomicUsize, Ordering};
use hbb_common::log;

/// Downgrade threshold * 100 (e.g., 66 means 0.66)
pub static DOWNGRADE_THRESHOLD_100: AtomicUsize = AtomicUsize::new(66);
/// Time in ms before starting to check for downgrade
pub static DOWNGRADE_START_CHECK: AtomicUsize = AtomicUsize::new(1_800_000);
/// Speed limit for blacklisted IPs in bit/s
pub static LIMIT_SPEED: AtomicUsize = AtomicUsize::new(32 * 1024 * 1024);
/// Total bandwidth limit in bit/s
pub static TOTAL_BANDWIDTH: AtomicUsize = AtomicUsize::new(1024 * 1024 * 1024);
/// Single connection bandwidth limit in bit/s
pub static SINGLE_BANDWIDTH: AtomicUsize = AtomicUsize::new(128 * 1024 * 1024);

/// Check and apply environment variable parameters
pub fn check_params() {
    check_param_f64(
        "DOWNGRADE_THRESHOLD",
        &DOWNGRADE_THRESHOLD_100,
        100.0,
        "DOWNGRADE_THRESHOLD",
        |v| format!("{}", v / 100.0),
    );

    check_param_usize(
        "DOWNGRADE_START_CHECK",
        &DOWNGRADE_START_CHECK,
        1000,
        "DOWNGRADE_START_CHECK",
        |v| format!("{}s", v / 1000),
    );

    check_param_f64(
        "LIMIT_SPEED",
        &LIMIT_SPEED,
        1024.0 * 1024.0,
        "LIMIT_SPEED",
        |v| format!("{}Mb/s", v / 1024.0 / 1024.0),
    );

    check_param_f64(
        "TOTAL_BANDWIDTH",
        &TOTAL_BANDWIDTH,
        1024.0 * 1024.0,
        "TOTAL_BANDWIDTH",
        |v| format!("{}Mb/s", v / 1024.0 / 1024.0),
    );

    check_param_f64(
        "SINGLE_BANDWIDTH",
        &SINGLE_BANDWIDTH,
        1024.0 * 1024.0,
        "SINGLE_BANDWIDTH",
        |v| format!("{}Mb/s", v / 1024.0 / 1024.0),
    );
}

fn check_param_f64<F: Fn(f64) -> String>(
    env_name: &str,
    atomic: &AtomicUsize,
    multiplier: f64,
    log_name: &str,
    format_fn: F,
) {
    let tmp = std::env::var(env_name)
        .map(|x| x.parse::<f64>().unwrap_or(0.0))
        .unwrap_or(0.0);
    if tmp > 0.0 {
        atomic.store((tmp * multiplier) as usize, Ordering::SeqCst);
    }
    log::info!("{}: {}", log_name, format_fn(atomic.load(Ordering::SeqCst) as f64));
}

fn check_param_usize<F: Fn(usize) -> String>(
    env_name: &str,
    atomic: &AtomicUsize,
    multiplier: usize,
    log_name: &str,
    format_fn: F,
) {
    let tmp = std::env::var(env_name)
        .map(|x| x.parse::<usize>().unwrap_or(0))
        .unwrap_or(0);
    if tmp > 0 {
        atomic.store(tmp * multiplier, Ordering::SeqCst);
    }
    log::info!("{}: {}", log_name, format_fn(atomic.load(Ordering::SeqCst)));
}
