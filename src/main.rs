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
use tokio::sync::{mpsc, watch};
use tracing::info;
use twitch_discord_summarizer::{
    aggregate, analysis, audio, config, cost, discord, logging, shutdown, stt, supervisor, twitch,
    types,
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
    let (chat_tx, chat_rx) = mpsc::channel::<types::ChatEvent>(1024);
    let (tr_tx, tr_rx) = mpsc::channel::<types::TranscriptSegment>(256);
    let (out_tx, out_rx) = mpsc::channel::<types::OutMessage>(32);
    let (status_tx, status_rx) = watch::channel(types::StreamStatus::Offline);

    let shutdown = shutdown::ShutdownController::new();
    shutdown.spawn_signal_listener();
    let mut sup = supervisor::Supervisor::new(shutdown.token());

    // --- Supervised producers (restart on failure/panic with backoff) ---
    sup.supervise("chat", {
        let channel = config.twitch_channel.clone();
        let chat_tx = chat_tx.clone();
        move |tok| twitch::chat::run(channel.clone(), chat_tx.clone(), tok)
    });

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
            };
            audio::pipeline::run(
                cfg,
                stt.clone(),
                http.clone(),
                cost.clone(),
                status_rx.clone(),
                tr_tx.clone(),
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
            chat_rx,
            tr_rx,
            status_rx.clone(),
            out_tx.clone(),
            shutdown.token(),
        )
    });

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
    drop(status_tx);

    shutdown.token().cancelled().await;
    info!("shutdown requested; draining workers");
    sup.shutdown_and_join(config.shutdown_timeout).await;
    info!("shutdown complete");
    Ok(())
}
