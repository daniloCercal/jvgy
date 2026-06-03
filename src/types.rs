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

/// A single chat message, normalized from Twitch IRC (with identity/badges).
#[derive(Debug, Clone)]
pub struct ChatEvent {
    /// Display name.
    pub user: String,
    /// Lowercase login.
    pub login: String,
    pub user_id: String,
    /// IRC message id (for threaded replies).
    pub message_id: String,
    pub text: String,
    pub sub_months: u32,
    pub is_mod: bool,
    pub is_vip: bool,
    pub is_sub: bool,
    pub is_founder: bool,
    pub is_broadcaster: bool,
}

/// A message the bot wants to send to chat. `reply_to` = a message id to
/// thread-reply to (Twitch `reply-parent-msg-id`); `None` = a normal message.
#[derive(Debug, Clone)]
pub struct OutboundChat {
    pub text: String,
    pub reply_to: Option<String>,
}

/// The streamer's current emotional state, inferred from her transcribed speech.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamerMood {
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub intensity: f32,
}

impl Default for StreamerMood {
    fn default() -> Self {
        Self {
            label: "neutra".into(),
            intensity: 0.0,
        }
    }
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
    /// The streamer's mood, inferred from her speech (drives chat interaction tone).
    #[serde(default)]
    pub streamer_mood: StreamerMood,
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
