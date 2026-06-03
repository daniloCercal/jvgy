# Deploy guide — low-RAM AWS VPS

The bot is **stateless** (no database, no media on disk). The only real
constraint on a small instance is **RAM**. Two ways to run it: Docker
(recommended) or a bare binary under `systemd`.

## 1. Pick an instance (ARM/Graviton = cheaper)

| Instance | RAM | Fit | Notes |
|----------|-----|-----|-------|
| `t4g.micro` | 1 GB | Comfortable | recommended default |
| `t4g.nano`  | 512 MB | Tight but OK | use `MEDIA_MODE=ffmpeg-direct` **and** add swap |

`t4g.*` are ARM64 (Graviton). Building **on the instance** compiles native
`aarch64` automatically — no cross-compile needed.

### Add swap on a nano (cheap safety net)

```sh
sudo fallocate -l 1G /swapfile && sudo chmod 600 /swapfile
sudo mkswap /swapfile && sudo swapon /swapfile
echo '/swapfile none swap sw 0 0' | sudo tee -a /etc/fstab
```

### Footprint knobs

- `MEDIA_MODE=ffmpeg-direct` — drops streamlink's ~50 MB Python RSS (resolves the
  HLS URL in-process). Best for a nano.
- `VAD_ENABLED=true` — skips silence, cutting STT cost/CPU.
- Active RAM target: bot ≤ 100 MB; with `ffmpeg-direct` total ≈ 120–140 MB.

## 2. Configuration

Create `/etc/summarizer.env` from `.env.example` and fill it in. Minimum:

```
TWITCH_CHANNEL=yourchannel
TWITCH_CLIENT_ID=...
TWITCH_CLIENT_SECRET=...
STT_PROVIDER=mimo
STT_API_KEY=...
LLM_API_KEY=...
DISCORD_WEBHOOK_URL=...
MEDIA_MODE=ffmpeg-direct
LOG_FORMAT=json
```

Secrets are read only from env and never logged. Lock the file down:
`sudo chmod 600 /etc/summarizer.env`.

## 3a. Run with Docker (recommended)

```sh
# On the t4g instance (native arm64 build):
docker compose up -d --build
docker compose logs -f
```

`docker-compose.yml` sets `restart: unless-stopped`, a `mem_limit`, and log
rotation. To stop gracefully (drains + posts a final summary):
`docker compose stop` (sends SIGTERM, waits up to the stop grace period).

> Cross-building from an x86 dev machine instead:
> `docker buildx build --platform linux/arm64 -t twitch-summarizer .`

## 3c. Pull a prebuilt image from GHCR (recommended for a tiny VPS)

`.github/workflows/docker.yml` builds the image on GitHub's runners (free) and
publishes it to `ghcr.io/danilocercal/jvgy`. The VPS then **only pulls** — no
Rust compile, no big RAM/CPU spike.

1. Push to GitHub → wait for the **Actions** run (Actions tab) to finish.
2. Make the image pullable from the VPS — either:
   - **Package public** (simplest; the image holds no secrets — config is via
     `.env` at runtime): GitHub → your profile → Packages → `jvgy` → Package
     settings → Change visibility → Public; **or**
   - **`docker login ghcr.io`** on the VPS with a PAT that has `read:packages`.
3. Deploy with the prebuilt image:
   ```sh
   docker compose -f docker-compose.prod.yml up -d
   docker compose -f docker-compose.prod.yml logs -f
   ```
   Update later with: `docker compose -f docker-compose.prod.yml pull && \
   docker compose -f docker-compose.prod.yml up -d`.

## 3b. Run as a bare binary under systemd

```sh
# Build (on the instance, or cross-compile to aarch64):
cargo build --release
sudo install -m 0755 target/release/twitch-discord-summarizer /usr/local/bin/summarizer
sudo apt-get install -y ffmpeg streamlink   # streamlink only if MEDIA_MODE=streamlink
sudo useradd -r -s /usr/sbin/nologin summarizer || true

sudo cp deploy/summarizer.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now summarizer
journalctl -u summarizer -f
```

The unit sets `Restart=always`, `MemoryMax=` (OOM guard), `KillSignal=SIGTERM`
and a stop timeout so the bot drains cleanly.

## 4. Operating notes

- **Graceful shutdown:** SIGTERM → stop ingestion → flush a final summary → kill
  subprocesses → exit within `SHUTDOWN_TIMEOUT_SECS`.
- **Kill-switch:** `DAILY_SPEND_CEILING_USD` stops STT/LLM and posts a notice when
  the daily cap is hit; resets at UTC midnight.
- **Egress:** continuous audio upload to STT ≈ 0.8 GB per 8h stream — negligible
  AWS cost.
- **Logs:** JSON to stdout → captured by Docker/journald. Keep rotation on
  (compose `max-size`, or journald defaults) so disk never fills.
- **Updates:** `git pull && docker compose up -d --build` (or rebuild + restart
  the systemd unit).
