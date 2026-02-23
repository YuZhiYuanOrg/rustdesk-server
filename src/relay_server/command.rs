use std::fmt::Write;
use std::sync::atomic::Ordering;

use async_speed_limit::Limiter;

use super::blacklist;
use super::config;
use super::state::{self, Usage};

/// Process a command and return the response
pub async fn check_cmd(cmd: &str, limiter: Limiter) -> String {
    let mut res = String::new();
    let mut parts = cmd.trim().split(' ');

    match parts.next() {
        Some("h") => res = help_text(),
        Some("blacklist-add" | "ba") => {
            if let Some(ips) = parts.next() {
                blacklist::add_to_list(&state::BLACKLIST, ips).await;
            }
        }
        Some("blacklist-remove" | "br") => {
            if let Some(ips) = parts.next() {
                blacklist::remove_from_list(&state::BLACKLIST, ips).await;
            }
        }
        Some("blacklist" | "b") => {
            if let Some(ip) = parts.next() {
                res = format!("{}\n", blacklist::is_in_list(&state::BLACKLIST, ip).await);
            } else {
                for ip in blacklist::get_all_list(&state::BLACKLIST).await {
                    let _ = writeln!(res, "{ip}");
                }
            }
        }
        Some("blocklist-add" | "Ba") => {
            if let Some(ips) = parts.next() {
                blacklist::add_to_list(&state::BLOCKLIST, ips).await;
            }
        }
        Some("blocklist-remove" | "Br") => {
            if let Some(ips) = parts.next() {
                blacklist::remove_from_list(&state::BLOCKLIST, ips).await;
            }
        }
        Some("blocklist" | "B") => {
            if let Some(ip) = parts.next() {
                res = format!("{}\n", blacklist::is_in_list(&state::BLOCKLIST, ip).await);
            } else {
                for ip in blacklist::get_all_list(&state::BLOCKLIST).await {
                    let _ = writeln!(res, "{ip}");
                }
            }
        }
        Some("downgrade-threshold" | "dt") => {
            res = handle_float_param(
                parts.next(),
                &config::DOWNGRADE_THRESHOLD_100,
                100.0,
                |v| format!("{}", v / 100.0),
            );
        }
        Some("downgrade-start-check" | "t") => {
            res = handle_usize_param(parts.next(), &config::DOWNGRADE_START_CHECK, 1000, |v| {
                format!("{}s", v / 1000)
            });
        }
        Some("limit-speed" | "ls") => {
            res = handle_float_param(parts.next(), &config::LIMIT_SPEED, 1024.0 * 1024.0, |v| {
                format!("{}Mb/s", v / 1024.0 / 1024.0)
            });
        }
        Some("total-bandwidth" | "tb") => {
            res = handle_total_bandwidth(parts.next(), &limiter);
        }
        Some("single-bandwidth" | "sb") => {
            res = handle_float_param(parts.next(), &config::SINGLE_BANDWIDTH, 1024.0 * 1024.0, |v| {
                format!("{}Mb/s", v / 1024.0 / 1024.0)
            });
        }
        Some("usage" | "u") => {
            res = format_usage().await;
        }
        _ => {}
    }
    res
}

fn help_text() -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n",
        "blacklist-add(ba) <ip>",
        "blacklist-remove(br) <ip>",
        "blacklist(b) <ip>",
        "blocklist-add(Ba) <ip>",
        "blocklist-remove(Br) <ip>",
        "blocklist(B) <ip>",
        "downgrade-threshold(dt) [value]",
        "downgrade-start-check(t) [value(second)]",
        "limit-speed(ls) [value(Mb/s)]",
        "total-bandwidth(tb) [value(Mb/s)]",
        "single-bandwidth(sb) [value(Mb/s)]",
        "usage(u)"
    )
}

fn handle_float_param<F: Fn(f64) -> String>(
    value: Option<&str>,
    atomic: &std::sync::atomic::AtomicUsize,
    multiplier: f64,
    format_fn: F,
) -> String {
    if let Some(v) = value {
        if let Ok(v) = v.parse::<f64>() {
            if v > 0.0 {
                atomic.store((v * multiplier) as usize, Ordering::SeqCst);
            }
        }
        String::new()
    } else {
        format!("{}\n", format_fn(atomic.load(Ordering::SeqCst) as f64))
    }
}

fn handle_usize_param<F: Fn(usize) -> String>(
    value: Option<&str>,
    atomic: &std::sync::atomic::AtomicUsize,
    multiplier: usize,
    format_fn: F,
) -> String {
    if let Some(v) = value {
        if let Ok(v) = v.parse::<usize>() {
            if v > 0 {
                atomic.store(v * multiplier, Ordering::SeqCst);
            }
        }
        String::new()
    } else {
        format!("{}\n", format_fn(atomic.load(Ordering::SeqCst)))
    }
}

fn handle_total_bandwidth(value: Option<&str>, limiter: &Limiter) -> String {
    if let Some(v) = value {
        if let Ok(v) = v.parse::<f64>() {
            if v > 0.0 {
                let val = (v * 1024.0 * 1024.0) as usize;
                config::TOTAL_BANDWIDTH.store(val, Ordering::SeqCst);
                limiter.set_speed_limit(val as f64);
            }
        }
        String::new()
    } else {
        format!(
            "{}Mb/s\n",
            config::TOTAL_BANDWIDTH.load(Ordering::SeqCst) as f64 / 1024.0 / 1024.0
        )
    }
}

async fn format_usage() -> String {
    let mut res = String::new();
    let mut tmp: Vec<(String, Usage)> = state::USAGE
        .read()
        .await
        .iter()
        .map(|x| (x.0.clone(), *x.1))
        .collect();
    tmp.sort_by(|a, b| ((b.1).1).partial_cmp(&(a.1).1).unwrap());

    for (ip, (elapsed, total, highest, speed)) in tmp {
        if elapsed == 0 {
            continue;
        }
        let _ = writeln!(
            res,
            "{}: {}s {:.2}MB {}kb/s {}kb/s {}kb/s",
            ip,
            elapsed / 1000,
            total as f64 / 1024.0 / 1024.0 / 8.0,
            highest,
            total / elapsed,
            speed
        );
    }
    res
}
