# PaneBot Node OS

## Base System
- Debian 13 (Trixie)
- Minimal install

---

## Required Stack
- Hyprland (via backports or current, depending on Debian release)
- uwsm
- mpv
- PipeWire
- rclone (required for cloud media, optional otherwise)

---

## Runtime Model
Node boots into:

    tty1 → autologin → uwsm → Hyprland → systemd user → panebot-daemon

---

## panebot-daemon
Runs as systemd user service.

Responsibilities:
- Manage mpv instances
- Expose WSS (port 9090)
- Handle IPC translation
- Track pane state

---

## Networking
- WSS over TLS (self-signed cert, generated on first boot)
- Local mode: binds `127.0.0.1`
- Remote mode: binds `0.0.0.0`

---

## Filesystem Layout

```
~/.config/panebot/
  pb.panes.conf
  pb.daemon.conf
  pb.hypr.conf
  pb.crt
  pb.key
  layouts/
  {pane}/
    mpv.conf
    playlist.m3u
    scripts/
```

---

## User Groups
The panebot user must be in:

    video, audio, input, render

---

## Optional
- rclone mount for cloud media (required for cloud media, optional otherwise)
- Browser cert trust for node TLS (`pb.crt` must be imported to trust self-signed WSS)
