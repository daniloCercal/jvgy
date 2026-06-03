# Twitch → Discord Stream Summarizer

Production-grade bot (**one stream per process**) that watches a Twitch stream's
chat and audio, transcribes audio when captions aren't available, aggregates
chat + transcript into a **bounded** rolling window, periodically (and on demand)
asks an LLM for insights, and posts them to a Discord channel. Built for a tiny
RAM footprint (idle ≤ 40 MB, active ≤ 100 MB for the bot process).

Stack: **Rust + tokio**. STT is pluggable (MiMo-V2.5-Pro audio / OpenAI / Deepgram);
analysis defaults to **MiMo 2.5 Pro** via an OpenAI-compatible endpoint.

## What it does

- **Chat** — anonymous Twitch IRC read (no token needed).
- **Online/offline** — Helix polling with debounce; posts 🔴 go-live / ⚫ offline
  notices to Discord.
- **Audio → STT** — `streamlink | ffmpeg` → 16 kHz mono PCM → energy VAD (skips
  silence) → bounded chunk channel (drops under backpressure) → STT.
- **Summaries** — bounded window (time + token capped) → incremental LLM call on
  interval / token threshold / `!summary` (or `!resumo`) in chat / when the stream
  ends → rich Discord embeds (topics, sentiment, key moments, running summary).
- **Cost control** — VAD gating, incremental prompts, and a daily spend ceiling
  with a kill-switch.

## Requirements

- **Rust** stable (edition 2021, ≥ 1.73).
- Runtime: **`ffmpeg`** and **`streamlink`** on `PATH` (only for live capture /
  replay; not needed to build or run the tests). Both are installed in the Docker
  image.

## Configure

Copy `.env.example` → `.env` and fill it in. Only `TWITCH_CHANNEL` is required to
boot. Key variables:

| Variable | Purpose |
|----------|---------|
| `TWITCH_CHANNEL` | Channel login **or** full URL (normalized) |
| `TWITCH_CLIENT_ID` / `TWITCH_CLIENT_SECRET` | App creds for online/offline detection ([dev.twitch.tv/console](https://dev.twitch.tv/console)). Chat needs none. |
| `STT_PROVIDER` | `mimo` (default) · `openai` · `deepgram` |
| `STT_BASE_URL` / `STT_API_KEY` / `STT_MODEL` | STT endpoint + key |
| `LLM_BASE_URL` / `LLM_API_KEY` / `LLM_MODEL` | Summary LLM (default MiMo) |
| `DISCORD_WEBHOOK_URL` | Where insights/notices go |
| `SUMMARY_INTERVAL_SECS` / `WINDOW_MAX_TOKENS` | Summary cadence + window budget |
| `DAILY_SPEND_CEILING_USD` | Kill-switch threshold |
| `AUDIO_REPLAY_FILE` | Replay a local recording instead of the live stream |

Secrets are read from env only and never logged. See `.env.example` for the full
list and defaults.

## Build, run, test

```sh
cargo build --release
cargo run                 # reads .env / environment
cargo test                # unit + integration (mocked STT/LLM, no network/keys)
```

`Ctrl-C` (or `SIGTERM` in a container) triggers a graceful drain: stop ingestion →
final summary → kill subprocesses → exit within `SHUTDOWN_TIMEOUT_SECS`.

### Replay a recorded sample (no live channel)

Point the audio pipeline at a local file to exercise the audio → STT path without
Twitch:

```sh
AUDIO_REPLAY_FILE=./sample.mp3 STT_PROVIDER=openai STT_API_KEY=... cargo run
```

## Docker

```sh
docker build -t twitch-summarizer .
docker run --rm --env-file .env twitch-summarizer
```

Multi-stage build → slim `debian:bookworm` runtime with `ffmpeg` + `streamlink`,
running as a non-root user. The bot uses bundled rustls roots (no system OpenSSL).

## Logging & observability

Structured logs via `tracing`: JSON by default (`LOG_FORMAT=json`), pretty text
for local dev (`LOG_FORMAT=text`). Level via `RUST_LOG` (preferred) or `LOG_LEVEL`.

## Architecture

See the approved design document for the full architecture, concurrency/memory
model, failure-mode matrix, and cost model. In short: a **supervisor** runs each
worker with restart-on-failure backoff; producers (chat, lifecycle, audio) feed
**bounded channels**; consumers (aggregator, output) own their receivers and drain
on shutdown.
