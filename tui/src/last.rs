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

const C_BG:      Color = Color::Rgb(10, 14, 20);
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

fn pane_dir(name: &str) -> PathBuf { config_dir().join(name) }

fn pane_socket(name: &str) -> PathBuf {
    pane_dir(name).join(format!("{}.sock", name))
}

fn pane_mpv_conf(name: &str) -> PathBuf {
    pane_dir(name).join(format!("{}.mpv.conf", name))
}

fn pane_playlist_file(name: &str) -> PathBuf {
    pane_dir(name).join(format!("{}.m3u", name))
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
    let mut out = String::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[i+1..i+3]) {
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b as char); i += 3; continue;
                }
            }
        }
        out.push(bytes[i] as char); i += 1;
    }
    out
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
        writeln!(f, "# type     = VIDEO | AUDIO | HTTP | YTDLP | RTSP")?;
        writeln!(f, "# slot     = left1-16:9        # slot names defined in ~/.config/panebot/layouts/default.layout")?;
        writeln!(f, "# geometry = 650x366+0+0       # fallback if no slot assigned")?;
        writeln!(f, "# playlist = ~/.config/panebot/name/name.m3u")?;
        writeln!(f)?;
        writeln!(f, "[MUSIC]")?;
        writeln!(f, "socket   = ~/.config/panebot/music/music.sock")?;
        writeln!(f, "type     = VIDEO")?;
        writeln!(f, "slot     = left0-1:1")?;
        writeln!(f, "playlist = ~/.config/panebot/music/music.m3u")?;
        writeln!(f)?;
        writeln!(f, "[MOVIES]")?;
        writeln!(f, "socket   = ~/.config/panebot/movies/movies.sock")?;
        writeln!(f, "type     = VIDEO")?;
        writeln!(f, "slot     = left1-16:9")?;
        writeln!(f, "playlist = ~/.config/panebot/movies/movies.m3u")?;
        writeln!(f)?;
        writeln!(f, "[WEB]")?;
        writeln!(f, "socket   = ~/.config/panebot/web/web.sock")?;
        writeln!(f, "type     = HTTP")?;
        writeln!(f, "slot     = left2-16:9")?;
        writeln!(f, "playlist = ~/.config/panebot/web/web.m3u")?;
        writeln!(f)?;
        writeln!(f, "[TV]")?;
        writeln!(f, "socket   = ~/.config/panebot/tv/tv.sock")?;
        writeln!(f, "type     = VIDEO")?;
        writeln!(f, "slot     = left3-4:3")?;
        writeln!(f, "playlist = ~/.config/panebot/tv/tv.m3u")?;

        // Create pane subdirs for all default panes
        for (name, ptype) in &[("music","VIDEO"),("movies","VIDEO"),("web","HTTP"),("tv","VIDEO")] {
            let _ = create_pane_files(name, ptype);
        }
    } else {
        // Ensure pane dirs exist for all panes already defined
        for pane in load_panes() {
            let _ = create_pane_files(&pane.name.to_lowercase(), &pane.pane_type);
        }
    }

    Ok(BootstrapResult { layout_name: layout_name.to_string(), conf_was_missing })
}

// ---------------------------------------------------------------------------
// Pane model
// ---------------------------------------------------------------------------

struct Pane {
    name:      String,
    socket:    String,
    pane_type: String,
    slot:      Option<String>,
    geometry:  Option<String>,
    playlist:  Option<String>,
    status:    String,
    volume:    i64,
    muted:     bool,
    title:     String,
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
    let mut ptype:    String = "VIDEO".to_string();
    let mut slot:     Option<String> = None;
    let mut geometry: Option<String> = None;
    let mut playlist: Option<String> = None;

