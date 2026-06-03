//! Stream lifecycle: poll Helix for online/offline, debounce transitions, drive
//! the audio pipeline via a `watch` channel, and post go-live/offline notices.

use crate::twitch::helix::HelixClient;
use crate::types::{OutMessage, StreamStatus};
use anyhow::Result;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

pub struct LifecycleCfg {
    pub channel: String,
    pub client_id: String,
    pub client_secret: String,
    pub poll: Duration,
    pub confirm_polls: u32,
    pub notify_online: bool,
    pub notify_offline: bool,
}

pub async fn run(
    cfg: LifecycleCfg,
    http: reqwest::Client,
    status_tx: watch::Sender<StreamStatus>,
    out_tx: mpsc::Sender<OutMessage>,
    shutdown: CancellationToken,
) -> Result<()> {
    if cfg.client_id.is_empty() || cfg.client_secret.is_empty() {
        warn!("TWITCH_CLIENT_ID/SECRET not set; online/offline detection disabled");
        shutdown.cancelled().await;
        return Ok(());
    }

    let mut helix = HelixClient::new(http, cfg.client_id.clone(), cfg.client_secret.clone());
    let mut online = false;
    let mut pending = 0u32;
    let mut online_since: Option<Instant> = None;

    loop {
        match helix.get_stream(&cfg.channel).await {
            Ok(live) => {
                let is_live = live.is_some();
                if is_live != online {
                    pending += 1;
                    if pending >= cfg.confirm_polls.max(1) {
                        online = is_live;
                        pending = 0;
                        if online {
                            let info = live.unwrap_or_default();
                            online_since = Some(Instant::now());
                            let _ = status_tx.send(StreamStatus::Online);
                            info!(channel = %cfg.channel, "stream ONLINE");
                            if cfg.notify_online {
                                let _ = out_tx.send(OutMessage::GoLive(info)).await;
                            }
                        } else {
                            let dur = online_since
                                .take()
                                .map(|t| t.elapsed().as_secs())
                                .unwrap_or(0);
                            let _ = status_tx.send(StreamStatus::Offline);
                            info!(channel = %cfg.channel, "stream OFFLINE");
                            if cfg.notify_offline {
                                let _ = out_tx
                                    .send(OutMessage::Offline { duration_secs: dur })
                                    .await;
                            }
                        }
                    }
                } else {
                    pending = 0;
                }
            }
            Err(e) => warn!(error = %e, "helix poll failed; keeping last state"),
        }

        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = tokio::time::sleep(cfg.poll) => {}
        }
    }
    Ok(())
}
