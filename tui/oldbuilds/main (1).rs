use crossterm::{
    event::{self, Event, KeyCode, poll},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, List, ListItem, ListState, Paragraph},
    Terminal,
};
use std::collections::HashMap;
use std::env;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Colors
// ---------------------------------------------------------------------------

const C_CYAN:    Color = Color::Rgb(60, 160, 160);
const C_ORANGE:  Color = Color::Rgb(224, 128, 48);
const C_PINK:    Color = Color::Rgb(208, 64, 112);
const C_DIM:     Color = Color::Rgb(100, 120, 120);
const C_HINT:    Color = Color::Rgb(140, 160, 160);
const C_CURSOR:  Color = Color::Rgb(28, 42, 58);
const C_CMD_BG:  Color = Color::Rgb(90, 55, 10);
const C_CMD_KEY: Color = Color::Rgb(255, 180, 60);
const C_CMD_HNT: Color = Color::Rgb(180, 130, 60);
const C_DIVIDER: Color = Color::Rgb(40, 58, 58);
const C_COMP_BG: Color = Color::Rgb(18, 28, 38);
const C_RED:     Color = Color::Rgb(200, 60, 60);
const C_GREEN:   Color = Color::Rgb(60, 180, 100);

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

fn config_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".config/panebot")
}

fn layouts_dir() -> PathBuf { config_dir().join("layouts") }

fn layout_path(name: &str) -> PathBuf {
    layouts_dir().join(format!("{}.layout", name))
}

fn pane_dir(name: &str) -> PathBuf { config_dir().join(name.to_lowercase()) }

fn pane_mpv_conf(name: &str) -> PathBuf {
    pane_dir(name).join(format!("{}.mpv.conf", name.to_lowercase()))
}

fn pane_playlist_file(name: &str) -> PathBuf {
    pane_dir(name).join(format!("{}.m3u", name.to_lowercase()))
}

fn pane_state_file(name: &str) -> PathBuf {
    pane_dir(name).join(format!("{}.state", name.to_lowercase()))
}

// ---------------------------------------------------------------------------
// String utilities
// ---------------------------------------------------------------------------

fn expand_tilde(path: &str) -> String {
    if path.starts_with('~') {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        path.replacen('~', &home.to_string_lossy(), 1)
    } else {
        path.to_string()
    }
}

fn url_decode(s: &str) -> String {
    let mut result = s.to_string();
    for _ in 0..3 {
        let next = percent_decode(&result);
        if next == result { break; }
        result = next;
    }
    result
}

fn percent_decode(s: &str) -> String {
    let mut out: Vec<u8> = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i+1..i+3]) {
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b); i += 3; continue;
                }
            }
        }
        out.push(bytes[i]); i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

fn strip_ext(s: &str) -> String {
    if let Some(pos) = s.rfind('.') { if pos > 0 { return s[..pos].to_string(); } }
    s.to_string()
}

fn is_hex_hash(s: &str) -> bool {
    s.len() >= 32 && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn display_name(raw: &str) -> String {
    if raw.starts_with("http://") || raw.starts_with("https://") || raw.starts_with("rtsp://") {
        let segment = raw.split('/').last().unwrap_or(raw);
        let decoded = url_decode(segment);
        // If the last segment is a hex hash, fall back to the domain
        if is_hex_hash(&decoded) || decoded.is_empty() {
            if let Some(host) = raw.split('/').nth(2) {
                return host.split('.').next().unwrap_or(raw).to_string();
            }
        }
        return strip_ext(&decoded);
    }
    if raw.starts_with('/') || raw.starts_with('~') || raw.starts_with('.') {
        let segment = raw.split('/').last().unwrap_or(raw);
        return strip_ext(segment);
    }
    raw.to_string()
}

// ---------------------------------------------------------------------------
// Media file utilities
// ---------------------------------------------------------------------------

static MEDIA_EXT: &[&str] = &[
    "mkv","mp4","avi","mov","wmv","flv","webm","m4v","ts","mpg","mpeg",
    "mp3","flac","ogg","opus","wav","aac","m4a","wma","m3u","m3u8","pls",
];

fn is_media(path: &std::path::Path) -> bool {
    path.extension().and_then(|e| e.to_str())
        .map(|e| MEDIA_EXT.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

fn expand_input(input: &str) -> Vec<String> {
    let expanded = expand_tilde(input);
    let path = std::path::Path::new(&expanded);
    if path.is_dir() {
        let mut files: Vec<String> = Vec::new();
        collect_media_files(path, &mut files);
        files.sort(); files
    } else { vec![expanded] }
}

fn collect_media_files(dir: &std::path::Path, out: &mut Vec<String>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut items: Vec<_> = entries.filter_map(|e| e.ok()).collect();
        items.sort_by_key(|e| e.path());
        for entry in items {
            let path = entry.path();
            if path.is_dir() {
                collect_media_files(&path, out);
            } else if is_media(&path) {
                out.push(path.to_string_lossy().to_string());
            }
        }
    }
}

fn complete_path(input: &str) -> Vec<String> {
    if input.is_empty() { return vec![]; }
    if input.starts_with("http") || input.starts_with("rtsp") { return vec![]; }
    let expanded = expand_tilde(input);
    let (dir, prefix) = if expanded.ends_with('/') {
        (expanded.clone(), String::new())
    } else {
        let p = PathBuf::from(&expanded);
        let d = p.parent().map(|d| d.to_string_lossy().to_string()).unwrap_or_else(|| ".".to_string());
        let f = p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        (d, f)
    };
    let mut matches: Vec<String> = std::fs::read_dir(&dir)
        .into_iter().flatten().filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().to_lowercase().starts_with(&prefix.to_lowercase()))
        .map(|e| {
            let s = e.path().to_string_lossy().to_string();
            if e.path().is_dir() { format!("{}/", s) } else { s }
        })
        .collect();
    matches.sort(); matches.truncate(8); matches
}

// ---------------------------------------------------------------------------
// Layout system
// ---------------------------------------------------------------------------

// Slot map: slot name -> geometry string e.g. "650x366+0+0"
type SlotMap = HashMap<String, String>;

fn load_layout(name: &str) -> SlotMap {
    let path = layout_path(name);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return HashMap::new(),
    };
    let mut slots = HashMap::new();
    let mut current_slot: Option<String> = None;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        if line.starts_with('[') && line.ends_with(']') {
            current_slot = Some(line[1..line.len()-1].to_string());
            continue;
        }
        if let Some(eq) = line.find('=') {
            let key = line[..eq].trim();
            let val = line[eq+1..].trim().to_string();
            if key == "geometry" {
                if let Some(ref slot) = current_slot {
                    slots.insert(slot.clone(), val);
                }
            }
        }
    }
    slots
}

fn write_default_layout() -> io::Result<()> {
    let path = layout_path("default");
    if path.exists() { return Ok(()); }
    let mut f = std::fs::File::create(&path)?;
    writeln!(f, "# panebot default.layout")?;
    writeln!(f, "# Define named slots with geometry strings.")?;
    writeln!(f, "# Panes reference a slot name instead of a raw geometry value.")?;
    writeln!(f)?;
    writeln!(f, "[left0-1:1]")?;
    writeln!(f, "geometry = 366x366+0+0")?;
    writeln!(f)?;
    writeln!(f, "[left1-16:9]")?;
    writeln!(f, "geometry = 650x366+0+374")?;
    writeln!(f)?;
    writeln!(f, "[left2-16:9]")?;
    writeln!(f, "geometry = 650x366+0+748")?;
    writeln!(f)?;
    writeln!(f, "[left3-4:3]")?;
    writeln!(f, "geometry = 650x488+0+1122")?;
    Ok(())
}

fn list_layouts() -> Vec<String> {
    let mut layouts: Vec<String> = std::fs::read_dir(layouts_dir())
        .into_iter().flatten().filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("layout"))
        .filter_map(|e| e.path().file_stem().map(|s| s.to_string_lossy().to_string()))
        .collect();
    layouts.sort();
    layouts
}

// ---------------------------------------------------------------------------
// Bootstrap — idempotent environment setup
// ---------------------------------------------------------------------------

struct BootstrapResult {
    layout_name:      String,
    conf_was_missing: bool,
}

fn bootstrap_environment(layout_name: &str) -> io::Result<BootstrapResult> {
    // Config dir
    std::fs::create_dir_all(config_dir())?;

    // Layouts dir
    std::fs::create_dir_all(layouts_dir())?;

    // Default layout
    write_default_layout()?;

    // Default panes.conf — track whether we created it
    let conf = config_dir().join("panes.conf");
    let conf_was_missing = !conf.exists();

    if conf_was_missing {
        let mut f = std::fs::File::create(&conf)?;
        writeln!(f, "# panebot panes.conf")?;
        writeln!(f, "# [PaneName]")?;
        writeln!(f, "# socket   = ~/.config/panebot/name/name.sock")?;
        writeln!(f, "# type     = video | audio | http | ytdlp | rtsp")?;
        writeln!(f, "# slot     = left1-16:9        # slot names defined in ~/.config/panebot/layouts/default.layout")?;
        writeln!(f, "# geometry = 650x366+0+0       # fallback if no slot assigned")?;
        writeln!(f, "# playlist = ~/.config/panebot/name/name.m3u")?;
        writeln!(f)?;
        writeln!(f, "[music]")?;
        writeln!(f, "socket   = ~/.config/panebot/music/music.sock")?;
        writeln!(f, "type     = video")?;
        writeln!(f, "slot     = left0-1:1")?;
        writeln!(f, "playlist = ~/.config/panebot/music/music.m3u")?;
        writeln!(f)?;
        writeln!(f, "[movies]")?;
        writeln!(f, "socket   = ~/.config/panebot/movies/movies.sock")?;
        writeln!(f, "type     = video")?;
        writeln!(f, "slot     = left1-16:9")?;
        writeln!(f, "playlist = ~/.config/panebot/movies/movies.m3u")?;
        writeln!(f)?;
        writeln!(f, "[web]")?;
        writeln!(f, "socket   = ~/.config/panebot/web/web.sock")?;
        writeln!(f, "type     = http")?;
        writeln!(f, "slot     = left2-16:9")?;
        writeln!(f, "playlist = ~/.config/panebot/web/web.m3u")?;
        writeln!(f)?;
        writeln!(f, "[tv]")?;
        writeln!(f, "socket   = ~/.config/panebot/tv/tv.sock")?;
        writeln!(f, "type     = video")?;
        writeln!(f, "slot     = left3-4:3")?;
        writeln!(f, "playlist = ~/.config/panebot/tv/tv.m3u")?;

        // Create pane subdirs for all default panes
        for (name, ptype) in &[("music","video"),("movies","video"),("web","http"),("tv","video")] {
            let _ = create_pane_files(name, ptype);
        }
    } else {
        for pane in load_panes() {
            let _ = create_pane_files(&pane.name, &pane.pane_type);
        }
    }

    Ok(BootstrapResult { layout_name: layout_name.to_string(), conf_was_missing })
}

// ---------------------------------------------------------------------------
// Pane model
// ---------------------------------------------------------------------------