    let flush = |name: &Option<String>, sock: &Option<String>, pt: &String,
                  sl: &Option<String>, geo: &Option<String>,
                  pl: &Option<String>, panes: &mut Vec<Pane>| {
        if let (Some(n), Some(s)) = (name, sock) {
            panes.push(Pane {
                name:      n.clone(),
                socket:    s.clone(),
                pane_type: pt.clone(),
                slot:      sl.clone(),
                geometry:  geo.clone(),
                playlist:  pl.clone(),
                status:    "Offline".to_string(),
                volume: 0, muted: false,
                title: "\u{2014}".to_string(),
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
            ptype    = "VIDEO".to_string();
            slot     = None;
            geometry = None;
            playlist = None;
            continue;
        }

        if let Some(eq) = line.find('=') {
            let key = line[..eq].trim();
            let val = line[eq+1..].trim().to_string();
            let val_exp = expand_tilde(&val);
            match key {
                "socket"   => socket   = Some(expand_tilde(&val)),
                "type"     => ptype    = val,
                "slot"     => slot     = if val == "-" { None } else { Some(val) },
                "geometry" => geometry = if val == "-" { None } else { Some(val) },
                "playlist" => playlist = if val == "-" { None } else { Some(val_exp) },
                _ => {}
            }
        }
    }
    flush(&current_name, &socket, &ptype, &slot, &geometry, &playlist, &mut panes);
    panes
}

fn write_pane_to_conf(p: &Pane) -> io::Result<()> {
    let conf = config_dir().join("panes.conf");
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(conf)?;
    writeln!(f)?;
    writeln!(f, "[{}]", p.name)?;
    writeln!(f, "socket   = {}", p.socket)?;
    writeln!(f, "type     = {}", p.pane_type)?;
    writeln!(f, "slot     = {}", p.slot.as_deref().unwrap_or("-"))?;
    writeln!(f, "geometry = {}", p.geometry.as_deref().unwrap_or("-"))?;
    writeln!(f, "playlist = {}", p.playlist.as_deref().unwrap_or("-"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// mpv IPC
// ---------------------------------------------------------------------------

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

fn type_flags(pane_type: &str) -> Vec<&'static str> {
    match pane_type {
        "AUDIO" => vec!["--vid=no",          "--force-window=yes", "--keep-open=yes"],
        "HTTP"  => vec!["--force-window=yes", "--keep-open=yes"],
        "YTDLP" => vec!["--force-window=yes", "--keep-open=yes", "--ytdl-format=bestvideo+bestaudio"],
        "RTSP"  => vec!["--force-window=yes", "--keep-open=yes", "--rtsp-transport=tcp"],
        _       => vec!["--force-window=yes", "--keep-open=yes"],
    }
}

fn launch_pane(pane: &Pane, slots: &SlotMap, playlist_path: Option<&str>) {
    let mut args: Vec<String> = Vec::new();
    args.push(format!("--input-ipc-server={}", pane.socket));
    args.push("--really-quiet".to_string());
    args.push("--pause".to_string());
    args.push("--mute=yes".to_string());
    for flag in type_flags(&pane.pane_type) { args.push(flag.to_string()); }
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
        writeln!(f, "# \"{}\" pane mpv config", name)?;
        writeln!(f, "# Add per-pane mpv overrides here")?;
        writeln!(f, "# Global type defaults: ~/.config/panebot/skels/{}.conf", pane_type)?;
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
        .style(Style::default().bg(C_BG))
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
    boot_lines: &[String],
    entries: &[StartupEntry],
    prompt: Option<&PromptState>,
    complete: bool,
) -> io::Result<()> {
    terminal.draw(|f| {
        let size = f.size();
        f.render_widget(Block::default().style(Style::default().bg(C_BG)), size);

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

        // Header
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
                if complete {
                    Span::styled(" :: Startup Complete :: Press ", Style::default().fg(C_HINT))
                } else {
                    Span::styled("  ::  Starting Up .... ::", Style::default().fg(C_DIM))
                },
                if complete { Span::styled("[Enter]", Style::default().fg(C_CYAN)) } else { Span::raw("") },
            ])).style(Style::default().bg(C_BG)),
            chunks[1],
        );
        f.render_widget(divider_line(size.width as usize, C_DIVIDER), chunks[2]);

        // Combine boot lines + pane entries into one list
        let max_name = entries.iter().map(|e| e.name.len() + 2).max().unwrap_or(8);
        let mut items: Vec<ListItem> = Vec::new();

        // Boot lines rendered with same [PaneBot] :: convention as pane entries
        for line in boot_lines.iter() {
            let item = if line.is_empty() {
                ListItem::new(Line::from(Span::raw(""))).style(Style::default().bg(C_BG))
            } else if line.contains("Bringin' The Panes") {
                ListItem::new(Line::from(vec![
                    Span::styled(":: ", Style::default().fg(C_ORANGE)),
                    Span::styled("Bringin' The Panes ", Style::default().fg(C_DIM)),
                    Span::styled("::", Style::default().fg(C_ORANGE)),
                ])).style(Style::default().bg(C_BG))
            } else {
                let parts: Vec<&str> = line.splitn(3, " :: ").collect();
                let mut spans = vec![
                    Span::styled(parts.get(0).unwrap_or(&"").to_string(), Style::default().fg(C_ORANGE)),
                    Span::styled(" :: ", Style::default().fg(C_DIM)),
                    Span::styled(parts.get(1).unwrap_or(&"").to_string(), Style::default().fg(C_HINT)),
                ];
                if let Some(val) = parts.get(2) {
                    spans.push(Span::styled(" :: ", Style::default().fg(C_DIM)));
                    spans.push(Span::styled(val.to_string(), Style::default().fg(C_CYAN)));
                }
                ListItem::new(Line::from(spans)).style(Style::default().bg(C_BG))
            };
            items.push(item);
        }

        for e in entries {
            let status_color = match e.status.as_str() {
                "Active"  => C_GREEN,
                "Offline" => C_RED,
                _         => C_DIM,
            };
            let padded_name = format!("{:<width$}", format!("\"{}\"", e.name), width = max_name);
            items.push(ListItem::new(Line::from(vec![
                Span::styled("[PaneBot]",  Style::default().fg(C_ORANGE)),
                Span::styled(" :: ",       Style::default().fg(C_DIM)),
                Span::styled(padded_name,  Style::default().fg(Color::White)),
                Span::styled(" :: ",       Style::default().fg(C_DIM)),
                Span::styled(format!("[{}]", e.status), Style::default().fg(status_color)),
            ])).style(Style::default().bg(C_BG)));
        }

        f.render_widget(List::new(items).style(Style::default().bg(C_BG)), chunks[3]);

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

fn startup_sequence(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    layout_name: &str,
) -> io::Result<(Vec<Pane>, SlotMap)> {

    // Bootstrap
    let boot = bootstrap_environment(layout_name)?;
    let slots = load_layout(&boot.layout_name);

    let mut boot_lines = vec![
        "[PaneBot] :: Checking Environment :: Done!".to_string(),
    ];

    let mut panes = load_panes();
    if boot.conf_was_missing {
        boot_lines.push("[PaneBot] :: No Panes Found! :: Creating Default Layout".to_string());
    }

    boot_lines.push(format!("[PaneBot] :: Loading Our Layout :: \"{}\"",
        boot.layout_name.chars().next().unwrap_or(' ').to_uppercase().collect::<String>()
        + &boot.layout_name[1..]));
    boot_lines.push(String::new()); // blank line
    boot_lines.push("[PaneBot] :: Bringin' The Panes ::".to_string());

    if panes.is_empty() {
        // Still no panes after bootstrap — nothing to do
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
            render_startup_screen(terminal, &boot_lines, &entries, None, false)?;
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
                render_startup_screen(terminal, &boot_lines, &entries, Some(&prompt), false)?;
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
            render_startup_screen(terminal, &boot_lines, &entries, None, false)?;
            std::thread::sleep(Duration::from_millis(120));
        }
    }

    render_startup_screen(terminal, &boot_lines, &entries, None, true)?;
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

// ---------------------------------------------------------------------------
// Dashboard render
// ---------------------------------------------------------------------------

fn render_dashboard_header<'a>() -> Paragraph<'a> {
    Paragraph::new(Line::from(vec![
        Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
        Span::styled("  ::  Active Panes ::", Style::default().fg(C_DIM)),
    ])).style(Style::default().bg(C_BG))
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
        Span::styled(format!("{:<12}", format!("\"{}\"", pane.name)),  Style::default().fg(name_color)),
        Span::styled(" :: ",                                           Style::default().fg(C_DIM)),
        Span::styled(format!("[{:<5}]", pane.pane_type),               Style::default().fg(C_CYAN)),
        Span::styled(" :: ",                                           Style::default().fg(C_DIM)),
        Span::styled(format!("[{:<7}]", pane.status),                  Style::default().fg(status_color)),
        Span::styled(" :: ",                                           Style::default().fg(C_DIM)),
        Span::styled(format!("{:<10}", vol_str),                       Style::default().fg(vol_color)),
        Span::styled(" :: ",                                           Style::default().fg(C_DIM)),
        Span::styled(pane.title.clone(),                               Style::default().fg(title_color)),
        if cmd_mode && selected { Span::styled("  [CMD]", Style::default().fg(C_CMD_KEY)) } else { Span::raw("") },
    ])).style(if selected { Style::default().bg(C_CURSOR) } else { Style::default().bg(C_BG) })
}

