//! Runtime configuration, loaded entirely from environment variables (12-factor)
//! with an optional local `.env`. All tunables live here so nothing requires a
//! recompile to change. Secrets are read here but never logged.

use anyhow::{anyhow, Result};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Json,
    Text,
}

#[derive(Debug, Clone)]
pub struct Config {
    // --- Twitch ---
    pub twitch_channel: String,
    pub twitch_client_id: String,
    pub twitch_client_secret: String,
    pub stream_poll: Duration,
    pub stream_confirm_polls: u32,
    pub notify_online: bool,
    pub notify_offline: bool,

    // --- Aggregation / summary ---
    pub summary_interval: Duration,
    pub window_max_secs: Duration,
    pub window_max_tokens: usize,

    // --- STT (transcription) ---
    pub stt_provider: String,
    pub stt_base_url: String,
    pub stt_api_key: String,
    pub stt_model: String,
    pub stt_chunk_secs: u32,
    pub vad_enabled: bool,
    pub vad_threshold: f64,
    /// When set, the audio pipeline transcodes this local file (looped) instead
    /// of pulling the live stream — for replaying a recorded sample in tests.
    pub audio_replay_file: Option<String>,
    /// `streamlink` (default, robust) or `ffmpeg-direct` (resolve the HLS URL in
    /// Rust and let ffmpeg read it — drops streamlink's ~50 MB Python RSS).
    pub media_mode: String,

    // --- LLM (analysis) ---
    pub llm_base_url: String,
    pub llm_api_key: String,
    pub llm_model: String,

    // --- Discord ---
    pub discord_webhook_url: String,

    // --- Cost control ---
    pub daily_spend_ceiling_usd: f64,

    // --- Interactive chat persona (Phase 3) ---
    /// Bot account login for SENDING in chat. `None` = anonymous read-only.
    pub twitch_bot_username: Option<String>,
    /// OAuth refresh token for the bot account (minted once; auto-refreshed).
    pub twitch_bot_refresh_token: Option<String>,
    pub chat_rate_per_30s: u32,
    pub proactive_min_interval: Duration,
    pub proactive_probability: f64,
    pub persona_file: String,
    pub easter_egg_user: String,
    pub easter_egg_chance: f64,
    pub easter_egg_text: String,

    // --- Ops ---
    pub log_level: String,
    pub log_format: LogFormat,
    pub shutdown_timeout: Duration,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        load_dotenv();

        let twitch_channel = normalize_channel(&env_required("TWITCH_CHANNEL")?);
        if twitch_channel.is_empty() {
            return Err(anyhow!(
                "TWITCH_CHANNEL normalized to an empty channel login"
            ));
        }

        Ok(Self {
            twitch_channel,
            twitch_client_id: env_or("TWITCH_CLIENT_ID", ""),
            twitch_client_secret: env_or("TWITCH_CLIENT_SECRET", ""),
            stream_poll: env_duration_secs("STREAM_POLL_SECS", 60)?,
            stream_confirm_polls: env_parse("STREAM_CONFIRM_POLLS", 2u32)?,
            notify_online: env_bool("NOTIFY_ONLINE", true)?,
            notify_offline: env_bool("NOTIFY_OFFLINE", true)?,
            summary_interval: env_duration_secs("SUMMARY_INTERVAL_SECS", 300)?,
            window_max_secs: env_duration_secs("WINDOW_MAX_SECS", 600)?,
            window_max_tokens: env_parse("WINDOW_MAX_TOKENS", 6000usize)?,
            stt_provider: env_or("STT_PROVIDER", "mimo").to_lowercase(),
            stt_base_url: env_or("STT_BASE_URL", "https://platform.xiaomimimo.com/v1"),
            stt_api_key: env_or("STT_API_KEY", ""),
            stt_model: env_or("STT_MODEL", "mimo-v2.5-pro"),
            stt_chunk_secs: env_parse("STT_CHUNK_SECS", 45u32)?,
            vad_enabled: env_bool("VAD_ENABLED", true)?,
            vad_threshold: env_parse("VAD_THRESHOLD", 0.012f64)?,
            audio_replay_file: {
                let v = env_or("AUDIO_REPLAY_FILE", "");
                if v.trim().is_empty() {
                    None
                } else {
                    Some(v)
                }
            },
            media_mode: env_or("MEDIA_MODE", "streamlink").to_lowercase(),
            llm_base_url: env_or("LLM_BASE_URL", "https://platform.xiaomimimo.com/v1"),
            llm_api_key: env_or("LLM_API_KEY", ""),
            llm_model: env_or("LLM_MODEL", "mimo-v2.5-pro"),
            discord_webhook_url: env_or("DISCORD_WEBHOOK_URL", ""),
            daily_spend_ceiling_usd: env_parse("DAILY_SPEND_CEILING_USD", 5.0f64)?,
            twitch_bot_username: opt(env_or("TWITCH_BOT_USERNAME", "")),
            twitch_bot_refresh_token: opt(env_or("TWITCH_BOT_REFRESH_TOKEN", "")),
            chat_rate_per_30s: env_parse("CHAT_RATE_PER_30S", 18u32)?,
            proactive_min_interval: env_duration_secs("PROACTIVE_MIN_INTERVAL_SECS", 120)?,
            proactive_probability: env_parse("PROACTIVE_PROBABILITY", 0.35f64)?,
            persona_file: env_or("PERSONA_FILE", "./persona.md"),
            easter_egg_user: env_or("EASTER_EGG_USER", "YoRods").to_lowercase(),
            easter_egg_chance: env_parse("EASTER_EGG_CHANCE", 0.005f64)?,
            easter_egg_text: env_or(
                "EASTER_EGG_TEXT",
                "Ó grande Reis dos Reis, me encoberte com seu líquido viscoso!",
            ),
            log_level: env_or("LOG_LEVEL", "info"),
            log_format: match env_or("LOG_FORMAT", "json").to_lowercase().as_str() {
                "text" | "plain" | "pretty" => LogFormat::Text,
                _ => LogFormat::Json,
            },
            shutdown_timeout: env_duration_secs("SHUTDOWN_TIMEOUT_SECS", 15)?,
        })
    }
}

