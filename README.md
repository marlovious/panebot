# panebot

> *Just the streams.*

---

Most video software wants to be your media center. panebot just wants to get out of the way.

It's a terminal UI for managing multiple `mpv` instances as named, independently controllable display targets — a keyboard-driven dashboard for people who'd rather type than click, and who think a clean wall of video with no chrome around it looks exactly right.

The use case is broader than it sounds: local playback, HTTP streams, RTSP camera feeds, debrid sources, live broadcast monitoring. Anything mpv can play, panebot can route to a named pane and give you a single place to see and control all of it.

---

## What it looks like

```
[PaneBot]  ::  Active Panes ::
:: "TV"      :: [video] :: [Playing] :: [Vol:100%] :: Harvesting the Dreamtime
   "Movies"  :: [video] :: [Stopped] :: [Vol: 75%] :: —
   "Web"     :: [http ] :: [Playing] :: [Vol:Mute] :: Star.Trek.S02e02.1080p.mkv
   "CAM1"    :: [rtsp ] :: [Playing] :: [Vol:Mute] :: —
```

---

## Features

- **Dashboard** — all panes at a glance, live status, volume, currently playing title
- **Playlist management** — browse, add, remove, and reorder items per pane
- **Command mode** — play/pause, seek, volume, next/prev without leaving the TUI
- **Multiple content types** — video, audio, HTTP streams, yt-dlp, RTSP/camera feeds
- **Per-pane mpv config** — independent mpv option overrides per pane
- **Persistent playlists** — m3u files per pane, restored on relaunch
- **Geometry support** — pin pane windows to specific screen positions and dimensions
- **Tab completion** — path completion when adding local content

---

## Pane types

| Type    | Use case                                      |
|---------|-----------------------------------------------|
| `video` | Local files, network filesystem               |
| `audio` | Music, podcasts, audio-only                   |
| `http`  | Hosted streams, cloud, debrid                 |
| `ytdlp` | YouTube via yt-dlp                            |
| `rtsp`  | Live feeds, IP cameras, broadcast backline    |

---

## Requirements

- Rust (to build)
- `mpv`
- `yt-dlp` (for ytdlp panes)

---

## Configuration

Panes are defined in `~/.config/panebot/panes.conf`:

```ini
[TV]
socket   = ~/.config/panebot/TV/TV.sock
type     = video
geometry = 960x540+0+0
playlist = -

[CAM1]
socket   = ~/.config/panebot/CAM1/CAM1.sock
type     = rtsp
geometry = 480x270+960+0
playlist = -
```

For first-time setup, run `panebot-setup.sh`.

---

## Keybindings

### Dashboard

| Key        | Action              |
|------------|---------------------|
| `↑ / ↓`   | Select pane         |
| `Tab`      | Enter command mode  |
| `Enter`    | Open playlist view  |
| `q`        | Quit                |

### Command mode

| Key          | Action       |
|--------------|--------------|
| `Space`      | Play / Pause |
| `m`          | Mute         |
| `= / -`      | Volume       |
| `← / →`      | Seek ±10s    |
| `↑ / ↓`      | Seek ±1m     |
| `n / N`      | Next / Prev  |
| `Tab`        | Exit         |

### Playlist view

| Key         | Action              |
|-------------|---------------------|
| `↑ / ↓`    | Select item         |
| `Tab`       | Item command mode   |
| `n`         | Add to playlist     |
| `c`         | Clear playlist      |
| `Backspace` | Return to dashboard |

---

## Roadmap

- HTTP endpoint for pushing content to panes from browser, file manager, or remote host
- Browser extension — right-click any stream URL, send to a named pane
- Per-host daemon for multi-display and network deployments
- Named layout files for switching between pane configurations

---

## Philosophy

mpv handles the rendering. panebot handles the orchestration. No Electron, no GUI framework, no media center assumptions. The terminal is the right control surface for this — fast, keyboard-native, SSH-accessible, and completely invisible to the content it's managing.

Just the streams.