struct Pane {
    name:         String,
    socket:       String,
    pane_type:    String,
    slot:         Option<String>,
    geometry:     Option<String>,
    playlist:     Option<String>,
    status:       String,
    volume:       i64,
    muted:        bool,
    title:        String,
    resume_index: Option<usize>,
}

fn resolve_geometry(pane: &Pane, slots: &SlotMap) -> Option<String> {
    if let Some(ref slot) = pane.slot {
        if let Some(geo) = slots.get(slot) {
            return Some(geo.clone());
        }
    }
    pane.geometry.clone()
}

fn load_panes() -> Vec<Pane> {
    let conf = config_dir().join("panes.conf");
    let content = match std::fs::read_to_string(&conf) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut panes: Vec<Pane> = Vec::new();
    let mut current_name: Option<String> = None;
    let mut socket:   Option<String> = None;
    let mut ptype:    String = "video".to_string();
    let mut slot:     Option<String> = None;
    let mut geometry: Option<String> = None;
    let mut playlist: Option<String> = None;

    let flush = |name: &Option<String>, sock: &Option<String>, pt: &String,
                  sl: &Option<String>, geo: &Option<String>,
                  pl: &Option<String>, panes: &mut Vec<Pane>| {
        if let (Some(n), Some(s)) = (name, sock) {
            panes.push(Pane {
                name:         n.clone(),
                socket:       s.clone(),
                pane_type:    pt.clone(),
                slot:         sl.clone(),
                geometry:     geo.clone(),
                playlist:     pl.clone(),
                status:       "Offline".to_string(),
                volume: 0, muted: false,
                title:        "\u{2014}".to_string(),
                resume_index: None,
            });
        }
    };

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }

        if line.starts_with('[') && line.ends_with(']') {
            flush(&current_name, &socket, &ptype, &slot, &geometry, &playlist, &mut panes);
            current_name = Some(line[1..line.len()-1].to_string());
            socket   = None;
            ptype    = "video".to_string();
            slot     = None;
            geometry = None;
            playlist = None;
            continue;
        }

        if let Some(eq) = line.find('=') {
            let key = line[..eq].trim();
            let val = line[eq+1..].trim().to_string();
            match key {
                "socket"   => socket   = Some(expand_tilde(&val)),
                "type"     => ptype    = val,
                "slot"     => slot     = if val == "-" { None } else { Some(val) },
                "geometry" => geometry = if val == "-" { None } else { Some(val) },
                "playlist" => playlist = if val == "-" { None } else { Some(expand_tilde(&val)) },
                _ => {}
            }
        }
    }
    flush(&current_name, &socket, &ptype, &slot, &geometry, &playlist, &mut panes);
    panes
}

// Update slot assignments in panes.conf without touching anything else.
fn save_pane_slots(assignments: &[(String, String)]) -> io::Result<()> {
    let conf = config_dir().join("panes.conf");
    let content = std::fs::read_to_string(&conf)?;
    let mut out = String::new();
    let mut current_section: Option<String> = None;
    let mut slot_written = false;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            if let Some(ref name) = current_section {
                if !slot_written {
                    if let Some((_, slot)) = assignments.iter().find(|(n, _)| n.to_lowercase() == name.to_lowercase()) {
                        out.push_str(&format!("slot     = {}\n", slot));
                    }
                }
            }
            current_section = Some(trimmed[1..trimmed.len()-1].to_string());
            slot_written = false;
            out.push_str(line); out.push('\n');
        } else if trimmed.starts_with("slot") {
            if let Some(ref name) = current_section {
                if let Some((_, slot)) = assignments.iter().find(|(n, _)| n.to_lowercase() == name.to_lowercase()) {
                    out.push_str(&format!("slot     = {}\n", slot));
                    slot_written = true;
                    continue;
                }
            }
            out.push_str(line); out.push('\n');
            slot_written = true;
        } else {
            out.push_str(line); out.push('\n');
        }
    }
    // Handle last section
    if let Some(ref name) = current_section {
        if !slot_written {
            if let Some((_, slot)) = assignments.iter().find(|(n, _)| n.to_lowercase() == name.to_lowercase()) {
                out.push_str(&format!("slot     = {}\n", slot));
            }
        }
    }
    std::fs::write(&conf, out)
}

// ---------------------------------------------------------------------------
// mpv IPC
// ---------------------------------------------------------------------------

fn capture_windows() -> Vec<CapturedWindow> {
    use std::io::Write;
    use std::process::Stdio;

    let script = r#"
tell application "System Events"
    repeat with proc in (every process whose name is "mpv")
        repeat with win in (every window of proc)
            set pos to position of win
            set sz to size of win
            set x to (item 1 of pos) * 2
            set y to (item 2 of pos) * 2
            set w to (item 1 of sz) * 2
            set h to (item 2 of sz) * 2
            log (name of win) & " → " & w & "x" & h & "+" & x & "+" & y
        end repeat
    end repeat
end tell
"#;

    let mut child = match std::process::Command::new("osascript")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(script.as_bytes());
    }
    let out = match child.wait_with_output() {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };

    // osascript `log` writes to stderr
    let output = String::from_utf8_lossy(&out.stderr);
    let mut results = Vec::new();

    for line in output.lines() {
        if let Some(arrow) = line.find('→') {
            let name = line[..arrow].trim().to_uppercase();
            let geo  = line[arrow + '→'.len_utf8()..].trim().to_string();
            if !name.is_empty() && !geo.is_empty() {
                results.push(CapturedWindow {
                    title:     name,
                    geometry:  geo,
                    slot_name: String::new(),
                });
            }
        }
    }
    results
}

fn socket_alive(socket: &str) -> bool { UnixStream::connect(socket).is_ok() }

fn query_mpv(socket: &str, property: &str) -> Option<String> {
    let mut stream = UnixStream::connect(socket).ok()?;
    let cmd = format!("{{\"command\":[\"get_property_string\",{}]}}\n",
        serde_json::Value::String(property.to_string()));
    stream.write_all(cmd.as_bytes()).ok()?;
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let line = line.ok()?;
        let v: serde_json::Value = serde_json::from_str(&line).ok()?;
        if v["error"] == "success" {
            return Some(v["data"].as_str().unwrap_or("").to_string());
        }
    }
    None
}

fn cmd_mpv(socket: &str, args: &[&str]) {
    if let Ok(mut stream) = UnixStream::connect(socket) {
        let arr: serde_json::Value = serde_json::Value::Array(
            args.iter().map(|a| serde_json::Value::String(a.to_string())).collect()
        );
        let cmd = format!("{{\"command\":{}}}\n", arr);
        let _ = stream.write_all(cmd.as_bytes());
    }
}

fn refresh_pane(pane: &mut Pane) {
    if !socket_alive(&pane.socket) {
        pane.status = "Offline".to_string();
        pane.title  = "\u{2014}".to_string();
        pane.volume = 0; pane.muted = false;
        return;
    }
    let s = pane.socket.clone();
    pane.status = match query_mpv(&s, "pause").unwrap_or_default().as_str() {
        "yes" => "Stopped", "no" => "Playing", _ => "Stopped",
    }.to_string();
    pane.muted  = query_mpv(&s, "mute").map(|v| v == "yes").unwrap_or(false);
    pane.volume = query_mpv(&s, "volume")
        .and_then(|v| v.parse::<f64>().ok()).map(|v| v as i64).unwrap_or(0);
    pane.title  = query_mpv(&s, "media-title").unwrap_or_else(|| "\u{2014}".to_string());

    // Checkpoint playlist position to disk
    if let Some(i) = query_mpv(&s, "playlist-pos").and_then(|v| v.parse::<usize>().ok()) {
        let state_path = pane_state_file(&pane.name);
        let _ = std::fs::write(&state_path, format!("{}", i));
    }
}

// ---------------------------------------------------------------------------
// Playlist model
// ---------------------------------------------------------------------------

struct PlaylistItem { index: usize, title: String }

fn fetch_playlist(socket: &str) -> Vec<PlaylistItem> {
    let mut stream = match UnixStream::connect(socket) { Ok(s) => s, Err(_) => return Vec::new() };
    let cmd = "{\"command\":[\"get_property\",\"playlist\"]}\n";
    if stream.write_all(cmd.as_bytes()).is_err() { return Vec::new(); }
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let line = match line { Ok(l) => l, Err(_) => continue };
        let v: serde_json::Value = match serde_json::from_str(&line) { Ok(v) => v, Err(_) => continue };
        if v["error"] == "success" {
            if let Some(arr) = v["data"].as_array() {
                return arr.iter().enumerate().map(|(i, item)| {
                    let raw = item["title"].as_str()
                        .or_else(|| item["filename"].as_str())
                        .unwrap_or("Unknown").to_string();
                    PlaylistItem { index: i, title: display_name(&raw) }
                }).collect();
            }
        }
    }
    Vec::new()
}

