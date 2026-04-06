use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Paths
//
// config_dir() resolution order:
//   Linux:  $XDG_CONFIG_HOME/panebot  (XDG spec)
//           $HOME/.config/panebot     (fallback)
//   macOS:  $HOME/.config/panebot
// ---------------------------------------------------------------------------

pub fn config_dir() -> PathBuf {
    #[cfg(target_os = "linux")]
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() { return PathBuf::from(xdg).join("panebot"); }
    }
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".config/panebot")
}

pub fn layouts_dir()             -> PathBuf { config_dir().join("layouts") }
pub fn panes_conf()              -> PathBuf { config_dir().join("pb.panes.conf") }
pub fn hosts_conf()              -> PathBuf { config_dir().join("pb.daemon.conf") }
pub fn cert_path()               -> PathBuf { config_dir().join("pb.crt") }
pub fn key_path()                -> PathBuf { config_dir().join("pb.key") }
pub fn hypr_rules_path()         -> PathBuf { config_dir().join("pb.hypr.conf") }
pub fn pane_dir(n: &str)         -> PathBuf { config_dir().join(n.to_lowercase()) }
pub fn pane_scripts(n: &str)     -> PathBuf { pane_dir(n).join("scripts") }
pub fn pane_socket(n: &str)      -> PathBuf { pane_dir(n).join(format!("{}.sock",     n.to_lowercase())) }
pub fn pane_mpv_conf(n: &str)    -> PathBuf { pane_dir(n).join(format!("{}.mpv.conf", n.to_lowercase())) }
pub fn pane_playlist(n: &str)    -> PathBuf { pane_dir(n).join(format!("{}.m3u",      n.to_lowercase())) }

// ---------------------------------------------------------------------------
// Shared structs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Pane {
    pub mpv_name:  String,
    pub pane_name: String,
    pub playlist:  Option<String>,
}

#[derive(Debug)]
pub struct Config {
    pub layout: String,
    pub panes:  Vec<Pane>,
    pub home:   String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Host {
    pub label:   String,
    pub address: String,
}

// Playlist item — filename is the source URL/path, title is mpv's resolved display name.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PlaylistItem {
    pub filename: String,
    pub title:    Option<String>,
}

impl PlaylistItem {
    pub fn display(&self) -> &str {
        if let Some(t) = self.title.as_deref().filter(|t| !t.is_empty()) {
            return t;
        }
        if !self.filename.starts_with("http") {
            self.filename.rsplit('/').next().unwrap_or(&self.filename)
        } else {
            &self.filename
        }
    }
}

// PaneState — shared between daemon (serializes) and TUI (deserializes).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct PaneState {
    pub mpv_name:     String,
    pub pane_name:    String,
    pub online:       bool,
    pub idle_active:  Option<bool>,
    pub paused:       Option<bool>,
    pub muted:        Option<bool>,
    pub volume:       Option<f64>,
    pub title:        Option<String>,
    pub playlist_pos: Option<i64>,
}

impl PaneState {
    pub fn new(mpv_name: &str, pane_name: &str) -> Self {
        PaneState { mpv_name: mpv_name.to_string(), pane_name: pane_name.to_string(), ..Default::default() }
    }
}

// ---------------------------------------------------------------------------
// Typed daemon events
//
// Daemon serializes these. TUI deserializes them.
// serde tag = "event" means { "event": "online", ... } in JSON.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "event")]
pub enum DaemonEvent {
    #[serde(rename = "node:snapshot")]
    NodeSnapshot {
        hostname:    String,
        platform:    String,
        ip:          String,
        layout:      String,
        home:        String,
        panes:       Vec<PaneInfo>,
        known_hosts: Vec<Host>,
    },
    #[serde(rename = "online")]
    Online  { pane: String, state: PaneState },
    #[serde(rename = "offline")]
    Offline { pane: String },
    #[serde(rename = "property-change")]
    PropertyChange { pane: String, property: String, value: serde_json::Value },
    #[serde(rename = "node:down")]
    NodeDown,
    #[serde(rename = "node:layout")]
    NodeLayout { layout: String },
    #[serde(rename = "node:playlist")]
    NodePlaylist { pane: String, items: Vec<PlaylistItem> },
    #[serde(rename = "node:playlist-saved")]
    NodePlaylistSaved { pane: String, path: String },
    #[serde(rename = "node:info")]
    NodeInfo { hostname: String, platform: String, panes: Vec<PaneState> },
    #[serde(rename = "node:restart-pane")]
    NodeRestartPane { pane: String },
    #[serde(rename = "node:restart-all")]
    NodeRestartAll,
}

// Lightweight pane descriptor in snapshot — just identity, not live state
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PaneInfo {
    pub name:      String,
    pub pane_name: String,
}

// ---------------------------------------------------------------------------
// Config parsing
// ---------------------------------------------------------------------------

pub fn home_dir() -> String {
    std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_else(|_| ".".to_string())
}

pub fn expand_tilde(path: &str, home: &str) -> String {
    if path.starts_with('~') { path.replacen('~', home, 1) } else { path.to_string() }
}

pub fn load_config() -> Config {
    let home = home_dir();
    let content = match std::fs::read_to_string(panes_conf()) {
        Ok(c)  => c,
        Err(_) => return Config { layout: "pb.left.stack".to_string(), panes: Vec::new(), home },
    };

    let mut layout    = "pb.left.stack".to_string();
    let mut panes     = Vec::new();
    let mut name:      Option<String> = None;
    let mut pane_name: Option<String> = None;
    let mut playlist:  Option<String> = None;

    macro_rules! flush { () => {
        if let Some(ref n) = name {
            panes.push(Pane {
                mpv_name:  n.clone(),
                pane_name: pane_name.clone().unwrap_or_else(|| n.clone()),
                playlist:  playlist.clone(),
            });
        }
    };}

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        if line.starts_with('[') && line.ends_with(']') {
            flush!();
            name = Some(line[1..line.len()-1].to_string()); pane_name = None; playlist = None;
            continue;
        }
        if let Some(eq) = line.find('=') {
            let key = line[..eq].trim();
            let val = line[eq+1..].trim().to_string();
            match key {
                "layout"    => layout    = val,
                "pane_name" => pane_name = Some(val),
                "playlist"  => playlist  = Some(expand_tilde(&val, &home)),
                _           => {}
            }
        }
    }
    flush!();
    Config { layout, panes, home }
}

pub fn load_hosts() -> Vec<Host> {
    let content = match std::fs::read_to_string(hosts_conf()) { Ok(c) => c, Err(_) => return Vec::new() };
    let mut hosts = Vec::new();
    let mut label: Option<String> = None;
    let mut address: Option<String> = None;

    macro_rules! flush { () => {
        if let (Some(l), Some(a)) = (label.take(), address.take()) {
            hosts.push(Host { label: l, address: a });
        }
    };}

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        if line.starts_with('[') && line.ends_with(']') {
            flush!(); label = Some(line[1..line.len()-1].to_string()); address = None; continue;
        }
        if let Some(eq) = line.find('=') {
            let key = line[..eq].trim(); let val = line[eq+1..].trim().to_string();
            if key == "address" { address = Some(val); }
        }
    }
    flush!();
    hosts
}
