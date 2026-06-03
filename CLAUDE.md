# CLAUDE.md — Twitch → Discord Stream Summarizer

## Project
Production-grade bot (one stream per process) that monitors a Twitch stream
(chat + audio/CC), transcribes audio only when captions are unavailable,
aggregates chat + transcript into a bounded rolling window, sends it to an LLM
periodically / on demand, and posts insights (topics, sentiment, key moments,
running summary) to a Discord channel. Target: tiny RAM footprint, runs in a
small container.

## Working agreement
- **Phase discipline.** Phase 1 = architecture/design ONLY — no implementation
  code. Produce the design doc, then STOP and wait for my explicit approval
  before Phase 2.
- **Surface assumptions.** Never silently invent provider behavior, API limits,
  or Twitch capabilities. If unsure, say so and propose how to verify.
- **Trade-offs, not edicts.** Prefer options + a recommendation over a single
  unexplained choice.
- **Ask before deviating** from this file or the approved design.

## Hard invariants (must hold in any design/implementation)
- No media on disk — in-memory bounded buffers + stdin/stdout pipes only.
- Every queue/channel/buffer has a fixed capacity and an explicit overflow
  policy. RAM safety = bounded buffers + backpressure; shed load, never grow
  memory to keep up.
- Memory budget: idle RSS ≤ 40 MB, active ≤ 100 MB, no unbounded growth over 6h.
- Production, not MVP: structured logging, graceful shutdown (drain + final
  flush), reconnect/respawn with backoff, retries + circuit breaker, secrets
  never logged.

## Notes
- Audio → STT is the PRIMARY path. Twitch rarely exposes real WebVTT captions;
  treat CC as a rare best-effort optimization, detected at runtime.
- Language (Rust vs Go) is decided in Phase 1, justified against the RAM
  constraint. Do NOT scaffold the project until the design is approved.
- Full task brief: docs/BRIEF.md (or pasted in chat).
