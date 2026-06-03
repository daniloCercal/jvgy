//! Audio acquisition + transcription pipeline: streamlink -> ffmpeg -> PCM ->
//! VAD gate -> bounded chunk channel -> STT. All in-memory, no disk.

pub mod pipeline;
pub mod vad;
pub mod wav;
