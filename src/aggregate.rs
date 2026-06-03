//! Aggregator worker: merges chat + transcript into a bounded window, evaluates
//! the trigger policy, runs incremental LLM summaries, and emits insights. Also
//! summarizes once when the stream goes offline and flushes a final summary on
//! shutdown.

pub mod trigger;
pub mod window;

use crate::analysis::Analyzer;
use crate::cost::CostGovernor;
use crate::types::{
    ChatEvent, OutMessage, StoreRecord, StreamStatus, StreamerMood, TranscriptSegment,
};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc, watch};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use trigger::Trigger;
use window::Window;

pub struct AggregateCfg {
    pub channel: String,
    pub summary_interval: Duration,
    pub window_max_secs: Duration,
    pub window_max_tokens: usize,
}

pub async fn run(
    cfg: AggregateCfg,
    analyzer: Analyzer,
    cost: Arc<CostGovernor>,
    mut chat_rx: broadcast::Receiver<ChatEvent>,
    mut tr_rx: mpsc::Receiver<TranscriptSegment>,
    mut status_rx: watch::Receiver<StreamStatus>,
    out_tx: mpsc::Sender<OutMessage>,
    mood_tx: watch::Sender<StreamerMood>,
    store_tx: Option<mpsc::Sender<StoreRecord>>,
    shutdown: CancellationToken,
) {
    let mut window = Window::new(cfg.window_max_secs, cfg.window_max_tokens);
    let token_threshold = (cfg.window_max_tokens as f64 * 0.6) as usize;
    let mut trigger = Trigger::new(cfg.summary_interval, token_threshold);
    let mut prior_summary = String::new();
    let mut tick = tokio::time::interval(Duration::from_secs(5));

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                if window.has_unsummarized() {
                    do_summary(&cfg, &analyzer, &cost, &mut window, &mut prior_summary, &out_tx, &mood_tx, &store_tx).await;
                }
                info!("aggregator drained");
                break;
            }
            res = chat_rx.recv() => {
                match res {
                    Ok(ev) => {
                        if is_summary_command(&ev.text) {
                            trigger.request_manual();
                        }
                        window.push_chat(ev.user, ev.text);
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => {}
                }
            }
            Some(seg) = tr_rx.recv() => {
                window.push_transcript(seg.text);
            }
            changed = status_rx.changed() => {
                if changed.is_ok() && !status_rx.borrow().is_online() && window.has_unsummarized() {
                    do_summary(&cfg, &analyzer, &cost, &mut window, &mut prior_summary, &out_tx, &mood_tx, &store_tx).await;
                    trigger.record_fire(Instant::now());
                }
            }
            _ = tick.tick() => {
                if trigger.should_fire(window.unsummarized_tokens(), Instant::now()) {
                    do_summary(&cfg, &analyzer, &cost, &mut window, &mut prior_summary, &out_tx, &mood_tx, &store_tx).await;
                    trigger.record_fire(Instant::now());
                }
            }
        }
    }
}

fn is_summary_command(text: &str) -> bool {
    let t = text.trim().to_lowercase();
    t == "!summary" || t == "!resumo" || t.starts_with("!summary ") || t.starts_with("!resumo ")
}

async fn do_summary(
    cfg: &AggregateCfg,
    analyzer: &Analyzer,
    cost: &Arc<CostGovernor>,
    window: &mut Window,
    prior_summary: &mut String,
    out_tx: &mpsc::Sender<OutMessage>,
    mood_tx: &watch::Sender<StreamerMood>,
    store_tx: &Option<mpsc::Sender<StoreRecord>>,
) {
    if !cost.allowed() {
        return; // kill-switch tripped
    }
    let delta = window.render_delta();
    if delta.trim().is_empty() {
        return;
    }
    match analyzer.summarize(&cfg.channel, prior_summary, &delta).await {
        Ok((insight, spent)) => {
            *prior_summary = insight.running_summary.clone();
            // Publish the streamer's mood for the interaction engine.
            let _ = mood_tx.send(insight.streamer_mood.clone());
            if let Some(s) = store_tx {
                let _ = s.try_send(StoreRecord::Summary(insight.clone()));
            }
            window.mark_summarized();
            if cost.record(spent) {
                let _ = out_tx
                    .send(OutMessage::Notice(format!(
                        "⚠️ Teto de gasto diário (US$ {:.2}) atingido — pausando STT/LLM até o reset.",
                        cost.ceiling_usd()
                    )))
                    .await;
            }
            let _ = out_tx.send(OutMessage::Insight(insight)).await;
            info!(spent_usd = cost.spent_usd(), "summary posted");
        }
        Err(e) => warn!(error = %e, "summary failed"),
    }
}
