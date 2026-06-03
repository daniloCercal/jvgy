//! Twitch chat ingestion + sending.
//!
//! - Reads PRIVMSGs, enriches them with identity/badges, and fans them out on a
//!   bounded `broadcast` channel (aggregator + interaction engine subscribe).
//! - If bot credentials are configured, authenticates via OAuth (auto-refreshing
//!   token) and sends replies/messages from a bounded outbound channel; otherwise
//!   stays anonymous read-only (no sending).

use crate::types::{ChatEvent, OutboundChat, StoreRecord};
use anyhow::Result;
use chrono::{Duration as ChronoDuration, Utc};
use std::convert::Infallible;
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use twitch_irc::login::{
    LoginCredentials, RefreshingLoginCredentials, StaticLoginCredentials, TokenStorage,
    UserAccessToken,
};
use twitch_irc::message::{ReplyToMessage, ServerMessage};
use twitch_irc::{ClientConfig, SecureTCPTransport, TwitchIRCClient};

pub struct ChatCfg {
    pub channel: String,
    pub bot_username: Option<String>,
    pub bot_refresh_token: Option<String>,
    pub client_id: String,
    pub client_secret: String,
}

/// Minimal `ReplyToMessage` so we can thread-reply by (channel, message id).
struct ReplyTarget {
    channel: String,
    msg_id: String,
}
impl ReplyToMessage for ReplyTarget {
    fn channel_login(&self) -> &str {
        &self.channel
    }
    fn message_id(&self) -> &str {
        &self.msg_id
    }
}

/// In-memory token storage seeded with the refresh token. Seeded as "expired" so
/// the first `get_credentials` performs a refresh to obtain a live access token.
/// No disk persistence (fits the ephemeral design); re-seeds from env on restart.
#[derive(Debug)]
struct MemTokenStorage {
    token: UserAccessToken,
}

impl MemTokenStorage {
    fn seeded(refresh_token: String) -> Self {
        let created_at = Utc::now() - ChronoDuration::seconds(100);
        Self {
            token: UserAccessToken {
                access_token: String::new(),
                refresh_token,
                created_at,
                // already expired -> forces a refresh on first use
                expires_at: Some(created_at + ChronoDuration::seconds(50)),
            },
        }
    }
}

#[async_trait::async_trait]
impl TokenStorage for MemTokenStorage {
    type LoadError = Infallible;
    type UpdateError = Infallible;

    async fn load_token(&mut self) -> Result<UserAccessToken, Infallible> {
        Ok(self.token.clone())
    }
    async fn update_token(&mut self, token: &UserAccessToken) -> Result<(), Infallible> {
        self.token = token.clone();
        Ok(())
    }
}

pub async fn run(
    cfg: ChatCfg,
    bcast_tx: broadcast::Sender<ChatEvent>,
    outbound_rx: mpsc::Receiver<OutboundChat>,
    store_tx: Option<mpsc::Sender<StoreRecord>>,
    shutdown: CancellationToken,
) -> Result<()> {
    let can_send = cfg.bot_username.is_some() && cfg.bot_refresh_token.is_some();

    if let (Some(username), Some(refresh)) = (cfg.bot_username.clone(), cfg.bot_refresh_token.clone())
    {
        let creds = RefreshingLoginCredentials::init_with_username(
            Some(username),
            cfg.client_id.clone(),
            cfg.client_secret.clone(),
            MemTokenStorage::seeded(refresh),
        );
        let config = ClientConfig::new_simple(creds);
        let (incoming, client) =
            TwitchIRCClient::<SecureTCPTransport, RefreshingLoginCredentials<MemTokenStorage>>::new(
                config,
            );
        client.join(cfg.channel.clone())?;
        info!(channel = %cfg.channel, "joined twitch chat (authenticated bot)");
        event_loop(cfg.channel, true, incoming, client, bcast_tx, outbound_rx, store_tx, shutdown).await
    } else {
        let creds = StaticLoginCredentials::anonymous();
        let config = ClientConfig::new_simple(creds);
        let (incoming, client) =
            TwitchIRCClient::<SecureTCPTransport, StaticLoginCredentials>::new(config);
        client.join(cfg.channel.clone())?;
        info!(channel = %cfg.channel, "joined twitch chat (anonymous, read-only)");
        let _ = can_send;
        event_loop(cfg.channel, false, incoming, client, bcast_tx, outbound_rx, store_tx, shutdown).await
    }
}

async fn event_loop<L>(
    channel: String,
    can_send: bool,
    mut incoming: mpsc::UnboundedReceiver<ServerMessage>,
    client: TwitchIRCClient<SecureTCPTransport, L>,
    bcast_tx: broadcast::Sender<ChatEvent>,
    mut outbound_rx: mpsc::Receiver<OutboundChat>,
    store_tx: Option<mpsc::Sender<StoreRecord>>,
    shutdown: CancellationToken,
) -> Result<()>
where
    L: LoginCredentials,
{
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                info!("chat shutting down");
                break;
            }
            msg = incoming.recv() => {
                match msg {
                    Some(ServerMessage::Privmsg(m)) => {
                        let ev = to_event(m);
                        if let Some(s) = &store_tx {
                            let _ = s.try_send(StoreRecord::Chat(ev.clone()));
                        }
                        // broadcast send fails only if there are no receivers; ignore.
                        let _ = bcast_tx.send(ev);
                    }
                    Some(_) => {}
                    None => { warn!("twitch chat stream ended"); break; }
                }
            }
            out = outbound_rx.recv() => {
                match out {
                    Some(msg) if can_send => send(&client, &channel, msg).await,
                    Some(_) => debug!("outbound message dropped (sending disabled)"),
                    None => { /* outbound closed; keep reading chat */ }
                }
            }
        }
    }
    drop(client);
    Ok(())
}

async fn send<L: LoginCredentials>(
    client: &TwitchIRCClient<SecureTCPTransport, L>,
    channel: &str,
    msg: OutboundChat,
) {
    let result = match &msg.reply_to {
        Some(id) => {
            let target = ReplyTarget {
                channel: channel.to_string(),
                msg_id: id.clone(),
            };
            client.say_in_reply_to(&target, msg.text).await
        }
        None => client.say(channel.to_string(), msg.text).await,
    };
    if let Err(e) = result {
        warn!(error = %e, "failed to send chat message");
    }
}

fn to_event(m: twitch_irc::message::PrivmsgMessage) -> ChatEvent {
    let has = |name: &str| m.badges.iter().any(|b| b.name == name);
    let sub_months = m
        .badge_info
        .iter()
        .find(|b| b.name == "subscriber" || b.name == "founder")
        .and_then(|b| b.version.parse::<u32>().ok())
        .unwrap_or(0);
    let (is_mod, is_vip, is_sub, is_founder, is_broadcaster) = (
        has("moderator"),
        has("vip"),
        has("subscriber") || has("founder"),
        has("founder"),
        has("broadcaster"),
    );
    ChatEvent {
        user: m.sender.name,
        login: m.sender.login,
        user_id: m.sender.id,
        message_id: m.message_id,
        text: m.message_text,
        sub_months,
        is_mod,
        is_vip,
        is_sub,
        is_founder,
        is_broadcaster,
    }
}