fn sync_m3u_from_mpv(socket: &str, m3u_path: &std::path::Path) {
    let mut stream = match UnixStream::connect(socket) { Ok(s) => s, Err(_) => return };
    let cmd = "{\"command\":[\"get_property\",\"playlist\"]}\n";
    if stream.write_all(cmd.as_bytes()).is_err() { return; }
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let line = match line { Ok(l) => l, Err(_) => continue };
        let v: serde_json::Value = match serde_json::from_str(&line) { Ok(v) => v, Err(_) => continue };
        if v["error"] == "success" {
            if let Some(arr) = v["data"].as_array() {
                let entries: Vec<&str> = arr.iter()
                    .filter_map(|item| item["filename"].as_str())
                    .collect();
                if let Ok(mut f) = std::fs::File::create(m3u_path) {
                    for e in entries { let _ = writeln!(f, "{}", e); }
                }
            }
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// Pane launch
// ---------------------------------------------------------------------------

fn launch_pane(pane: &Pane, slots: &SlotMap, playlist_path: Option<&str>) {
    let mut args: Vec<String> = Vec::new();
    args.push(format!("--input-ipc-server={}", pane.socket));
    args.push("--really-quiet".to_string());
    args.push("--pause".to_string());
    args.push("--mute=yes".to_string());
    args.push("--force-window=yes".to_string());
    args.push(format!("--title={}", pane.name.to_uppercase()));
    if let Some(idx) = pane.resume_index {
        args.push(format!("--playlist-start={}", idx));
    }
    let mpv_conf = pane_mpv_conf(&pane.name);
    if mpv_conf.exists() { args.push(format!("--include={}", mpv_conf.to_string_lossy())); }
    if let Some(geo) = resolve_geometry(pane, slots) {
        args.push(format!("--geometry={}", geo));
    }
    if let Some(pl) = playlist_path { args.push(pl.to_string()); }
    else { args.push("--idle=yes".to_string()); }
    let _ = std::process::Command::new("mpv")
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn create_pane_files(name: &str, pane_type: &str) -> io::Result<()> {
    let dir = pane_dir(name);
    std::fs::create_dir_all(&dir)?;
    let mpv_conf = pane_mpv_conf(name);
    if !mpv_conf.exists() {
        let mut f = std::fs::File::create(&mpv_conf)?;
        writeln!(f, "# panebot mpv config — \"{}\" [{}]", name, pane_type)?;
        writeln!(f, "# Edit these to tune mpv behavior for this pane.")?;
        writeln!(f, "# These are loaded at launch via --include")?;
        writeln!(f)?;
        match pane_type {
            "audio" => {
                writeln!(f, "vid=no")?;
            }
            "ytdlp" => {
                writeln!(f, "ytdl-format=bestvideo+bestaudio")?;
            }
            "rtsp" => {
                writeln!(f, "rtsp-transport=tcp")?;
                writeln!(f, "# hwdec=auto        # uncomment for hardware decoding")?;
                writeln!(f, "# vf=scale=640:-2   # uncomment to scale down for monitoring")?;
            }
            "http" => {
                writeln!(f, "# ytdl=yes          # uncomment if using yt-dlp for HTTP streams")?;
            }
            _ => {
                writeln!(f, "# VIDEO pane — no special defaults")?;
            }
        }
    }
    let pl = pane_playlist_file(name);
    if !pl.exists() { std::fs::File::create(&pl)?; }
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared render helpers
// ---------------------------------------------------------------------------

fn divider_line(width: usize, color: Color) -> Paragraph<'static> {
    Paragraph::new(Span::styled("-".repeat(width), Style::default().fg(color)))
        .style(Style::default())
}

fn render_completions<'a>(completions: &[String], selected: usize) -> List<'a> {
    List::new(completions.iter().enumerate().map(|(i, c)| {
        let name  = c.split('/').last().unwrap_or(c);
        let color = if c.ends_with('/') { C_ORANGE } else { C_CYAN };
        let style = if i == selected { Style::default().fg(Color::White).bg(C_CURSOR) }
                    else { Style::default().fg(color).bg(C_COMP_BG) };
        ListItem::new(Span::styled(format!(" {} ", name), style))
    }).collect::<Vec<_>>()).style(Style::default().bg(C_COMP_BG))
}

// ---------------------------------------------------------------------------
// Startup
// ---------------------------------------------------------------------------

struct StartupEntry {
    name:    String,
    status:  String,
}

struct PromptState {
    has_stored:  bool,
    has_default: bool,
    browsing:    bool,
    browse_buf:  String,
    completions: Vec<String>,
    comp_sel:    usize,
}

enum LaunchChoice {
    LastPlaylist,
    NewPlaylist(String),
    PaneDefault,
    Empty,
}

fn render_startup_screen(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    entries: &[StartupEntry],
    prompt: Option<&PromptState>,
    complete: bool,
) -> io::Result<()> {
    terminal.draw(|f| {
        let size = f.size();
        f.render_widget(Block::default(), size);

        let prompt_h: u16 = if let Some(p) = prompt {
            if p.browsing { 2 } else { 1 }
        } else { 0 };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(prompt_h),
                Constraint::Length(1),
            ])
            .split(size);

        // Header — above divider
        let header_line = if complete {
            Line::from(vec![
                Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
                Span::styled(" :: Startup Complete :: Press ", Style::default().fg(C_HINT)),
                Span::styled("[Enter]", Style::default().fg(C_CYAN)),
            ])
        } else {
            Line::from(vec![
                Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
                Span::styled(" :: ", Style::default().fg(C_ORANGE)),
                Span::styled("Starting Up", Style::default().fg(C_HINT)),
                Span::styled(" :: ", Style::default().fg(C_ORANGE)),
            ])
        };
        f.render_widget(Paragraph::new(header_line).style(Style::default()), chunks[1]);
        f.render_widget(divider_line(size.width as usize, C_DIVIDER), chunks[2]);

        // Pane entries
        let max_name = entries.iter().map(|e| e.name.len() + 2).max().unwrap_or(8);
        let mut items: Vec<ListItem> = vec![
            ListItem::new(Line::from(vec![
                Span::styled(":: ", Style::default().fg(C_ORANGE)),
                Span::styled("Bringin' The Panes", Style::default().fg(C_HINT)),
                Span::styled(" ::", Style::default().fg(C_ORANGE)),
            ])).style(Style::default()),
            blank_line(),
        ];

        for e in entries {
            let status_color = match e.status.as_str() {
                "Active"  => C_GREEN,
                "Offline" => C_RED,
                _         => C_DIM,
            };
            let padded_name = format!("{:<width$}", format!("\"{}\"", e.name.to_uppercase()), width = max_name);
            items.push(ListItem::new(Line::from(vec![
                Span::styled("[PaneBot]",  Style::default().fg(C_ORANGE)),
                Span::styled(" :: ",       Style::default().fg(C_DIM)),
                Span::styled(padded_name,  Style::default().fg(Color::White)),
                Span::styled(" :: ",       Style::default().fg(C_DIM)),
                Span::styled(format!("[{}]", e.status), Style::default().fg(status_color)),
            ])).style(Style::default()));
        }

        f.render_widget(List::new(items).style(Style::default()), chunks[3]);

        if let Some(p) = prompt {
            let mut opt_spans: Vec<Span> = vec![Span::styled("  ", Style::default())];
            opt_spans.push(Span::styled("[Enter]", Style::default().fg(C_CMD_KEY)));
            opt_spans.push(Span::styled(
                " Last Playlist",
                Style::default().fg(if p.has_stored { C_CMD_HNT } else { C_DIM }),
            ));
            opt_spans.push(Span::styled("  ::  ", Style::default().fg(C_DIM)));
            opt_spans.push(Span::styled("[n]", Style::default().fg(C_CMD_KEY)));
            opt_spans.push(Span::styled(" New Playlist", Style::default().fg(C_CMD_HNT)));
            opt_spans.push(Span::styled("  ::  ", Style::default().fg(C_DIM)));
            opt_spans.push(Span::styled("[e]", Style::default().fg(C_CMD_KEY)));
            opt_spans.push(Span::styled(" Empty Playlist", Style::default().fg(C_CMD_HNT)));
            if p.has_default {
                opt_spans.push(Span::styled("  ::  ", Style::default().fg(C_DIM)));
                opt_spans.push(Span::styled("[p]", Style::default().fg(C_CMD_KEY)));
                opt_spans.push(Span::styled(" Pane Default", Style::default().fg(C_CMD_HNT)));
            }
            let mut lines = vec![Line::from(opt_spans)];
            if p.browsing {
                lines.push(Line::from(vec![
                    Span::styled("  Playlist: ", Style::default().fg(C_HINT)),
                    Span::styled(p.browse_buf.clone(), Style::default().fg(Color::White)),
                    Span::styled("_", Style::default().fg(C_ORANGE)),
                ]));
            }
            f.render_widget(
                Paragraph::new(lines).style(Style::default().bg(C_CMD_BG)),
                chunks[4],
            );
        }
    })?;
    Ok(())
}

fn show_boot_status(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, msg: &str, val: &str) -> io::Result<()> {
    terminal.draw(|f| {
        let size = f.size();
        f.render_widget(Block::default(), size);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(0),
                Constraint::Length(1),
            ])
            .split(size);
        let mut spans = vec![
            Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
            Span::styled(" :: ", Style::default().fg(C_ORANGE)),
            Span::styled(msg.to_string(), Style::default().fg(C_HINT)),
        ];
        if !val.is_empty() {
            spans.push(Span::styled(" :: ", Style::default().fg(C_ORANGE)));
            spans.push(Span::styled(val.to_string(), Style::default().fg(C_CYAN)));
        }
        spans.push(Span::styled(" :: ", Style::default().fg(C_ORANGE)));
        f.render_widget(Paragraph::new(Line::from(spans)).style(Style::default()), chunks[1]);
        f.render_widget(divider_line(size.width as usize, C_DIVIDER), chunks[2]);
    })?;
    std::thread::sleep(Duration::from_millis(1200));
    Ok(())
}

fn startup_sequence(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    layout_name: &str,
) -> io::Result<(Vec<Pane>, SlotMap)> {

    // Bootstrap
    let boot = bootstrap_environment(layout_name)?;
    let slots = load_layout(&boot.layout_name);
    let mut panes = load_panes();

    show_boot_status(terminal, "Checking Environment", "Done!")?;
    if boot.conf_was_missing {
        show_boot_status(terminal, "No Panes Found!", "Creating Default Layout")?;
    }
    show_boot_status(terminal, "Assigning Slots Layout", &boot.layout_name)?;

    if panes.is_empty() {
        return Ok((Vec::new(), slots));
    }

    let mut entries: Vec<StartupEntry> = Vec::new();

    for i in 0..panes.len() {
        let alive = socket_alive(&panes[i].socket);

        if alive {
            refresh_pane(&mut panes[i]);
            entries.push(StartupEntry {
                name:   panes[i].name.clone(),
                status: "Active".to_string(),
            });
            render_startup_screen(terminal, &entries, None, false)?;
            std::thread::sleep(Duration::from_millis(120));
        } else {
            let stored_pl   = pane_playlist_file(&panes[i].name);
            let has_stored  = stored_pl.exists() && stored_pl.metadata().map(|m| m.len() > 0).unwrap_or(false);
            let has_default = panes[i].playlist.is_some();

            entries.push(StartupEntry {
                name:   panes[i].name.clone(),
                status: "Offline".to_string(),
            });

            let mut prompt = PromptState {
                has_stored, has_default,
                browsing:    false,
                browse_buf:  String::new(),
                completions: Vec::new(),
                comp_sel:    0,
            };

            let choice = loop {
                render_startup_screen(terminal, &entries, Some(&prompt), false)?;
                if let Event::Key(k) = event::read()? {
                    if prompt.browsing {
                        match k.code {
                            KeyCode::Enter => {
                                let p = prompt.browse_buf.trim().to_string();
                                break LaunchChoice::NewPlaylist(p);
                            }
                            KeyCode::Esc => { prompt.browsing = false; prompt.browse_buf.clear(); }
                            KeyCode::Tab => {
                                if prompt.completions.is_empty() {
                                    prompt.completions = complete_path(&prompt.browse_buf);
                                    prompt.comp_sel = 0;
                                } else {
                                    prompt.comp_sel = (prompt.comp_sel + 1) % prompt.completions.len();
                                }
                                if !prompt.completions.is_empty() {
                                    prompt.browse_buf = prompt.completions[prompt.comp_sel].clone();
                                }
                            }
                            KeyCode::Backspace => {
                                prompt.browse_buf.pop();
                                prompt.completions = complete_path(&prompt.browse_buf);
                                prompt.comp_sel = 0;
                            }
                            KeyCode::Char(c) => {
                                prompt.browse_buf.push(c);
                                prompt.completions = complete_path(&prompt.browse_buf);
                                prompt.comp_sel = 0;
                            }
                            _ => {}
                        }
                    } else {
                        match k.code {
                            KeyCode::Enter => {
                                break if has_stored { LaunchChoice::LastPlaylist } else { LaunchChoice::Empty };
                            }
                            KeyCode::Char('n') => { prompt.browsing = true; prompt.browse_buf.clear(); }
                            KeyCode::Char('e') => break LaunchChoice::Empty,
                            KeyCode::Char('p') if has_default => break LaunchChoice::PaneDefault,
                            _ => {}
                        }
                    }
                }
            };

            let playlist_arg: Option<String> = match &choice {
                LaunchChoice::LastPlaylist   => Some(stored_pl.to_string_lossy().to_string()),
                LaunchChoice::PaneDefault    => panes[i].playlist.clone(),
                LaunchChoice::NewPlaylist(p) => if p.is_empty() { None } else { Some(p.clone()) },
                LaunchChoice::Empty          => None,
            };

            launch_pane(&panes[i], &slots, playlist_arg.as_deref());

            let mut attempts = 0;
            let started = loop {
                std::thread::sleep(Duration::from_millis(200));
                if socket_alive(&panes[i].socket) { break true; }
                attempts += 1;
                if attempts > 15 { break false; }
            };

            if let Some(last) = entries.iter_mut().rfind(|e| e.name == panes[i].name) {
                last.status = if started { "Active".to_string() } else { "Offline".to_string() };
            }
            render_startup_screen(terminal, &entries, None, false)?;
            std::thread::sleep(Duration::from_millis(120));
        }
    }

    render_startup_screen(terminal, &entries, None, true)?;
    loop {
        if let Event::Key(k) = event::read()? {
            match k.code {
                KeyCode::Enter | KeyCode::Char(' ') => break,
                KeyCode::Char('q') => return Ok((Vec::new(), slots)),
                _ => {}
            }
        }
    }
    Ok((panes, slots))
}