fn render_dashboard_statusbar<'a>(cmd_mode: bool) -> Paragraph<'a> {
    let spans = if cmd_mode {
        Line::from(vec![
            Span::styled("[Space]",      Style::default().fg(C_CMD_KEY)), Span::styled(" Play :: ",     Style::default().fg(C_CMD_HNT)),
            Span::styled("[m]",          Style::default().fg(C_CMD_KEY)), Span::styled(" Mute :: ",     Style::default().fg(C_CMD_HNT)),
            Span::styled("[Left/Right]", Style::default().fg(C_CMD_KEY)), Span::styled(" Seek 10s :: ", Style::default().fg(C_CMD_HNT)),
            Span::styled("[Up/Down]",    Style::default().fg(C_CMD_KEY)), Span::styled(" Seek 1m :: ",  Style::default().fg(C_CMD_HNT)),
            Span::styled("[=/-]",        Style::default().fg(C_CMD_KEY)), Span::styled(" Vol :: ",      Style::default().fg(C_CMD_HNT)),
            Span::styled("[n/N]",        Style::default().fg(C_CMD_KEY)), Span::styled(" Next/Prev :: ", Style::default().fg(C_CMD_HNT)),
            Span::styled("[R]",          Style::default().fg(C_CMD_KEY)), Span::styled(" Relaunch :: ", Style::default().fg(C_CMD_HNT)),
            Span::styled("[F]",          Style::default().fg(C_CMD_KEY)), Span::styled(" Full :: ", Style::default().fg(C_CMD_HNT)),
            Span::styled("[Tab]",        Style::default().fg(C_CMD_KEY)), Span::styled(" Exit Cmd",     Style::default().fg(C_CMD_HNT)),
        ])
    } else {
        Line::from(vec![
            Span::styled("[j/k]",     Style::default().fg(C_CYAN)), Span::styled(" Select :: ",       Style::default().fg(C_HINT)),
            Span::styled("[Tab]",     Style::default().fg(C_CYAN)), Span::styled(" Cmd Mode :: ",     Style::default().fg(C_HINT)),
            Span::styled("[Enter]",   Style::default().fg(C_CYAN)), Span::styled(" Pane Details :: ", Style::default().fg(C_HINT)),
            Span::styled("[q]",       Style::default().fg(C_CYAN)), Span::styled(" Exit PaneBot",     Style::default().fg(C_HINT)),
        ])
    };
    Paragraph::new(spans).style(Style::default().bg(if cmd_mode { C_CMD_BG } else { C_BG }))
}

