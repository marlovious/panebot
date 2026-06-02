# <img src="assets/panebot.png" width="130" align="left" style="margin-right: 12px" /> PaneBot
### Just the streams.

PaneBot sends streams to named audio/video mpv instances (panes), with a control surface and orchestrator for local or remote playback.

Use it as a local operator console for multiple mpv windows, or point it at remote PaneBot nodes on your network. Search, generate, scrape, or collect streams however you want; PaneBot just runs them where you send them.

![PaneBot Dashboard](assets/dashboard.png)

PaneBot is not a media center. It does not own your library, metadata, recommendations, or discovery layer. Give it URLs, files, or M3U playlists and it executes them on named targets.

---

## What It Does

- Keeps multiple independent mpv panes running
- Sends URLs, files, streams, or playlists into any pane
- Controls local or remote panes from the same terminal UI
- Exposes mpv's keyboard/control surface without wrapping all of mpv
- Lets browser, terminal, file-manager, or custom tools act as stream senders
- Keeps playlist generation and discovery outside PaneBot

The core model is small:

```text
sender -> stream/feed/path/url -> named pane -> isolated mpv runtime
```

---

## Two Ways To Run It

### Operator-Managed Session

Run `panebot-tui` on a local machine. It starts or connects to a local daemon, shows live pane state, and gives you keyboard control over the running mpv panes.

This is the desktop/operator tool: useful for VJ work, browsing and sending videos, monitoring feeds, testing mpv scripts, or keeping a stack of local panes alive.

### Service-Managed Session

Run `panebot-daemon` as a service for remote control. The daemon owns the panes and exposes the same WebSocket protocol over the network.

This is the node/executor tool: useful for TVs, display nodes, signage boxes, galleries, venue screens, monitoring walls, or any machine that should receive and run streams.

Same daemon. Same protocol. Different lifecycle owner.

---

## Pieces

**`panebot-daemon`** owns mpv. It launches one mpv instance per pane, talks to each instance over mpv JSON IPC, tracks state, and exposes a small WSS protocol on port `9090`.

**`panebot-tui`** is the operator surface. It connects to a daemon, renders live pane state, switches nodes, sends commands, and gives you playlist/detail controls.

**`senders/`** are starter ways to inject streams. The terminal sender and Chrome extension are useful tools, but also examples: anything that can produce a URL/path/M3U and speak the protocol can feed PaneBot.

**mpv** remains the playback engine. Existing mpv configs, scripts, keybindings, and input behavior should work.

---

## Quick Start

```bash
git clone https://github.com/marlovious/panebot
cd panebot/tui

cargo install --path panebot-daemon
cargo install --path panebot-tui

panebot-tui
```

On first local launch, PaneBot creates a config under `~/.config/panebot/`, starts the daemon, and launches the default panes.

Install the terminal sender:

```bash
cd ../senders/terminal
cargo install --path .
```

Send a file or URL:

```bash
pbsend --pane=standard https://example.com/video.mp4
pbsend --pane=music ~/playlists/music.m3u
```

---

## Configuration

PaneBot's main config is intentionally plain:

```ini
layout = pb.left.stack

[music]
pane_name = Music

[wide-top]
pane_name = Wide Top

[wide-bottom]
pane_name = Wide Bottom

[standard]
pane_name = Standard
```

Each section defines a pane. The section name is the stable mpv/runtime name. `pane_name` is the display label.

PaneBot creates per-pane folders under `~/.config/panebot/`:

```text
~/.config/panebot/
├── pb.panes.conf
├── pb.daemon.conf
├── layouts/
├── music/
│   ├── music.mpv.conf
│   ├── music.m3u
│   ├── music.sock
│   └── scripts/
├── wide-top/
├── wide-bottom/
└── standard/
```

Per-pane `.mpv.conf` files are yours to edit. Per-pane `.m3u` files are launch feeds and save targets. PaneBot can load external playlists too.

---

## Local And Remote

`pb.daemon.conf` controls daemon mode:

```ini
# local: bind to 127.0.0.1
# remote: bind to 0.0.0.0 for LAN control

mode = local

[living-room-node]
address = wss://192.168.1.50:9090
```

The TUI can switch between known daemons. A local pane stack and a remote display node use the same protocol.

For a dedicated remote display setup, see [panebot-node](https://github.com/marlovious/panebot-node).

---

## Sending Streams

PaneBot senders are deliberately small. The basic command is just:

```json
{
  "command": "loadfile",
  "pane": "standard",
  "args": ["URL_OR_PATH", "append-play"]
}
```

Modes:

- `replace` replaces current playback
- `append` queues the item
- `append-play` queues it and starts playback if idle

This is the extension surface. Write a browser extension, a shell script, a yazi plugin, a playlist generator, or a service that reacts to upstream events. PaneBot only needs the stream and the target pane.

---

## Why M3U

By default, each pane starts with an M3U playlist. This becomes its running playlist state. Users can edit or generate this file beforehand to create persistent “channels” of media.

M3U playlists can be generated from:

- IPTV channel lists
- Internet radio streams
- Podcast/RSS feeds
- Camera/NVR feeds
- Local media folders
- yt-dlp supported sites
- Custom scripts or services

Modified running playlist state can also be saved back to the M3U.

Live sends can still replace or append to a running pane at any time.

Prefer normal mpv startup behavior? Change the pane's mpv config whenever you like.

---

## Architecture

The short version:

```text
senders / TUI / custom tools
          |
          | WSS JSON protocol
          v
panebot-daemon
          |
          | mpv JSON IPC
          v
one mpv process per pane
```

The daemon is the only process that owns mpv. Clients and senders talk to the daemon. mpv remains authoritative for playback state.

See [`docs/architecture.md`](docs/architecture.md) for the full design and [`docs/protocol.md`](docs/protocol.md) for the protocol reference.

---

## Requirements

- mpv
- Rust, for building from source

---

## License

MIT
