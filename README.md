# PaneBot

Distributed mpv orchestration system. Control multiple video panes across one or more machines from a terminal UI or browser extension. Built for media walls, unattended nodes, and anyone who wants real control over what's playing and where.

[ [Concept](#concept) | [Use Cases](#use-cases) | [Architecture](#architecture) | [Quick Start](#quick-start) | [Configuration](#configuration) | [Key Bindings](#key-bindings) ]

---

## Concept

PaneBot doesn't care where your streams come from. Debrid, YouTube, RTMP, local files, clipboard URLs — it routes them to the right pane and gets out of the way.

A node is a machine running `panebot-daemon`. Each daemon manages a set of mpv instances (panes), monitors their state over IPC, and serves a WebSocket API on port 9090. The TUI connects to any node on your LAN and gives you full control. The browser extension lets you send URLs from any tab directly to any pane on any node.

```
Browser Extension
      │
      │ wss://
      ▼
┌─────────────────────────────────────────────┐
│  PaneBot Dashboard (panebot-tui)            │
│  Terminal UI — connects to any node         │
└────────────────┬────────────────────────────┘
                 │ wss://
        ┌────────┴────────┐
        │                 │
        ▼                 ▼
┌──────────────┐   ┌──────────────┐
│  PaneBot     │   │  PaneBot     │
│  Daemon      │   │  Daemon      │
│  (macOS)     │   │  (Linux)     │
└──────┬───────┘   └──────┬───────┘
       │ IPC              │ IPC
  ┌────┴────┐        ┌────┴────┐
  │   mpv   │        │   mpv   │  ×N
  │  ×N     │        │         │
  └─────────┘        └─────────┘
```

---

## Use Cases

- Multi-screen media walls and art installations
- Unattended kiosk and digital signage nodes
- Event and venue AV control from a single operator machine
- Home theater with independent zone control
- Broadcast monitoring — multiple live streams on one display
- Personal media server with full remote playback control
- Programmatic URL routing to screens from any client or script
- Development and testing of mpv-based media pipelines

---

## Architecture

### PaneBot Dashboard — `panebot-tui`

Ratatui terminal UI. Connects to any daemon over WSS. Three-screen navigation:

```
Log  ←[h/l]→  Panes  ←[h/l]→  Details (Playlist)
```

Pane state (playing, paused, stopped, volume, title) is updated live from the daemon. Command mode (`Tab`) gives direct mpv control — seek, volume, fullscreen, passthrough.

### PaneBot Service — `panebot-daemon`

Async Tokio WebSocket server (port 9090, WSS). One daemon per node. Manages:

- Launching and monitoring mpv instances over Unix sockets
- Broadcasting typed events to all connected clients
- Accepting commands (loadfile, playlist ops, layout switch, restart)
- Serving its known host list so the TUI and extension can discover other nodes

Self-signed TLS cert generated on first boot (`pb.crt`, `pb.key`). On Linux, runs as a systemd user service under `uwsm`.

### PaneBot Node OS — Linux

Cheap ThinkPad running Debian testing + Hyprland. The daemon starts with the session, panes spawn into the tiling layout in order, the display mirrors to HDMI. No desktop, no browser, no overhead — just streams.

### Workspace

```
panebot/
├── Cargo.toml              — workspace root
├── panebot-lib/            — shared types, config parsers, M3U utilities
├── panebot-daemon/         — node daemon
└── panebot-tui/            — terminal control interface
```

Binaries link statically. A node ships just `panebot-daemon`. A control machine ships just `panebot-tui`. No shared runtime required on target machines.

---

## Directory Layout

```
~/.config/panebot/
├── pb.panes.conf          # pane definitions + active layout
├── pb.daemon.conf         # mode (local/remote) + known remote nodes
├── pb.crt                 # TLS certificate (auto-generated)
├── pb.key                 # TLS private key (auto-generated)
├── pb.hypr.conf           # Hyprland window rules (sourced by hyprland.conf)
├── panebot-daemon.log     # daemon log
├── layouts/
│   ├── pb.left.stack.layout
│   └── pb.right.stack.layout
├── music/
│   ├── music.mpv.conf
│   ├── music.m3u          # playlist — launch config and save target
│   ├── music.sock         # mpv IPC socket
│   └── scripts/
├── wide-top/
├── wide-bottom/
└── standard/
```

Each pane gets its own directory named after its `mpv_name`. The `.mpv.conf` is written once by bootstrap and edited freely. The `.m3u` is the launch playlist — mpv loads it at startup, and the daemon can save the live playlist back to it.

---

## Playlists and Live State

mpv is truth. The `.m3u` file is a launch config, not a live record. Once mpv is running, the daemon queries its IPC socket for the current playlist, track position, and state. The TUI reflects live mpv state — not what's on disk.

Saving a playlist (`S` in Details) queries mpv directly and writes the current live playlist back to any path you choose.

```
pb.panes.conf ──► mpv --playlist=music.m3u
                       │
                       ▼
                  [mpv running]
                       │
              observe_property: pause, volume,
              media-title, playlist-pos, mute,
              idle-active
                       │
                       ▼
              panebot-daemon broadcasts
              DaemonEvent::PropertyChange
              to all connected clients
```

---

## Quick Start

### macOS

```bash
git clone https://github.com/marlovious/panebot
cd panebot
cargo install --path panebot-daemon
cargo install --path panebot-tui

# first run — bootstraps config, generates TLS cert, launches mpv panes
panebot-daemon &
panebot-tui
```

### Linux (Hyprland + systemd)

```bash
git clone https://github.com/marlovious/panebot
cd panebot
cargo install --path panebot-daemon

# enable as systemd user service
systemctl --user enable --now panebot-daemon

# add to hyprland.conf
echo "source = ~/.config/panebot/pb.hypr.conf" >> ~/.config/hypr/hyprland.conf
```

### Browser Extension

Load `tui/extension/` as an unpacked extension in Chrome or Brave. On first connection to each node, visit `https://nodeip:9090` in the browser to accept the self-signed certificate. Both the local node and any remote node must be trusted this way.

---

## Configuration

### `pb.panes.conf`

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

The section header (`music`, `wide-top`) is the `mpv_name` — permanent identifier that drives the directory, socket, and config file. `pane_name` is display only and can be changed freely.

### `pb.daemon.conf`

```ini
# mode = local    bind to 127.0.0.1 (default)
# mode = remote   bind to 0.0.0.0, accept LAN connections

mode = remote

[linux-node]
address = wss://192.168.1.x:9090
```

### Layout files

```ini
# ~/.config/panebot/layouts/pb.left.stack.layout

[music]
geometry = 366x366+0+0

[wide-top]
geometry = 650x366+0+374
```

On macOS, geometry is passed as `--geometry` to mpv. On Linux, pane spawn order drives Hyprland master layout placement.

---

## Key Bindings

### Dashboard

| Key | Action |
|-----|--------|
| `j` / `k` | Navigate panes |
| `h` / `l` | Switch screen (Log ↔ Panes ↔ Details) |
| `Tab` | Enter command mode |
| `S` | Solo pane (mute others, fullscreen, play) |
| `M` | Mute all others |
| `P` | Pause / unpause all |
| `r` | Restart selected pane |
| `R` | Restart all panes |
| `W` | Switch layout |
| `C` | Connect to different node |
| `q` | Quit |

### Command Mode (`Tab`)

| Key | Action |
|-----|--------|
| `Space` | Toggle pause |
| `m` | Toggle mute |
| `f` | Toggle fullscreen |
| `h` / `l` | Seek ±5s |
| `j` / `k` | Seek ±60s |
| `9` / `0` | Volume ±5 |
| `v` | Enter mpv passthrough |
| `Tab` | Exit command mode |

### Details (Playlist)

| Key | Action |
|-----|--------|
| `j` / `k` | Navigate items |
| `Enter` | Play now |
| `n` | Queue next |
| `Space` | Mark item |
| `/` | Search |
| `D` | Delete item(s) |
| `M` | Move marked items |
| `C` | Crop to marked / playing |
| `A` | Add URL or path |
| `S` | Save playlist |
| `G` | Jump to index |
| `h` | Back to dashboard |

### mpv Passthrough (`v`)

All keys forward directly to mpv. Exit with `v`.

---

## Requirements

- mpv
- Rust (build only)
- Hyprland (Linux node, optional)
- uwsm (Linux systemd session, optional)

---

## License

MIT
