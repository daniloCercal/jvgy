//! Interactive chat-persona engine. Subscribes to the chat broadcast, and for
//! each message: rolls the easter egg, and replies (threaded) when the bot is
//! @mentioned — generating an in-character line via the LLM, tone-matched to the
//! streamer's mood and the sender's credibility. Sends are gated by a token
//! bucket (Twitch limit) and the cost governor. Proactive participation and the
//! Helix-backed user registry land in the next increment.

pub mod decision;
pub mod persona;

use crate::analysis::Analyzer;
use crate::cost::CostGovernor;
use crate::discord::ratelimit::RateLimiter;
use crate::types::{ChatEvent, OutboundChat, StreamerMood};
use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{broadcast, mpsc, watch};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

pub struct InteractionCfg {
    pub bot_login: String,
    pub persona: String,
    pub chat_rate_per_30s: u32,
    pub easter_egg_user: String,
    pub easter_egg_chance: f64,
    pub easter_egg_text: String,
    pub reply_max_chars: usize,
}

const RECENT_CONTEXT: usize = 15;

pub async fn run(
    cfg: InteractionCfg,
    analyzer: Analyzer,
    cost: Arc<CostGovernor>,
    mut chat_rx: broadcast::Receiver<ChatEvent>,
    mood_rx: watch::Receiver<StreamerMood>,
    out_tx: mpsc::Sender<OutboundChat>,
    shutdown: CancellationToken,
) {
    let mut limiter = RateLimiter::per_window(cfg.chat_rate_per_30s, 30.0);
    let mut recent: VecDeque<String> = VecDeque::with_capacity(RECENT_CONTEXT);
    info!(bot = %cfg.bot_login, "interaction engine started");

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => { info!("interaction shutting down"); break; }
            res = chat_rx.recv() => {
                let ev = match res {
                    Ok(ev) => ev,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(skipped = n, "interaction lagged; shedding");
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                push_recent(&mut recent, &ev);

                // Easter egg: threaded reply, bypasses relevance, respects rate.
                if decision::easter_egg_hit(&ev.login, &cfg.easter_egg_user, cfg.easter_egg_chance, rand_unit())
                    && limiter.try_acquire_at(Instant::now())
                {
                    let _ = out_tx.try_send(OutboundChat {
                        text: cfg.easter_egg_text.clone(),
                        reply_to: Some(ev.message_id.clone()),
                    });
                    continue;
                }

                // @mention -> generate and post a threaded reply.
                if decision::is_mention(&ev.text, &cfg.bot_login) {
                    if !cost.allowed() || !limiter.try_acquire_at(Instant::now()) {
                        continue;
                    }
                    let mood = mood_rx.borrow().clone();
                    let ctx = recent.iter().cloned().collect::<Vec<_>>().join("\n");
                    let (system, user) =
                        persona::build_reply_prompt(&cfg.persona, &cfg.bot_login, &mood, &ev, &ctx);
                    match analyzer.reply(system, user).await {
                        Ok((text, spent)) => {
                            cost.record(spent);
                            let clean = persona::sanitize(&text, cfg.reply_max_chars);
                            if !clean.is_empty() {
                                let _ = out_tx.try_send(OutboundChat {
                                    text: clean,
                                    reply_to: Some(ev.message_id.clone()),
                                });
                            }
                        }
                        Err(e) => warn!(error = %e, "reply generation failed"),
                    }
                }
            }
        }
    }
}

fn push_recent(recent: &mut VecDeque<String>, ev: &ChatEvent) {
    if recent.len() >= RECENT_CONTEXT {
        recent.pop_front();
    }
    recent.push_back(format!("{}: {}", ev.user, ev.text));
}

thread_local! {
    static RNG: Cell<u64> = Cell::new({
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64 | 1)
            .unwrap_or(0x9E37_79B9_7F4A_7C15)
    });
}

/// Non-crypto uniform `f64` in `[0, 1)` for the easter-egg roll.
fn rand_unit() -> f64 {
    RNG.with(|cell| {
        let mut x = cell.get();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        cell.set(x);
        (x >> 11) as f64 / (1u64 << 53) as f64
    })
}