// Shared status line: [PaneBot] :: msg ::
fn panebot_status_line(msg: &str) -> ListItem<'static> {
    ListItem::new(Line::from(vec![
        Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
        Span::styled(" :: ", Style::default().fg(C_ORANGE)),
        Span::styled(msg.to_string(), Style::default().fg(C_DIM)),
        Span::styled(" :: ", Style::default().fg(C_ORANGE)),
    ])).style(Style::default())
}

fn blank_line() -> ListItem<'static> {
    ListItem::new(Line::from(Span::raw(""))).style(Style::default())
}

// Write a layout file from a list of (slot_name, geometry) pairs
fn write_layout_file(name: &str, slots: &[(String, String)]) -> io::Result<()> {
    std::fs::create_dir_all(layouts_dir())?;
    let mut f = std::fs::File::create(layout_path(name))?;
    writeln!(f, "# panebot layout — {}", name)?;
    writeln!(f)?;
    for (slot, geo) in slots {
        writeln!(f, "[{}]", slot)?;
        writeln!(f, "geometry = {}", geo)?;
        writeln!(f)?;
    }
    Ok(())
}

fn render_layout_capture(lc: &LayoutCaptureState) -> (Paragraph<'static>, List<'static>, Paragraph<'static>) {

    let layout_label = if lc.layout_name.is_empty() {
        format!("{}_", lc.name_buf)
    } else {
        lc.layout_name.clone()
    };

    let header = Paragraph::new(Line::from(match &lc.step {
        LayoutCaptureStep::NamingSlots(_) => vec![
            Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
            Span::styled(" :: ", Style::default().fg(C_ORANGE)),
            Span::styled("Define Slot Names For Layout ", Style::default().fg(C_DIM)),
            Span::styled(lc.layout_name.clone(), Style::default().fg(C_CYAN)),
            Span::styled(" :: ", Style::default().fg(C_ORANGE)),
        ],
        LayoutCaptureStep::ModifyPick => vec![
            Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
            Span::styled(" :: ", Style::default().fg(C_ORANGE)),
            Span::styled("Modify Layout", Style::default().fg(C_HINT)),
            Span::styled(" :: ", Style::default().fg(C_ORANGE)),
        ],
        LayoutCaptureStep::ModifyCapture => vec![
            Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
            Span::styled(" :: ", Style::default().fg(C_ORANGE)),
            Span::styled("Confirm Geometry Changes :: ", Style::default().fg(C_HINT)),
            Span::styled(lc.layout_name.clone(), Style::default().fg(C_CYAN)),
            Span::styled(" :: ", Style::default().fg(C_ORANGE)),
        ],
        _ => vec![
            Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
            Span::styled(" :: ", Style::default().fg(C_ORANGE)),
            Span::styled("Layout Capture", Style::default().fg(C_DIM)),
            Span::styled(" :: ", Style::default().fg(C_ORANGE)),
            Span::styled(layout_label, Style::default().fg(C_CYAN)),
        ],
    })).style(Style::default());

    let mut items: Vec<ListItem> = Vec::new();

    match &lc.step {
        LayoutCaptureStep::NameLayout => {
            items.push(ListItem::new(Line::from(vec![
                Span::styled("Layout name: ", Style::default().fg(C_HINT)),
                Span::styled(format!("{}_", lc.name_buf), Style::default().fg(Color::White)),
            ])).style(Style::default()));
        }
        LayoutCaptureStep::NamingSlots(cur) => {
            let cur = *cur;
            if lc.captured.is_empty() {
                items.push(panebot_status_line("No Panes Found — Are mpv windows open?"));
            } else {
                for (i, win) in lc.captured.iter().enumerate() {
                    let slot = if !win.slot_name.is_empty() {
                        win.slot_name.clone()
                    } else if i == cur {
                        format!("{}_", lc.name_buf)
                    } else {
                        String::new()
                    };
                    let is_cur = i == cur;
                    items.push(ListItem::new(Line::from(vec![
                        Span::styled(format!("Window {} ", i + 1), Style::default().fg(C_HINT)),
                        Span::styled(" :: ", Style::default().fg(C_ORANGE)),
                        Span::styled(format!("{:<8}", win.title.clone()), Style::default().fg(if is_cur { Color::White } else { C_DIM })),
                        Span::styled(" :: ", Style::default().fg(C_ORANGE)),
                        Span::styled(format!("{:<20}", win.geometry.clone()), Style::default().fg(C_DIM)),
                        Span::styled(" :: ", Style::default().fg(C_ORANGE)),
                        Span::styled(slot, Style::default().fg(if is_cur { C_CYAN } else { C_DIM })),
                    ])).style(if is_cur { Style::default().bg(C_CURSOR) } else { Style::default() }));
                }
            }
        }
        LayoutCaptureStep::Done => {
            items.push(ListItem::new(Line::from(vec![
                Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
                Span::styled(" :: ", Style::default().fg(C_ORANGE)),
                Span::styled("Layout ", Style::default().fg(C_HINT)),
                Span::styled(lc.layout_name.clone(), Style::default().fg(C_CYAN)),
                Span::styled(" saved. Use ", Style::default().fg(C_HINT)),
                Span::styled("[W]", Style::default().fg(C_CYAN)),
                Span::styled(" to switch layouts.", Style::default().fg(C_HINT)),
                Span::styled(" :: ", Style::default().fg(C_ORANGE)),
            ])).style(Style::default()));
        }
        LayoutCaptureStep::ModifyPick => {
            let layouts = list_layouts();
            if layouts.is_empty() {
                items.push(panebot_status_line("No saved layouts found."));
            } else {
                for (i, name) in layouts.iter().enumerate() {
                    let is_cur = i == lc.modify_sel;
                    items.push(ListItem::new(Line::from(vec![
                        Span::styled(if is_cur { ">> " } else { "   " }, Style::default().fg(C_ORANGE)),
                        Span::styled(name.clone(), Style::default().fg(if is_cur { Color::White } else { C_CYAN })),
                    ])).style(if is_cur { Style::default().bg(C_CURSOR) } else { Style::default() }));
                }
            }
        }
        LayoutCaptureStep::ModifyCapture => {
            for win in lc.captured.iter() {
                let old_geo = lc.modify_old.get(&win.slot_name).cloned().unwrap_or_default();
                let changed = old_geo != win.geometry;
                items.push(ListItem::new(Line::from(vec![
                    Span::styled(format!("{:<20}", win.slot_name.clone()), Style::default().fg(C_DIM)),
                    Span::styled(" :: ", Style::default().fg(C_ORANGE)),
                    Span::styled(format!("{:<20}", old_geo), Style::default().fg(C_DIM)),
                    Span::styled(" → ", Style::default().fg(C_ORANGE)),
                    Span::styled(win.geometry.clone(), Style::default().fg(if changed { C_CYAN } else { C_DIM })),
                ])).style(Style::default()));
            }
        }
    }

    let statusbar = match &lc.step {
        LayoutCaptureStep::NameLayout | LayoutCaptureStep::NamingSlots(_) =>
            Paragraph::new(Line::from(vec![
                Span::styled("[Enter]", Style::default().fg(C_CMD_KEY)), Span::styled(" Confirm  ", Style::default().fg(C_CMD_HNT)),
                Span::styled("[Esc]",   Style::default().fg(C_CMD_KEY)), Span::styled(" Cancel",    Style::default().fg(C_CMD_HNT)),
            ])).style(Style::default().bg(C_CMD_BG)),
        LayoutCaptureStep::ModifyPick =>
            Paragraph::new(Line::from(vec![
                Span::styled("[Tab]",   Style::default().fg(C_CMD_KEY)), Span::styled(" Cycle  ",  Style::default().fg(C_CMD_HNT)),
                Span::styled("[Enter]", Style::default().fg(C_CMD_KEY)), Span::styled(" Select  ", Style::default().fg(C_CMD_HNT)),
                Span::styled("[Esc]",   Style::default().fg(C_CMD_KEY)), Span::styled(" Cancel",   Style::default().fg(C_CMD_HNT)),
            ])).style(Style::default().bg(C_CMD_BG)),
        LayoutCaptureStep::ModifyCapture =>
            Paragraph::new(Line::from(vec![
                Span::styled("[Enter]", Style::default().fg(C_CMD_KEY)), Span::styled(" Save  ", Style::default().fg(C_CMD_HNT)),
                Span::styled("[Esc]",   Style::default().fg(C_CMD_KEY)), Span::styled(" Cancel", Style::default().fg(C_CMD_HNT)),
            ])).style(Style::default().bg(C_CMD_BG)),
        LayoutCaptureStep::Done =>
            Paragraph::new(Line::from(vec![
                Span::styled("[Enter]", Style::default().fg(C_CYAN)), Span::styled(" Return", Style::default().fg(C_HINT)),
            ])).style(Style::default()),
        _ => Paragraph::new(Line::from(vec![])).style(Style::default()),
    };

    (header, List::new(items).style(Style::default()), statusbar)
}

