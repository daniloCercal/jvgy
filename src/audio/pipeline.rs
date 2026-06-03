//! The media + STT worker. While the stream is online it spawns
//! `streamlink | ffmpeg`, reads 16 kHz mono s16le PCM, slices it into fixed
//! chunks, drops silent/over-budget chunks (backpressure = shed, never grow),
//! transcribes voiced chunks, and emits `TranscriptSegment`s. No disk I/O.

use crate::audio::vad;
use crate::cost::CostGovernor;
use crate::stt::SttProvider;
use crate::types::{StoreRecord, StreamStatus, TranscriptSegment};
use anyhow::{anyhow, Context, Result};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

const SAMPLE_RATE: u32 = 16_000;
const BYTES_PER_SEC: usize = SAMPLE_RATE as usize * 2; // mono s16le

#[derive(Clone)]
pub struct AudioCfg {
    pub channel: String,
    pub chunk_secs: u32,
    pub vad_enabled: bool,
    pub vad_threshold: f64,
    /// Replay a local file (looped) instead of pulling the live stream.
    pub replay_file: Option<String>,
    /// `streamlink` or `ffmpeg-direct` (see config).
    pub media_mode: String,
}

pub async fn run(
    cfg: AudioCfg,
    stt: SttProvider,
    http: reqwest::Client,
    cost: Arc<CostGovernor>,
    mut status_rx: watch::Receiver<StreamStatus>,
    tr_tx: mpsc::Sender<TranscriptSegment>,
    store_tx: Option<mpsc::Sender<StoreRecord>>,
    shutdown: CancellationToken,
) -> Result<()> {
    let replay = cfg.replay_file.is_some();
    loop {
        // Idle until the stream is online (or shutdown). Replay mode runs
        // unconditionally (no Twitch needed).
        if !replay {
            while !status_rx.borrow().is_online() {
                tokio::select! {
                    _ = shutdown.cancelled() => return Ok(()),
                    r = status_rx.changed() => {
                        if r.is_err() { return Ok(()); }
                    }
                }
            }
        }
        if shutdown.is_cancelled() {
            return Ok(());
        }

        info!(channel = %cfg.channel, "stream online; starting capture pipeline");

        // Bounded chunk channel (capacity 2): if STT falls behind, the reader
        // drops chunks rather than buffering audio in memory.
        let (chunk_tx, chunk_rx) = mpsc::channel::<Vec<u8>>(2);

        let reader = tokio::spawn(capture_loop(
            cfg.clone(),
            http.clone(),
            chunk_tx,
            status_rx.clone(),
            shutdown.clone(),
        ));
        let consumer = tokio::spawn(stt_loop(
            stt.clone(),
            http.clone(),
            cost.clone(),
            cfg.clone(),
            chunk_rx,
            tr_tx.clone(),
            store_tx.clone(),
            shutdown.clone(),
        ));

        // Reader ends on offline/shutdown/EOF; dropping its sender ends consumer.
        if let Err(e) = reader.await {
            warn!(error = %e, "capture task join error");
        }
        let _ = consumer.await;

        if shutdown.is_cancelled() {
            return Ok(());
        }
        // Pipeline ended (subprocess died / EOF) while still expected to run:
        // brief backoff before respawning to avoid a tight loop.
        if replay || status_rx.borrow().is_online() {
            tokio::select! {
                _ = shutdown.cancelled() => return Ok(()),
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            }
        }
    }
}

/// Spawn ffmpeg with the given input args, transcoding to 16 kHz mono s16le on
/// stdout (no stdin). Used by replay and ffmpeg-direct modes.
fn spawn_ffmpeg_stdout(input_args: &[&str]) -> Result<Child> {
    let mut args: Vec<&str> = vec!["-hide_banner", "-loglevel", "error"];
    args.extend_from_slice(input_args);
    args.extend_from_slice(&["-vn", "-ac", "1", "-ar", "16000", "-f", "s16le", "pipe:1"]);
    Command::new("ffmpeg")
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(Into::into)
}

