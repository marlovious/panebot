# Architecture

PaneBot is built around a single principle: the daemon owns mpv, everything else is a client.

This has real consequences. There is exactly one process on a machine that ever writes to an mpv socket. Playback state is authoritative in one place and read everywhere else. The TUI, the browser extension, and any other client speak the same protocol — the daemon doesn't know or care which one it's talking to. Kill the TUI, reconnect it. Playback was never interrupted.

---

## Daemon

The daemon is the only process that matters for playback. Everything else is optional.

On startup it reads `pb.panes.conf`, bootstraps any missing pane directories and configs, then spawns mpv for each configured pane. Each mpv instance is launched with `--input-ipc-server` pointing to a per-pane Unix socket. The daemon opens that socket, sends `observe_property` subscriptions for the properties it tracks, and begins the read loop.

```
daemon startup
  ├── read pb.panes.conf
  ├── for each pane:
  │     ├── ensure config dir and mpv.conf exist
  │     ├── spawn mpv --input-ipc-server=<socket>
  │     └── open socket, subscribe to properties
  └── listen on wss://0.0.0.0:9090
```

Property subscriptions are the core of the zero-polling model. Instead of asking mpv "are you paused?" on a timer, the daemon sends:

```json
{"command": ["observe_property", 1, "pause"]}
{"command": ["observe_property", 2, "volume"]}
{"command": ["observe_property", 3, "media-title"]}
{"command": ["observe_property", 4, "playlist-pos"]}
{"command": ["observe_property", 5, "playlist-count"]}
{"command": ["observe_property", 6, "idle-active"]}
```

mpv pushes a `property-change` event every time a value changes. The daemon forwards it to all connected clients. No polling anywhere in the stack.

When a client connects, the daemon sends a `node:snapshot` — complete current state of all panes. The client is immediately synchronized.

---

## IPC Layer

Each mpv instance communicates over a Unix domain socket using mpv's native JSON IPC protocol. The daemon maintains one persistent connection per pane. Commands from WebSocket clients are forwarded to the appropriate socket. Events from mpv are read asynchronously and broadcast to clients.

If mpv crashes, the daemon detects the broken socket, emits an `offline` event to all clients, and respawns.

---

## WebSocket Layer

The daemon exposes a single WebSocket endpoint on port 9090, TLS. All clients connect here.

The protocol is intentionally minimal. Two message types flow out: `node:snapshot` on connect, `property-change` as events happen. One type flows in: commands.

The `keypress` command passes key events directly to mpv's IPC. This gives any client the full mpv keyboard surface — seek, chapters, subtitles, audio tracks, speed, scripts — through a single whitelisted command. The daemon stays thin. mpv stays authoritative.

```
client → {"command": "keypress", "pane": "wide-top", "args": ["SPACE"]}
daemon → mpv socket: {"command": ["keypress", "SPACE"]}
```

---

## State Model

State lives in the daemon's in-memory `PaneState` per pane. Populated at startup from mpv subscriptions, updated on every `property-change`. Never written to disk — the daemon is stateless across restarts.

The per-pane `.m3u` file is the only persistence. The daemon loads it when mpv starts. When clients modify the playlist, the daemon updates the file atomically and syncs mpv via IPC. Playlists survive daemon and mpv restarts cleanly.

---

## Pane Lifecycle

```
pane config exists?
  no  → bootstrap (create dir, write mpv.conf from defaults)
  yes → proceed

mpv running?
  no  → spawn mpv with socket path and mpv.conf
  yes → attach to existing socket

socket open?
  no  → retry with backoff
  yes → subscribe properties, snapshot clients, begin event loop
```

Panes are independent. One pane crashing does not affect others.

---

## Client Architecture

### TUI

Ratatui application running in a dedicated async task. Maintains a local `UiState` — the rendered view of what the daemon has reported. On connect it receives the snapshot. On each `property-change` it updates the relevant pane and schedules a redraw.

The TUI sends commands on keypress. It never reads from mpv directly. It is a view, not a controller in the ownership sense. It can be run locally or pointed at any daemon on the network — the experience is identical.

### Browser Extension

Injects a context menu item. On activation it opens a WebSocket connection, sends a `loadfile` command with the target URL, and closes. No persistent connection, no state. The extension is URL ingress — it gets links into panes with one click from any browser tab.

Requires the daemon's self-signed certificate to be trusted in the browser. One-time setup per machine.

---

## Deployment

### Local

Daemon runs as a user process — or systemd user service on Linux. TUI connects to localhost. Extension connects to localhost. No network configuration required. This is the baseline experience on both macOS and Linux.

### Remote

Put the daemon in `remote` mode and it binds to `0.0.0.0`, accepting connections from anywhere on the LAN. The TUI and browser extension connect by IP or hostname — the experience is identical to local.

This is where PaneBot's design pays off. A daemon running on a dedicated display — a TV, a media wall panel, a signage screen — becomes a fully controllable mpv execution target. It exposes the complete mpv surface over a documented WebSocket protocol. Any client that speaks JSON can connect, receive the live state snapshot, send URLs, manage playlists, and respond to playback events.

The `idle-active` event is particularly useful in this context. When a pane finishes playing, the daemon emits it immediately. Any connected client can respond — advance a playlist, load the next item, trigger an external action. The daemon doesn't care what the client does with it. That's intentional.

A daemon in remote mode is also a natural target for programmatic control. Automated playlist management, URL ingress from browser extensions or external systems, event-driven playback — all of this is first-class because the protocol is open and the daemon is stateless. There is no proprietary API to integrate against. There is a WebSocket endpoint and a documented protocol.

For a reference deployment on dedicated display hardware, see [panebot-node](#).

---

## Design Decisions

**Why Rust.** The daemon manages process lifecycle, shared mutable state across async tasks, and Unix IPC with tight latency requirements. Rust's ownership model eliminates the class of concurrency bugs — races on pane state, use-after-free on socket handles — that would be painful to debug here. The compiled binary ships with no runtime dependencies.

**Why a single daemon per machine.** One process owns all panes. One WebSocket endpoint, one snapshot, one event stream. Cross-pane operations — move a playlist item between panes — happen atomically. Clients don't need to enumerate processes.

**Why INI config.** Human-editable, diff-friendly, zero parsing dependencies. The daemon bootstraps missing files at startup — you only edit what you care about.

**Why M3U for playlists.** mpv reads M3U natively. Playlists survive restarts without any special restore logic — mpv opens the file and continues. Editable with any text editor, importable from any tool that exports M3U.

**Why `keypress` for full mpv surface.** Enumerating every mpv IPC command as a first-class daemon operation would be hundreds of items and would need updating whenever mpv adds something. `keypress` passes the event directly to mpv through one whitelisted command. Full mpv surface, always. The daemon stays thin.