/// Normalize a channel login or a full Twitch URL to the bare lowercase login.
/// `gaules`, `https://twitch.tv/Gaules/`, `twitch.tv/gaules?x=1` -> `gaules`.
pub fn normalize_channel(raw: &str) -> String {
    let mut s = raw.trim();
    for prefix in ["https://", "http://"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest;
        }
    }
    s = s.strip_prefix("www.").unwrap_or(s);
    s = s.strip_prefix("twitch.tv/").unwrap_or(s);
    let s = s.split(['/', '?', '#']).next().unwrap_or(s);
    s.trim().to_lowercase()
}

fn env_required(key: &str) -> Result<String> {
    let v = std::env::var(key).map_err(|_| anyhow!("missing required env var {key}"))?;
    if v.trim().is_empty() {
        return Err(anyhow!("required env var {key} is empty"));
    }
    Ok(v)
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// `None` for empty/whitespace strings, else `Some(trimmed-not-required)`.
fn opt(s: String) -> Option<String> {
    if s.trim().is_empty() {
        None
    } else {
        Some(s)
    }
}

fn env_parse<T>(key: &str, default: T) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match std::env::var(key) {
        Ok(v) => v
            .trim()
            .parse::<T>()
            .map_err(|e| anyhow!("invalid value for {key}: {e}")),
        Err(_) => Ok(default),
    }
}

fn env_duration_secs(key: &str, default_secs: u64) -> Result<Duration> {
    let secs: u64 = env_parse(key, default_secs)?;
    Ok(Duration::from_secs(secs))
}

fn env_bool(key: &str, default: bool) -> Result<bool> {
    match std::env::var(key) {
        Ok(v) => match v.trim().to_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Ok(true),
            "0" | "false" | "no" | "off" => Ok(false),
            other => Err(anyhow!("invalid bool for {key}: {other}")),
        },
        Err(_) => Ok(default),
    }
}

/// Minimal `.env` loader: `KEY=VALUE` lines, `#` comments, no interpolation.
/// Existing process env always wins. Path overridable via `DOTENV_PATH`.
fn load_dotenv() {
    let path = std::env::var("DOTENV_PATH").unwrap_or_else(|_| ".env".to_string());
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return;
    };
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let k = k.trim();
            let v = v.trim().trim_matches('"').trim_matches('\'');
            if std::env::var(k).is_err() {
                std::env::set_var(k, v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_channel;

    #[test]
    fn normalizes_bare_login() {
        assert_eq!(normalize_channel("gaules"), "gaules");
        assert_eq!(normalize_channel("  Gaules "), "gaules");
    }

    #[test]
    fn normalizes_full_urls() {
        assert_eq!(normalize_channel("https://twitch.tv/Gaules"), "gaules");
        assert_eq!(normalize_channel("http://www.twitch.tv/gaules/"), "gaules");
        assert_eq!(normalize_channel("twitch.tv/Gaules/?x=1"), "gaules");
    }
}
