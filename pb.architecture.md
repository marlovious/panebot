# Architecture

PaneBot is built around a single architectural principle: the daemon owns mpv, everything else is a client.

This sounds obvious but it has real consequences. It means there is exactly one process per node that ever writes to an mpv socket. It means playback state is authoritative in one place and read everywhere else. It means the TUI, the browser extension, and any other client all speak the same protocol and the daemon doesn't need to know or care which one it's talking to. It means you can kill the TUI, reconnect it, and playback was never interrupted.

---

## Daemon

The daemon is the only process on a node that matters for playback. Everything else is optional.

On startup it reads `pb.panes.conf`, bootstraps any pane directories and configs that don't exist, then spawns mpv for each configured pane. Each mpv instance is launched with `--input-ipc-server` pointing to a per-pane Unix socket. The daemon opens that socket, sends `observe_property` subscriptions for the properties it tracks, and begins the read loop.

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

mpv then pushes a `property-change` event every time the value changes. The daemon forwards these to all connected WebSocket clients. No polling anywhere in the stack.

When a client connects, the daemon sends a `node:snapshot` — the complete current state of all panes. The client is immediately synchronized without needing to poll for state it missed.

---

## IPC Layer

Each mpv instance communicates over a Unix domain socket using mpv's native JSON IPC protocol. The daemon maintains one persistent connection per pane. Commands from WebSocket clients are serialized and forwarded to the appropriate socket. Responses and events from mpv are read asynchronously and broadcast to clients.

The socket connection is owned by the daemon for the lifetime of the mpv process. If mpv crashes, the daemon detects the broken socket, emits an `offline` event to all clients, and can optionally respawn.

---

## WebSocket Layer

The daemon exposes a single WebSocket endpoint on port 9090, TLS with a self-signed certificate. All clients — TUI, extension, custom tools, anything else — connect here.

The protocol is intentionally simple. Two message types flow from daemon to clients: `node:snapshot` on connect, and `property-change` as events happen. One message type flows from clients to daemon: commands.

Commands are typed but not rigidly enumerated. The daemon whitelist covers the necessary playback surface. The `keypress` command passes key events directly to mpv's IPC — this gives clients access to the full mpv keyboard surface (seek, chapter navigation, subtitle and audio track cycling, speed control) without the daemon needing to proxy every possible command as a first-class operation.

```
client → {"command": "keypress", "pane": "wide-top", "args": ["SPACE"]}
daemon → mpv socket: {"command": ["keypress", "SPACE"]}
```

---

## State Model

State lives in the daemon's in-memory `PaneState` per pane. It is populated at startup from mpv property subscriptions and updated on every `property-change` event. It is never written to disk — the daemon is stateless across restarts, and mpv holds the actual playback state (including the playlist via `playlist.m3u`).

The per-pane `.m3u` file (e.g. `music.m3u`, `wide-top.m3u`) is the only persistence. The daemon loads it when mpv starts. When clients modify the playlist (add, remove, reorder), the daemon updates the file atomically and syncs mpv via IPC. This means playlists survive daemon and mpv restarts cleanly.

---

## Pane Lifecycle

```
pane config exists?
  no  → bootstrap (create dir, write mpv.conf from type defaults)
  yes → proceed

mpv running?
  no  → spawn mpv with correct socket path and mpv.conf
  yes → attach to existing socket (reconnect case)

socket open?
  no  → retry with backoff
  yes → subscribe properties, snapshot clients, begin event loop
```

Panes are independent. One pane crashing does not affect others. The daemon manages each pane's lifecycle separately.

---

## Client Architecture

### TUI

The TUI is a Ratatui application running in a dedicated async task. It maintains a local copy of `UiState` — the rendered view of whatever the daemon has told it. On connect it receives the snapshot and populates state. On each `property-change` event it updates the relevant pane entry and schedules a redraw.

The TUI sends commands on keypress. It never reads from mpv directly — all state flows through the daemon's WebSocket.

The TUI can be killed and restarted without affecting playback. It is a view, not a controller in the ownership sense.

### Browser Extension

The extension injects a context menu item. On activation it opens a WebSocket connection to the configured node, sends a `loadfile` command with the target URL, and closes the connection. It does not maintain a persistent connection or track state.

The extension requires the node's self-signed certificate to be trusted in the browser. This is a one-time per-machine setup.

---

## Deployment Model

### Single Workstation

Daemon runs as a user process or systemd user service. TUI connects to localhost. Extension connects to localhost. No network configuration required.

### Multi-Node

Each node runs `panebot-daemon` as a systemd user service. TUI connects to any node by hostname or IP. Extension is configured with the address of the local node (or any remote node).

Nodes are minimal — Debian, Hyprland, the daemon binary. No application logic runs on nodes. They are dumb execution targets.

### Event/Installation

For closed-network deployments, nodes boot to Hyprland and start the daemon automatically. A control machine runs the TUI. URLs and playlists are pushed from the control surface. Nodes never need to be touched after initial setup.

---

## Design Decisions

**Why Rust.** The daemon manages process lifecycle, shared mutable state across async tasks, and Unix IPC with tight latency requirements. Rust's ownership model prevents the class of concurrency bugs — race conditions on pane state, use-after-free on socket handles — that would be painful to debug in this context. The compiled binary ships with no runtime dependencies.

**Why a single daemon per node.** One process owns all panes on a node. This means one WebSocket endpoint, one snapshot, one event stream. Clients don't need to know how many panes a node has until they ask. It also means the daemon can coordinate cross-pane operations — move an item from one playlist to another — atomically.

**Why INI config.** Human-editable, diff-friendly, zero parsing dependencies. Pane configs, type defaults, layout names — all text. The daemon bootstraps missing files at startup, so the user only edits what they care about.

**Why M3U for playlists.** mpv reads M3U natively. Persisting playlists as M3U means they survive daemon restarts without any special restore logic — mpv opens the file and continues. It also means playlists are editable with any text editor and importable from any tool that exports M3U.

**Why `keypress` for full mpv surface.** Enumerating every mpv IPC command as a first-class daemon command would be hundreds of operations and would need updating whenever mpv adds something. `keypress` passes the key event directly to mpv, giving clients the full mpv keyboard surface through one whitelisted command. The daemon stays thin.