// ---------------------------------------------------------------------------
// Playlist render
// ---------------------------------------------------------------------------

fn render_playlist_header<'a>(pane: &Pane) -> Paragraph<'a> {
    let vol_str = if pane.muted { "[Vol:Mute]".to_string() } else { format!("[Vol:{:>3}%]", pane.volume) };
    Paragraph::new(Line::from(vec![
        Span::styled("[PaneBot]",                                    Style::default().fg(C_ORANGE)),
        Span::styled(" :: ",                                         Style::default().fg(C_DIM)),
        Span::styled(format!("\"{}\"", pane.name),                   Style::default().fg(Color::White)),
        Span::styled(" :: ",                                         Style::default().fg(C_DIM)),
        Span::styled(format!("[{}]", pane.pane_type),                Style::default().fg(C_CYAN)),
        Span::styled(" :: ",                                         Style::default().fg(C_DIM)),
        Span::styled(format!("[{}]", pane.status),                   Style::default().fg(if pane.status == "Playing" { C_ORANGE } else { C_DIM })),
        Span::styled(" :: ",                                         Style::default().fg(C_DIM)),
        Span::styled(vol_str,                                        Style::default().fg(C_PINK)),
        Span::styled(" ::",                                          Style::default().fg(C_DIM)),
    ])).style(Style::default().bg(C_BG))
}

