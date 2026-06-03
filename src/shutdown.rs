//! Graceful-shutdown plumbing. A single `CancellationToken` is shared across all
//! workers; OS signals (SIGINT/SIGTERM on Unix, Ctrl-C/Ctrl-Break on Windows)
//! trip it. Workers select on `cancelled()` and unwind their own drain logic.

use tokio_util::sync::CancellationToken;
use tracing::info;

#[derive(Clone)]
pub struct ShutdownController {
    token: CancellationToken,
}

impl ShutdownController {
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
        }
    }

    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    /// Spawn a background task that cancels the token on the first OS signal.
    pub fn spawn_signal_listener(&self) {
        let token = self.token.clone();
        tokio::spawn(async move {
            wait_for_signal().await;
            info!("received shutdown signal");
            token.cancel();
        });
    }
}

impl Default for ShutdownController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(unix)]
async fn wait_for_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    let mut int = signal(SignalKind::interrupt()).expect("install SIGINT handler");
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
}

#[cfg(windows)]
async fn wait_for_signal() {
    use tokio::signal::windows::{ctrl_break, ctrl_c};
    let mut cc = ctrl_c().expect("install Ctrl-C handler");
    let mut cb = ctrl_break().expect("install Ctrl-Break handler");
    tokio::select! {
        _ = cc.recv() => {}
        _ = cb.recv() => {}
    }
}
