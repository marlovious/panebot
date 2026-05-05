# Protocol Reference

The PaneBot daemon speaks a minimal JSON-over-WebSocket protocol designed around one principle: mpv is authoritative, the daemon is a relay.

State flows out as events — the daemon never waits to be asked. Commands flow in as typed JSON. The `keypress` command gives any client the full mpv keyboard surface without the daemon enumerating every possible operation. The protocol is stable, documented, and intentionally small.

The daemon listens on port 9090 (TLS). All messages are JSON objects.

---

## Connection

On connect, the daemon immediately sends a `node:snapshot` containing the complete current state of all panes. No request required. A client that connects mid-session is immediately synchronized.

---

## Daemon → Client Messages

### `node:snapshot`

Sent on connect. Complete state of all panes.

```json
{
  "event": "node:snapshot",
  "hostname": "panebot",
  "platform": "linux",
  "ip": "192.168.1.100",
  "layout": "pb.left.stack",
  "panes": [
    {
      "name": "wide-top",
      "pane_name": "Wide Top",
      "online": true,
      "paused": false,
      "muted": false,
      "volume": 80,
      "title": "Stalker (1979)",
      "playlist_pos": 2,
      "idle_active": false
    }
  ],
  "known_hosts": [
    { "label": "remote-display", "address": "wss://192.168.1.101:9090" }
  ]
}
```

### `property-change`

Emitted whenever an observed mpv property changes on any pane.

```json
{
  "event": "property-change",
  "pane": "wide-top",
  "property": "pause",
  "value": true
}
```

**Observed properties:**

| Property | Type | Description |
|---|---|---|
| `pause` | bool | Playback paused |
| `volume` | float | Volume 0–100 |
| `mute` | bool | Muted |
| `media-title` | string | Current title |
| `playlist-pos` | int | Current playlist index |
| `playlist-count` | int | Total playlist items |
| `time-pos` | float | Playback position (seconds) |
| `duration` | float | Total duration (seconds) |
| `idle-active` | bool | True when playback ended |

`idle-active: true` signals that the current pane has finished playing and the playlist is exhausted.

### `online` / `offline`

Emitted when a pane's mpv process comes up or goes down.

```json
{ "event": "online",  "pane": "wide-top" }
{ "event": "offline", "pane": "wide-top" }
```

### `node:playlist`

Response to a `playlist-get` command.

```json
{
  "event": "node:playlist",
  "pane": "wide-top",
  "items": [
    { "filename": "https://...", "title": "Stalker" },
    { "filename": "https://...", "title": "Andrei Rublev" }
  ]
}
```

---

## Client → Daemon Messages

All commands follow this shape:

```json
{
  "command": "<command>",
  "pane": "<pane-name>",
  "args": [...]
}
```

### `loadfile`

Load a URL or file path into a pane.

```json
{
  "command": "loadfile",
  "pane": "wide-top",
  "args": ["https://example.com/stream.m3u8", "replace"]
}
```

**Second arg options:**
- `"replace"` — replace current playback immediately
- `"append"` — append to playlist
- `"append-play"` — append and play if nothing is currently playing

### `stop`

Stop playback on a pane.

```json
{ "command": "stop", "pane": "wide-top", "args": [] }
```

### `set_property`

Set any mpv property on a pane.

```json
{ "command": "set_property", "pane": "music",    "args": ["volume", 60] }
{ "command": "set_property", "pane": "wide-top", "args": ["pause",  true] }
{ "command": "set_property", "pane": "wide-top", "args": ["mute",   false] }
```

### `keypress`

Send a key event directly to mpv. Exposes the full mpv keyboard surface to any client through a single command.

```json
{ "command": "keypress", "pane": "wide-top", "args": ["SPACE"] }
{ "command": "keypress", "pane": "wide-top", "args": ["RIGHT"] }
{ "command": "keypress", "pane": "wide-top", "args": ["#"] }
```

Any key mpv recognizes in its `input.conf` works here — including keys bound by user scripts. Common examples:

| Key | mpv action |
|-----|------------|
| `SPACE` | Toggle pause |
| `RIGHT` / `LEFT` | Seek ±5s |
| `UP` / `DOWN` | Seek ±60s |
| `9` / `0` | Volume ±5 |
| `m` | Toggle mute |
| `f` | Toggle fullscreen |
| `j` / `J` | Cycle subtitle track |
| `#` | Cycle audio track |
| `<` / `>` | Previous / next playlist item |
| `l` | Toggle loop |
| `s` | Screenshot |

### `playlist-get`

Request the current playlist for a pane.

```json
{ "command": "playlist-get", "pane": "wide-top", "args": [] }
```

Daemon responds with a `node:playlist` event.

### `playlist-remove`

Remove an item by index.

```json
{ "command": "playlist-remove", "pane": "wide-top", "args": [2] }
```

### `playlist-move`

Move an item from one index to another.

```json
{ "command": "playlist-move", "pane": "wide-top", "args": [3, 1] }
```

---

## TLS

The daemon generates a self-signed certificate at first run: `~/.config/panebot/pb.crt` and `~/.config/panebot/pb.key`.

**TUI** — connects with certificate verification disabled. Appropriate for LAN use.

**Browser extension** — requires the certificate to be trusted in the browser. Visit `https://<host>:9090` once per machine and accept the certificate.

**Programmatic clients** — disable verification for LAN, or trust the certificate explicitly.

---

## Pane Management (TBD)

These commands are implemented in the daemon but not yet exposed in the TUI.

### `panebot:clone-pane`

Clone an existing pane — creates a new mpv instance copying the source pane's config.

```json
{
  "command": "panebot:clone-pane",
  "pane": "wide-top",
  "new_name": "wide-top-2",
  "pane_name": "Wide Top 2"
}
```

### `panebot:remove-pane`

Remove a pane — stops mpv, removes from active pane list, updates config.

```json
{ "command": "panebot:remove-pane", "pane": "wide-top-2", "args": [] }
```

---

## Example: Connect and Control

```javascript
const ws = new WebSocket('wss://192.168.1.100:9090');

ws.onmessage = (msg) => {
  const event = JSON.parse(msg.data);

  if (event.event === 'node:snapshot') {
    console.log('Connected. Panes:', event.panes.map(p => p.name));
  }

  if (event.event === 'property-change' && event.property === 'idle-active' && event.value) {
    console.log(`Pane ${event.pane} finished playing`);
  }
};

// Send a URL to a pane
ws.send(JSON.stringify({
  command: 'loadfile',
  pane: 'wide-top',
  args: ['https://example.com/video.mp4', 'replace']
}));

// Full mpv control via keypress
ws.send(JSON.stringify({
  command: 'keypress',
  pane: 'wide-top',
  args: ['SPACE']
}));
```