fn render_playlist_row(item: &PlaylistItem, selected: bool, item_cmd: bool) -> ListItem<'static> {
    ListItem::new(Line::from(vec![
        Span::styled(if selected { ">> " } else { "   " }, Style::default().fg(C_ORANGE)),
        Span::styled(format!("{:<3}", item.index),         Style::default().fg(C_PINK)),
        Span::styled(" :: ",                               Style::default().fg(C_DIM)),
        Span::styled(item.title.clone(),                   Style::default().fg(if selected { Color::White } else { C_CYAN })),
        if item_cmd && selected { Span::styled("  [CMD]", Style::default().fg(C_CMD_KEY)) } else { Span::raw("") },
    ])).style(if selected { Style::default().bg(C_CURSOR) } else { Style::default().bg(C_BG) })
}

fn render_playlist_statusbar<'a>(
    item_cmd: bool, send_pane_select: bool, move_input: bool, move_buf: &str, add_input: bool, add_buf: &str,
) -> Paragraph<'a> {
    if send_pane_select {
        return Paragraph::new(Line::from(vec![
            Span::styled("[j/k]", Style::default().fg(C_HINT)),
            Span::styled(" Select Pane :: ", Style::default().fg(C_DIM)),
            Span::styled("[Enter]", Style::default().fg(C_CMD_KEY)),
            Span::styled(" Send Here :: ", Style::default().fg(C_CMD_HNT)),
            Span::styled("[Esc]", Style::default().fg(C_CMD_KEY)),
            Span::styled(" Cancel", Style::default().fg(C_CMD_HNT)),
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
            Span::styled("Add to playlist: ", Style::default().fg(C_HINT)),
            Span::styled(add_buf.to_string(), Style::default().fg(Color::White)),
            Span::styled("_", Style::default().fg(C_ORANGE)),
        ])).style(Style::default().bg(C_CMD_BG));
    }
    let spans = if item_cmd {
        Line::from(vec![
            Span::styled("[Enter]", Style::default().fg(C_CMD_KEY)), Span::styled(" Play Now :: ",        Style::default().fg(C_CMD_HNT)),
            Span::styled("[r]",     Style::default().fg(C_CMD_KEY)), Span::styled(" Remove :: ",          Style::default().fg(C_CMD_HNT)),
            Span::styled("[m]",     Style::default().fg(C_CMD_KEY)), Span::styled(" Move Pos :: ",        Style::default().fg(C_CMD_HNT)),
            Span::styled("[s]",     Style::default().fg(C_CMD_KEY)), Span::styled(" Send To Pane :: ",    Style::default().fg(C_CMD_HNT)),
            Span::styled("[Tab]",   Style::default().fg(C_CMD_KEY)), Span::styled(" Exit Modify",         Style::default().fg(C_CMD_HNT)),
        ])
    } else {
        Line::from(vec![
            Span::styled("[j/k]",       Style::default().fg(C_CYAN)), Span::styled(" Select :: ",  Style::default().fg(C_HINT)),
            Span::styled("[Tab]",       Style::default().fg(C_CYAN)), Span::styled(" Modify :: ",  Style::default().fg(C_HINT)),
            Span::styled("[c]",         Style::default().fg(C_CYAN)), Span::styled(" Crop :: ",    Style::default().fg(C_HINT)),
            Span::styled("[n]",         Style::default().fg(C_CYAN)), Span::styled(" Add :: ",     Style::default().fg(C_HINT)),
            Span::styled("[Backspace]", Style::default().fg(C_CYAN)), Span::styled(" Return",      Style::default().fg(C_HINT)),
        ])
    };
    Paragraph::new(spans).style(Style::default().bg(if item_cmd { C_CMD_BG } else { C_BG }))
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

