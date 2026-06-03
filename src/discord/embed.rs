//! Discord webhook payload builders (rich embeds) + text chunking helpers that
//! respect Discord's length limits.

use crate::types::{Insight, StreamInfo};
use serde_json::{json, Value};

const EMBED_DESC_LIMIT: usize = 4096;
const FIELD_VALUE_LIMIT: usize = 1024;

/// Split `text` into chunks of at most `max` chars, preferring to break on
/// newlines/spaces. Always makes progress (never returns empty pieces).
pub fn chunk_text(text: &str, max: usize) -> Vec<String> {
    let max = max.max(1);
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut cur_len = 0usize;
    for word in split_keep_ws(text) {
        let wlen = word.chars().count();
        if wlen > max {
            // Hard-split an oversized token on char boundaries.
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
                cur_len = 0;
            }
            for ch in word.chars() {
                if cur_len + 1 > max {
                    out.push(std::mem::take(&mut cur));
                    cur_len = 0;
                }
                cur.push(ch);
                cur_len += 1;
            }
        } else {
            if cur_len + wlen > max {
                out.push(std::mem::take(&mut cur));
                cur_len = 0;
            }
            cur.push_str(word);
            cur_len += wlen;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

fn split_keep_ws(text: &str) -> Vec<&str> {
    // Split into words while keeping trailing whitespace attached, so rejoining
    // preserves readability.
    let mut parts = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b' ' || *b == b'\n' {
            parts.push(&text[start..=i]);
            start = i + 1;
        }
    }
    if start < text.len() {
        parts.push(&text[start..]);
    }
    parts
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

pub fn insight_payload(channel: &str, insight: &Insight) -> Value {
    let mut fields = Vec::new();
    if !insight.topics.is_empty() {
        fields.push(json!({
            "name": "Tópicos",
            "value": truncate_chars(&insight.topics.join(", "), FIELD_VALUE_LIMIT),
            "inline": false,
        }));
    }
    if !insight.sentiment.mood.is_empty() {
        fields.push(json!({
            "name": "Clima do chat",
            "value": format!("{} ({:+.2})", insight.sentiment.mood, insight.sentiment.score),
            "inline": true,
        }));
    }
    if !insight.key_moments.is_empty() {
        let moments = insight
            .key_moments
            .iter()
            .map(|m| format!("• `{}` {}", m.timestamp, m.note))
            .collect::<Vec<_>>()
            .join("\n");
        fields.push(json!({
            "name": "Momentos",
            "value": truncate_chars(&moments, FIELD_VALUE_LIMIT),
            "inline": false,
        }));
    }

    // Split a long summary across multiple embeds (Discord: 4096/embed, ~10/msg)
    // instead of truncating. First embed carries the title + fields.
    let chunks = chunk_text(&insight.running_summary, EMBED_DESC_LIMIT);
    let mut embeds = Vec::new();
    for (idx, chunk) in chunks.iter().enumerate().take(8) {
        let mut embed = json!({ "description": chunk, "color": 0x9146FF });
        if idx == 0 {
            embed["title"] = json!(format!("📋 Resumo — {channel}"));
            embed["fields"] = json!(fields);
        }
        embeds.push(embed);
    }

    json!({ "embeds": embeds })
}

pub fn golive_payload(channel: &str, info: &StreamInfo) -> Value {
    let mut embed = json!({
        "title": format!("🔴 {channel} está AO VIVO!"),
        "url": format!("https://twitch.tv/{channel}"),
        "color": 0xFF0000,
        "fields": [
            { "name": "Título", "value": truncate_chars(&nonempty(&info.title, "—"), FIELD_VALUE_LIMIT), "inline": false },
            { "name": "Categoria", "value": nonempty(&info.game, "—"), "inline": true },
            { "name": "Viewers", "value": info.viewers.to_string(), "inline": true },
        ],
    });
    if !info.started_at.trim().is_empty() {
        // Discord renders an ISO8601 timestamp in the embed footer ("no ar desde").
        embed["timestamp"] = json!(info.started_at);
    }
    json!({ "embeds": [embed] })
}

pub fn offline_payload(channel: &str, duration_secs: u64) -> Value {
    let h = duration_secs / 3600;
    let m = (duration_secs % 3600) / 60;
    json!({
        "embeds": [{
            "title": format!("⚫ {channel} encerrou a stream"),
            "description": format!("Duração: {h}h{m:02}m"),
            "color": 0x555555,
        }]
    })
}

pub fn notice_payload(text: &str) -> Value {
    json!({ "content": truncate_chars(text, 2000) })
}

fn nonempty(s: &str, fallback: &str) -> String {
    if s.trim().is_empty() {
        fallback.to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_respects_max_and_preserves_content() {
        let text = "uma frase comprida ".repeat(20);
        let chunks = chunk_text(&text, 50);
        assert!(chunks.iter().all(|c| c.chars().count() <= 50));
        let rejoined: String = chunks.concat();
        assert_eq!(rejoined.replace(['\n'], " ").trim(), text.replace(['\n'], " ").trim());
    }

    #[test]
    fn chunk_hard_splits_oversized_token() {
        let text = "x".repeat(120);
        let chunks = chunk_text(&text, 50);
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|c| c.chars().count() <= 50));
    }

    #[test]
    fn insight_payload_has_embed() {
        let insight = Insight {
            topics: vec!["valorant".into()],
            running_summary: "resumo".into(),
            ..Default::default()
        };
        let v = insight_payload("gaules", &insight);
        assert!(v["embeds"][0]["title"].as_str().unwrap().contains("gaules"));
    }
}
