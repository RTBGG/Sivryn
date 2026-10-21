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
| Audio engine | **Own engine (cpal/WASAPI) instead of WebRTC's audio device module** | The microphone chain and playback are ours: noise filter choice, echo cancellation, voice activity / push-to-talk gate, per-user volume, output device, UI sounds | More code to maintain; WebRTC's ADM did device handling for free |
| Noise suppression | **DeepFilterNet3 (default), RNNoise and WebRTC NS selectable** | DeepFilterNet removes keyboards, fans and background voices best (~1–3 % of a core, ~30 ms); RNNoise is the light option | DeepFilterNet only exists as a GitHub dependency and needs `tract` pinned to 0.21.4; +8 MB model in the binary |
| Echo cancellation / AGC | **sonora** (pure Rust port of WebRTC AEC3 and AGC2) | Keeps echo cancellation with our own pipeline, no C++ build | Young crate (0.2); two of its crates are vendored (see `vendor/SIVRYN-PATCHES.md`) |
| Local storage | **SQLite (bundled) + OS credential store** | MLS keys are single-use, so decrypted history must be kept locally; per-row XChaCha20-Poly1305 under a key in Windows Credential Manager, together with the session token | Channel/author IDs and timestamps are readable in the database |
| Camera | **nokhwa (Media Foundation)** | Cross-platform later (V4L2, AVFoundation) without new code | MJPEG decoded on the CPU (zune-jpeg); NV12/YUY2 preferred when the camera offers them |
| Global hotkeys | **Polling `GetAsyncKeyState` every 10 ms** | No keyboard hook: no input lag if the app stalls, nothing for anti-cheat to flag, mouse side buttons work | Up to 10 ms detection delay; Windows only for now |
| Screen share audio | **WASAPI process loopback** | Window share: only that app's sound. Screen share: everything except Sivryn, so voices never echo back | Windows 10 2004 or newer |
| UI design | **Graphite & violet, Inter bundled, dark + light** | Modern and calm; identical text rendering on every platform | +0.9 MB font in the binary |
| Attachments | **Encrypted files on a local volume** | Each file gets its own XChaCha20-Poly1305 key, which travels inside the MLS message; the server stores only ciphertext under `SIVRYN_FILES_DIR` | No server-side thumbnails or deduplication; S3 would need another backend |
| Edits, reactions, replies | **MLS messages of their own kind** (`edit`, `reaction`, pointing at a message) | The server never sees what was changed or which emoji was used | Clients fold them into the message; history pages contain them too |
| Link previews | **Built by the sender** (like Signal) | Receivers never contact the linked site, the server never sees the URL; preview images are uploaded encrypted | The sender's IP is visible to the linked site; can be turned off |
| Video playback | **Media Foundation source reader** | Decodes H.264/HEVC/VP9/AV1 with the GPU where available, no extra codecs shipped | Windows only; formats depend on installed codecs |
| Emoji | **Twemoji font bundled** | Same look everywhere, works in Blitz's text renderer | +1.5 MB in the binary |
| Text selection | **Blitz patch** | Select within one message, double/triple click, Ctrl+C | Selection can't span several messages |
| Friends and DMs | **Request + accept; DMs from friends and (optionally) server members** | Familiar, and blocking or turning off server DMs stops unwanted messages | Groups are capped at 10 members |

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
   **HDR displays**: libwebrtc captures 8-bit frames, which look washed out from an HDR desktop.
   For a screen or window on a display with HDR on (DXGI color space G2084), Sivryn captures
   with Windows Graphics Capture in 16-bit float (scRGB) instead and tone-maps on the GPU with
   a D3D11 compute shader: divide by Windows' SDR white level (so SDR content looks as on an
   SDR screen), keep values below a knee, roll the highlights off smoothly, encode as sRGB.
   The result enters the normal path through libyuv.
5. **Receive**: decoded frames → RGBA → `watch` channel (latest frame only, no lag build-up)
   → `VideoSurface` uploads to a wgpu texture and draws it letterboxed into the canvas.

**Still to verify on real hardware:** that libwebrtc actually picks the hardware AV1 encoder
when one is detected (otherwise it silently falls back to software AV1, which is
CPU-intensive at 1440p60), plus end-to-end latency and quality against Discord.

## End-to-end encryption

- Every channel is one MLS group whose members are **devices**.
- DMs and groups are MLS groups like any channel.
- The server is the delivery service. It stores one-time key packages (only claimable by
  people who share a guild, a DM or a friendship), orders commits per channel through a primary key on
  `(channel_id, epoch)` (a losing commit gets `409`, and the client discards it and catches
  up), and relays welcomes.
- Messages are stored as MLS ciphertext. The server cannot read them. Edits and reactions
  are encrypted messages of their own kind; files are encrypted with a per-file key carried in
  the message.
