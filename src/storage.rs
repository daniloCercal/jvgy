//! Optional persistence to Supabase (Postgres via PostgREST). A single worker
//! consumes `StoreRecord`s from all producers, batches them per table, and
//! flushes on size or a timer. Buffers are bounded (drop-oldest on overflow) so
//! a slow/down Supabase never grows memory — logs are best-effort.

use crate::types::StoreRecord;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

pub struct StorageCfg {
    pub url: String,
    pub service_key: String,
    pub channel: String,
}

const BATCH: usize = 100;
const FLUSH_SECS: u64 = 5;
const MAX_BUFFER: usize = 2000;

pub async fn run(
    cfg: StorageCfg,
    http: reqwest::Client,
    mut rx: mpsc::Receiver<StoreRecord>,
    shutdown: CancellationToken,
) {
    let rest = format!("{}/rest/v1", cfg.url.trim_end_matches('/'));
    let key = cfg.service_key;
    let mut chat: Vec<Value> = Vec::new();
    let mut tr: Vec<Value> = Vec::new();
    let mut sum: Vec<Value> = Vec::new();
    let mut tick = tokio::time::interval(Duration::from_secs(FLUSH_SECS));
    info!("supabase storage writer started");

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                flush_all(&http, &rest, &key, &mut chat, &mut tr, &mut sum).await;
                info!("storage drained");
                break;
            }
            _ = tick.tick() => {
                flush_all(&http, &rest, &key, &mut chat, &mut tr, &mut sum).await;
            }
            rec = rx.recv() => {
                match rec {
                    Some(StoreRecord::Chat(ev)) => {
                        push_bounded(&mut chat, json!({
                            "channel": cfg.channel,
                            "user_id": ev.user_id,
                            "login": ev.login,
                            "display_name": ev.user,
                            "text": ev.text,
                            "sub_months": ev.sub_months,
                            "is_mod": ev.is_mod,
                            "is_vip": ev.is_vip,
                            "is_sub": ev.is_sub,
                            "is_founder": ev.is_founder,
                            "is_broadcaster": ev.is_broadcaster,
                            "message_id": ev.message_id,
                        }));
                        if chat.len() >= BATCH {
                            flush(&http, &rest, &key, "twitch_chat_messages", &mut chat).await;
                        }
                    }
                    Some(StoreRecord::Transcript(seg)) => {
                        push_bounded(&mut tr, json!({
                            "channel": cfg.channel,
                            "source": "stt",
                            "text": seg.text,
                        }));
                        if tr.len() >= BATCH {
                            flush(&http, &rest, &key, "twitch_transcripts", &mut tr).await;
                        }
                    }
                    Some(StoreRecord::Summary(i)) => {
                        push_bounded(&mut sum, json!({
                            "channel": cfg.channel,
                            "topics": i.topics,
                            "sentiment": serde_json::to_value(&i.sentiment).unwrap_or(Value::Null),
                            "key_moments": serde_json::to_value(&i.key_moments).unwrap_or(Value::Null),
                            "running_summary": i.running_summary,
                            "streamer_mood": serde_json::to_value(&i.streamer_mood).unwrap_or(Value::Null),
                        }));
                        // Summaries are rare; flush immediately.
                        flush(&http, &rest, &key, "twitch_summaries", &mut sum).await;
                    }
                    None => {
                        flush_all(&http, &rest, &key, &mut chat, &mut tr, &mut sum).await;
                        break;
                    }
                }
            }
        }
    }
}

fn push_bounded(buf: &mut Vec<Value>, v: Value) {
    if buf.len() >= MAX_BUFFER {
        buf.remove(0); // drop oldest (shed; bound memory if Supabase is down)
    }
    buf.push(v);
}

async fn flush_all(
    http: &reqwest::Client,
    rest: &str,
    key: &str,
    chat: &mut Vec<Value>,
    tr: &mut Vec<Value>,
    sum: &mut Vec<Value>,
) {
    flush(http, rest, key, "twitch_chat_messages", chat).await;
    flush(http, rest, key, "twitch_transcripts", tr).await;
    flush(http, rest, key, "twitch_summaries", sum).await;
}

async fn flush(http: &reqwest::Client, rest: &str, key: &str, table: &str, buf: &mut Vec<Value>) {
    if buf.is_empty() {
        return;
    }
    let body = Value::Array(std::mem::take(buf)); // clears buf (shed on failure too)
    let url = format!("{rest}/{table}");
    let result = http
        .post(&url)
        .header("apikey", key)
        .header("Authorization", format!("Bearer {key}"))
        .header("Content-Type", "application/json")
        .header("Prefer", "return=minimal")
        .json(&body)
        .send()
        .await;
    match result {
        Ok(r) if r.status().is_success() => {}
        Ok(r) => warn!(table, status = %r.status(), "supabase insert non-success (batch dropped)"),
        Err(e) => warn!(table, error = %e, "supabase insert failed (batch dropped)"),
    }
}
