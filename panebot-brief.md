# PaneBot — Project Brief for Claude Session Handoff

## What It Is
PaneBot is a distributed mpv orchestration system. A Rust workspace with 3 crates that lets you control multiple mpv video player instances across one or more machines from a terminal UI, browser extension, or programmatically. Built for unattended video wall / media node use — think hostel lounge display, personal media dashboard, or remote video wall.

**North star: Just The Streams.** PaneBot doesn't care where the stream comes from. It routes it to the right pane and gets out of the way.

## Repository
`~/gitz/marlovious.panebot/tui/`
3 crates: `panebot-lib`, `panebot-daemon`, `panebot-tui`

## Architecture

### panebot-lib
Shared types used by both daemon and TUI:
- `DaemonEvent` — typed serde enum, tag = "event", all variants have explicit `#[serde(rename = "...")]` with colons preserved (e.g. `"node:snapshot"`, `"node:playlist"`)
- `PaneState` — shared struct, serde bidirectional
- `PlaylistItem` — with `display()` helper (title > filename-only for local paths > full URL for http)
- `PaneInfo`, `Pane`, `Host`, `Config`
- Path helpers: `config_dir()`, `pane_socket()`, `pane_mpv_conf()`, `cert_path()`, `key_path()`, `hypr_rules_path()`

### panebot-daemon
Async Tokio **WSS** server on port 9090 (TLS, self-signed cert generated on first bootstrap). Per-pane mpv IPC monitor via unix socket. Broadcasts typed `DaemonEvent` JSON to all connected clients.

Key internals:
- `SharedPanes = Arc<tokio::sync::RwLock<Vec<Pane>>>` — mutable at runtime for add/clone/remove
- `SharedState = Arc<Mutex<HashMap<String, PaneState>>>` — live mpv state per pane
- `PaneCommands = Arc<Mutex<HashMap<String, mpsc::Sender<String>>>>` — send IPC commands to mpv
- `monitor_pane()` — connects to mpv unix socket, subscribes to properties, broadcasts changes
- Observes: `pause`, `volume`, `media-title`, `playlist-pos`, `mute`, `idle-active`
- Playlist save: daemon queries mpv socket directly for `get_property playlist`, writes file itself
- `mode = local/remote` in `pb.daemon.conf` — local binds `127.0.0.1`, remote binds `0.0.0.0`
- Both modes now serve WSS — TLS is the security layer, not the binding address
- Hostname truncated to 10 chars in log: `[hostname] [HH:MM:SS] message`
- Bootstrap: creates config skeleton, per-pane dirs, mpv.conf stubs, empty playlists, TLS cert+key
- TLS cert: `rcgen` generates self-signed cert on first run → `pb.crt` + `pb.key` in config dir
- `write_panes_conf()` and `broadcast_snapshot()` helpers for runtime pane management
- `broadcast_snapshot()` IP bug fixed — now uses actual `display_ip` not hardcoded `127.0.0.1`

Node commands (prefix `panebot:`):
- `node-info`, `restart-all`, `restart-pane`, `playlist-get`, `playlist-save`, `layout`, `clone-pane`, `remove-pane`, `shutdown`

### panebot-tui
Ratatui terminal UI. Connects to daemon via **WSS** with self-signed cert acceptance (custom `NoCertVerification` rustls verifier).

Key architecture:
- `App` — thin wrapper around `SessionState` + `UiState`
- `SessionState` — network/node data, wiped clean on reconnect: `pane_order`, `panes`, `hostname`, `platform`, `ip`, `layout`, `home`, `is_remote`, `owns_daemon`, `layouts`
- `UiState` — survives reconnect: selection, screen state, playlist items, input buffers, passthrough mode
- `process_event()` — deserializes `DaemonEvent` enum, no string matching
- Three-screen navigation: Log ← `h/Left` | Dashboard | `l/Right` → Details
- Command mode (`Tab`): send mpv IPC commands to selected pane
- Passthrough mode (`v` in command mode): forward all keys directly to mpv. Steel blue `[MPV]` badge, dark blue row background, blue footer bar. Exit with `v`.
- `resolve_daemon_addr()` — always shows host picker (even single host)
- `spawn_daemon()` — on Linux checks `systemctl --user is-active panebot-daemon` first; only spawns binary if service not running
- Dashboard header: `[hostname] :: ip :: Panes: 3/4 :: Layout: name`
- Details screen: playlist view, mark/play/queue/delete/move/crop/add/save operations
- `LOCAL_ADDR = "wss://127.0.0.1:9090"`