fn render_slot_assign(lc: &LayoutCaptureState) -> (Paragraph<'static>, List<'static>, Paragraph<'static>) {
    let cur = if let LayoutCaptureStep::AssigningPanes(c) = &lc.step { *c } else { 0 };

    let header = Paragraph::new(Line::from(if matches!(lc.step, LayoutCaptureStep::Done) {
        vec![
            Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
            Span::styled(" :: ", Style::default().fg(C_ORANGE)),
            Span::styled("Slots Saved. :: Press ", Style::default().fg(C_HINT)),
            Span::styled("[Enter]", Style::default().fg(C_CYAN)),
            Span::styled(" to Continue.", Style::default().fg(C_HINT)),
            Span::styled(" :: ", Style::default().fg(C_ORANGE)),
        ]
    } else {
        vec![
            Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
            Span::styled(" :: ", Style::default().fg(C_ORANGE)),
            Span::styled("Assign Panes", Style::default().fg(C_DIM)),
            Span::styled(" :: ", Style::default().fg(C_ORANGE)),
            Span::styled(lc.layout_name.clone(), Style::default().fg(C_CYAN)),
            Span::styled(" :: ", Style::default().fg(C_ORANGE)),
        ]
    })).style(Style::default());

    let mut items: Vec<ListItem> = Vec::new();
    for (i, win) in lc.captured.iter().enumerate() {
        let assigned = lc.assign_bufs.get(i).cloned().unwrap_or_default();
        let is_cur = i == cur;
        let val = if is_cur { format!("{}_", assigned) } else { assigned };
        let slot_color = if is_cur { Color::White } else { C_DIM };
        let geo_color  = if is_cur { C_CYAN } else { C_DIM };
        items.push(ListItem::new(Line::from(vec![
            Span::styled(format!("{:<20}", win.slot_name.clone()), Style::default().fg(if is_cur { C_ORANGE } else { C_DIM })),
            Span::styled(" :: ", Style::default().fg(C_ORANGE)),
            Span::styled(format!("{:<18}", win.geometry.clone()), Style::default().fg(geo_color)),
            Span::styled(" :: ", Style::default().fg(C_ORANGE)),
            Span::styled(val, Style::default().fg(slot_color)),
        ])).style(if is_cur { Style::default().bg(C_CURSOR) } else { Style::default() }));
    }

    let statusbar = if matches!(lc.step, LayoutCaptureStep::Done) {
        items.clear();
        Paragraph::new(Line::from(vec![])).style(Style::default().bg(Color::Reset))
    } else {
        Paragraph::new(Line::from(vec![
            Span::styled("[Tab]",   Style::default().fg(C_CMD_KEY)), Span::styled(" Cycle  ",   Style::default().fg(C_CMD_HNT)),
            Span::styled("[Enter]", Style::default().fg(C_CMD_KEY)), Span::styled(" Confirm  ", Style::default().fg(C_CMD_HNT)),
            Span::styled("[Esc]",   Style::default().fg(C_CMD_KEY)), Span::styled(" Cancel",    Style::default().fg(C_CMD_HNT)),
        ])).style(Style::default().bg(C_CMD_BG))
    };

    (header, List::new(items).style(Style::default()), statusbar)
}

// ---------------------------------------------------------------------------
// Dashboard render
// ---------------------------------------------------------------------------

fn render_dashboard_header<'a>(active: usize, total: usize, layout: &str) -> Paragraph<'a> {
    Paragraph::new(Line::from(vec![
        Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
        Span::styled(" :: ", Style::default().fg(C_ORANGE)),
        Span::styled("Active Panes: ", Style::default().fg(C_DIM)),
        Span::styled(format!("{}/{}", active, total), Style::default().fg(C_CYAN)),
        Span::styled(" :: ", Style::default().fg(C_ORANGE)),
        Span::styled("Layout: ", Style::default().fg(C_DIM)),
        Span::styled(layout.to_string(), Style::default().fg(C_CYAN)),
        Span::styled(" :: ", Style::default().fg(C_ORANGE)),
    ])).style(Style::default())
}

fn render_dashboard_row(pane: &Pane, selected: bool, cmd_mode: bool) -> ListItem<'static> {
    let arrow   = if selected { "::" } else { "  " };
    let offline = pane.status == "Offline";
    let vol_str = if offline { "[Offline]".to_string() }
                  else if pane.muted { "[Vol:Mute]".to_string() }
                  else { format!("[Vol:{:>3}%]", pane.volume) };
    let status_color = match pane.status.as_str() { "Playing" => C_ORANGE, "Offline" => C_RED, _ => C_DIM };
    let vol_color    = if offline { C_RED } else if pane.muted { C_DIM } else { C_PINK };
    let name_color   = if selected { Color::White } else { C_ORANGE };
    let title_color  = if offline { C_DIM } else if selected { Color::White } else { C_CYAN };

    ListItem::new(Line::from(vec![
        Span::styled(format!("{} ", arrow),                            Style::default().fg(C_ORANGE)),
        Span::styled(format!("{:<12}", format!("\"{}\"", pane.name.to_uppercase())),  Style::default().fg(name_color)),
        Span::styled(" :: ",                                           Style::default().fg(C_DIM)),
        Span::styled(format!("[{:<5}]", pane.pane_type.to_uppercase()),               Style::default().fg(C_CYAN)),
        Span::styled(" :: ",                                           Style::default().fg(C_DIM)),
        Span::styled(format!("[{:<7}]", pane.status),                  Style::default().fg(status_color)),
        Span::styled(" :: ",                                           Style::default().fg(C_DIM)),
        Span::styled(format!("{:<10}", vol_str),                       Style::default().fg(vol_color)),
        Span::styled(" :: ",                                           Style::default().fg(C_DIM)),
        Span::styled(pane.title.clone(),                               Style::default().fg(title_color)),
        if cmd_mode && selected { Span::styled("  [CMD]", Style::default().fg(C_CMD_KEY)) } else { Span::raw("") },
    ])).style(if selected { Style::default().bg(C_CURSOR) } else { Style::default() })
}

fn render_dashboard_statusbar<'a>(cmd_mode: bool, layout_pick: bool) -> Paragraph<'a> {
    let spans = if layout_pick {
        Line::from(vec![
            Span::styled("[j/k]",   Style::default().fg(C_CYAN)), Span::styled(" Select:",  Style::default().fg(C_HINT)),
            Span::styled("[Enter]", Style::default().fg(C_CYAN)), Span::styled(" Switch:",  Style::default().fg(C_HINT)),
            Span::styled("[Esc]",   Style::default().fg(C_CYAN)), Span::styled(" Cancel",   Style::default().fg(C_HINT)),
        ])
    } else if cmd_mode {
        Line::from(vec![
            Span::styled("[Space]", Style::default().fg(C_CMD_KEY)), Span::styled(" Play:",     Style::default().fg(C_CMD_HNT)),
            Span::styled("[m]",     Style::default().fg(C_CMD_KEY)), Span::styled(" Mute:",     Style::default().fg(C_CMD_HNT)),
            Span::styled("[h/l]",   Style::default().fg(C_CMD_KEY)), Span::styled(" 10s:",      Style::default().fg(C_CMD_HNT)),
            Span::styled("[↑/↓]",   Style::default().fg(C_CMD_KEY)), Span::styled(" 1m:",       Style::default().fg(C_CMD_HNT)),
            Span::styled("[=/-]",   Style::default().fg(C_CMD_KEY)), Span::styled(" Vol:",      Style::default().fg(C_CMD_HNT)),
            Span::styled("[n/N]",   Style::default().fg(C_CMD_KEY)), Span::styled(" Skip:",     Style::default().fg(C_CMD_HNT)),
            Span::styled("[f]",     Style::default().fg(C_CMD_KEY)), Span::styled(" Full:",     Style::default().fg(C_CMD_HNT)),
            Span::styled("[R]",     Style::default().fg(C_CMD_KEY)), Span::styled(" Relaunch:", Style::default().fg(C_CMD_HNT)),
            Span::styled("[Tab]",   Style::default().fg(C_CMD_KEY)), Span::styled(" Exit",      Style::default().fg(C_CMD_HNT)),
        ])
    } else {
        Line::from(vec![
            Span::styled("[j/k]",   Style::default().fg(C_CYAN)), Span::styled(" Select:",   Style::default().fg(C_HINT)),
            Span::styled("[Tab]",   Style::default().fg(C_CYAN)), Span::styled(" Cmd:",      Style::default().fg(C_HINT)),
            Span::styled("[Enter]", Style::default().fg(C_CYAN)), Span::styled(" Playlist:", Style::default().fg(C_HINT)),
            Span::styled("[W]",     Style::default().fg(C_CYAN)), Span::styled(" Layout:",   Style::default().fg(C_HINT)),
            Span::styled("[L]",     Style::default().fg(C_CYAN)), Span::styled(" Capture:",  Style::default().fg(C_HINT)),
            Span::styled("[q]",     Style::default().fg(C_CYAN)), Span::styled(" Exit",      Style::default().fg(C_HINT)),
        ])
    };
    Paragraph::new(spans).style(Style::default().bg(if cmd_mode || layout_pick { C_CMD_BG } else { Color::Reset }))
}

// ---------------------------------------------------------------------------
// Playlist render
// ---------------------------------------------------------------------------

fn render_playlist_header<'a>(pane: &Pane) -> Paragraph<'a> {
    let vol_str = if pane.muted { "[Vol:Mute]".to_string() } else { format!("[Vol:{:>3}%]", pane.volume) };
    Paragraph::new(Line::from(vec![
        Span::styled("[PaneBot]",                                    Style::default().fg(C_ORANGE)),
        Span::styled(" :: ",                                         Style::default().fg(C_ORANGE)),
        Span::styled(format!("\"{}\"", pane.name.to_uppercase()),   Style::default().fg(Color::White)),
        Span::styled(" :: ",                                         Style::default().fg(C_ORANGE)),
        Span::styled(format!("[{}]", pane.pane_type.to_uppercase()), Style::default().fg(C_CYAN)),
        Span::styled(" :: ",                                         Style::default().fg(C_ORANGE)),
        Span::styled(format!("[{}]", pane.status),                   Style::default().fg(if pane.status == "Playing" { C_ORANGE } else { C_DIM })),
        Span::styled(" :: ",                                         Style::default().fg(C_ORANGE)),
        Span::styled(vol_str,                                        Style::default().fg(C_PINK)),
        Span::styled(" ::",                                          Style::default().fg(C_ORANGE)),
    ])).style(Style::default())
}

fn render_playlist_row(item: &PlaylistItem, selected: bool, item_cmd: bool) -> ListItem<'static> {
    ListItem::new(Line::from(vec![
        Span::styled(if selected { ">> " } else { "   " }, Style::default().fg(C_ORANGE)),
        Span::styled(format!("{:<3}", item.index),         Style::default().fg(C_PINK)),
        Span::styled(" :: ",                               Style::default().fg(C_DIM)),
        Span::styled(item.title.clone(),                   Style::default().fg(if selected { Color::White } else { C_CYAN })),
        if item_cmd && selected { Span::styled("  [CMD]", Style::default().fg(C_CMD_KEY)) } else { Span::raw("") },
    ])).style(if selected { Style::default().bg(C_CURSOR) } else { Style::default() })
}

