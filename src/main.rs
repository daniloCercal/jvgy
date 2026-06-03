//! Twitch -> Discord stream summarizer (one stream per process).
//!
//! Bootstrap: load config -> init logging -> build the channel topology and
//! clients -> start supervised producers + long-lived consumers -> run until a
//! shutdown signal -> drain within a bounded timeout.
//!
//! Data flow:
//!   chat  ──ChatEvent──┐
//!   audio ─Transcript──┤→ aggregator ─Insight→ output → Discord
//!   lifecycle ─status(watch)→ audio ; ─GoLive/Offline→ output

use anyhow::Context;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, watch};
use tracing::info;
use twitch_discord_summarizer::{
    aggregate, analysis, audio, config, cost, discord, interaction, logging, shutdown, storage, stt,
    supervisor, twitch, types,
};

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> anyhow::Result<()> {
    let config = Arc::new(config::Config::from_env().context("loading configuration")?);
    logging::init(&config);
    info!(
        channel = %config.twitch_channel,
        stt_provider = %config.stt_provider,
        "starting twitch-discord-summarizer"
    );

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .context("building http client")?;

    let cost = Arc::new(cost::CostGovernor::new(config.daily_spend_ceiling_usd));

    // --- Bounded channel topology (capacities + policies per design s4) ---
    // Chat fans out to multiple consumers (aggregator + interaction) via broadcast.
    let (chat_tx, _) = broadcast::channel::<types::ChatEvent>(1024);
    let agg_chat_rx = chat_tx.subscribe(); // subscribe before the producer starts
    let (tr_tx, tr_rx) = mpsc::channel::<types::TranscriptSegment>(256);
    let (out_tx, out_rx) = mpsc::channel::<types::OutMessage>(32);
    let (out_chat_tx, out_chat_rx) = mpsc::channel::<types::OutboundChat>(32);
    let (status_tx, status_rx) = watch::channel(types::StreamStatus::Offline);
    let (mood_tx, mood_rx) = watch::channel(types::StreamerMood::default());

    // Optional Supabase persistence: single channel, many producers, one writer.
    let store_enabled = config.supabase_url.is_some() && config.supabase_service_key.is_some();
    let (store_tx, store_rx) = mpsc::channel::<types::StoreRecord>(1024);
    let store_opt: Option<mpsc::Sender<types::StoreRecord>> =
        if store_enabled { Some(store_tx.clone()) } else { None };

    let shutdown = shutdown::ShutdownController::new();
    shutdown.spawn_signal_listener();
    let mut sup = supervisor::Supervisor::new(shutdown.token());

    // --- Chat: long-lived (twitch-irc reconnects internally); owns the outbound
    // receiver, so it isn't restart-supervised. ---
    sup.spawn("chat", {
        let cfg = twitch::chat::ChatCfg {
            channel: config.twitch_channel.clone(),
            bot_username: config.twitch_bot_username.clone(),
            bot_refresh_token: config.twitch_bot_refresh_token.clone(),
            client_id: config.twitch_client_id.clone(),
            client_secret: config.twitch_client_secret.clone(),
        };
        let chat_tx = chat_tx.clone();
        let store_opt = store_opt.clone();
        let tok = shutdown.token();
        async move {
            if let Err(e) = twitch::chat::run(cfg, chat_tx, out_chat_rx, store_opt, tok).await {
                tracing::warn!(error = %e, "chat worker exited");
            }
        }
    });

    // --- Supervised producers (restart on failure/panic with backoff) ---

    sup.supervise("lifecycle", {
        let config = config.clone();
        let http = http.clone();
        let status_tx = status_tx.clone();
        let out_tx = out_tx.clone();
        move |tok| {
            let cfg = twitch::lifecycle::LifecycleCfg {
                channel: config.twitch_channel.clone(),
                client_id: config.twitch_client_id.clone(),
                client_secret: config.twitch_client_secret.clone(),
                poll: config.stream_poll,
                confirm_polls: config.stream_confirm_polls,
                notify_online: config.notify_online,
                notify_offline: config.notify_offline,
            };
            twitch::lifecycle::run(cfg, http.clone(), status_tx.clone(), out_tx.clone(), tok)
        }
    });

    sup.supervise("audio", {
        let config = config.clone();
        let http = http.clone();
        let cost = cost.clone();
        let tr_tx = tr_tx.clone();
        let status_rx = status_rx.clone();
        let store_opt = store_opt.clone();
        let stt = stt::SttProvider::from_config(
            &config.stt_provider,
            &config.stt_base_url,
            &config.stt_api_key,
            &config.stt_model,
            "pt",
        );
        move |tok| {
            let cfg = audio::pipeline::AudioCfg {
                channel: config.twitch_channel.clone(),
                chunk_secs: config.stt_chunk_secs,
                vad_enabled: config.vad_enabled,
                vad_threshold: config.vad_threshold,
                replay_file: config.audio_replay_file.clone(),
                media_mode: config.media_mode.clone(),
            };
            audio::pipeline::run(
                cfg,
                stt.clone(),
                http.clone(),
                cost.clone(),
                status_rx.clone(),
                tr_tx.clone(),
                store_opt.clone(),
                tok,
            )
        }
    });

    // --- Long-lived consumers (own receivers; internal resilience) ---
    sup.spawn("aggregate", {
        let analyzer = analysis::Analyzer::new(
            http.clone(),
            config.llm_base_url.clone(),
            config.llm_api_key.clone(),
            config.llm_model.clone(),
        );
        let cfg = aggregate::AggregateCfg {
            channel: config.twitch_channel.clone(),
            summary_interval: config.summary_interval,
            window_max_secs: config.window_max_secs,
            window_max_tokens: config.window_max_tokens,
        };
        aggregate::run(
            cfg,
            analyzer,
            cost.clone(),
            agg_chat_rx,
            tr_rx,
            status_rx.clone(),
            out_tx.clone(),
            mood_tx,
            store_opt.clone(),
            shutdown.token(),
        )
    });

    // --- Supabase persistence writer (only when configured) ---
    if store_enabled {
        sup.spawn("storage", {
            storage::run(
                storage::StorageCfg {
                    url: config.supabase_url.clone().unwrap_or_default(),
                    service_key: config.supabase_service_key.clone().unwrap_or_default(),
                    channel: config.twitch_channel.clone(),
                },
                http.clone(),
                store_rx,
                shutdown.token(),
            )
        });
    }

    // --- Interactive chat persona (only when a bot account is configured) ---
    if config.twitch_bot_username.is_some() && config.twitch_bot_refresh_token.is_some() {
        sup.spawn("interaction", {
            let analyzer = analysis::Analyzer::new(
                http.clone(),
                config.llm_base_url.clone(),
                config.llm_api_key.clone(),
                config.llm_model.clone(),
            );
            let cfg = interaction::InteractionCfg {
                bot_login: config
                    .twitch_bot_username
                    .clone()
                    .unwrap_or_default()
                    .to_lowercase(),
                persona: interaction::persona::load(&config.persona_file),
                chat_rate_per_30s: config.chat_rate_per_30s,
                proactive_min_interval: config.proactive_min_interval,
                proactive_probability: config.proactive_probability,
                client_id: config.twitch_client_id.clone(),
                client_secret: config.twitch_client_secret.clone(),
                easter_egg_user: config.easter_egg_user.clone(),
                easter_egg_chance: config.easter_egg_chance,
                easter_egg_text: config.easter_egg_text.clone(),
                reply_max_chars: 200,
            };
            interaction::run(
                cfg,
                analyzer,
                http.clone(),
                cost.clone(),
                chat_tx.subscribe(),
                mood_rx.clone(),
                out_chat_tx.clone(),
                shutdown.token(),
            )
        });
    }

    sup.spawn("output", {
        discord::run(
            config.twitch_channel.clone(),
            config.discord_webhook_url.clone(),
            http.clone(),
            out_rx,
            shutdown.token(),
        )
    });

    // Drop the bootstrap-held sender clones so channels close once workers exit.
    drop(chat_tx);
    drop(tr_tx);
    drop(out_tx);
    drop(out_chat_tx);
    drop(status_tx);
    drop(store_tx);

    shutdown.token().cancelled().await;
    info!("shutdown requested; draining workers");
    sup.shutdown_and_join(config.shutdown_timeout).await;
    info!("shutdown complete");
    Ok(())
}