async fn capture_loop(
    cfg: AudioCfg,
    http: reqwest::Client,
    chunk_tx: mpsc::Sender<Vec<u8>>,
    mut status_rx: watch::Receiver<StreamStatus>,
    shutdown: CancellationToken,
) -> Result<()> {
    // ffmpeg either reads the source directly (replay file / resolved HLS URL)
    // or transcodes from streamlink's stdout (default). Optional handles let us
    // tear everything down cleanly.
    let mut streamlink: Option<Child> = None;
    let mut pump: Option<JoinHandle<()>> = None;

    let mut ffmpeg = if let Some(file) = &cfg.replay_file {
        spawn_ffmpeg_stdout(&["-re", "-stream_loop", "-1", "-i", file])
            .context("spawning ffmpeg for replay (is it installed?)")?
    } else if cfg.media_mode == "ffmpeg-direct" {
        // Option B: resolve the HLS audio_only URL in-process, drop streamlink.
        let url = crate::twitch::hls::resolve_audio_url(&http, &cfg.channel)
            .await
            .context("resolving HLS audio URL (ffmpeg-direct)")?;
        info!(channel = %cfg.channel, "resolved direct HLS audio URL");
        spawn_ffmpeg_stdout(&[
            "-reconnect", "1", "-reconnect_streamed", "1", "-reconnect_delay_max", "2", "-i", &url,
        ])
        .context("spawning ffmpeg (is it installed?)")?
    } else {
        // Option A (default): streamlink | ffmpeg.
        let mut sl = Command::new("streamlink")
            .args([
                "--stdout",
                "--twitch-disable-ads",
                "--loglevel",
                "none",
                &format!("twitch.tv/{}", cfg.channel),
                "audio_only",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("spawning streamlink (is it installed?)")?;
        let mut sl_out = sl.stdout.take().context("streamlink stdout")?;

        let mut ff = Command::new("ffmpeg")
            .args([
                "-hide_banner", "-loglevel", "error", "-i", "pipe:0", "-vn", "-ac", "1", "-ar",
                "16000", "-f", "s16le", "pipe:1",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("spawning ffmpeg (is it installed?)")?;
        let mut ff_in = ff.stdin.take().context("ffmpeg stdin")?;

        // Pump streamlink -> ffmpeg stdin. Dropping ff_in closes stdin on EOF.
        pump = Some(tokio::spawn(async move {
            let _ = tokio::io::copy(&mut sl_out, &mut ff_in).await;
        }));
        streamlink = Some(sl);
        ff
    };
    let mut ff_out = ffmpeg.stdout.take().context("ffmpeg stdout")?;

    let chunk_bytes = (cfg.chunk_secs as usize).max(1) * BYTES_PER_SEC;
    let mut read_buf = vec![0u8; 8192];
    let mut chunk = Vec::with_capacity(chunk_bytes);

    let result = loop {
        tokio::select! {
            _ = shutdown.cancelled() => break Ok(()),
            r = status_rx.changed() => {
                if r.is_err() || !status_rx.borrow().is_online() {
                    break Ok(());
                }
            }
            n = ff_out.read(&mut read_buf) => {
                match n {
                    Ok(0) => break Ok(()), // EOF: subprocess ended
                    Ok(n) => {
                        chunk.extend_from_slice(&read_buf[..n]);
                        if chunk.len() >= chunk_bytes {
                            let full = std::mem::replace(&mut chunk, Vec::with_capacity(chunk_bytes));
                            // Shed (drop) the chunk if STT is behind.
                            let _ = chunk_tx.try_send(full);
                        }
                    }
                    Err(e) => break Err(anyhow!("ffmpeg read error: {e}")),
                }
            }
        }
    };

    // Tear down subprocesses deterministically.
    if let Some(p) = pump {
        p.abort();
    }
    let _ = ffmpeg.start_kill();
    if let Some(mut sl) = streamlink {
        let _ = sl.start_kill();
        let _ = sl.wait().await;
    }
    let _ = ffmpeg.wait().await;
    result
}

async fn stt_loop(
    stt: SttProvider,
    http: reqwest::Client,
    cost: Arc<CostGovernor>,
    cfg: AudioCfg,
    mut chunk_rx: mpsc::Receiver<Vec<u8>>,
    tr_tx: mpsc::Sender<TranscriptSegment>,
    store_tx: Option<mpsc::Sender<StoreRecord>>,
    shutdown: CancellationToken,
) {
    let minutes = cfg.chunk_secs as f64 / 60.0;
    while let Some(pcm) = chunk_rx.recv().await {
        if shutdown.is_cancelled() {
            break;
        }
        if !cost.allowed() {
            continue; // kill-switch tripped
        }
        if cfg.vad_enabled && !vad::is_voiced(&pcm, cfg.vad_threshold) {
            continue; // skip silence
        }
        match stt.transcribe(&http, &pcm).await {
            Ok(text) if !text.trim().is_empty() => {
                cost.record(stt.cost_per_min() * minutes);
                let seg = TranscriptSegment { text };
                if let Some(s) = &store_tx {
                    let _ = s.try_send(StoreRecord::Transcript(seg.clone()));
                }
                if tr_tx.send(seg).await.is_err() {
                    break;
                }
            }
            Ok(_) => {}
            Err(e) => warn!(error = %e, "stt failed for chunk"),
        }
    }
}
