//! Twitch -> Discord stream summarizer — library crate.
//!
//! Exposed so integration tests (and the thin `main` binary) can use the same
//! modules. See `main.rs` for how the workers are wired together.

pub mod aggregate;
pub mod analysis;
pub mod audio;
pub mod backoff;
pub mod config;
pub mod cost;
pub mod discord;
pub mod interaction;
pub mod logging;
pub mod shutdown;
pub mod storage;
pub mod stt;
pub mod supervisor;
pub mod twitch;
pub mod types;
pub mod util;