Color constants:
- `C_ORANGE`, `C_CYAN`, `C_DIM`, `C_HINT`, `C_DIVIDER`, `C_RED`, `C_GREEN`, `C_WHITE`, `C_MPV` (steel blue `80,120,180`)
- Normal selection: `Color::Rgb(25, 48, 48)`
- MPV mode selection: `Color::Rgb(20, 30, 55)` dark blue

### Web Extension (Chrome/Brave)
- Popup: single `connectAndLoad()` WSS connection, 600ms timer, collects snapshot + state then closes
- Paste button in URL bar for clipboard URLs
- Context menu on regular web pages (not Stremio — it swallows right-click)
- Remote node access: WSS resolves the browser Private Network Access block — remote nodes need their daemons rebuilt with WSS too
- `known_hosts` from local daemon's snapshot → connects to each remote node via WSS
- `host_permissions` needs updating from `ws://` to `wss://`

## Config Files
All in `~/.config/panebot/`:
- `pb.panes.conf` — layout name + pane definitions `[mpv_name]` with `pane_name`, optional `playlist`
- `pb.daemon.conf` — `mode = local/remote` + remote host entries `[label]` with `address = wss://...`
- `pb.crt`, `pb.key` — auto-generated TLS cert and key (rcgen, self-signed)
- `pb.hypr.conf` — Hyprland window rules (sourced from hyprland.conf via `source = ~/.config/panebot/pb.hypr.conf`). Currently empty — daemon will generate named rules here in future.
- `layouts/` — `.layout` files (macOS geometry `WxH+X+Y`)
- Per-pane dirs: `{mpv_name}/{mpv_name}.mpv.conf`, `{mpv_name}/{mpv_name}.sock`, `{mpv_name}/{mpv_name}.m3u`, `{mpv_name}/scripts/`

## Design Philosophy
- **Just The Streams** — PaneBot doesn't care about source. Route the stream, get out of the way.
- **mpv is truth** — playlist state lives in mpv, not filesystem. m3u is launch config only.
- **Pane names are immutable** — `mpv_name` drives directories, sockets, conf files. Set once, never change. `pane_name` is display only, change freely.
- **No pane_type** — removed in favor of user agency. Users configure mpv.conf directly.
- **Playlist save** — daemon queries mpv socket for `get_property playlist`, writes `#EXTM3U` directly.
- **No Lua scripts** — deferred.
- **WSS done** — self-signed cert, generated on first boot. No nginx, no manual cert management.
- **No federation** — deferred. Extension now connects directly to each node via WSS.
- **No token auth** — deferred. WSS is the security layer for now.

## Node OS (Linux)
- Debian testing (14), Hyprland on Wayland
- Auto-login: systemd getty override, auto-login for panebot user
- Session: `uwsm` manages Hyprland as a systemd user service. `.profile` launches uwsm on tty1.
- panebot user in `video,audio,input,render` groups
- Hyprland layout: `layout = master` in `general {}`. Pane spawn order drives slot placement.
- Audio: `audio-device=alsa/hdmi:CARD=PCH,DEV=0` in mpv.conf for HDMI output
- mpv video: `vo=gpu`, `gpu-api=opengl` (or `vulkan`), `gpu-context=wayland`
- No tearing — Hyprland explicit sync handles it
- Mirror mode: `monitor = eDP-1, 1920x1080, 0x0, 1` + `monitor = HDMI-A-1, 1920x1080, 0x0, 1`
- rclone mount: systemd user service for `premiumize:` → `~/premiumize/`
- **panebot-daemon runs as systemd user service**: `~/.config/systemd/user/panebot-daemon.service`
  - `After=graphical-session.target`, `PartOf=graphical-session.target`
  - `Restart=on-failure`, `RestartSec=2`, `Slice=app-graphical.slice`
  - uwsm provides proper `graphical-session.target` sequencing

