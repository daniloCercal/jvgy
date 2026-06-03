//! Worker supervision. Each supervised worker runs in its own task and is
//! restarted with exponential backoff + jitter on error OR panic, until shutdown
//! is requested. A panic in one worker never brings down the process (design s5).

use crate::backoff::Backoff;
use std::future::Future;
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// A worker that has run at least this long before exiting is considered healthy,
/// so its restart backoff is reset.
const HEALTHY_RUNTIME: Duration = Duration::from_secs(60);

pub struct Supervisor {
    shutdown: CancellationToken,
    handles: Vec<(&'static str, JoinHandle<()>)>,
}

impl Supervisor {
    pub fn new(shutdown: CancellationToken) -> Self {
        Self {
            shutdown,
            handles: Vec::new(),
        }
    }

    /// Spawn `name`'s supervision loop. `factory` builds a fresh worker future
    /// each restart; it receives the shared shutdown token.
    pub fn supervise<F, Fut>(&mut self, name: &'static str, factory: F)
    where
        F: FnMut(CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        let shutdown = self.shutdown.clone();
        let handle = tokio::spawn(async move {
            let mut factory = factory;
            let mut backoff = Backoff::default();

            while !shutdown.is_cancelled() {
                let started = Instant::now();

                // Run the worker in its own task so a panic surfaces as a
                // JoinError instead of unwinding the supervision loop.
                let worker = tokio::spawn(factory(shutdown.clone()));
                match worker.await {
                    Ok(Ok(())) => info!(worker = name, "worker exited cleanly"),
                    Ok(Err(e)) => warn!(worker = name, error = %e, "worker returned error"),
                    Err(join) if join.is_panic() => {
                        warn!(worker = name, "worker panicked; will restart")
                    }
                    Err(_) => break, // task was cancelled/aborted
                }

                if shutdown.is_cancelled() {
                    break;
                }
                if started.elapsed() >= HEALTHY_RUNTIME {
                    backoff.reset();
                }

                let delay = backoff.next_delay();
                warn!(
                    worker = name,
                    delay_ms = delay.as_millis() as u64,
                    "restarting worker after backoff"
                );
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    _ = tokio::time::sleep(delay) => {}
                }
            }
            info!(worker = name, "supervision loop ended");
        });

        self.handles.push((name, handle));
    }

    /// Spawn a long-lived task that owns non-cloneable state (e.g. a channel
    /// receiver) and is NOT restarted — it must handle its own resilience and
    /// exit on shutdown. Its handle is tracked for draining.
    pub fn spawn<Fut>(&mut self, name: &'static str, fut: Fut)
    where
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        self.handles.push((name, tokio::spawn(fut)));
    }

    /// Cancel all workers and wait for their supervision loops to finish, bounded
    /// by `timeout`. Workers that don't drain in time are abandoned (logged).
    pub async fn shutdown_and_join(self, timeout: Duration) {
        self.shutdown.cancel();
        for (name, handle) in self.handles {
            match tokio::time::timeout(timeout, handle).await {
                Ok(Ok(())) => info!(worker = name, "drained"),
                Ok(Err(e)) => warn!(worker = name, error = %e, "join error during drain"),
                Err(_) => warn!(worker = name, "drain timed out; abandoning"),
            }
        }
    }
}
