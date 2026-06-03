//! Anonymous Twitch chat ingestion. Reads PRIVMSGs and forwards them as
//! `ChatEvent`s on a bounded channel (drop-newest on overflow = load shedding).
//! `twitch-irc` handles reconnects internally; if the stream ends we return and
//! the supervisor restarts us with backoff.

use crate::types::ChatEvent;
use anyhow::Result;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use twitch_irc::login::StaticLoginCredentials;
use twitch_irc::message::ServerMessage;
use twitch_irc::{ClientConfig, SecureTCPTransport, TwitchIRCClient};

pub async fn run(
    channel: String,
    tx: mpsc::Sender<ChatEvent>,
    shutdown: CancellationToken,
) -> Result<()> {
    let config = ClientConfig::default(); // anonymous (justinfan)
    let (mut incoming, client) =
        TwitchIRCClient::<SecureTCPTransport, StaticLoginCredentials>::new(config);
    client.join(channel.clone())?;
    info!(channel = %channel, "joined twitch chat (anonymous)");

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                info!("chat shutting down");
                break;
            }
            msg = incoming.recv() => {
                match msg {
                    Some(ServerMessage::Privmsg(m)) => {
                        let ev = ChatEvent { user: m.sender.name, text: m.message_text };
                        if tx.try_send(ev).is_err() {
                            debug!("chat channel full/closed; dropping message");
                        }
                    }
                    Some(_) => {}
                    None => {
                        warn!("twitch chat stream ended");
                        break;
                    }
                }
            }
        }
    }
    drop(client);
    Ok(())
}