## Hyprland Architecture (Linux)
- Window placement driven by `master` layout + spawn order from `pb.panes.conf`
- No floating rules — tiling windows resize and push neighbors
- `pb.hypr.conf` sourced from hyprland.conf — panebot will write named window rules here
- Hyprland socket1 (command) — open/write/close per call (not persistent — Hyprland requirement)
- Hyprland socket2 (events) — persistent listener planned for `openwindow` (sequential spawn sequencing) and `configreloaded` (re-inject rules)
- Named rules required for runtime enable/disable: `windowrule { name = panebot-music ... }`
- Static effects (`move`, `size`, `float`) only fire at window creation — always restart panes after layout switch
- `hyprctl --batch` for multiple rule injections — reduces IPC overhead
- TUI Hyprland passthrough planned: keys → `hyprctl dispatch` calls via socket1
- Save layout: `hyprctl clients -j` → extract geometry by title → write layout file
- Node hardware: cheap ThinkPad X270/X280 or similar with Intel QuickSync (hwdec=vaapi)

## Cargo Dependencies (key)
- `tokio-tungstenite = "0.24"` with `rustls-tls-native-roots` feature
- `tokio-rustls = "0.26"` with `ring` feature
- `rustls = "0.23"` with `default-features = false`, features `["ring", "std", "tls12"]`
- `rustls-pemfile = "2"`, `rcgen = "0.13"` (daemon only)
- Must call `let _ = rustls::crypto::ring::default_provider().install_default();` at top of both daemon and TUI `main()`

## Pending / Known Issues
1. **Web extension WSS update** — manifest `host_permissions` and popup.js addresses need `wss://` + remote nodes need daemon rebuilt
2. **Hyprland rule injection** — daemon should write named window rules to `pb.hypr.conf` and call `hyprctl reload` at startup and layout switch
3. **Socket2 sequential spawn** — daemon should listen on Hyprland socket2 for `openwindow` events to sequence pane spawning deterministically
4. **Clone/remove pane TUI keys** — daemon commands wired, no key bindings assigned yet
5. **Fullscreen flag in dashboard** — mpv `fullscreen` property not yet observed
6. **Shuffle/loop/chapter** — observable via IPC, not wired in daemon
7. **Playlist change push** — currently manual `playlist-get` after operations
8. **Clipboard key in TUI** — discussed, not implemented
9. **Hyprland TUI passthrough** — planned: `H` enters Hyprland passthrough, keys → socket1 dispatchers
10. **Save layout from TUI** — planned: key → `hyprctl clients -j` → write layout file
11. **Fullscreen black screen on mirrored outputs** — known Hyprland issue with mirrored monitors

## File Rotation Protocol
Always `cp X.new.rs X.last.rs` before writing. Never overwrite without rotating. Always re-read files before editing after any prior edit. Files live at `/home/claude/panebot-{lib,daemon,tui}.{new,last}.rs` and outputs at `/mnt/user-data/outputs/`.

## Session History
- Sessions 1-13: Initial architecture, mpv IPC, TUI, extension
- Session 14: Playlist system IPC migration, extension rewrite
- Session 15: Typed DaemonEvent enum, SessionState/UiState split, SharedPanes RwLock, clone/remove pane, playlist save via daemon socket, Linux node OS setup, i3 layout work, extension paste button, MPV passthrough mode visual improvements
- Session 16: Migrated node OS from i3/X11 to Hyprland/Wayland. uwsm + systemd user service for daemon. WSS/TLS added (rcgen self-signed cert, tokio-rustls, NoCertVerification in TUI). broadcast_snapshot IP bug fixed. spawn_daemon() checks systemd service on Linux. Hyprland architecture designed: master layout, socket1/socket2 IPC plan, named window rules, sequential spawn via openwindow event, save layout via hyprctl clients. pb.hypr.conf include file convention established. North star clarified: Just The Streams.
