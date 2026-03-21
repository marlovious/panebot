use std::collections::HashMap;
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
pub fn types_conf()      -> PathBuf { config_dir().join("pb.types.conf") }
pub fn scripts_lib()     -> PathBuf { config_dir().join("scripts") }
pub fn pane_dir(n: &str) -> PathBuf { config_dir().join(n.to_lowercase()) }

pub fn pane_socket(n: &str)   -> PathBuf { pane_dir(n).join(format!("{}.sock",     n.to_lowercase())) }
pub fn pane_mpv_conf(n: &str) -> PathBuf { pane_dir(n).join(format!("{}.mpv.conf", n.to_lowercase())) }
pub fn pane_playlist(n: &str) -> PathBuf { pane_dir(n).join(format!("{}.m3u",      n.to_lowercase())) }
pub fn pane_scripts(n: &str)  -> PathBuf { pane_dir(n).join("scripts") }

// ---------------------------------------------------------------------------
// Structs
//
// Pane.geometry removed — geometry is now owned entirely by layout files.
// Geometry is never stored in panes.conf or on the Pane struct.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Pane {
    pub name:      String,
    pub pane_type: String,
    pub playlist:  Option<String>,
}

#[derive(Debug, Clone)]
pub struct PaneType {
    pub options: Vec<String>,
    pub scripts: Vec<String>,
}

#[derive(Debug)]
pub struct Config {
    pub layout: String,
    pub panes:  Vec<Pane>,
}

// ---------------------------------------------------------------------------
// String helpers
// ---------------------------------------------------------------------------

pub fn expand_tilde(path: &str) -> String {
    if path.starts_with('~') {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        path.replacen('~', &home, 1)
    } else {
        path.to_string()
    }
}

// ---------------------------------------------------------------------------
// Parse pb.panes.conf
// ---------------------------------------------------------------------------

pub fn load_config() -> Config {
    let content = match std::fs::read_to_string(panes_conf()) {
        Ok(c)  => c,
        Err(_) => return Config { layout: "pb.left.stack".to_string(), panes: Vec::new() },
    };

    let mut layout   = "pb.left.stack".to_string();
    let mut panes    = Vec::new();
    let mut name:     Option<String> = None;
    let mut ptype    = "video".to_string();
    let mut playlist: Option<String> = None;

    macro_rules! flush {
        () => {
            if let Some(ref n) = name {
                panes.push(Pane {
                    name:      n.clone(),
                    pane_type: ptype.clone(),
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
            name     = Some(line[1..line.len()-1].to_string());
            ptype    = "video".to_string();
            playlist = None;
            continue;
        }

        if let Some(eq) = line.find('=') {
            let key = line[..eq].trim();
            let val = line[eq+1..].trim().to_string();
            match key {
                "layout"   => layout   = val,
                "type"     => ptype    = val,
                "playlist" => playlist = Some(expand_tilde(&val)),
                _          => {}
            }
        }
    }
    flush!();

    Config { layout, panes }
}

// ---------------------------------------------------------------------------
// Parse pb.types.conf
// ---------------------------------------------------------------------------

pub fn load_types() -> HashMap<String, PaneType> {
    let mut types   = HashMap::new();
    let content = match std::fs::read_to_string(types_conf()) {
        Ok(c)  => c,
        Err(_) => return types,
    };

    let mut current: Option<String> = None;
    let mut options: Vec<String>    = Vec::new();
    let mut scripts: Vec<String>    = Vec::new();

    macro_rules! flush {
        () => {
            if let Some(ref n) = current {
                types.insert(n.clone(), PaneType {
                    options: options.clone(),
                    scripts: scripts.clone(),
                });
            }
        };
    }

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }

        if line.starts_with('[') && line.ends_with(']') {
            flush!();
            current = Some(line[1..line.len()-1].to_string());
            options = Vec::new();
            scripts = Vec::new();
            continue;
        }

        if let Some(eq) = line.find('=') {
            let key = line[..eq].trim();
            let val = line[eq+1..].trim().to_string();
            if key == "scripts" {
                scripts = val.split(',').map(|s| s.trim().to_string()).collect();
            } else {
                options.push(format!("{}={}", key, val));
            }
        }
    }
    flush!();

    types
}