fn render_playlist_statusbar<'a>(
    item_cmd: bool, send_pane_select: bool, move_input: bool, move_buf: &str, add_input: bool, add_buf: &str,
) -> Paragraph<'a> {
    if send_pane_select {
        return Paragraph::new(Line::from(vec![
            Span::styled("[j/k]",   Style::default().fg(C_HINT)),    Span::styled(" Select:",  Style::default().fg(C_DIM)),
            Span::styled("[Enter]", Style::default().fg(C_CMD_KEY)), Span::styled(" Send:",    Style::default().fg(C_CMD_HNT)),
            Span::styled("[Esc]",   Style::default().fg(C_CMD_KEY)), Span::styled(" Cancel",   Style::default().fg(C_CMD_HNT)),
        ])).style(Style::default().bg(C_CMD_BG));
    }
    if move_input {
        return Paragraph::new(Line::from(vec![
            Span::styled("Move to position: ", Style::default().fg(C_HINT)),
            Span::styled(move_buf.to_string(), Style::default().fg(Color::White)),
            Span::styled("_", Style::default().fg(C_ORANGE)),
        ])).style(Style::default().bg(C_CMD_BG));
    }
    if add_input {
        return Paragraph::new(Line::from(vec![
            Span::styled("Add: ", Style::default().fg(C_HINT)),
            Span::styled(add_buf.to_string(), Style::default().fg(Color::White)),
            Span::styled("_", Style::default().fg(C_ORANGE)),
        ])).style(Style::default().bg(C_CMD_BG));
    }
    let spans = if item_cmd {
        Line::from(vec![
            Span::styled("[Enter]", Style::default().fg(C_CMD_KEY)), Span::styled(" Play:",     Style::default().fg(C_CMD_HNT)),
            Span::styled("[r]",     Style::default().fg(C_CMD_KEY)), Span::styled(" Remove:",   Style::default().fg(C_CMD_HNT)),
            Span::styled("[m]",     Style::default().fg(C_CMD_KEY)), Span::styled(" Move:",     Style::default().fg(C_CMD_HNT)),
            Span::styled("[s]",     Style::default().fg(C_CMD_KEY)), Span::styled(" Send:",     Style::default().fg(C_CMD_HNT)),
            Span::styled("[Tab]",   Style::default().fg(C_CMD_KEY)), Span::styled(" Exit",      Style::default().fg(C_CMD_HNT)),
        ])
    } else {
        Line::from(vec![
            Span::styled("[j/k]",       Style::default().fg(C_CYAN)), Span::styled(" Select:",  Style::default().fg(C_HINT)),
            Span::styled("[Tab]",       Style::default().fg(C_CYAN)), Span::styled(" Modify:",  Style::default().fg(C_HINT)),
            Span::styled("[c]",         Style::default().fg(C_CYAN)), Span::styled(" Crop:",    Style::default().fg(C_HINT)),
            Span::styled("[n]",         Style::default().fg(C_CYAN)), Span::styled(" Add:",     Style::default().fg(C_HINT)),
            Span::styled("[Backspace]", Style::default().fg(C_CYAN)), Span::styled(" Return",   Style::default().fg(C_HINT)),
        ])
    };
    Paragraph::new(spans).style(Style::default().bg(if item_cmd { C_CMD_BG } else { Color::Reset }))
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

enum Screen {
    Dashboard,
    Playlist(usize),
    LayoutCapture,
    SlotAssign,
    CaptureReady,
}

#[derive(Clone)]
struct CapturedWindow {
    title:    String,
    geometry: String,
    slot_name: String,
}

enum LayoutCaptureStep {
    NameLayout,
    NamingSlots(usize),
    ModifyPick,
    ModifyCapture,
    AssigningPanes(usize),
    Done,
}

struct LayoutCaptureState {
    step:        LayoutCaptureStep,
    captured:    Vec<CapturedWindow>,
    name_buf:    String,
    layout_name: String,
    assign_bufs: Vec<String>,
    modify_sel:  usize,
    modify_old:  HashMap<String, String>,
}

impl LayoutCaptureState {
    fn new() -> Self {
        LayoutCaptureState {
            step:        LayoutCaptureStep::NameLayout,
            captured:    Vec::new(),
            name_buf:    String::new(),
            layout_name: String::new(),
            assign_bufs: Vec::new(),
            modify_sel:  0,
            modify_old:  HashMap::new(),
        }
    }
}

struct DashboardState {
    cursor:           usize,
    cmd_mode:         bool,
    layout_pick:      bool,
    layout_pick_list: Vec<String>,
    layout_pick_sel:  usize,
    pending_layout:   Option<String>,
}

impl DashboardState {
    fn new() -> Self {
        DashboardState {
            cursor: 0, cmd_mode: false,
            layout_pick: false, layout_pick_list: Vec::new(), layout_pick_sel: 0,
            pending_layout: None,
        }
    }
}

struct PlaylistState {
    items:           Vec<PlaylistItem>,
    cursor:          usize,
    item_cmd:        bool,
    move_input:      bool,
    move_buf:        String,
    add_input:       bool,
    add_buf:         String,
    completions:     Vec<String>,
    comp_sel:        usize,
    send_pane_select: bool,
    send_pane_cursor: usize,
}

impl PlaylistState {
    fn new() -> Self {
        PlaylistState {
            items: Vec::new(), cursor: 0,
            item_cmd: false,
            move_input: false, move_buf: String::new(),
            add_input: false,  add_buf: String::new(),
            completions: Vec::new(), comp_sel: 0,
            send_pane_select: false, send_pane_cursor: 0,
        }
    }

    fn reset(&mut self) {
        self.cursor = 0;
        self.item_cmd = false;
        self.move_input = false; self.move_buf.clear();
        self.add_input = false;  self.add_buf.clear();
        self.completions.clear(); self.comp_sel = 0;
        self.send_pane_select = false; self.send_pane_cursor = 0;
    }
}

// ---------------------------------------------------------------------------
// Main loop
// ---------------------------------------------------------------------------

