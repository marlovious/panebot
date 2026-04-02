use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Paths
//
// config_dir() resolution order:
//   Linux:  $XDG_CONFIG_HOME/panebot  (XDG spec)
//           $HOME/.config/panebot     (fallback)
//   macOS:  $HOME/.config/panebot
//
// No external crate dependencies — $HOME via std::env.
// ---------------------------------------------------------------------------

pub fn config_dir() -> PathBuf {
    #[cfg(target_os = "linux")]
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("panebot");
        }
    }

    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".config/panebot")
}

pub fn layouts_dir()     -> PathBuf { config_dir().join("layouts") }
pub fn panes_conf()      -> PathBuf { config_dir().join("pb.panes.conf") }
pub fn hosts_conf()      -> PathBuf { config_dir().join("pb.daemon.conf") }
pub fn pane_dir(mpv_name: &str)     -> PathBuf { config_dir().join(mpv_name.to_lowercase()) }
pub fn pane_scripts(mpv_name: &str) -> PathBuf { pane_dir(mpv_name).join("scripts") }

pub fn pane_socket(mpv_name: &str)   -> PathBuf { pane_dir(mpv_name).join(format!("{}.sock",     mpv_name.to_lowercase())) }
pub fn pane_mpv_conf(mpv_name: &str) -> PathBuf { pane_dir(mpv_name).join(format!("{}.mpv.conf", mpv_name.to_lowercase())) }
pub fn pane_playlist(mpv_name: &str) -> PathBuf { pane_dir(mpv_name).join(format!("{}.m3u",      mpv_name.to_lowercase())) }

// ---------------------------------------------------------------------------
// Structs
//
// Pane.geometry removed — geometry is now owned entirely by layout files.
// Geometry is never stored in panes.conf or on the Pane struct.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Pane {
    pub mpv_name:  String,          // instance name — drives all paths, never changes
    pub pane_name: String,          // display name — TUI and mpv window title, change freely
    pub playlist:  Option<String>,  // external playlist path — passed to mpv at launch
}

#[derive(Debug)]
pub struct Config {
    pub layout: String,
    pub panes:  Vec<Pane>,
    pub home:   String,        // expanded $HOME — use for ~ substitution everywhere
}

// ---------------------------------------------------------------------------
// Host — remote daemon entry from pb.daemon.conf
//
// pb.daemon.conf format:
//
//   # pb.daemon.conf
//   # Leave empty for local-only mode (connects to 127.0.0.1:9090).
//
//   [my-linux-box]
//   address = ws://192.168.1.x:9090
//
//   [studio-display]
//   address = ws://192.168.1.y:9090
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Host {
    pub label:   String,
    pub address: String,
}

pub fn load_hosts() -> Vec<Host> {
    let content = match std::fs::read_to_string(hosts_conf()) {
        Ok(c)  => c,
        Err(_) => return Vec::new(),
    };

    let mut hosts   = Vec::new();
    let mut label:   Option<String> = None;
    let mut address: Option<String> = None;

    macro_rules! flush {
        () => {
            if let (Some(l), Some(a)) = (label.take(), address.take()) {
                hosts.push(Host { label: l, address: a });
            }
        };
    }

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }

        if line.starts_with('[') && line.ends_with(']') {
            flush!();
            label   = Some(line[1..line.len()-1].to_string());
            address = None;
            continue;
        }

        if let Some(eq) = line.find('=') {
            let key = line[..eq].trim();
            let val = line[eq+1..].trim().to_string();
            if key == "address" { address = Some(val); }
        }
    }
    flush!();

    hosts
}

// ---------------------------------------------------------------------------
// Parse pb.panes.conf
// ---------------------------------------------------------------------------

pub fn home_dir() -> String {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string())
}

pub fn expand_tilde(path: &str, home: &str) -> String {
    if path.starts_with('~') {
        path.replacen('~', home, 1)
    } else {
        path.to_string()
    }
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

    macro_rules! flush {
        () => {
            if let Some(ref n) = name {
                panes.push(Pane {
                    mpv_name:  n.clone(),
                    pane_name: pane_name.clone().unwrap_or_else(|| n.clone()),
                    playlist:  playlist.clone(),
                });
            }
        };
    }

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }

        if line.starts_with('[') && line.ends_with(']') {
            flush!();
            name      = Some(line[1..line.len()-1].to_string());
            pane_name = None;
            playlist  = None;
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
