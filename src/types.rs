//! Shared domain types passed between workers via bounded channels.

use serde::{Deserialize, Serialize};

/// Live stream metadata from Helix (present only while online).
#[derive(Debug, Clone, Default)]
pub struct StreamInfo {
    pub title: String,
    pub game: String,
    pub started_at: String,
    pub viewers: u64,
}

/// Current stream status, broadcast to the audio pipeline via a `watch` channel.
/// Metadata for notifications travels separately via `OutMessage::GoLive`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StreamStatus {
    #[default]
    Offline,
    Online,
}

impl StreamStatus {
    pub fn is_online(&self) -> bool {
        matches!(self, StreamStatus::Online)
    }
}

/// A single chat message, normalized from Twitch IRC.
#[derive(Debug, Clone)]
pub struct ChatEvent {
    pub user: String,
    pub text: String,
}

/// A finalized transcript span from the STT provider.
#[derive(Debug, Clone)]
pub struct TranscriptSegment {
    pub text: String,
}

/// Messages bound for the Discord output worker.
#[derive(Debug, Clone)]
pub enum OutMessage {
    GoLive(StreamInfo),
    Offline { duration_secs: u64 },
    Insight(Insight),
    Notice(String),
}

/// Structured analysis returned by the LLM (design s9 output schema).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Insight {
    #[serde(default)]
    pub topics: Vec<String>,
    #[serde(default)]
    pub sentiment: Sentiment,
    #[serde(default)]
    pub key_moments: Vec<KeyMoment>,
    #[serde(default)]
    pub running_summary: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Sentiment {
    #[serde(default)]
    pub mood: String,
    /// -1.0 (negative) .. 1.0 (positive).
    #[serde(default)]
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyMoment {
    #[serde(rename = "t", default)]
    pub timestamp: String,
    #[serde(default)]
    pub note: String,
}