- Voice and video E2EE: `media_key = MLS-Exporter("sivryn media e2ee v1", channel_id, 32)`
  feeds LiveKit's frame encryption (AES-GCM). A new epoch (someone joins or leaves) means a
  new key.

**Client state:** each device's MLS state (identity, signing key, groups, unused key
packages) is exported after every change into an encrypted SQLite database, next to the
decrypted message history. Groups keep the keys of the last 16 epochs, so after being offline
a client applies missed commits first and still decrypts the messages sent in between
(`GET /messages?after=`). Key packages are topped up when fewer than 20 are left
(`GET /devices/{id}/key-packages`). Decrypted messages are stored before the MLS state that
consumed their keys, so a crash can cause a refetch but never a lost message.

If two devices create the same channel group at the same moment, the server accepts only one
first commit; the other device drops its provisional group (epoch 0, only itself) and joins from
the winner's welcome, even when the welcome arrives before its own commit is rejected.

**Known limits:** messages sent before a device joined cannot be decrypted (forward secrecy by
design).

## Audio pipeline

```
mic ─ cpal ─ resample 48k ─ gain ─ HPF+AEC3 ─ noise filter ─ AGC2 ─ gate ─ LiveKit (Opus 64k, DTX, RED)
                                      ▲
LiveKit ─ per-user volume ─ mixer ─ cpal ─ speakers/headset
          sounds, mic test ─┘   └─ far-end reference for AEC3
```

Everything runs in 10 ms frames at 48 kHz on two threads that exist only while needed
(call, mic test, sound). Device callbacks only touch lock-free ring buffers. RNNoise always
runs as the speech detector for automatic voice activity, whatever noise filter is selected.
Screen share audio (process loopback) is published separately as `ScreenshareAudio`: Opus
stereo at 128 kbit/s without DTX, with its own volume per viewer.

Each remote voice has a small jitter buffer in the mixer: it starts once 60 ms are buffered,
drains gently above 120 ms (one sample pair per frame, following clock drift without audible
cuts) and is cut back only after a stall (above 300 ms). The mixer also measures each voice it
plays; that level, together with the local voice detection, drives the speaking indicators
without the delay of LiveKit's server-side speaker report. Both audio threads run at raised
priority, and voices that ran dry are logged every 30 s.

## Event flow

REST writes go to PostgreSQL and then to `EventBus::publish`, addressed to a guild, a set of
users (DMs, friends) or devices (MLS welcomes). Every gateway connection subscribes to the bus
and keeps what is addressed to it. Presence is counted per user across their connections. With NATS configured, events cross instances. A client that falls too
far behind is disconnected and resyncs on reconnect.

Presence carries a status (online, away, busy). Invisible users are announced as offline to
everyone else; busy users get no desktop notifications.

## Roles and channel permissions

Permissions are a 64-bit mask per role. A member's guild permissions are the union of
`@everyone` and their roles; the owner and administrators have everything. In a channel,
overwrites are applied Discord style, the category's first, then the channel's: within each,
`@everyone`, then all of the member's roles together (denies before allows), then the member's
own overwrite. The same function (`sivryn_protocol::permissions::channel_permissions`) runs on
the server, which enforces it, and is reported to clients per channel.

Members can only manage roles and members below their own highest role and can't grant
permissions they lack. Channels someone can't see are left out of their guild, their read
states and voice lists; events about them only go to those who can see them.

Private channels stay end-to-end encrypted for exactly their readers: `/channels/{id}/members`
returns only members who may view the channel, and when roles or overwrites change the server
sends `ChannelAccessChanged`, after which devices in the channel's MLS group add the members who
gained access and remove those who lost it.

## Read states and notifications

The server stores, per user and channel, the newest message ID they have read (`read_states`).
Message IDs are UUIDv7, so "unread" is just "newer than the stored ID", and the counts are
computed on the server when a client starts. While running, the client counts new messages
itself and acknowledges the newest one once per channel when it is read (opening the channel,
or the window gaining focus with that channel open). Mutes (with an optional end time) and the
per-server notification level (all messages, mentions only, nothing) are stored on the server
too, so they follow the account to other devices. Notifications are Windows toasts (WinRT,
registered under the app ID `Sivryn.Desktop`); clicking one opens its channel.

## Client caching

Files are cached on disk in `%LOCALAPPDATA%\Sivryn\cache\files` exactly as the server stores
them: attachments and link preview images stay encrypted there (their keys only live in the
encrypted message store), public files such as server icons, avatars and banners are plain.
The cache is trimmed to 1 GB at startup, least recently used first. Decrypted files are only
kept in memory for the session. The share
picker's live previews use the same Windows Graphics Capture path as sharing itself, one frame
per second per source, scaled down on the capture thread.
