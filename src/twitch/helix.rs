//! Minimal Twitch Helix client: app access token (client-credentials) + stream
//! status. Used only for online/offline detection; chat needs no auth.

use crate::types::StreamInfo;
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub struct HelixClient {
    http: reqwest::Client,
    client_id: String,
    client_secret: String,
    token: Option<(String, Instant)>,
}

#[derive(Deserialize)]
struct TokenResp {
    access_token: String,
    expires_in: u64,
}

#[derive(Deserialize)]
struct StreamsResp {
    data: Vec<StreamData>,
}

#[derive(Deserialize)]
struct StreamData {
    title: Option<String>,
    game_name: Option<String>,
    started_at: Option<String>,
    viewer_count: Option<u64>,
}

impl HelixClient {
    pub fn new(http: reqwest::Client, client_id: String, client_secret: String) -> Self {
        Self {
            http,
            client_id,
            client_secret,
            token: None,
        }
    }

    async fn ensure_token(&mut self) -> Result<String> {
        if let Some((tok, exp)) = &self.token {
            if Instant::now() < *exp {
                return Ok(tok.clone());
            }
        }
        let resp = self
            .http
            .post("https://id.twitch.tv/oauth2/token")
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("grant_type", "client_credentials"),
            ])
            .send()
            .await
            .context("requesting app token")?
            .error_for_status()
            .context("app token http status")?
            .json::<TokenResp>()
            .await
            .context("decoding app token")?;
        let exp = Instant::now() + Duration::from_secs(resp.expires_in.saturating_sub(60));
        self.token = Some((resp.access_token.clone(), exp));
        Ok(resp.access_token)
    }

    /// `Some(info)` if live, `None` if offline.
    pub async fn get_stream(&mut self, login: &str) -> Result<Option<StreamInfo>> {
        let token = self.ensure_token().await?;
        let resp = self
            .http
            .get("https://api.twitch.tv/helix/streams")
            .query(&[("user_login", login)])
            .header("Client-Id", &self.client_id)
            .bearer_auth(&token)
            .send()
            .await
            .context("helix streams request")?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            self.token = None; // force refresh next poll
            return Err(anyhow!("helix unauthorized; token cleared"));
        }
        let body = resp
            .error_for_status()
            .context("helix streams status")?
            .json::<StreamsResp>()
            .await
            .context("decoding streams")?;
        Ok(body.data.into_iter().next().map(|d| StreamInfo {
            title: d.title.unwrap_or_default(),
            game: d.game_name.unwrap_or_default(),
            started_at: d.started_at.unwrap_or_default(),
            viewers: d.viewer_count.unwrap_or(0),
        }))
    }

    /// Fetch account creation timestamps for up to 100 user ids (for "tempo de
    /// casa"/account-age). Returns a map id -> created_at.
    pub async fn get_users_created_at(
        &mut self,
        ids: &[String],
    ) -> Result<HashMap<String, DateTime<Utc>>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let token = self.ensure_token().await?;
        let mut req = self
            .http
            .get("https://api.twitch.tv/helix/users")
            .header("Client-Id", &self.client_id)
            .bearer_auth(&token);
        for id in ids.iter().take(100) {
            req = req.query(&[("id", id)]);
        }
        let resp = req
            .send()
            .await
            .context("helix users request")?
            .error_for_status()
            .context("helix users status")?
            .json::<UsersResp>()
            .await
            .context("helix users decode")?;
        let mut out = HashMap::new();
        for u in resp.data {
            if let Ok(dt) = DateTime::parse_from_rfc3339(&u.created_at) {
                out.insert(u.id, dt.with_timezone(&Utc));
            }
        }
        Ok(out)
    }
}

#[derive(Deserialize)]
struct UsersResp {
    data: Vec<UserData>,
}
#[derive(Deserialize)]
struct UserData {
    id: String,
    created_at: String,
}
