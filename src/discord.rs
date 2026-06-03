//! Discord output worker: formats insights/notices as webhook payloads and posts
//! them, paced by a 30/min token bucket. If no webhook is configured it logs the
//! payloads instead (useful for dry runs).

pub mod embed;
pub mod ratelimit;

use crate::types::OutMessage;
use ratelimit::RateLimiter;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

pub async fn run(
    channel: String,
    webhook_url: String,
    http: reqwest::Client,
    mut rx: mpsc::Receiver<OutMessage>,
    shutdown: CancellationToken,
) {
    if webhook_url.is_empty() {
        warn!("DISCORD_WEBHOOK_URL not set; output disabled (payloads logged only)");
    }
    let mut limiter = RateLimiter::per_minute(30);

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                // Best-effort drain of any buffered messages (e.g., final summary).
                while let Ok(msg) = rx.try_recv() {
                    post(&channel, &webhook_url, &http, &mut limiter, msg).await;
                }
                info!("output drained");
                break;
            }
            maybe = rx.recv() => {
                match maybe {
                    Some(msg) => post(&channel, &webhook_url, &http, &mut limiter, msg).await,
                    None => { info!("output channel closed"); break; }
                }
            }
        }
    }
}

async fn post(
    channel: &str,
    webhook_url: &str,
    http: &reqwest::Client,
    limiter: &mut RateLimiter,
    msg: OutMessage,
) {
    let payload = match &msg {
        OutMessage::GoLive(info) => embed::golive_payload(channel, info),
        OutMessage::Offline { duration_secs } => embed::offline_payload(channel, *duration_secs),
        OutMessage::Insight(insight) => embed::insight_payload(channel, insight),
        OutMessage::Notice(text) => embed::notice_payload(text),
    };

    if webhook_url.is_empty() {
        info!(payload = %payload, "discord disabled; would post");
        return;
    }

    limiter.acquire().await;
    match http.post(webhook_url).json(&payload).send().await {
        Ok(resp) if resp.status().is_success() => {}
        Ok(resp) => warn!(status = %resp.status(), "discord webhook non-success"),
        Err(e) => warn!(error = %e, "discord webhook request failed"),
    }
}
