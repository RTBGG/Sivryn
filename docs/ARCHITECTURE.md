# Architecture

Sivryn exists because screen sharing on mainstream platforms looks bad. Every decision below
puts screen sharing quality first, then easy self-hosting, then everything else.

```
 Desktop client (Rust)                             Server (Docker Compose)
┌──────────────────────────────┐             ┌──────────────────────────────────┐
│ Dioxus Native UI (Vello/wgpu)│   HTTPS     │ Caddy (TLS, one domain)          │
│  └ video → wgpu texture      │────────────▶│  ├ /api, /gateway → sivryn-server│
│ client-core (REST, gateway)  │  WebSocket  │  └ /rtc, /twirp   → LiveKit       │
│ crypto (OpenMLS)             │             │ sivryn-server (axum)             │
│ media                        │             │  ├ PostgreSQL                    │
│  ├ capture (libwebrtc)       │  WebRTC     │  └ event bus (in-process / NATS) │
│  ├ HW encode AV1 / H.264     │════════════▶│ LiveKit SFU (UDP 7882, TCP 7881) │
│  └ LiveKit Rust SDK          │  (SRTP+E2EE)│                                  │
└──────────────────────────────┘             └──────────────────────────────────┘
```

## Decisions

| Area | Choice | Why | Trade-off |
|---|---|---|---|
| SFU | **LiveKit** | Mature, no transcoding, simulcast/SVC, AV1, Rust SDK, single Docker image | Written in Go and runs as its own service |
| Client | **Native Rust with native media** | Full control over capture and encoder (resolution, fps, bitrate, codec, degradation), low RAM | More UI work than a web stack |
| UI toolkit | **Dioxus Native (Blitz)** | GPU rendering without a webview, video as a wgpu texture, the same RSX later for web and mobile | Still experimental. Video rendering sits behind `VideoSurface` so it can be swapped |
| First platform | **Windows** | Where the founding users are | `DesktopCapturer` from libwebrtc already covers macOS and Linux |
| Codecs | **Adaptive AV1 → H.264** | AV1 needs 30–50 % less bitrate for the same quality and has screen content tools. H.264 is decodable everywhere | Detection and fallback logic |
| Backend | **axum + tokio** | De-facto standard, tower middleware | none |
| Database | **PostgreSQL + sqlx** | Robust, full-text search later, migrations embedded in the binary | One more container |
| Realtime protocol | **JSON over WebSocket** | Debuggable, any language can build bots and clients | Larger than binary formats, which doesn't matter for chat traffic |
| Scaling | **In-process bus, optional NATS** | Small instances need no extra service | Abstraction from day one |
| Auth | **Local accounts + optional OIDC** | Works out of the box, and homelabs can use SSO | Two login paths |
| Model | **Discord-like, standalone instances** | Familiar and simple | No federation |
| E2EE | **MLS for text and media** | The server never sees content, and keys rotate on membership changes | No server-side search; devices must be added to groups |
| Deployment | **Docker Compose + Caddy** | Automatic HTTPS, one domain, `setup.sh` generates secrets | Ports 80/443 must be free (otherwise disable the Caddy profile) |
| License | **AGPL-3.0** | Hosted forks must share improvements | Some companies avoid AGPL |

## Screen sharing pipeline

1. **Capture**: `libwebrtc::desktop_capturer` (Windows Graphics Capture on Windows) on a
   dedicated thread at the configured frame rate, cursor included.
2. **Convert**: BGRA → I420 with libyuv, then downscale to the instance limit if needed
   (e.g. 4K → 1440p), keeping the aspect ratio.
3. **Codec choice** (`sivryn_media::codec`): Media Foundation is asked which hardware
   encoders exist. The first codec from the server's preference list that is
   hardware-encodable *and* decodable by all viewers wins. Otherwise H.264 is used.
4. **Encode and publish**: LiveKit `TrackPublishOptions` with the chosen codec, the hardware
   encoder backend, the max bitrate and fps from the server, and a degradation preference
   from the content hint:
   - *Sharp text* → `MaintainResolution` (drop frames, never blur)
   - *Smooth motion* → `MaintainFramerate` (lower resolution, keep 60 fps)

   AV1/VP9 use `L1T3` (temporal SVC only), so weak viewers get fewer fps but full
   resolution. H.264 uses simulcast.
5. **Receive**: decoded frames → RGBA → `watch` channel (latest frame only, no lag build-up)
   → `VideoSurface` uploads to a wgpu texture and draws it letterboxed into the canvas.

**Still to verify on real hardware:** that libwebrtc actually picks the hardware AV1 encoder
when one is detected (otherwise it silently falls back to software AV1, which is
CPU-intensive at 1440p60), plus end-to-end latency and quality against Discord.

## End-to-end encryption

- Every channel is one MLS group whose members are **devices**.
- The server is the delivery service. It stores one-time key packages (only claimable by
  people who share a guild), orders commits per channel through a primary key on
  `(channel_id, epoch)` (a losing commit gets `409`, and the client discards it and catches
  up), and relays welcomes.
- Messages are stored as MLS ciphertext. The server cannot read them.
- Voice and video E2EE: `media_key = MLS-Exporter("sivryn media e2ee v1", channel_id, 32)`
  feeds LiveKit's frame encryption (AES-GCM). A new epoch (someone joins or leaves) means a
  new key.

**Known MVP limits:** MLS state lives in memory, so every client start registers a new
device. If two devices create the same channel group at the same moment, both stay in their
own group until one of them re-joins. Messages sent before a device joined cannot be
decrypted (forward secrecy by design).

## Event flow

REST writes go to PostgreSQL and then to `EventBus::publish`. Every gateway connection
subscribes to the bus and filters by the guilds the user belongs to; MLS welcomes are
filtered by device. With NATS configured, events cross instances. A client that falls too
far behind is disconnected and resyncs on reconnect.
