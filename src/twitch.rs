//! Twitch integration: anonymous chat ingestion, Helix client, and the
//! stream-lifecycle (online/offline detection + notifications) worker.

pub mod chat;
pub mod helix;
pub mod lifecycle;
