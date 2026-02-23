use std::sync::atomic::Ordering;

use async_speed_limit::Limiter;
use hbb_common::{bail, log, tokio, tokio::time::{interval, Duration}, ResultType};

use super::config;
use super::state;
use super::stream::StreamTrait;

/// Relay data between two streams
pub async fn relay(
    addr: std::net::SocketAddr,
    stream: &mut impl StreamTrait,
    peer: &mut Box<dyn StreamTrait>,
    total_limiter: Limiter,
    id: String,
) -> ResultType<()> {
    let ip = addr.ip().to_string();
    let mut tm = std::time::Instant::now();
    let mut elapsed = 0;
    let mut total = 0;
    let mut total_s = 0;
    let mut highest_s = 0;
    let mut downgrade = false;
    let mut blacked = false;

    let sb = config::SINGLE_BANDWIDTH.load(Ordering::SeqCst) as f64;
    let limiter = Limiter::new(sb);
    let blacklist_limiter = Limiter::new(config::LIMIT_SPEED.load(Ordering::SeqCst) as f64);
    let downgrade_threshold =
        (sb * config::DOWNGRADE_THRESHOLD_100.load(Ordering::SeqCst) as f64 / 100.0 / 1000.0)
            as usize;

    let mut timer = interval(Duration::from_secs(3));
    let mut last_recv_time = std::time::Instant::now();

    loop {
        tokio::select! {
            res = peer.recv() => {
                if let Some(Ok(bytes)) = res {
                    last_recv_time = std::time::Instant::now();
                    if process_data(&bytes, &limiter, &blacklist_limiter, &total_limiter, blacked, downgrade, &mut total, &mut total_s).await {
                        stream.send_raw(bytes.into()).await?;
                    }
                } else {
                    break;
                }
            }
            res = stream.recv() => {
                if let Some(Ok(bytes)) = res {
                    last_recv_time = std::time::Instant::now();
                    if process_data(&bytes, &limiter, &blacklist_limiter, &total_limiter, blacked, downgrade, &mut total, &mut total_s).await {
                        peer.send_raw(bytes.into()).await?;
                    }
                } else {
                    break;
                }
            }
            _ = timer.tick() => {
                if last_recv_time.elapsed().as_secs() > 30 {
                    bail!("Timeout");
                }
            }
        }

        let n = tm.elapsed().as_millis() as usize;
        if n >= 1_000 {
            if state::BLOCKLIST.read().await.get(&ip).is_some() {
                log::info!("{} blocked", ip);
                break;
            }
            blacked = state::BLACKLIST.read().await.get(&ip).is_some();
            tm = std::time::Instant::now();
            let speed = total_s / n;
            if speed > highest_s {
                highest_s = speed;
            }
            elapsed += n;
            state::USAGE.write().await.insert(
                id.clone(),
                (elapsed as _, total as _, highest_s as _, speed as _),
            );
            total_s = 0;

            if elapsed > config::DOWNGRADE_START_CHECK.load(Ordering::SeqCst)
                && !downgrade
                && total > elapsed * downgrade_threshold
            {
                downgrade = true;
                log::info!(
                    "Downgrade {}, exceed downgrade threshold {}bit/ms in {}ms",
                    id,
                    downgrade_threshold,
                    elapsed
                );
            }
        }
    }
    Ok(())
}

async fn process_data(
    bytes: &[u8],
    limiter: &Limiter,
    blacklist_limiter: &Limiter,
    total_limiter: &Limiter,
    blacked: bool,
    downgrade: bool,
    total: &mut usize,
    total_s: &mut usize,
) -> bool {
    let nb = bytes.len() * 8;
    if blacked || downgrade {
        blacklist_limiter.consume(nb).await;
    } else {
        limiter.consume(nb).await;
    }
    total_limiter.consume(nb).await;
    *total += nb;
    *total_s += nb;
    !bytes.is_empty()
}
