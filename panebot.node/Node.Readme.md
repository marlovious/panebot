# PaneBot — Node Provisioning

## What Is a Node

A PaneBot node is a headless Debian 13 machine running Hyprland via uwsm, with `panebot-daemon` as a systemd user service. The daemon manages multiple mpv instances, exposes them over WSS on port 9090, and optionally mounts cloud storage via rclone. The TUI and web extension connect to nodes remotely.

## System Requirements

- Debian 13 (Trixie) — minimal install. PaneBot hardware nodes use Debian 13 specifically. Other distros are not tested.
- GPU with working DRM/KMS (Intel, AMD, or Nvidia with open drivers)
- Network access — static IP or stable DHCP lease recommended
- A user account (not root) in `video`, `audio`, `input`, `render` groups

---

## Packages

> **Note:** `hyprland` and `uwsm` are only available via **Debian 13 backports**. Add backports to your sources first:
> ```bash
> echo "deb http://deb.debian.org/debian trixie-backports main contrib non-free" | sudo tee /etc/apt/sources.list.d/backports.list
> sudo apt update
> ```

```bash
apt install \
  hyprland/trixie-backports uwsm/trixie-backports \
  pipewire pipewire-pulse wireplumber \
  mpv \
  curl wget git \
  rclone \
  libinotifytools0 inotify-tools libnotify-bin python3-xdg \
  build-essential pkg-config libssl-dev
```

Rust toolchain:
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

---

## getty Autologin

Override the getty service for tty1 to log in as your user automatically.

```bash
mkdir -p /etc/systemd/system/getty@tty1.service.d/
```

`/etc/systemd/system/getty@tty1.service.d/autologin.conf`:
```ini
[Service]
ExecStart=
ExecStart=-/sbin/agetty --autologin YOUR_USERNAME --noclear %I 38400 linux
```

```bash
systemctl daemon-reload
systemctl restart getty@tty1
```

---

## uwsm + Hyprland

uwsm manages Hyprland as a systemd session. On login, `.profile` detects tty1 and hands off.

`~/.profile`:
```bash
if uwsm check may-start; then
    exec uwsm start hyprland.desktop
fi
```

Hyprland config lives at `~/.config/hypr/hyprland.conf`. Source the panebot rules file from it:

```
source = ~/.config/panebot/pb.hypr.conf
```

`pb.hypr.conf` is written by the daemon. Create it empty on first boot:
```bash
mkdir -p ~/.config/panebot
touch ~/.config/panebot/pb.hypr.conf
```

---

## panebot-daemon Service

Build the workspace on the node:
```bash
cd ~/gitz/marlovious.panebot/tui
cargo build --release
```

Symlink or copy the binaries:
```bash
cp target/release/panebot-daemon ~/.local/bin/
cp target/release/panebot-tui ~/.local/bin/
```

`~/.config/systemd/user/panebot-daemon.service`:
```ini
[Unit]
Description=PaneBot Daemon
After=graphical-session.target

[Service]
ExecStart=%h/.local/bin/panebot-daemon
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
```

Enable and start:
```bash
systemctl --user daemon-reload
systemctl --user enable panebot-daemon
systemctl --user start panebot-daemon
```

Check status:
```bash
systemctl --user status panebot-daemon
journalctl --user -u panebot-daemon -f
```

---

## panebot Config

All config lives in `~/.config/panebot/`.

`pb.daemon.conf` — **set `mode = remote`**. This is required for the daemon to bind `0.0.0.0` and be reachable over the network. Without it the daemon only listens on `127.0.0.1` and remote TUI/extension connections will fail. The node-installer will handle this automatically in future.
```ini
mode = remote

[mac-studio]
address = wss://10.0.0.10:9090

[linux-other]
address = wss://10.0.0.20:9090
```

`pb.panes.conf` — define your panes:
```ini
layout = default

[main]
pane_name = Main
playlist = main.m3u

[ambient]
pane_name = Ambient
playlist = ambient.m3u
```

On first run the daemon bootstraps per-pane directories and mpv.conf stubs under `~/.config/panebot/{mpv_name}/`.

---

## rclone Mount

Configure rclone with your provider first (`rclone config`), then set up a systemd user service to mount on login.

`~/.config/systemd/user/rclone-mount.service`:
```ini
[Unit]
Description=rclone mount
After=network-online.target

[Service]
Type=notify
ExecStartPre=/bin/mkdir -p %h/premiumize
ExecStart=rclone mount premiumize: %h/premiumize \
    --vfs-cache-mode full \
    --vfs-cache-max-size 4G \
    --buffer-size 256M \
    --dir-cache-time 12h \
    --poll-interval 15s \
    --allow-other
ExecStop=/bin/fusermount -u %h/premiumize
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
```

```bash
systemctl --user daemon-reload
systemctl --user enable rclone-mount
systemctl --user start rclone-mount
```

Verify:
```bash
ls ~/premiumize/
```

---

## Browser Cert Trust

The daemon generates a self-signed cert on first boot (`~/.config/panebot/pb.crt`). Browsers must trust it before the web extension can connect.

On any machine that will use the extension to control this node, visit:
```
https://NODE_IP:9090
```
Accept the certificate warning once. The extension will connect from then on.
