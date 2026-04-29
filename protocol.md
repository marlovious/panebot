# Protocol Reference

PaneBot uses a JSON-over-WebSocket protocol. The daemon listens on port 9090 (TLS). All messages are JSON objects.

---

## Connection

On connect, the daemon immediately sends a `node:snapshot` containing the complete current state of all panes. No request required.

---

## Daemon → Client Messages

### `node:snapshot`

Sent on client connect. Complete state of all panes.

```json
{
  "event": "node:snapshot",
  "panes": [
    {
      "name": "wide-top",
      "pane_name": "wide-top",
      "state": "playing",
      "title": "Stalker (1979)",
      "volume": 80,
      "muted": false,
      "playlist_pos": 2,
      "playlist_count": 12,
      "duration": 9576.0,
      "position": 1823.4
    }
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
  "data": true
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

`idle-active: true` is the primary signal for playlist exhaustion and auto-advance triggers.

### `online` / `offline`

Emitted when a pane's mpv process comes up or goes down.

```json
{ "event": "online", "pane": "wide-top" }
{ "event": "offline", "pane": "wide-top" }
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
- `"append-play"` — append and play if nothing is playing

### `stop`

Stop playback on a pane.

```json
{
  "command": "stop",
  "pane": "wide-top",
  "args": []
}
```

### `set_property`

Set any mpv property on a pane.

```json
{ "command": "set_property", "pane": "music", "args": ["volume", 60] }
{ "command": "set_property", "pane": "wide-top", "args": ["pause", true] }
{ "command": "set_property", "pane": "wide-top", "args": ["mute", false] }
```

### `keypress`

Send a key event directly to mpv. Gives access to the full mpv keyboard surface.

```json
{ "command": "keypress", "pane": "wide-top", "args": ["SPACE"] }
{ "command": "keypress", "pane": "wide-top", "args": ["RIGHT"] }
{ "command": "keypress", "pane": "wide-top", "args": ["j"] }
{ "command": "keypress", "pane": "wide-top", "args": ["#"] }
```

Any key that mpv recognizes in its `input.conf` works here. This includes:

- `SPACE` — toggle pause
- `RIGHT` / `LEFT` — seek forward/backward 5s
- `UP` / `DOWN` — seek forward/backward 1m
- `9` / `0` — volume down/up
- `m` — toggle mute
- `f` — toggle fullscreen
- `j` / `J` — cycle subtitle track
- `#` — cycle audio track
- `<` / `>` — previous/next playlist item
- `l` — toggle loop
- `s` — screenshot
- Any key from the user's `input.conf`

### `playlist-get`

Request the current playlist for a pane. Daemon responds with a `playlist` event.

```json
{ "command": "playlist-get", "pane": "wide-top", "args": [] }
```

Response:

```json
{
  "event": "playlist",
  "pane": "wide-top",
  "items": [
    { "index": 0, "filename": "https://...", "title": "Stalker", "current": false },
    { "index": 1, "filename": "https://...", "title": "Andrei Rublev", "current": true }
  ]
}
```

### `playlist-remove`

Remove an item from the playlist by index.

```json
{ "command": "playlist-remove", "pane": "wide-top", "args": [2] }
```

### `playlist-move`

Move a playlist item from one index to another.

```json
{ "command": "playlist-move", "pane": "wide-top", "args": [3, 1] }
```

---

## TLS

The daemon generates a self-signed certificate at first run, stored at `~/.config/panebot/pb.crt` and `~/.config/panebot/pb.key`.

**TUI** — connects with certificate verification disabled (`NoCertVerification`). Appropriate for LAN use.

**Browser extension** — requires the certificate to be trusted in the browser. Visit `https://<node-ip>:9090` in the browser and accept the certificate. One-time setup per machine.

**Programmatic clients** — either disable verification (LAN) or trust the certificate explicitly.

---

## Node Management Commands (TBD)

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

Remove a pane — kills mpv, removes from active pane list, updates config.

```json
{
  "command": "panebot:remove-pane",
  "pane": "wide-top-2"
}
```

---

## Example: Connect and Control

```javascript
const ws = new WebSocket('wss://192.168.1.100:9090');

ws.onmessage = (msg) => {
  const event = JSON.parse(msg.data);
  if (event.event === 'node:snapshot') {
    console.log('Panes:', event.panes.map(p => p.name));
  }
  if (event.event === 'property-change' && event.property === 'idle-active' && event.data) {
    console.log(`Pane ${event.pane} finished playing`);
  }
};

// Load a URL
ws.send(JSON.stringify({
  command: 'loadfile',
  pane: 'wide-top',
  args: ['https://example.com/video.mp4', 'replace']
}));

// Toggle pause
ws.send(JSON.stringify({
  command: 'keypress',
  pane: 'wide-top',
  args: ['SPACE']
}));
```
