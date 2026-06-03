//! Resolve a Twitch channel's live HLS `audio_only` playlist URL without
//! streamlink — the same reverse-engineered flow streamlink/yt-dlp use:
//!   1. GraphQL `PlaybackAccessToken` (public web Client-ID) -> {value, signature}
//!   2. usher.ttvnw.net master playlist
//!   3. parse out the `audio_only` variant URL
//!
//! NOTE: this uses Twitch's public web Client-ID and the **undocumented** usher
//! endpoint; it can break if Twitch changes them (the trade-off vs. streamlink's
//! maintained robustness). Same ToS grey area as streamlink — operator must hold
//! rights/permission; nothing is persisted.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

const WEB_CLIENT_ID: &str = "kimne78kx3ncx6brgo4mv6wki5h1ko";

#[derive(Deserialize)]
struct GqlResp {
    data: Option<GqlData>,
}
#[derive(Deserialize)]
struct GqlData {
    #[serde(rename = "streamPlaybackAccessToken")]
    token: Option<AccessToken>,
}
#[derive(Deserialize)]
struct AccessToken {
    value: String,
    signature: String,
}

pub async fn resolve_audio_url(http: &reqwest::Client, channel: &str) -> Result<String> {
    let token = fetch_access_token(http, channel).await?;
    let master = fetch_master_playlist(http, channel, &token).await?;
    pick_audio_only(&master).ok_or_else(|| anyhow!("no playable variant in master playlist"))
}

async fn fetch_access_token(http: &reqwest::Client, channel: &str) -> Result<AccessToken> {
    let query = r#"query PlaybackAccessToken($login: String!, $isLive: Boolean!, $vodID: ID!, $isVod: Boolean!, $playerType: String!) { streamPlaybackAccessToken(channelName: $login, params: {platform: "web", playerBackend: "mediaplayer", playerType: $playerType}) @include(if: $isLive) { value signature __typename } videoPlaybackAccessToken(id: $vodID, params: {platform: "web", playerBackend: "mediaplayer", playerType: $playerType}) @include(if: $isVod) { value signature __typename } }"#;
    let body = serde_json::json!({
        "operationName": "PlaybackAccessToken",
        "query": query,
        "variables": {
            "isLive": true,
            "login": channel,
            "isVod": false,
            "vodID": "",
            "playerType": "embed"
        }
    });
    let resp: GqlResp = http
        .post("https://gql.twitch.tv/gql")
        .header("Client-ID", WEB_CLIENT_ID)
        .json(&body)
        .send()
        .await
        .context("gql request")?
        .error_for_status()
        .context("gql status")?
        .json()
        .await
        .context("gql decode")?;
    resp.data
        .and_then(|d| d.token)
        .context("no stream access token (channel offline or invalid)")
}

async fn fetch_master_playlist(
    http: &reqwest::Client,
    channel: &str,
    token: &AccessToken,
) -> Result<String> {
    // usher wants a random player id; nanos is fine (not security-sensitive).
    let p = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(1)
        % 9_000_000
        + 1_000_000)
        .to_string();
    let url = format!("https://usher.ttvnw.net/api/channel/hls/{channel}.m3u8");
    let text = http
        .get(url)
        .query(&[
            ("client_id", WEB_CLIENT_ID),
            ("token", token.value.as_str()),
            ("sig", token.signature.as_str()),
            ("allow_source", "true"),
            ("allow_audio_only", "true"),
            ("type", "any"),
            ("p", p.as_str()),
            ("fast_bread", "true"),
            ("playlist_include_framerate", "true"),
        ])
        .send()
        .await
        .context("usher request")?
        .error_for_status()
        .context("usher status")?
        .text()
        .await
        .context("usher body")?;
    Ok(text)
}

/// Parse a master playlist and return the `audio_only` media-playlist URL,
/// falling back to the last variant URL if no audio-only rendition exists.
fn pick_audio_only(master: &str) -> Option<String> {
    let lines: Vec<&str> = master.lines().collect();
    let mut last_url: Option<String> = None;
    let mut i = 0;
    while i < lines.len() {
        if lines[i].starts_with("#EXT-X-STREAM-INF") {
            let inf = lines[i];
            // The URL is the next non-comment, non-empty line.
            let mut j = i + 1;
            while j < lines.len() && (lines[j].starts_with('#') || lines[j].trim().is_empty()) {
                j += 1;
            }
            if j < lines.len() {
                let url = lines[j].trim().to_string();
                if inf.to_lowercase().contains("audio_only") {
                    return Some(url);
                }
                last_url = Some(url);
                i = j;
            }
        }
        i += 1;
    }
    last_url
}

#[cfg(test)]
mod tests {
    use super::pick_audio_only;

    #[test]
    fn picks_audio_only_variant() {
        let master = "#EXTM3U\n\
#EXT-X-MEDIA:TYPE=VIDEO,GROUP-ID=\"chunked\",NAME=\"1080p\"\n\
#EXT-X-STREAM-INF:BANDWIDTH=6000000,VIDEO=\"chunked\"\n\
https://cdn.example/1080p.m3u8\n\
#EXT-X-MEDIA:TYPE=VIDEO,GROUP-ID=\"audio_only\",NAME=\"Audio Only\"\n\
#EXT-X-STREAM-INF:BANDWIDTH=160000,VIDEO=\"audio_only\"\n\
https://cdn.example/audio.m3u8\n";
        assert_eq!(
            pick_audio_only(master).as_deref(),
            Some("https://cdn.example/audio.m3u8")
        );
    }

    #[test]
    fn falls_back_to_last_variant_without_audio_only() {
        let master = "#EXTM3U\n\
#EXT-X-STREAM-INF:BANDWIDTH=6000000,VIDEO=\"chunked\"\n\
https://cdn.example/1080p.m3u8\n\
#EXT-X-STREAM-INF:BANDWIDTH=3000000,VIDEO=\"720p\"\n\
https://cdn.example/720p.m3u8\n";
        assert_eq!(
            pick_audio_only(master).as_deref(),
            Some("https://cdn.example/720p.m3u8")
        );
    }
}