enum Screen {
    Dashboard,
    Playlist(usize),
}

struct DashboardState {
    cursor:   usize,
    cmd_mode: bool,
    last_relaunch: std::collections::HashMap<usize, std::time::Instant>,
}

impl DashboardState {
    fn new() -> Self {
        DashboardState { cursor: 0, cmd_mode: false, last_relaunch: std::collections::HashMap::new() }
    }

    fn can_relaunch(&self, idx: usize) -> bool {
        self.last_relaunch.get(&idx)
            .map(|t| t.elapsed().as_secs() >= 10)
            .unwrap_or(true)
    }

    fn mark_relaunch(&mut self, idx: usize) {
        self.last_relaunch.insert(idx, std::time::Instant::now());
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
    let layout_name = args.windows(2)
        .find(|w| w[0] == "--layout")
        .map(|w| w[1].clone())
        .unwrap_or_else(|| "default".to_string());

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let (mut panes, slots) = startup_sequence(&mut terminal, &layout_name)?;

    if panes.is_empty() {
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        return Ok(());
    }

    let mut screen = Screen::Dashboard;
    let mut dash   = DashboardState::new();
    let mut pl     = PlaylistState::new();

    loop {
        // Refresh pane states; only fetch playlist when on that screen
        for (i, pane) in panes.iter_mut().enumerate() {
            let was_active = pane.status != "Offline";
            refresh_pane(pane);
            // Auto-relaunch if pane went offline unexpectedly and cooldown has passed
            if was_active && pane.status == "Offline" && dash.can_relaunch(i) {
                let stored_pl = pane_playlist_file(&pane.name);
                let playlist_arg = if stored_pl.exists() && stored_pl.metadata().map(|m| m.len() > 0).unwrap_or(false) {
                    Some(stored_pl.to_string_lossy().to_string())
                } else { None };
                launch_pane(pane, &slots, playlist_arg.as_deref());
                dash.mark_relaunch(i);
            }
        }
        if let Screen::Playlist(idx) = &screen {
            pl.items = fetch_playlist(&panes[*idx].socket);
        }

        terminal.draw(|f| {
            let size      = f.size();
            let cmd_active = dash.cmd_mode || pl.item_cmd;
            let div_color  = if cmd_active { Color::Rgb(90, 55, 10) } else { C_DIVIDER };
            let comp_h     = if pl.send_pane_select { panes.len().saturating_sub(1) as u16 + 1 }
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

            f.render_widget(Block::default().style(Style::default().bg(C_BG)), size);

            match &screen {
                Screen::Dashboard => {
                    f.render_widget(render_dashboard_header(), chunks[1]);
                    f.render_widget(divider_line(size.width as usize, C_DIVIDER), chunks[2]);
                    let items: Vec<ListItem> = panes.iter().enumerate()
                        .map(|(i, p)| render_dashboard_row(p, i == dash.cursor, dash.cmd_mode && i == dash.cursor))
                        .collect();
                    let mut st = ListState::default(); st.select(Some(dash.cursor));
                    f.render_stateful_widget(List::new(items).style(Style::default().bg(C_BG)), chunks[3], &mut st);
                    f.render_widget(divider_line(size.width as usize, div_color), chunks[5]);
                    f.render_widget(render_dashboard_statusbar(dash.cmd_mode), chunks[6]);
                }
                Screen::Playlist(idx) => {
                    f.render_widget(render_playlist_header(&panes[*idx]), chunks[1]);
                    f.render_widget(divider_line(size.width as usize, C_DIVIDER), chunks[2]);
                    let items: Vec<ListItem> = pl.items.iter().enumerate()
                        .map(|(i, item)| render_playlist_row(item, i == pl.cursor, pl.item_cmd && i == pl.cursor))
                        .collect();
                    let mut st = ListState::default(); st.select(Some(pl.cursor));
                    f.render_stateful_widget(List::new(items).style(Style::default().bg(C_BG)), chunks[3], &mut st);
                    if pl.send_pane_select {
                        let pane_items: Vec<ListItem> = panes.iter().enumerate()
                            .filter(|(i, _)| *i != *idx)
                            .map(|(i, p)| {
                                let selected = i == pl.send_pane_cursor;
                                ListItem::new(Line::from(vec![
                                    Span::styled(if selected { ">> " } else { "   " }, Style::default().fg(C_ORANGE)),
                                    Span::styled(format!("\"{}\"", p.name), Style::default().fg(if selected { Color::White } else { C_CYAN })),
                                    Span::styled(format!(" [{}]", p.pane_type), Style::default().fg(C_DIM)),
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
                Screen::Dashboard => {
                    let socket  = panes[dash.cursor].socket.clone();
                    let offline = panes[dash.cursor].status == "Offline";

                    if dash.cmd_mode {
                        // R works regardless of online/offline state
                        if key.code == KeyCode::Char('R') {
                            if !offline { cmd_mpv(&socket, &["quit"]); }
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
                                KeyCode::Right      => { cmd_mpv(&socket, &["seek", "10"]); }
                                KeyCode::Left       => { cmd_mpv(&socket, &["seek", "-10"]); }
                                KeyCode::Up         => { cmd_mpv(&socket, &["seek", "60"]); }
                                KeyCode::Char('F')  => { cmd_mpv(&socket, &["cycle", "fullscreen"]); }
                                _ => {}
                            }
                        }
                    } else {
                        match key.code {
                            KeyCode::Char('q') => break,
                            KeyCode::Up   | KeyCode::Char('k') => { if dash.cursor > 0 { dash.cursor -= 1; } }
                            KeyCode::Down | KeyCode::Char('j') => { if dash.cursor < panes.len() - 1 { dash.cursor += 1; } }
                            KeyCode::Tab  => { dash.cmd_mode = true; }
                            KeyCode::Enter => {
                                pl.reset();
                                screen = Screen::Playlist(dash.cursor);
                            }
                            _ => {}
                        }
                    }
                }

                Screen::Playlist(idx) => {
                    let idx    = *idx;
                    let socket = panes[idx].socket.clone();

                    if pl.add_input {
                        let is_stream = matches!(panes[idx].pane_type.as_str(), "HTTP" | "YTDLP" | "RTSP");
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
                                    // Query the filename at this playlist position
                                    if let Some(filename) = query_mpv(&src_socket, "path") {
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