fn main() -> Result<(), io::Error> {
    // Parse --layout flag, default to "default"
    let args: Vec<String> = env::args().collect();
    let mut layout_name = args.windows(2)
        .find(|w| w[0] == "--layout")
        .map(|w| w[1].clone())
        .unwrap_or_else(|| "default".to_string());

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let (mut panes, mut slots) = startup_sequence(&mut terminal, &layout_name)?;

    if panes.is_empty() {
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        return Ok(());
    }

    let mut screen = Screen::Dashboard;
    let mut dash   = DashboardState::new();
    let mut pl     = PlaylistState::new();
    let mut lc     = LayoutCaptureState::new();

    loop {
        // Refresh pane states; only fetch playlist when on that screen
        for pane in panes.iter_mut() { refresh_pane(pane); }
        if let Screen::Playlist(idx) = &screen {
            pl.items = fetch_playlist(&panes[*idx].socket);
        }
        terminal.draw(|f| {
            let size      = f.size();
            let cmd_active = dash.cmd_mode || pl.item_cmd;
            let div_color  = if cmd_active { Color::Rgb(90, 55, 10) } else { C_DIVIDER };
            let comp_h     = if dash.layout_pick && !dash.layout_pick_list.is_empty() { dash.layout_pick_list.len() as u16 + 1 }
                             else if pl.send_pane_select { panes.len().saturating_sub(1) as u16 + 1 }
                             else if pl.add_input && !pl.completions.is_empty() { pl.completions.len() as u16 + 1 }
                             else { 0 };

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),      // [0] padding
                    Constraint::Length(1),      // [1] header
                    Constraint::Length(1),      // [2] divider
                    Constraint::Min(1),         // [3] content
                    Constraint::Length(comp_h), // [4] completions
                    Constraint::Length(1),      // [5] divider
                    Constraint::Length(1),      // [6] statusbar
                ])
                .split(size);

            f.render_widget(Block::default(), size);

            match &screen {
                Screen::CaptureReady => {
                    f.render_widget(
                        Paragraph::new(Line::from(vec![
                            Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
                            Span::styled(" :: ", Style::default().fg(C_ORANGE)),
                            Span::styled("Layout Capture", Style::default().fg(C_HINT)),
                            Span::styled(" :: ", Style::default().fg(C_ORANGE)),
                        ])),
                        chunks[1],
                    );
                    f.render_widget(divider_line(size.width as usize, C_ORANGE), chunks[2]);
                    let ready_items = vec![
                        ListItem::new(Line::from(vec![
                            Span::styled("[n]", Style::default().fg(C_CMD_KEY)),
                            Span::styled(" New Layout  ", Style::default().fg(C_CMD_HNT)),
                            Span::styled("[m]", Style::default().fg(C_CMD_KEY)),
                            Span::styled(" Modify Existing", Style::default().fg(C_CMD_HNT)),
                        ])).style(Style::default()),
                    ];
                    f.render_widget(List::new(ready_items).style(Style::default()), chunks[3]);
                    f.render_widget(divider_line(size.width as usize, C_ORANGE), chunks[5]);
                    f.render_widget(
                        Paragraph::new(Line::from(vec![
                            Span::styled("[n]", Style::default().fg(C_CMD_KEY)), Span::styled(" New  ", Style::default().fg(C_CMD_HNT)),
                            Span::styled("[m]", Style::default().fg(C_CMD_KEY)), Span::styled(" Modify  ", Style::default().fg(C_CMD_HNT)),
                            Span::styled("[Esc]", Style::default().fg(C_CMD_KEY)), Span::styled(" Cancel", Style::default().fg(C_CMD_HNT)),
                        ])).style(Style::default().bg(C_CMD_BG)),
                        chunks[6],
                    );
                }
                Screen::SlotAssign => {
                    let (header, list, statusbar) = render_slot_assign(&lc);
                    f.render_widget(header, chunks[1]);
                    f.render_widget(divider_line(size.width as usize, C_ORANGE), chunks[2]);
                    f.render_widget(list, chunks[3]);
                    f.render_widget(divider_line(size.width as usize, C_ORANGE), chunks[5]);
                    f.render_widget(statusbar, chunks[6]);
                }
                Screen::LayoutCapture => {
                    let (header, list, statusbar) = render_layout_capture(&lc);
                    f.render_widget(header, chunks[1]);
                    f.render_widget(divider_line(size.width as usize, C_ORANGE), chunks[2]);
                    f.render_widget(list, chunks[3]);
                    f.render_widget(divider_line(size.width as usize, C_ORANGE), chunks[5]);
                    f.render_widget(statusbar, chunks[6]);
                }
                Screen::Dashboard => {
                    let active = panes.iter().filter(|p| p.status != "Offline").count();
                    f.render_widget(render_dashboard_header(active, panes.len(), &layout_name), chunks[1]);
                    f.render_widget(divider_line(size.width as usize, C_DIVIDER), chunks[2]);
                    let items: Vec<ListItem> = panes.iter().enumerate()
                        .map(|(i, p)| render_dashboard_row(p, i == dash.cursor, dash.cmd_mode && i == dash.cursor))
                        .collect();
                    let mut st = ListState::default(); st.select(Some(dash.cursor));
                    f.render_stateful_widget(List::new(items).style(Style::default()), chunks[3], &mut st);
                    if dash.layout_pick && !dash.layout_pick_list.is_empty() {
                        let pick_items: Vec<ListItem> = dash.layout_pick_list.iter().enumerate().map(|(i, name)| {
                            let selected = i == dash.layout_pick_sel;
                            ListItem::new(Line::from(vec![
                                Span::styled(if selected { ">> " } else { "   " }, Style::default().fg(C_ORANGE)),
                                Span::styled(name.clone(), Style::default().fg(if selected { Color::White } else { C_CYAN })),
                            ])).style(if selected { Style::default().bg(C_CURSOR) } else { Style::default().bg(C_COMP_BG) })
                        }).collect();
                        let mut ps = ListState::default(); ps.select(Some(dash.layout_pick_sel));
                        f.render_stateful_widget(List::new(pick_items).style(Style::default().bg(C_COMP_BG)), chunks[4], &mut ps);
                    }
                    f.render_widget(divider_line(size.width as usize, div_color), chunks[5]);
                    f.render_widget(render_dashboard_statusbar(dash.cmd_mode, dash.layout_pick), chunks[6]);
                }
                Screen::Playlist(idx) => {
                    f.render_widget(render_playlist_header(&panes[*idx]), chunks[1]);
                    f.render_widget(divider_line(size.width as usize, C_DIVIDER), chunks[2]);
                    let items: Vec<ListItem> = pl.items.iter().enumerate()
                        .map(|(i, item)| render_playlist_row(item, i == pl.cursor, pl.item_cmd && i == pl.cursor))
                        .collect();
                    let mut st = ListState::default(); st.select(Some(pl.cursor));
                    f.render_stateful_widget(List::new(items).style(Style::default()), chunks[3], &mut st);
                    if pl.send_pane_select {
                        let pane_items: Vec<ListItem> = panes.iter().enumerate()
                            .filter(|(i, _)| *i != *idx)
                            .map(|(i, p)| {
                                let selected = i == pl.send_pane_cursor;
                                ListItem::new(Line::from(vec![
                                    Span::styled(if selected { ">> " } else { "   " }, Style::default().fg(C_ORANGE)),
                                    Span::styled(format!("\"{}\"", p.name.to_uppercase()), Style::default().fg(if selected { Color::White } else { C_CYAN })),
                                    Span::styled(format!(" [{}]", p.pane_type.to_uppercase()), Style::default().fg(C_DIM)),
                                ])).style(if selected { Style::default().bg(C_CURSOR) } else { Style::default().bg(C_COMP_BG) })
                            }).collect();
                        let mut ps = ListState::default(); ps.select(Some(pl.send_pane_cursor));
                        f.render_stateful_widget(List::new(pane_items).style(Style::default().bg(C_COMP_BG)), chunks[4], &mut ps);
                    } else if pl.add_input && !pl.completions.is_empty() {
                        let mut cs = ListState::default(); cs.select(Some(pl.comp_sel));
                        f.render_stateful_widget(render_completions(&pl.completions, pl.comp_sel), chunks[4], &mut cs);
                    }
                    f.render_widget(divider_line(size.width as usize, div_color), chunks[5]);
                    f.render_widget(render_playlist_statusbar(pl.item_cmd, pl.send_pane_select, pl.move_input, &pl.move_buf, pl.add_input, &pl.add_buf), chunks[6]);
                }
            }
        })?;

        if !poll(Duration::from_millis(100))? { continue; }

        if let Event::Key(key) = event::read()? {
            match &screen {
                Screen::CaptureReady => {
                    match key.code {
                        KeyCode::Esc => { screen = Screen::Dashboard; }
                        KeyCode::Char('n') => {
                            lc = LayoutCaptureState::new();
                            lc.captured = capture_windows();
                            screen = Screen::LayoutCapture;
                        }
                        KeyCode::Char('m') => {
                            lc = LayoutCaptureState::new();
                            lc.step = LayoutCaptureStep::ModifyPick;
                            screen = Screen::LayoutCapture;
                        }
                        _ => {}
                    }
                }
                Screen::Dashboard => {
                    let socket  = panes[dash.cursor].socket.clone();
                    let offline = panes[dash.cursor].status == "Offline";

                    if dash.cmd_mode {
                        // R works regardless of online/offline state
                        if key.code == KeyCode::Char('R') {
                            if !offline {
                                panes[dash.cursor].resume_index = query_mpv(&socket, "playlist-pos")
                                    .and_then(|v| v.parse::<usize>().ok());
                                cmd_mpv(&socket, &["quit"]);
                            } else {
                                let state_path = pane_state_file(&panes[dash.cursor].name);
                                if let Ok(contents) = std::fs::read_to_string(&state_path) {
                                    panes[dash.cursor].resume_index = contents.trim().parse().ok();
                                }
                            }
                            // Wait for socket to go cold
                            let mut attempts = 0;
                            while socket_alive(&socket) && attempts < 20 {
                                std::thread::sleep(Duration::from_millis(100));
                                attempts += 1;
                            }
                            let stored_pl = pane_playlist_file(&panes[dash.cursor].name);
                            let playlist_arg = if stored_pl.exists() && stored_pl.metadata().map(|m| m.len() > 0).unwrap_or(false) {
                                Some(stored_pl.to_string_lossy().to_string())
                            } else { None };
                            launch_pane(&panes[dash.cursor], &slots, playlist_arg.as_deref());
                            // Clear resume position after use
                            panes[dash.cursor].resume_index = None;
                            dash.cmd_mode = false;
                        } else if offline {
                            if key.code == KeyCode::Tab { dash.cmd_mode = false; }
                        } else {
                            match key.code {
                                KeyCode::Tab        => { dash.cmd_mode = false; }
                                KeyCode::Char(' ')  => { cmd_mpv(&socket, &["cycle", "pause"]); }
                                KeyCode::Char('m')  => { cmd_mpv(&socket, &["cycle", "mute"]); }
                                KeyCode::Char('n')  => { cmd_mpv(&socket, &["playlist-next"]); }
                                KeyCode::Char('N')  => { cmd_mpv(&socket, &["playlist-prev"]); }
                                KeyCode::Char('=')  => { cmd_mpv(&socket, &["add", "volume", "5"]); }
                                KeyCode::Char('-')  => { cmd_mpv(&socket, &["add", "volume", "-5"]); }
                                KeyCode::Right | KeyCode::Char('l') => { cmd_mpv(&socket, &["seek", "10"]); }
                                KeyCode::Left  | KeyCode::Char('h') => { cmd_mpv(&socket, &["seek", "-10"]); }
                                KeyCode::Up                         => { cmd_mpv(&socket, &["seek", "60"]); }
                                KeyCode::Down                       => { cmd_mpv(&socket, &["seek", "-60"]); }
                                KeyCode::Char('f')  => { cmd_mpv(&socket, &["cycle", "fullscreen"]); }
                                _ => {}
                            }
                        }
                    } else if dash.layout_pick {
                        match key.code {
                            KeyCode::Esc => { dash.layout_pick = false; }
                            KeyCode::Up   | KeyCode::Char('k') => { if dash.layout_pick_sel > 0 { dash.layout_pick_sel -= 1; } }
                            KeyCode::Down | KeyCode::Char('j') => { if dash.layout_pick_sel < dash.layout_pick_list.len().saturating_sub(1) { dash.layout_pick_sel += 1; } }
                            KeyCode::Enter => {
                                if let Some(name) = dash.layout_pick_list.get(dash.layout_pick_sel).cloned() {
                                    let new_slots = load_layout(&name);
                                    lc = LayoutCaptureState::new();
                                    lc.layout_name = name.clone();
                                    lc.captured = {
                                        let mut v: Vec<CapturedWindow> = new_slots.iter().map(|(slot, geo)| CapturedWindow {
                                            title:     slot.clone(),
                                            geometry:  geo.clone(),
                                            slot_name: slot.clone(),
                                        }).collect();
                                        v.sort_by(|a, b| a.title.cmp(&b.title));
                                        v
                                    };
                                    lc.assign_bufs = vec![String::new(); lc.captured.len()];
                                    lc.step = LayoutCaptureStep::AssigningPanes(0);
                                    dash.pending_layout = Some(name);
                                    dash.layout_pick = false;
                                    screen = Screen::SlotAssign;
                                }
                            }
                            _ => {}
                        }
                    } else {
                        match key.code {
                            KeyCode::Char('q') => break,
                            KeyCode::Up   | KeyCode::Char('k') => { if dash.cursor > 0 { dash.cursor -= 1; } }
                            KeyCode::Down | KeyCode::Char('j') => { if dash.cursor < panes.len() - 1 { dash.cursor += 1; } }
                            KeyCode::Tab  => { dash.cmd_mode = true; }
                            KeyCode::Char('W') => {
                                dash.layout_pick_list = list_layouts();
                                dash.layout_pick_sel = 0;
                                dash.layout_pick = true;
                            }
                            KeyCode::Char('L') => {
                                screen = Screen::CaptureReady;
                            }
                            KeyCode::Enter => {
                                pl.reset();
                                screen = Screen::Playlist(dash.cursor);
                            }
                            _ => {}
                        }
                    }
                }

                Screen::LayoutCapture => {
                    match key.code {
                        KeyCode::Esc => { screen = Screen::Dashboard; }
                        KeyCode::Enter => {
                            match &lc.step {
                                LayoutCaptureStep::NameLayout => {
                                    let name = lc.name_buf.trim().to_string();
                                    if !name.is_empty() {
                                        lc.layout_name = name;
                                        lc.name_buf.clear();
                                        lc.step = LayoutCaptureStep::NamingSlots(0);
                                    }
                                }
                                LayoutCaptureStep::NamingSlots(cur) => {
                                    let cur = *cur;
                                    let name = lc.name_buf.trim().to_string();
                                    if !name.is_empty() {
                                        if cur < lc.captured.len() { lc.captured[cur].slot_name = name; }
                                        let next = cur + 1;
                                        lc.name_buf.clear();
                                        let len = lc.captured.len();
                                        if next < len {
                                            lc.step = LayoutCaptureStep::NamingSlots(next);
                                        } else {
                                            let slots: Vec<(String, String)> = lc.captured.iter()
                                                .map(|win| (win.slot_name.clone(), win.geometry.clone()))
                                                .collect();
                                            let _ = write_layout_file(&lc.layout_name, &slots);
                                            lc.name_buf.clear();
                                            lc.step = LayoutCaptureStep::Done;
                                        }
                                    }
                                }
                                LayoutCaptureStep::Done => { screen = Screen::Dashboard; }
                                LayoutCaptureStep::ModifyPick => {
                                    let layouts = list_layouts();
                                    if let Some(name) = layouts.get(lc.modify_sel) {
                                        let existing = load_layout(name);
                                        lc.layout_name = name.clone();
                                        lc.modify_old = existing.clone();
                                        // Capture windows and match to existing slots by order
                                        let captured = capture_windows();
                                        let slot_names: Vec<String> = {
                                            let mut v: Vec<String> = existing.keys().cloned().collect();
                                            v.sort();
                                            v
                                        };
                                        lc.captured = captured.into_iter().enumerate().map(|(i, mut w)| {
                                            w.slot_name = slot_names.get(i).cloned().unwrap_or_else(|| format!("slot{}", i + 1));
                                            w
                                        }).collect();
                                        lc.step = LayoutCaptureStep::ModifyCapture;
                                    }
                                }
                                LayoutCaptureStep::ModifyCapture => {
                                    let slots: Vec<(String, String)> = lc.captured.iter()
                                        .map(|win| (win.slot_name.clone(), win.geometry.clone()))
                                        .collect();
                                    let _ = write_layout_file(&lc.layout_name, &slots);
                                    lc.step = LayoutCaptureStep::Done;
                                }
                                _ => {}
                            }
                        }
                        KeyCode::Tab => {
                            if matches!(lc.step, LayoutCaptureStep::ModifyPick) {
                                let max = list_layouts().len().saturating_sub(1);
                                lc.modify_sel = if lc.modify_sel >= max { 0 } else { lc.modify_sel + 1 };
                            }
                        }
                        KeyCode::Backspace => {
                            if matches!(lc.step, LayoutCaptureStep::NameLayout | LayoutCaptureStep::NamingSlots(_)) {
                                lc.name_buf.pop();
                            }
                        }
                        KeyCode::Char(c) => {
                            if matches!(lc.step, LayoutCaptureStep::NameLayout | LayoutCaptureStep::NamingSlots(_)) {
                                lc.name_buf.push(c);
                            }
                        }
                        _ => {}
                    }
                }

                Screen::SlotAssign => {
                    match key.code {
                        KeyCode::Esc => { dash.pending_layout = None; screen = Screen::Dashboard; }
                        KeyCode::Enter => {
                            if matches!(lc.step, LayoutCaptureStep::Done) {
                                if let Some(new_layout_name) = dash.pending_layout.take() {
                                    let new_slots = load_layout(&new_layout_name);
                                    for pane in panes.iter_mut() {
                                        if pane.status == "Offline" { continue; }
                                        pane.resume_index = query_mpv(&pane.socket, "playlist-pos")
                                            .and_then(|v| v.parse().ok());
                                        cmd_mpv(&pane.socket, &["quit"]);
                                    }
                                    std::thread::sleep(Duration::from_millis(500));
                                    for pane in panes.iter_mut() {
                                        let stored_pl = pane_playlist_file(&pane.name);
                                        let pl_arg = if stored_pl.exists() && stored_pl.metadata().map(|m| m.len() > 0).unwrap_or(false) {
                                            Some(stored_pl.to_string_lossy().to_string())
                                        } else { None };
                                        launch_pane(pane, &new_slots, pl_arg.as_deref());
                                        pane.resume_index = None;
                                    }
                                    slots = new_slots;
                                    layout_name = new_layout_name;
                                }
                                screen = Screen::Dashboard;
                            } else if let LayoutCaptureStep::AssigningPanes(cur) = &lc.step {
                                let cur = *cur;
                                let pane_name = lc.assign_bufs.get(cur).cloned().unwrap_or_default().trim().to_string();
                                if !pane_name.is_empty() {
                                    let slot_name = lc.captured.get(cur).map(|w| w.slot_name.clone()).unwrap_or_default();
                                    if let Some(pane) = panes.iter_mut().find(|p| p.name.to_uppercase() == pane_name.to_uppercase()) {
                                        pane.slot = Some(slot_name);
                                    }
                                }
                                let next = cur + 1;
                                let len = lc.captured.len();
                                lc.step = if next < len {
                                    if next >= lc.assign_bufs.len() { lc.assign_bufs.push(String::new()); }
                                    LayoutCaptureStep::AssigningPanes(next)
                                } else {
                                    let assignments: Vec<(String, String)> = panes.iter()
                                        .filter_map(|p| p.slot.clone().map(|s| (p.name.clone(), s)))
                                        .collect();
                                    let _ = save_pane_slots(&assignments);
                                    LayoutCaptureStep::Done
                                };
                            }
                        }
                        KeyCode::Tab => {
                            if let LayoutCaptureStep::AssigningPanes(cur) = &lc.step {
                                let cur = *cur;
                                let n = panes.len();
                                if n > 0 {
                                    while lc.assign_bufs.len() <= cur { lc.assign_bufs.push(String::new()); }
                                    let current = &lc.assign_bufs[cur];
                                    let idx = panes.iter().position(|p| p.name.to_uppercase() == current.to_uppercase()).map(|i| (i + 1) % n).unwrap_or(0);
                                    lc.assign_bufs[cur] = panes[idx].name.to_uppercase();
                                }
                            }
                        }
                        KeyCode::BackTab => {
                            if let LayoutCaptureStep::AssigningPanes(cur) = &lc.step {
                                let cur = *cur;
                                if cur > 0 {
                                    lc.step = LayoutCaptureStep::AssigningPanes(cur - 1);
                                }
                            }
                        }
                        KeyCode::Backspace => {
                            if let LayoutCaptureStep::AssigningPanes(cur) = &lc.step {
                                let cur = *cur;
                                while lc.assign_bufs.len() <= cur { lc.assign_bufs.push(String::new()); }
                                lc.assign_bufs[cur].pop();
                            }
                        }
                        KeyCode::Char(c) => {
                            if let LayoutCaptureStep::AssigningPanes(cur) = &lc.step {
                                let cur = *cur;
                                while lc.assign_bufs.len() <= cur { lc.assign_bufs.push(String::new()); }
                                lc.assign_bufs[cur].push(c);
                            }
                        }
                        _ => {}
                    }
                }

                Screen::Playlist(idx) => {
                    let idx    = *idx;
                    let socket = panes[idx].socket.clone();

                    if pl.add_input {
                        let is_stream = matches!(panes[idx].pane_type.as_str(), "http" | "ytdlp" | "rtsp");
                        match key.code {
                            KeyCode::Esc => { pl.add_input = false; pl.add_buf.clear(); pl.completions.clear(); }
                            KeyCode::Enter => {
                                let input = pl.add_buf.trim().to_string();
                                if !input.is_empty() {
                                    let m3u_path = pane_playlist_file(&panes[idx].name);
                                    if is_stream {
                                        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&m3u_path) {
                                            let _ = writeln!(f, "{}", input);
                                        }
                                        cmd_mpv(&socket, &["loadfile", &input, "append-play"]);
                                    } else {
                                        let files = expand_input(&input);
                                        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&m3u_path) {
                                            for path in &files { let _ = writeln!(f, "{}", path); }
                                        }
                                        for path in &files {
                                            cmd_mpv(&socket, &["loadfile", path, "append-play"]);
                                        }
                                    }
                                }
                                pl.add_input = false; pl.add_buf.clear(); pl.completions.clear();
                            }
                            KeyCode::Tab => {
                                if !is_stream {
                                    if pl.completions.is_empty() { pl.completions = complete_path(&pl.add_buf); pl.comp_sel = 0; }
                                    else { pl.comp_sel = (pl.comp_sel + 1) % pl.completions.len(); }
                                    if !pl.completions.is_empty() { pl.add_buf = pl.completions[pl.comp_sel].clone(); }
                                }
                            }
                            KeyCode::BackTab => {
                                if !is_stream && !pl.completions.is_empty() {
                                    pl.comp_sel = if pl.comp_sel == 0 { pl.completions.len() - 1 } else { pl.comp_sel - 1 };
                                    pl.add_buf = pl.completions[pl.comp_sel].clone();
                                }
                            }
                            KeyCode::Backspace => {
                                pl.add_buf.pop();
                                pl.completions.clear(); pl.comp_sel = 0;
                            }
                            KeyCode::Char(c) => {
                                pl.add_buf.push(c);
                                pl.completions.clear(); pl.comp_sel = 0;
                            }
                            _ => {}
                        }
                    } else if pl.move_input {
                        match key.code {
                            KeyCode::Char(c) if c.is_ascii_digit() => { pl.move_buf.push(c); }
                            KeyCode::Backspace => { pl.move_buf.pop(); }
                            KeyCode::Enter => {
                                if let Ok(dest) = pl.move_buf.parse::<usize>() {
                                    if !pl.items.is_empty() {
                                        let src = pl.items[pl.cursor].index;
                                        cmd_mpv(&socket, &["playlist-move", &src.to_string(), &dest.to_string()]);
                                        let m3u_path = pane_playlist_file(&panes[idx].name);
                                        sync_m3u_from_mpv(&socket, &m3u_path);
                                    }
                                }
                                pl.move_input = false; pl.move_buf.clear();
                            }
                            KeyCode::Esc => { pl.move_input = false; pl.move_buf.clear(); }
                            _ => {}
                        }
                    } else if pl.send_pane_select {
                        match key.code {
                            KeyCode::Esc => { pl.send_pane_select = false; }
                            KeyCode::Up   | KeyCode::Char('k') => { if pl.send_pane_cursor > 0 { pl.send_pane_cursor -= 1; } }
                            KeyCode::Down | KeyCode::Char('j') => { if pl.send_pane_cursor < panes.len() - 1 { pl.send_pane_cursor += 1; } }
                            KeyCode::Enter => {
                                let dest_idx = pl.send_pane_cursor;
                                if dest_idx != idx && !pl.items.is_empty() {
                                    let item_idx = pl.items[pl.cursor].index;
                                    // Get the raw filename from mpv before removing
                                    let src_socket = panes[idx].socket.clone();
                                    let dest_socket = panes[dest_idx].socket.clone();
                                    // Query the filename at the selected playlist position
                                    let filename_prop = format!("playlist/{}/filename", item_idx);
                                    if let Some(filename) = query_mpv(&src_socket, &filename_prop) {
                                        // Add to destination pane
                                        cmd_mpv(&dest_socket, &["loadfile", &filename, "append-play"]);
                                        let dest_m3u = pane_playlist_file(&panes[dest_idx].name);
                                        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&dest_m3u) {
                                            let _ = writeln!(f, "{}", filename);
                                        }
                                        // Remove from source pane
                                        cmd_mpv(&src_socket, &["playlist-remove", &item_idx.to_string()]);
                                        let src_m3u = pane_playlist_file(&panes[idx].name);
                                        sync_m3u_from_mpv(&src_socket, &src_m3u);
                                        if pl.cursor > 0 { pl.cursor -= 1; }
                                    }
                                }
                                pl.send_pane_select = false;
                                pl.item_cmd = false;
                            }
                            _ => {}
                        }
                    } else if pl.item_cmd {
                        match key.code {
                            KeyCode::Tab => { pl.item_cmd = false; }
                            KeyCode::Enter => {
                                if !pl.items.is_empty() {
                                    let ii = pl.items[pl.cursor].index;
                                    cmd_mpv(&socket, &["set_property", "playlist-pos", &ii.to_string()]);
                                }
                                pl.item_cmd = false;
                            }
                            KeyCode::Char('r') => {
                                if !pl.items.is_empty() {
                                    let ii = pl.items[pl.cursor].index;
                                    cmd_mpv(&socket, &["playlist-remove", &ii.to_string()]);
                                    let m3u_path = pane_playlist_file(&panes[idx].name);
                                    sync_m3u_from_mpv(&socket, &m3u_path);
                                    if pl.cursor > 0 { pl.cursor -= 1; }
                                }
                                pl.item_cmd = false;
                            }
                            KeyCode::Char('m') => { pl.move_input = true; pl.move_buf.clear(); }
                            KeyCode::Char('s') => {
                                pl.send_pane_select = true;
                                pl.send_pane_cursor = if idx > 0 { 0 } else { 1.min(panes.len() - 1) };
                            }
                            _ => {}
                        }
                    } else {
                        match key.code {
                            KeyCode::Up   | KeyCode::Char('k') => { if pl.cursor > 0 { pl.cursor -= 1; } }
                            KeyCode::Down | KeyCode::Char('j') => { if pl.cursor < pl.items.len().saturating_sub(1) { pl.cursor += 1; } }
                            KeyCode::Tab  => { pl.item_cmd = true; }
                            KeyCode::Char('c') => { cmd_mpv(&socket, &["playlist-clear"]); }
                            KeyCode::Char('n') => { pl.add_input = true; pl.add_buf.clear(); pl.completions.clear(); }
                            KeyCode::Backspace => {
                                screen = Screen::Dashboard;
                                pl.reset();
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}
