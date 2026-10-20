# Sivryn

**Self-hosted voice, video and screen sharing for your friends, without compromising on quality.**

Sivryn is an open-source, Discord-style chat platform written in Rust. You run it on your own
server under your own domain with a single `docker compose up`. It is built around one goal:
screen sharing that looks sharp. That means up to 1440p60 at 12 Mbit/s by default, AV1 when
your GPU can encode it, and limits that you as the admin control.

> **Status: early development.** The server, the protocol and the media engine compile, and
> the core paths have tests. Nothing has been battle-tested yet. See the [roadmap](#roadmap).

## Features

- **Screen sharing first.** Native capture (Windows Graphics Capture today; ScreenCaptureKit
  and PipeWire via the same code path later), hardware encoding, and AV1 with H.264 fallback.
  Temporal SVC keeps text sharp for viewers on weak connections.
- **Voice and video** over a LiveKit SFU. Sivryn's own audio engine brings AI noise
  suppression (DeepFilterNet3 or RNNoise), echo cancellation, voice activity or push-to-talk
  with global hotkeys, per-user volume and screen share audio that never echoes your friends'
  voices back. Cameras are published with simulcast.
- **End-to-end encryption.** Text channels use MLS (RFC 9420). Voice and video can be
  E2EE per channel, with keys derived from the same MLS group and rotated on every join or leave.
- **Familiar model:** servers (guilds), text and voice channels, roles, permissions, invites,
  bans and an AFK channel. Friends, direct messages and group chats with voice, video and screen
  sharing. Replies, edits, reactions, emoji, link previews and files (images, GIFs, videos with
  an inline player), all end-to-end encrypted.
- **Easy to self-host:** Docker Compose with automatic HTTPS via Caddy, PostgreSQL and LiveKit.
  Media runs on two ports.
- **Native desktop client** built with Dioxus Native: a GPU-rendered UI with no webview,
  where video frames go straight into GPU textures. Dark and light themes, German and English.
  Logins, keys and message history are kept encrypted on the device.

## Self-hosting

Requirements: a Linux server with Docker, a domain pointing to it, and ports
`80/tcp`, `443/tcp+udp`, `7881/tcp` and `7882/udp` open.

```sh
git clone https://github.com/RTBGG/Sivryn.git
cd Sivryn/deploy
./setup.sh            # asks for your domain, generates .env with random secrets
docker compose up -d
```

Open the desktop app, enter `https://your.domain` and register. The first account becomes the
instance admin. Registration defaults to invite-only. Change `SIVRYN_REGISTRATION` in
`deploy/.env` to `open` or `closed` as you like.

Files sent in chats are stored encrypted on the `files` volume (only ciphertext; the keys
travel inside the encrypted messages). `SIVRYN_MAX_UPLOAD_MB` limits the size per file
(default 100).

If you already run a reverse proxy, remove `caddy` from `COMPOSE_PROFILES` and proxy
`/rtc*` and `/twirp*` to `127.0.0.1:7880` (LiveKit) and everything else to
`127.0.0.1:8080` (Sivryn).

### Coolify

Use [`deploy/docker-compose.coolify.yml`](deploy/docker-compose.coolify.yml). It leaves HTTPS
to Coolify's proxy, lets Coolify generate all secrets and needs no repository files at runtime.

1. **New Resource → Public Repository** with `https://github.com/RTBGG/Sivryn`, build pack
   **Docker Compose**, compose file `/deploy/docker-compose.coolify.yml`.
2. Assign two domains: `sivryn` → `https://chat.example.com` (port 8080) and
   `livekit` → `https://livekit.example.com` (port 7880). LiveKit needs its own subdomain.
3. Open `7881/tcp` and `7882/udp` in your firewall. WebRTC media goes directly to LiveKit,
   not through the proxy.
4. Deploy, then register in the desktop app with `https://chat.example.com`.

Registration mode and screen share limits are listed under the resource's environment variables.

### Screen share quality

Every limit is configurable per instance in `deploy/.env`:

| Variable | Default | Meaning |
|---|---|---|
| `SIVRYN_SCREENSHARE_MAX_WIDTH` / `_HEIGHT` | `2560` / `1440` | Larger captures are downscaled |
| `SIVRYN_SCREENSHARE_MAX_FPS` | `60` | |
| `SIVRYN_SCREENSHARE_MAX_BITRATE` | `12000000` | bits per second |
| `SIVRYN_SCREENSHARE_CODECS` | `av1,h264` | Preference order. The sender uses the first one it can encode in hardware |

## Development

```sh
cargo test                                  # all crates
cargo run -p sivryn-server                  # needs PostgreSQL + env vars, see deploy/.env.example
cargo run -p sivryn-desktop                 # desktop client
```

The server's integration tests (`crates/sivryn-server/tests`) start a real server per test,
each in its own PostgreSQL schema. They are skipped unless `DATABASE_URL` points to a database
the tests may create schemas in (in CI a missing `DATABASE_URL` fails the run):

```sh
DATABASE_URL=postgres://postgres:postgres@localhost:5432/postgres cargo test -p sivryn-server
```

The first build of `sivryn-media` and `sivryn-desktop` downloads a prebuilt libwebrtc
(about 200 MB). On Windows you need the MSVC build tools. `.cargo/config.toml` enables the
static CRT that libwebrtc requires.

| Crate | Purpose |
|---|---|
| [`sivryn-protocol`](crates/sivryn-protocol) | Wire types shared by server and clients (REST + gateway JSON) |
| [`sivryn-server`](crates/sivryn-server) | axum API, WebSocket gateway, MLS delivery service, LiveKit tokens |
| [`sivryn-client-core`](crates/sivryn-client-core) | UI-independent API and gateway client |
| [`sivryn-crypto`](crates/sivryn-crypto) | MLS groups per channel, message encryption, media key export |
| [`sivryn-media`](crates/sivryn-media) | Screen capture, hardware codec selection, LiveKit calls |
| [`sivryn-desktop`](apps/desktop) | Dioxus Native desktop client |

Design decisions and their trade-offs are documented in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

### Releases

Pushing a version tag runs [`release.yml`](.github/workflows/release.yml):

```sh
git tag -a v0.1.0-alpha.2 -m "Sivryn v0.1.0-alpha.2"
git push origin v0.1.0-alpha.2
```

It attaches `sivryn-desktop-<tag>-windows-x64.exe` and `SHA256SUMS.txt` to the GitHub release
and pushes the server image to `ghcr.io/rtbgg/sivryn-server:<version>`. Tags with a suffix such
as `-alpha.2` become pre-releases and don't update the image's `latest` tag. If you want custom
release notes, create the release on GitHub before pushing the tag; the workflow then only adds
the files. Otherwise it generates notes from the commits.

The GHCR package is private after its first push. Make it public once under
*Package settings → Change visibility* so servers can pull it without logging in. Until there is
a non-pre-release, set `SIVRYN_VERSION` (e.g. `0.1.0-alpha.2`) in `deploy/.env` or let compose
build the image itself.

## Roadmap

- [x] Server: accounts, sessions, guilds, channels, roles, invites, gateway
- [x] MLS delivery service (key packages, ordered commits, welcomes)
- [x] LiveKit integration with per-source publish permissions
- [x] Media engine: capture, AV1/H.264 selection with hardware detection, E2EE media keys
- [x] Desktop client: login, servers, channels, encrypted chat, voice, screen share UI
- [x] Verify end-to-end on one machine (AV1 hardware encoding, 1440p received by a viewer)
- [x] Persist MLS state, sessions and message history on the client (credential store + SQLite)
- [x] Own audio engine: DeepFilterNet3/RNNoise, echo cancellation, voice activity, push-to-talk
- [x] Screen share audio (WASAPI process loopback), camera, device selection, per-user volume
- [x] Settings, global hotkeys, dark/light theme, German/English UI
- [ ] Verify across real networks (NAT, TURN) with several people
- [x] Friends, DMs and group calls, replies, edits, reactions, emoji, encrypted files and link previews
- [ ] OIDC login, rate limiting, search
- [ ] Linux and macOS clients, browser client, mobile

## License

Sivryn is licensed under the [GNU Affero General Public License v3.0 or later](LICENSE).
If you run a modified version as a service, you must publish your changes.
