use std::io;
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
// PaneType removed — pane_type is a display label only, shown in TUI,
// does not affect mpv behavior.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Pane {
    pub mpv_name:  String,          // instance name — drives all paths, never changes
    pub pane_name: String,          // display name — TUI and mpv window title, change freely
    pub pane_type: String,          // display label only
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
    let mut ptype     = "video".to_string();
    let mut playlist:  Option<String> = None;

    macro_rules! flush {
        () => {
            if let Some(ref n) = name {
                panes.push(Pane {
                    mpv_name:  n.clone(),
                    pane_name: pane_name.clone().unwrap_or_else(|| n.clone()),
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
            name      = Some(line[1..line.len()-1].to_string());
            pane_name = None;
            ptype     = "video".to_string();
            playlist  = None;
            continue;
        }

        if let Some(eq) = line.find('=') {
            let key = line[..eq].trim();
            let val = line[eq+1..].trim().to_string();
            match key {
                "layout"    => layout    = val,
                "type"      => ptype     = val,
                "pane_name" => pane_name = Some(val),
                "playlist"  => playlist  = Some(expand_tilde(&val, &home)),
                _           => {}
            }
        }
    }
    flush!();

    Config { layout, panes, home }
}

// ---------------------------------------------------------------------------
// M3U helpers
//
// All playlist operations go through these functions.
// The .m3u file is the source of truth — mpv is reloaded after every write.
// ---------------------------------------------------------------------------

// Recursively walk a directory, returning all file paths sorted.
pub fn walk_dir(dir: &str) -> Vec<String> {
    let mut results = Vec::new();
    let path = PathBuf::from(dir.trim_end_matches('/'));
    if let Ok(entries) = std::fs::read_dir(&path) {
        let mut children: Vec<_> = entries.filter_map(|e| e.ok()).collect();
        children.sort_by_key(|e| e.file_name());
        for entry in children {
            let p = entry.path();
            if p.is_dir() {
                let sub = p.to_string_lossy().to_string() + "/";
                results.extend(walk_dir(&sub));
            } else {
                results.push(p.to_string_lossy().to_string());
            }
        }
    }
    results
}

// Read .m3u — returns only non-empty, non-comment lines.
// Directory entries (trailing /) are expanded recursively and written back.
pub fn read_m3u(mpv_name: &str) -> Vec<String> {
    let path = pane_playlist(mpv_name);
    let content = match std::fs::read_to_string(&path) {
        Ok(c)  => c,
        Err(_) => return Vec::new(),
    };

    let raw: Vec<String> = content.lines()
        .filter(|l| { let t = l.trim(); !t.is_empty() && !t.starts_with('#') })
        .map(|l| l.trim().to_string())
        .collect();

    let mut expanded = Vec::new();
    let mut dirty    = false;
    for entry in &raw {
        if entry.ends_with('/') {
            expanded.extend(walk_dir(entry));
            dirty = true;
        } else {
            expanded.push(entry.clone());
        }
    }

    if dirty {
        let _ = write_m3u(mpv_name, &expanded);
    }

    expanded
}

// Write items back to .m3u, preserving the #EXTM3U header.
pub fn write_m3u(mpv_name: &str, items: &[String]) -> io::Result<()> {
    let path = pane_playlist(mpv_name);
    let mut out = String::from("#EXTM3U\n");
    for item in items {
        out.push_str(item);
        out.push('\n');
    }
    std::fs::write(&path, out)
}

// Append one entry, returns the updated list.
pub fn m3u_append(mpv_name: &str, entry: &str) -> io::Result<Vec<String>> {
    let mut items = read_m3u(mpv_name);
    items.push(entry.trim().to_string());
    write_m3u(mpv_name, &items)?;
    Ok(items)
}

// Remove entry at index. Refuses if it is the currently-playing position.
// Returns Ok(Some(items)) on success, Ok(None) if blocked (playing item).
pub fn m3u_remove(mpv_name: &str, idx: usize, current_pos: i64) -> io::Result<Option<Vec<String>>> {
    if current_pos >= 0 && current_pos as usize == idx {
        return Ok(None);
    }
    let mut items = read_m3u(mpv_name);
    if idx < items.len() {
        items.remove(idx);
        write_m3u(mpv_name, &items)?;
    }
    Ok(Some(items))
}

// Query mpv directly for its current playlist and write it to the pane's .m3u.
// Opens a fresh unix socket connection — no daemon involvement.
// Returns the number of items written.
#[cfg(unix)]
pub fn save_playlist(mpv_name: &str) -> io::Result<usize> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    let socket = pane_socket(mpv_name);
    let mut stream = UnixStream::connect(&socket)?;

    let cmd = "{\"command\":[\"get_property\",\"playlist\"]}\n";
    stream.write_all(cmd.as_bytes())?;

    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response)?;

    // Extract "filename":"..." values from the JSON response.
    // Avoids pulling serde_json into lib — the pattern is stable and simple.
    let items: Vec<String> = response
        .split("\"filename\":\"")
        .skip(1)
        .filter_map(|chunk| chunk.split('"').next().map(|s| s.to_string()))
        .collect();

    if items.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "no playlist items"));
    }

    write_m3u(mpv_name, &items)?;
    Ok(items.len())
}
// Returns Ok(None) if nothing is playing.
pub fn m3u_crop(mpv_name: &str, current_pos: i64) -> io::Result<Option<Vec<String>>> {
    if current_pos < 0 {
        return Ok(None);
    }
    let items = read_m3u(mpv_name);
    let idx = current_pos as usize;
    if idx >= items.len() {
        return Ok(None);
    }
    let kept = vec![items[idx].clone()];
    write_m3u(mpv_name, &kept)?;
    Ok(Some(kept))
}
