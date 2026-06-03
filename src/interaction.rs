//! Interactive chat-persona engine. Subscribes to the chat broadcast and:
//! - builds a bounded user registry (badges + lazily-fetched account age);
//! - rolls the YoRods easter egg;
//! - replies (threaded) when @mentioned, tone-matched to the streamer's mood and
//!   the sender's credibility;
//! - participates proactively at a natural cadence (cooldown + relevance +
//!   probability) without spamming.
//! All sends are gated by a token bucket (Twitch limit) and the cost governor.

pub mod decision;
pub mod persona;
pub mod registry;

use crate::analysis::Analyzer;
use crate::cost::CostGovernor;
use crate::discord::ratelimit::RateLimiter;
use crate::twitch::helix::HelixClient;
use crate::types::{ChatEvent, OutboundChat, StreamerMood};
use registry::Registry;
use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc, watch};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

pub struct InteractionCfg {
    pub bot_login: String,
    pub persona: String,
    pub chat_rate_per_30s: u32,
    pub proactive_min_interval: Duration,
    pub proactive_probability: f64,
    pub client_id: String,
    pub client_secret: String,
    pub easter_egg_user: String,
    pub easter_egg_chance: f64,
    pub easter_egg_text: String,
    pub reply_max_chars: usize,
}

const RECENT_CONTEXT: usize = 15;
const REGISTRY_CAP: usize = 2000;

pub async fn run(
    cfg: InteractionCfg,
    analyzer: Analyzer,
    http: reqwest::Client,
    cost: Arc<CostGovernor>,
    mut chat_rx: broadcast::Receiver<ChatEvent>,
    mood_rx: watch::Receiver<StreamerMood>,
    out_tx: mpsc::Sender<OutboundChat>,
    shutdown: CancellationToken,
) {
    let mut limiter = RateLimiter::per_window(cfg.chat_rate_per_30s, 30.0);
    let mut recent: VecDeque<String> = VecDeque::with_capacity(RECENT_CONTEXT);
    let mut reg = Registry::new(REGISTRY_CAP);
    let mut helix = if !cfg.client_id.is_empty() && !cfg.client_secret.is_empty() {
        Some(HelixClient::new(
            http.clone(),
            cfg.client_id.clone(),
            cfg.client_secret.clone(),
        ))
    } else {
        None
    };
    let mut last_proactive = Instant::now();
    let mut proactive_tick = tokio::time::interval(Duration::from_secs(15));
    info!(bot = %cfg.bot_login, "interaction engine started");

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => { info!("interaction shutting down"); break; }

            _ = proactive_tick.tick() => {
                if decision::proactive_due(Instant::now(), last_proactive, cfg.proactive_min_interval)
                    && !recent.is_empty()
                    && cost.allowed()
                    && rand_unit() < cfg.proactive_probability
                    && limiter.try_acquire_at(Instant::now())
                {
                    let mood = mood_rx.borrow().clone();
                    let ctx = join_recent(&recent);
                    let (system, user) =
                        persona::build_proactive_prompt(&cfg.persona, &cfg.bot_login, &mood, &ctx);
                    match analyzer.reply(system, user).await {
                        Ok((text, spent)) => {
                            cost.record(spent);
                            let clean = persona::sanitize(&text, cfg.reply_max_chars);
                            if !clean.is_empty() {
                                let _ = out_tx.try_send(OutboundChat { text: clean, reply_to: None });
                                last_proactive = Instant::now();
                            }
                        }
                        Err(e) => warn!(error = %e, "proactive generation failed"),
                    }
                }
            }

            res = chat_rx.recv() => {
                let ev = match res {
                    Ok(ev) => ev,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(skipped = n, "interaction lagged; shedding");
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                reg.observe(&ev);
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
                    // Lazily enrich account age (Helix) for the user we're replying to.
                    if let Some(h) = helix.as_mut() {
                        if reg.needs_age(&ev.user_id) {
                            if let Ok(map) = h.get_users_created_at(std::slice::from_ref(&ev.user_id)).await {
                                if let Some(dt) = map.get(&ev.user_id) {
                                    reg.set_age(&ev.user_id, (chrono::Utc::now() - *dt).num_days());
                                }
                            }
                        }
                    }
                    let mood = mood_rx.borrow().clone();
                    let ctx = join_recent(&recent);
                    let desc = reg
                        .get(&ev.user_id)
                        .map(persona::describe_user)
                        .unwrap_or_else(|| ev.user.clone());
                    let (system, user) = persona::build_reply_prompt(
                        &cfg.persona, &cfg.bot_login, &mood, &desc, &ev.text, &ctx,
                    );
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

fn join_recent(recent: &VecDeque<String>) -> String {
    recent.iter().cloned().collect::<Vec<_>>().join("\n")
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

/// Non-crypto uniform `f64` in `[0, 1)` for the easter-egg / proactive rolls.
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
