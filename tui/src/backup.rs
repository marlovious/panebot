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

fn expand_tilde(path: &str) -> String {
    if path.starts_with('~') {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        path.replacen('~', &home.to_string_lossy(), 1)
    } else {
        path.to_string()
    }
}

// ---------------------------------------------------------------------------
// panes.conf  INI format
//
// [PaneName]
// socket   = /path/to/name.sock
// type     = video
// geometry = 650x366+0+0
// playlist = /path/to/name.m3u
// ---------------------------------------------------------------------------

struct Pane {
    name:      String,
    socket:    String,
    pane_type: String,
    geometry:  Option<String>,
    playlist:  Option<String>,
    // live state
    status:    String,
    volume:    i64,
    muted:     bool,
    title:     String,
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
    let mut geometry: Option<String> = None;
    let mut playlist: Option<String> = None;

    let flush = |name: &Option<String>, sock: &Option<String>, pt: &String,
                  geo: &Option<String>, pl: &Option<String>, panes: &mut Vec<Pane>| {
        if let (Some(n), Some(s)) = (name, sock) {
            panes.push(Pane {
                name:      n.clone(),
                socket:    s.clone(),
                pane_type: pt.clone(),
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
            // flush previous pane
            flush(&current_name, &socket, &ptype, &geometry, &playlist, &mut panes);
            current_name = Some(line[1..line.len()-1].to_string());
            socket   = None;
            ptype    = "video".to_string();
            geometry = None;
            playlist = None;
            continue;
        }

        if let Some(eq) = line.find('=') {
            let key = line[..eq].trim();
            let val = line[eq+1..].trim().to_string();
            let val_exp = expand_tilde(&val);
            match key {
                "socket"   => socket   = Some(val_exp),
                "type"     => ptype    = val,
                "geometry" => geometry = if val == "-" { None } else { Some(val) },
                "playlist" => playlist = if val == "-" { None } else { Some(val_exp) },
                _ => {}
            }
        }
    }
    flush(&current_name, &socket, &ptype, &geometry, &playlist, &mut panes);
    panes
}

fn write_pane_to_conf(p: &Pane) -> io::Result<()> {
    let conf = config_dir().join("panes.conf");
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(conf)?;
    writeln!(f)?;
    writeln!(f, "[{}]", p.name)?;
    writeln!(f, "socket   = {}", p.socket)?;
    writeln!(f, "type     = {}", p.pane_type)?;
    writeln!(f, "geometry = {}", p.geometry.as_deref().unwrap_or("-"))?;
    writeln!(f, "playlist = {}", p.playlist.as_deref().unwrap_or("-"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Display name cleaning
// ---------------------------------------------------------------------------

fn display_name(raw: &str) -> String {
    if raw.starts_with("http://") || raw.starts_with("https://") || raw.starts_with("rtsp://") {
        let segment = raw.split('/').last().unwrap_or(raw);
        return strip_ext(&url_decode(segment));
    }
    if raw.starts_with('/') || raw.starts_with('~') || raw.starts_with('.') {
        let segment = raw.split('/').last().unwrap_or(raw);
        return strip_ext(segment);
    }
    raw.to_string()
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

// ---------------------------------------------------------------------------
// Media / directory expansion
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
        let mut files: Vec<String> = std::fs::read_dir(path)
            .into_iter().flatten().filter_map(|e| e.ok())
            .filter(|e| e.path().is_file() && is_media(&e.path()))
            .map(|e| e.path().to_string_lossy().to_string())
            .collect();
        files.sort(); files
    } else { vec![expanded] }
}

// ---------------------------------------------------------------------------
// Path completion
// ---------------------------------------------------------------------------

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
// MPV IPC
// ---------------------------------------------------------------------------

fn socket_alive(socket: &str) -> bool { UnixStream::connect(socket).is_ok() }

fn query_mpv(socket: &str, property: &str) -> Option<String> {
    let mut stream = UnixStream::connect(socket).ok()?;
    let cmd = format!("{{\"command\":[\"get_property_string\",\"{}\"]}}\n", property);
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
        let args_json: Vec<String> = args.iter().map(|a| format!("\"{}\"", a)).collect();
        let cmd = format!("{{\"command\":[{}]}}\n", args_json.join(","));
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

// ---------------------------------------------------------------------------
// MPV launcher
// ---------------------------------------------------------------------------

fn type_flags(pane_type: &str) -> Vec<&'static str> {
    match pane_type {
        "audio" => vec!["--vid=no",          "--force-window=yes", "--keep-open=yes"],
        "http"  => vec!["--force-window=yes", "--keep-open=yes"],
        "ytdlp" => vec!["--force-window=yes", "--keep-open=yes", "--ytdl-format=bestvideo+bestaudio"],
        "rtsp"  => vec!["--force-window=yes", "--keep-open=yes", "--rtsp-transport=tcp"],
        _       => vec!["--force-window=yes", "--keep-open=yes"],
    }
}

fn launch_pane(pane: &Pane, playlist_path: Option<&str>) {
    let mut args: Vec<String> = Vec::new();
    args.push(format!("--input-ipc-server={}", pane.socket));
    args.push("--really-quiet".to_string());
    for flag in type_flags(&pane.pane_type) { args.push(flag.to_string()); }
    let mpv_conf = pane_mpv_conf(&pane.name);
    if mpv_conf.exists() { args.push(format!("--include={}", mpv_conf.to_string_lossy())); }
    if let Some(geo) = &pane.geometry { args.push(format!("--geometry={}", geo)); }
    if let Some(pl) = playlist_path { args.push(pl.to_string()); }
    else { args.push("--idle=yes".to_string()); }
    let _ = std::process::Command::new("mpv")
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

// ---------------------------------------------------------------------------
// Pane file creation
// ---------------------------------------------------------------------------

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
// Shared UI helpers
// ---------------------------------------------------------------------------

fn divider_line(width: usize, color: Color) -> Paragraph<'static> {
    Paragraph::new(Span::styled("-".repeat(width), Style::default().fg(color)))
        .style(Style::default().bg(C_BG))
}

// ---------------------------------------------------------------------------
// Startup sequence
// ---------------------------------------------------------------------------

// One status line per pane, accumulates above the divider.
// Format: [PaneBot] :: "Name" :: [Status] ... message.

struct StartupEntry {
    name:    String,
    status:  String,   // "Running", "Offline", "Starting"
    message: String,
}

fn render_startup_screen(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    entries: &[StartupEntry],
    prompt: Option<&PromptState>,
) -> io::Result<()> {
    terminal.draw(|f| {
        let size = f.size();
        f.render_widget(Block::default().style(Style::default().bg(C_BG)), size);

        let prompt_h: u16 = if prompt.is_some() { 3 } else { 0 };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),   // top pad
                Constraint::Min(1),      // entry list
                Constraint::Length(1),   // divider
                Constraint::Length(prompt_h), // prompt block
                Constraint::Length(1),   // statusbar
            ])
            .split(size);

        // entry list
        let items: Vec<ListItem> = entries.iter().map(|e| {
            let status_color = match e.status.as_str() {
                "Running"  => C_GREEN,
                "Starting" => C_ORANGE,
                _          => C_RED,
            };
            let spans = Line::from(vec![
                Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
                Span::styled(" :: ",     Style::default().fg(C_DIM)),
                Span::styled(format!("\"{}\"", e.name), Style::default().fg(Color::White)),
                Span::styled(" :: ",     Style::default().fg(C_DIM)),
                Span::styled(format!("[{}]", e.status), Style::default().fg(status_color)),
                Span::styled(format!(" ... {}", e.message), Style::default().fg(C_HINT)),
            ]);
            ListItem::new(spans).style(Style::default().bg(C_BG))
        }).collect();

        f.render_widget(List::new(items).style(Style::default().bg(C_BG)), chunks[1]);
        f.render_widget(divider_line(size.width as usize, C_DIVIDER), chunks[2]);

        // launch prompt
        if let Some(p) = prompt {
            let mut lines: Vec<Line> = Vec::new();

            lines.push(Line::from(vec![
                Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
                Span::styled(" :: ",      Style::default().fg(C_DIM)),
                Span::styled(format!("\"{}\"", p.pane_name), Style::default().fg(Color::White)),
                Span::styled(" :: ",      Style::default().fg(C_DIM)),
                Span::styled("[Offline]", Style::default().fg(C_RED)),
                Span::styled(" :: Launch With?", Style::default().fg(C_HINT)),
            ]));

            let mut opt_spans: Vec<Span> = vec![Span::styled("  ", Style::default())];
            opt_spans.push(Span::styled("[Enter]", Style::default().fg(C_CMD_KEY)));
            opt_spans.push(Span::styled(
                if p.has_stored { " Last Playlist" } else { " Last Playlist" },
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
            lines.push(Line::from(opt_spans));

            // browse input if active
            if p.browsing {
                lines.push(Line::from(vec![
                    Span::styled("  Playlist: ", Style::default().fg(C_HINT)),
                    Span::styled(p.browse_buf.clone(), Style::default().fg(Color::White)),
                    Span::styled("_", Style::default().fg(C_ORANGE)),
                ]));
            }

            f.render_widget(
                Paragraph::new(lines).style(Style::default().bg(C_CMD_BG)),
                chunks[3],
            );
        }

        // statusbar hint
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
                Span::styled(" :: Starting up....", Style::default().fg(C_DIM)),
            ])).style(Style::default().bg(C_BG)),
            chunks[4],
        );
    })?;
    Ok(())
}

struct PromptState {
    pane_name:  String,
    has_stored: bool,
    has_default: bool,
    browsing:   bool,
    browse_buf: String,
    completions: Vec<String>,
    comp_sel:   usize,
}

enum LaunchChoice {
    LastPlaylist,
    NewPlaylist(String),
    PaneDefault,
    Empty,
}

fn startup_sequence(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> io::Result<Vec<Pane>> {
    let conf_path = config_dir().join("panes.conf");

    let mut panes = load_panes();

    // first-run: no config or no panes defined
    if !conf_path.exists() || panes.is_empty() {
        let entries = vec![StartupEntry {
            name:    "PaneBot".to_string(),
            status:  "Offline".to_string(),
            message: if !conf_path.exists() {
                format!("panes.conf not found -- Press [Enter] to create panes.")
            } else {
                "No panes defined -- Press [Enter] to create panes.".to_string()
            },
        }];
        render_startup_screen(terminal, &entries, None)?;
        loop {
            if let Event::Key(k) = event::read()? {
                match k.code {
                    KeyCode::Enter => break,
                    KeyCode::Char('q') => return Ok(Vec::new()),
                    _ => {}
                }
            }
        }
        panes = run_wizard(terminal)?;
        if panes.is_empty() { return Ok(Vec::new()); }
    }

    let mut entries: Vec<StartupEntry> = Vec::new();

    for i in 0..panes.len() {
        let alive = socket_alive(&panes[i].socket);

        if alive {
            refresh_pane(&mut panes[i]);
            entries.push(StartupEntry {
                name:    panes[i].name.clone(),
                status:  "Running".to_string(),
                message: "Loaded.".to_string(),
            });
            render_startup_screen(terminal, &entries, None)?;
            std::thread::sleep(Duration::from_millis(120));
        } else {
            let stored_pl   = pane_playlist_file(&panes[i].name);
            let has_stored  = stored_pl.exists() && stored_pl.metadata().map(|m| m.len() > 0).unwrap_or(false);
            let has_default = panes[i].playlist.is_some();

            let mut prompt = PromptState {
                pane_name:   panes[i].name.clone(),
                has_stored, has_default,
                browsing:    false,
                browse_buf:  String::new(),
                completions: Vec::new(),
                comp_sel:    0,
            };

            let choice = loop {
                render_startup_screen(terminal, &entries, Some(&prompt))?;
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

            launch_pane(&panes[i], playlist_arg.as_deref());

            // wait for socket
            let mut attempts = 0;
            let started = loop {
                std::thread::sleep(Duration::from_millis(200));
                if socket_alive(&panes[i].socket) { break true; }
                attempts += 1;
                if attempts > 15 { break false; }
            };

            if started {
                refresh_pane(&mut panes[i]);
                entries.push(StartupEntry {
                    name:    panes[i].name.clone(),
                    status:  "Starting".to_string(),
                    message: "Loaded.".to_string(),
                });
            } else {
                entries.push(StartupEntry {
                    name:    panes[i].name.clone(),
                    status:  "Offline".to_string(),
                    message: "Could not start.".to_string(),
                });
            }
            render_startup_screen(terminal, &entries, None)?;
            std::thread::sleep(Duration::from_millis(120));
        }
    }

    std::thread::sleep(Duration::from_millis(500));
    Ok(panes)
}

// ---------------------------------------------------------------------------
// First-run wizard
// ---------------------------------------------------------------------------

enum WizardScreen {
    HowMany, PaneName, ContentType, Geometry, Width, AspectRatio,
    ScreenPosition, TileWith, DefaultPlaylist, BrowsePlaylist, Confirm,
}

struct WizardState {
    total: usize, current: usize, panes: Vec<Pane>,
    name: String, pane_type: String,
    width: u32, height: u32,
    geometry: Option<String>, playlist: Option<String>,
    input_buf: String, completions: Vec<String>, comp_sel: usize,
    error: Option<String>,
}

impl WizardState {
    fn new() -> Self {
        WizardState {
            total: 0, current: 1, panes: Vec::new(),
            name: String::new(), pane_type: String::new(),
            width: 0, height: 0, geometry: None, playlist: None,
            input_buf: String::new(), completions: Vec::new(),
            comp_sel: 0, error: None,
        }
    }
    fn reset_pane(&mut self) {
        self.name.clear(); self.pane_type.clear();
        self.width = 0; self.height = 0;
        self.geometry = None; self.playlist = None;
        self.input_buf.clear(); self.completions.clear();
        self.comp_sel = 0; self.error = None;
    }
}

fn run_wizard(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<Vec<Pane>> {
    let mut state  = WizardState::new();
    let mut screen = WizardScreen::HowMany;

    std::fs::create_dir_all(config_dir())?;
    let conf = config_dir().join("panes.conf");
    if !conf.exists() {
        let mut f = std::fs::File::create(&conf)?;
        writeln!(f, "# panebot panes.conf")?;
        writeln!(f, "# [PaneName]")?;
        writeln!(f, "# socket   = /path/to/name.sock")?;
        writeln!(f, "# type     = video")?;
        writeln!(f, "# geometry = 650x366+0+0")?;
        writeln!(f, "# playlist = /path/to/name.m3u")?;
    }

    loop {
        render_wizard(terminal, &screen, &state)?;
        if let Event::Key(k) = event::read()? {
            match screen {
                WizardScreen::HowMany => match k.code {
                    KeyCode::Char(c) if c.is_ascii_digit() => { state.input_buf.push(c); }
                    KeyCode::Backspace => { state.input_buf.pop(); }
                    KeyCode::Enter => match state.input_buf.trim().parse::<usize>() {
                        Ok(n) if n > 0 => {
                            state.total = n; state.input_buf.clear(); state.error = None;
                            screen = WizardScreen::PaneName;
                        }
                        _ => { state.error = Some("Enter a number greater than 0".to_string()); }
                    },
                    KeyCode::Char('q') => return Ok(state.panes),
                    _ => {}
                },

                WizardScreen::PaneName => match k.code {
                    KeyCode::Char(c) if !c.is_whitespace() => { state.input_buf.push(c); }
                    KeyCode::Backspace => { state.input_buf.pop(); }
                    KeyCode::Enter => {
                        if state.input_buf.trim().is_empty() {
                            state.error = Some("Name cannot be empty".to_string());
                        } else {
                            state.name = state.input_buf.trim().to_string();
                            state.input_buf.clear(); state.error = None;
                            screen = WizardScreen::ContentType;
                        }
                    }
                    KeyCode::Char('q') => return Ok(state.panes),
                    _ => {}
                },

                WizardScreen::ContentType => match k.code {
                    KeyCode::Char('1') => { state.pane_type = "video".into(); state.error = None; screen = WizardScreen::Geometry; }
                    KeyCode::Char('2') => { state.pane_type = "audio".into(); state.error = None; screen = WizardScreen::Geometry; }
                    KeyCode::Char('3') => { state.pane_type = "http".into();  state.error = None; screen = WizardScreen::Geometry; }
                    KeyCode::Char('4') => { state.pane_type = "ytdlp".into(); state.error = None; screen = WizardScreen::Geometry; }
                    KeyCode::Char('5') => { state.pane_type = "rtsp".into();  state.error = None; screen = WizardScreen::Geometry; }
                    KeyCode::Char('q') => return Ok(state.panes),
                    _ => { state.error = Some("Press 1-5".to_string()); }
                },

                WizardScreen::Geometry => match k.code {
                    KeyCode::Char('1') => {
                        state.geometry = None; state.error = None;
                        screen = if state.current == 1 { WizardScreen::ScreenPosition } else { WizardScreen::TileWith };
                    }
                    KeyCode::Char('2') => { state.error = None; screen = WizardScreen::Width; }
                    KeyCode::Char('q') => return Ok(state.panes),
                    _ => { state.error = Some("Press 1 or 2".to_string()); }
                },

                WizardScreen::Width => match k.code {
                    KeyCode::Char(c) if c.is_ascii_digit() => { state.input_buf.push(c); }
                    KeyCode::Backspace => { state.input_buf.pop(); }
                    KeyCode::Enter => match state.input_buf.trim().parse::<u32>() {
                        Ok(w) if w > 0 => {
                            state.width = w; state.input_buf.clear(); state.error = None;
                            screen = WizardScreen::AspectRatio;
                        }
                        _ => { state.error = Some("Enter a valid pixel width".to_string()); }
                    },
                    KeyCode::Char('q') => return Ok(state.panes),
                    _ => {}
                },

                WizardScreen::AspectRatio => {
                    let ratio: Option<f64> = match k.code {
                        KeyCode::Char('1') => Some(16.0/9.0),
                        KeyCode::Char('2') => Some(9.0/16.0),
                        KeyCode::Char('3') => Some(4.0/3.0),
                        KeyCode::Char('4') => Some(1.0),
                        KeyCode::Char('5') => Some(2.39),
                        KeyCode::Char('6') => Some(2.0),
                        KeyCode::Char('7') => Some(4.0/5.0),
                        KeyCode::Char('8') => Some(3.0/2.0),
                        KeyCode::Char('9') => Some(21.0/9.0),
                        KeyCode::Char('0') => Some(1.85),
                        KeyCode::F(1)      => Some(16.0/10.0),
                        KeyCode::Char('q') => return Ok(state.panes),
                        _ => None,
                    };
                    if let Some(r) = ratio {
                        state.height = (state.width as f64 / r).round() as u32;
                        state.error = None;
                        screen = if state.current == 1 { WizardScreen::ScreenPosition } else { WizardScreen::TileWith };
                    } else {
                        state.error = Some("Press 1-9, 0=1.85:1, F1=16:10".to_string());
                    }
                },

                WizardScreen::ScreenPosition => match k.code {
                    KeyCode::Char('1') => {
                        let geo = if state.width > 0 { format!("{}x{}+0+0", state.width, state.height) } else { "+0+0".to_string() };
                        state.geometry = Some(geo); state.error = None;
                        screen = WizardScreen::DefaultPlaylist;
                    }
                    KeyCode::Char('2') => {
                        // custom — take raw input
                        state.input_buf.clear(); state.error = None;
                        // for now treat as Top Left until custom input is built
                        let geo = if state.width > 0 { format!("{}x{}+0+0", state.width, state.height) } else { "+0+0".to_string() };
                        state.geometry = Some(geo);
                        screen = WizardScreen::DefaultPlaylist;
                    }
                    KeyCode::Char('q') => return Ok(state.panes),
                    _ => { state.error = Some("Press 1 or 2".to_string()); }
                },

                WizardScreen::TileWith => match k.code {
                    KeyCode::Char('1') | KeyCode::Char('2') => {
                        let horiz = k.code == KeyCode::Char('1');
                        let geo = if let Some(prev) = state.panes.last() {
                            if let Some(ref pg) = prev.geometry {
                                tile_geo(pg, state.width, state.height, horiz)
                            } else {
                                if state.width > 0 { format!("{}x{}+0+0", state.width, state.height) } else { "+0+0".to_string() }
                            }
                        } else {
                            if state.width > 0 { format!("{}x{}+0+0", state.width, state.height) } else { "+0+0".to_string() }
                        };
                        state.geometry = Some(geo); state.error = None;
                        screen = WizardScreen::DefaultPlaylist;
                    }
                    KeyCode::Char('3') => { state.error = None; screen = WizardScreen::ScreenPosition; }
                    KeyCode::Char('q') => return Ok(state.panes),
                    _ => { state.error = Some("Press 1, 2, or 3".to_string()); }
                },

                WizardScreen::DefaultPlaylist => match k.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        state.input_buf.clear(); state.completions.clear(); state.error = None;
                        screen = WizardScreen::BrowsePlaylist;
                    }
                    KeyCode::Char('n') | KeyCode::Char('N') => {
                        state.playlist = None; state.error = None;
                        screen = WizardScreen::Confirm;
                    }
                    KeyCode::Char('q') => return Ok(state.panes),
                    _ => { state.error = Some("Press y or n".to_string()); }
                },

                WizardScreen::BrowsePlaylist => match k.code {
                    KeyCode::Enter => {
                        let p = state.input_buf.trim().to_string();
                        state.playlist = if p.is_empty() { None } else { Some(p) };
                        state.input_buf.clear(); state.completions.clear(); state.error = None;
                        screen = WizardScreen::Confirm;
                    }
                    KeyCode::Esc => {
                        state.playlist = None; state.input_buf.clear();
                        state.completions.clear(); screen = WizardScreen::Confirm;
                    }
                    KeyCode::Tab => {
                        if state.completions.is_empty() { state.completions = complete_path(&state.input_buf); state.comp_sel = 0; }
                        else { state.comp_sel = (state.comp_sel + 1) % state.completions.len(); }
                        if !state.completions.is_empty() { state.input_buf = state.completions[state.comp_sel].clone(); }
                    }
                    KeyCode::BackTab => {
                        if !state.completions.is_empty() {
                            state.comp_sel = if state.comp_sel == 0 { state.completions.len()-1 } else { state.comp_sel-1 };
                            state.input_buf = state.completions[state.comp_sel].clone();
                        }
                    }
                    KeyCode::Backspace => { state.input_buf.pop(); state.completions = complete_path(&state.input_buf); state.comp_sel = 0; }
                    KeyCode::Char(c)   => { state.input_buf.push(c); state.completions = complete_path(&state.input_buf); state.comp_sel = 0; }
                    _ => {}
                },

                WizardScreen::Confirm => match k.code {
                    KeyCode::Enter => {
                        let socket = pane_socket(&state.name).to_string_lossy().to_string();
                        let _ = create_pane_files(&state.name, &state.pane_type);
                        let pane = Pane {
                            name: state.name.clone(), socket,
                            pane_type: state.pane_type.clone(),
                            geometry: state.geometry.clone(), playlist: state.playlist.clone(),
                            status: "Offline".to_string(), volume: 0, muted: false,
                            title: "\u{2014}".to_string(),
                        };
                        let _ = write_pane_to_conf(&pane);
                        state.panes.push(pane);
                        if state.current >= state.total {
                            return Ok(state.panes);
                        } else {
                            state.current += 1; state.reset_pane();
                            screen = WizardScreen::PaneName;
                        }
                    }
                    KeyCode::Char('r') => { state.reset_pane(); screen = WizardScreen::PaneName; }
                    KeyCode::Char('q') => return Ok(state.panes),
                    _ => {}
                },
            }
        }
    }
}

fn tile_geo(prev_geo: &str, width: u32, height: u32, horizontal: bool) -> String {
    let parts: Vec<&str> = prev_geo.splitn(2, '+').collect();
    let dims: Vec<&str>  = parts[0].split('x').collect();
    let prev_w: u32 = dims.get(0).and_then(|s| s.parse().ok()).unwrap_or(0);
    let prev_h: u32 = dims.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let rest: Vec<&str> = if parts.len() > 1 { parts[1].split('+').collect() } else { vec!["0","0"] };
    let prev_x: u32 = rest.get(0).and_then(|s| s.parse().ok()).unwrap_or(0);
    let prev_y: u32 = rest.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let (nx, ny) = if horizontal { (prev_x + prev_w, prev_y) } else { (prev_x, prev_y + prev_h) };
    if width > 0 { format!("{}x{}+{}+{}", width, height, nx, ny) } else { format!("+{}+{}", nx, ny) }
}

fn render_wizard(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    screen: &WizardScreen,
    state: &WizardState,
) -> io::Result<()> {
    terminal.draw(|f| {
        let size = f.size();
        f.render_widget(Block::default().style(Style::default().bg(C_BG)), size);

        let comp_h: u16 = match screen {
            WizardScreen::BrowsePlaylist if !state.completions.is_empty() => state.completions.len() as u16 + 1,
            _ => 0,
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(comp_h),
                Constraint::Length(1),
            ])
            .split(size);

        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
                Span::styled(
                    format!("  ::  New Pane Setup  ::  [{}/{}] ::", state.current, state.total.max(1)),
                    Style::default().fg(C_DIM),
                ),
            ])).style(Style::default().bg(C_BG)),
            chunks[1],
        );
        f.render_widget(divider_line(size.width as usize, C_DIVIDER), chunks[2]);

        let mut body: Vec<Line> = Vec::new();

        match screen {
            WizardScreen::HowMany => {
                body.push(Line::from(Span::raw("")));
                body.push(Line::from(vec![
                    Span::styled(" How many panes? ", Style::default().fg(C_HINT)),
                    Span::styled(state.input_buf.clone(), Style::default().fg(Color::White)),
                    Span::styled("_", Style::default().fg(C_ORANGE)),
                ]));
            }
            WizardScreen::PaneName => {
                body.push(Line::from(Span::raw("")));
                body.push(Line::from(Span::styled(format!(" [Pane #{}]", state.current), Style::default().fg(C_ORANGE))));
                body.push(Line::from(Span::styled("-".repeat(size.width as usize), Style::default().fg(C_DIVIDER))));
                body.push(Line::from(vec![
                    Span::styled(" Name?: ", Style::default().fg(C_HINT)),
                    Span::styled(state.input_buf.clone(), Style::default().fg(Color::White)),
                    Span::styled("_", Style::default().fg(C_ORANGE)),
                ]));
            }
            WizardScreen::ContentType => {
                body.push(Line::from(Span::raw("")));
                body.push(Line::from(Span::styled(format!(" [Pane #{}] :: \"{}\"", state.current, state.name), Style::default().fg(C_ORANGE))));
                body.push(Line::from(Span::styled("-".repeat(size.width as usize), Style::default().fg(C_DIVIDER))));
                body.push(Line::from(Span::raw("")));
                body.push(Line::from(Span::styled(" Content Type:", Style::default().fg(C_HINT))));
                for (k, t, d) in &[
                    ("1","[Video]","Playback from local/network filesystem"),
                    ("2","[Audio]","Music, podcasts, audio-only"),
                    ("3","[HTTP]", "Hosted video / cloud / debrid streams"),
                    ("4","[YTDLP]","YouTube specific instance"),
                    ("5","[RTSP]", "Live feeds, camera feeds"),
                ] {
                    body.push(Line::from(vec![
                        Span::styled(format!("  [{}]", k), Style::default().fg(C_CMD_KEY)),
                        Span::styled(format!(" :: {:<8} - {}", t, d), Style::default().fg(C_CYAN)),
                    ]));
                }
            }
            WizardScreen::Geometry => {
                body.push(Line::from(Span::raw("")));
                body.push(Line::from(Span::styled(format!(" [Pane #{}] :: \"{}\" :: [{}]", state.current, state.name, state.pane_type), Style::default().fg(C_ORANGE))));
                body.push(Line::from(Span::styled("-".repeat(size.width as usize), Style::default().fg(C_DIVIDER))));
                body.push(Line::from(Span::raw("")));
                body.push(Line::from(Span::styled(" Geometry:", Style::default().fg(C_HINT))));
                body.push(Line::from(vec![Span::styled("  [1]", Style::default().fg(C_CMD_KEY)), Span::styled(" :: Dynamic  - Resizes with window", Style::default().fg(C_CYAN))]));
                body.push(Line::from(vec![Span::styled("  [2]", Style::default().fg(C_CMD_KEY)), Span::styled(" :: Static   - Locked dimensions",   Style::default().fg(C_CYAN))]));
            }
            WizardScreen::Width => {
                body.push(Line::from(Span::raw("")));
                body.push(Line::from(Span::styled(format!(" [Pane #{}] :: \"{}\" :: [{}] :: Static", state.current, state.name, state.pane_type), Style::default().fg(C_ORANGE))));
                body.push(Line::from(Span::styled("-".repeat(size.width as usize), Style::default().fg(C_DIVIDER))));
                body.push(Line::from(Span::raw("")));
                body.push(Line::from(vec![
                    Span::styled(" Width? (pixels): ", Style::default().fg(C_HINT)),
                    Span::styled(state.input_buf.clone(), Style::default().fg(Color::White)),
                    Span::styled("_", Style::default().fg(C_ORANGE)),
                ]));
            }
            WizardScreen::AspectRatio => {
                body.push(Line::from(Span::raw("")));
                body.push(Line::from(Span::styled(format!(" [Pane #{}] :: \"{}\" :: {}px wide", state.current, state.name, state.width), Style::default().fg(C_ORANGE))));
                body.push(Line::from(Span::styled("-".repeat(size.width as usize), Style::default().fg(C_DIVIDER))));
                body.push(Line::from(Span::raw("")));
                body.push(Line::from(Span::styled(" Aspect Ratio:", Style::default().fg(C_HINT))));
                for (k, r, l) in &[
                    ("1",  "16:9   (1.77:1)", "Standard Widescreen"),
                    ("2",  "9:16   (0.56:1)", "Vertical Video"),
                    ("3",  "4:3    (1.33:1)", "Classic Television"),
                    ("4",  "1:1    (1.0:1) ", "Square"),
                    ("5",  "2.39:1 (2.4:1) ", "Cinematic Widescreen"),
                    ("6",  "2:1    (2.0:1) ", "Panoramic"),
                    ("7",  "4:5    (0.8:1) ", "Portrait"),
                    ("8",  "3:2    (1.5:1) ", "DSLR / Older Laptops"),
                    ("9",  "21:9   (2.33:1)", "Ultrawide"),
                    ("0",  "1.85:1          ", "Widescreen Motion Picture"),
                    ("F1", "16:10  (1.6:1) ", "Computer Monitors"),
                ] {
                    body.push(Line::from(vec![
                        Span::styled(format!("  [{:>2}]", k), Style::default().fg(C_CMD_KEY)),
                        Span::styled(format!(" :: {}  - {}", r, l), Style::default().fg(C_CYAN)),
                    ]));
                }
            }
            WizardScreen::ScreenPosition => {
                let dim = if state.width > 0 { format!("{}x{}", state.width, state.height) } else { "Dynamic".to_string() };
                body.push(Line::from(Span::raw("")));
                body.push(Line::from(Span::styled(format!(" [Pane #{}] :: \"{}\" :: {}", state.current, state.name, dim), Style::default().fg(C_ORANGE))));
                body.push(Line::from(Span::styled("-".repeat(size.width as usize), Style::default().fg(C_DIVIDER))));
                body.push(Line::from(Span::raw("")));
                body.push(Line::from(Span::styled(" Screen Position:", Style::default().fg(C_HINT))));
                body.push(Line::from(vec![Span::styled("  [1]", Style::default().fg(C_CMD_KEY)), Span::styled(" :: Top Left",                   Style::default().fg(C_CYAN))]));
                body.push(Line::from(vec![Span::styled("  [2]", Style::default().fg(C_CMD_KEY)), Span::styled(" :: Custom Value  (see dox)", Style::default().fg(C_CYAN))]));
            }
            WizardScreen::TileWith => {
                let dim = if state.width > 0 { format!("{}x{}", state.width, state.height) } else { "Dynamic".to_string() };
                body.push(Line::from(Span::raw("")));
                body.push(Line::from(Span::styled(format!(" [Pane #{}] :: \"{}\" :: {}", state.current, state.name, dim), Style::default().fg(C_ORANGE))));
                body.push(Line::from(Span::styled("-".repeat(size.width as usize), Style::default().fg(C_DIVIDER))));
                body.push(Line::from(Span::raw("")));
                body.push(Line::from(Span::styled(" Tile With Previous Pane?:", Style::default().fg(C_HINT))));
                body.push(Line::from(vec![Span::styled("  [1]", Style::default().fg(C_CMD_KEY)), Span::styled(" :: Horizontally",    Style::default().fg(C_CYAN))]));
                body.push(Line::from(vec![Span::styled("  [2]", Style::default().fg(C_CMD_KEY)), Span::styled(" :: Vertically",      Style::default().fg(C_CYAN))]));
                body.push(Line::from(vec![Span::styled("  [3]", Style::default().fg(C_CMD_KEY)), Span::styled(" :: Custom Position", Style::default().fg(C_CYAN))]));
            }
            WizardScreen::DefaultPlaylist => {
                let dim = if state.width > 0 { format!("{}x{}", state.width, state.height) } else { "Dynamic".to_string() };
                let geo = state.geometry.as_deref().unwrap_or("+0+0");
                body.push(Line::from(Span::raw("")));
                body.push(Line::from(Span::styled(format!(" [Pane #{}] :: \"{}\" :: {} :: {}", state.current, state.name, dim, geo), Style::default().fg(C_ORANGE))));
                body.push(Line::from(Span::styled("-".repeat(size.width as usize), Style::default().fg(C_DIVIDER))));
                body.push(Line::from(Span::raw("")));
                body.push(Line::from(vec![
                    Span::styled(" Configure Default Playlist?: ", Style::default().fg(C_HINT)),
                    Span::styled("[y/n]", Style::default().fg(C_CMD_KEY)),
                    Span::styled("_", Style::default().fg(C_ORANGE)),
                ]));
            }
            WizardScreen::BrowsePlaylist => {
                body.push(Line::from(Span::raw("")));
                body.push(Line::from(Span::styled(format!(" [Pane #{}] :: \"{}\" :: Default Playlist", state.current, state.name), Style::default().fg(C_ORANGE))));
                body.push(Line::from(Span::styled("-".repeat(size.width as usize), Style::default().fg(C_DIVIDER))));
                body.push(Line::from(Span::raw("")));
                body.push(Line::from(vec![
                    Span::styled(" Browse: ", Style::default().fg(C_HINT)),
                    Span::styled(state.input_buf.clone(), Style::default().fg(Color::White)),
                    Span::styled("_", Style::default().fg(C_ORANGE)),
                ]));
                body.push(Line::from(Span::styled("  [Tab] complete  ::  [Enter] confirm  ::  [Esc] skip", Style::default().fg(C_DIM))));
            }
            WizardScreen::Confirm => {
                let dim = if state.width > 0 { format!("{}x{}", state.width, state.height) } else { "Dynamic".to_string() };
                let geo = state.geometry.as_deref().unwrap_or("+0+0");
                let pl  = state.playlist.as_deref().unwrap_or("-");
                body.push(Line::from(Span::raw("")));
                body.push(Line::from(Span::styled(" Create This Pane?", Style::default().fg(C_HINT))));
                body.push(Line::from(Span::raw("")));
                body.push(Line::from(vec![
                    Span::styled(format!(" \"{}\"", state.name),   Style::default().fg(C_ORANGE)),
                    Span::styled(" :: ",                           Style::default().fg(C_DIM)),
                    Span::styled(format!("[{}]", state.pane_type), Style::default().fg(C_CYAN)),
                    Span::styled(" :: ",                           Style::default().fg(C_DIM)),
                    Span::styled(dim,                              Style::default().fg(C_PINK)),
                    Span::styled(" :: ",                           Style::default().fg(C_DIM)),
                    Span::styled(geo.to_string(),                  Style::default().fg(C_HINT)),
                    Span::styled(" :: ",                           Style::default().fg(C_DIM)),
                    Span::styled(pl.to_string(),                   Style::default().fg(C_DIM)),
                ]));
                body.push(Line::from(Span::raw("")));
                body.push(Line::from(vec![
                    Span::styled(" [Enter]", Style::default().fg(C_CMD_KEY)),
                    Span::styled(" Confirm  ::  ", Style::default().fg(C_CMD_HNT)),
                    Span::styled("[r]",      Style::default().fg(C_CMD_KEY)),
                    Span::styled(" Redo  ::  ",   Style::default().fg(C_CMD_HNT)),
                    Span::styled("[q]",      Style::default().fg(C_CMD_KEY)),
                    Span::styled(" Abort",        Style::default().fg(C_CMD_HNT)),
                ]));
            }
        }

        if let Some(ref err) = state.error {
            body.push(Line::from(Span::raw("")));
            body.push(Line::from(Span::styled(format!(" ! {}", err), Style::default().fg(C_RED))));
        }

        let items: Vec<ListItem> = body.into_iter()
            .map(|l| ListItem::new(l).style(Style::default().bg(C_BG)))
            .collect();
        f.render_widget(List::new(items).style(Style::default().bg(C_BG)), chunks[3]);

        if comp_h > 0 {
            let comp_items: Vec<ListItem> = state.completions.iter().enumerate().map(|(i, c)| {
                let name  = c.split('/').last().unwrap_or(c);
                let color = if c.ends_with('/') { C_ORANGE } else { C_CYAN };
                let style = if i == state.comp_sel { Style::default().fg(Color::White).bg(C_CURSOR) }
                            else { Style::default().fg(color).bg(C_COMP_BG) };
                ListItem::new(Span::styled(format!(" {} ", name), style))
            }).collect();
            f.render_widget(List::new(comp_items).style(Style::default().bg(C_COMP_BG)), chunks[4]);
        }

        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("[q]", Style::default().fg(C_CYAN)),
                Span::styled(" Abort wizard", Style::default().fg(C_HINT)),
            ])).style(Style::default().bg(C_BG)),
            chunks[5],
        );
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Dashboard
// ---------------------------------------------------------------------------

fn render_dashboard_header<'a>() -> Paragraph<'a> {
    Paragraph::new(Line::from(vec![
        Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
        Span::styled("  ::  Active Panes ::", Style::default().fg(C_DIM)),
    ])).style(Style::default().bg(C_BG))
}

fn render_dashboard_row(pane: &Pane, selected: bool, cmd_mode: bool) -> ListItem<'static> {
    let arrow   = if selected { ">>>" } else { "   " };
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
            Span::styled("[Tab]",        Style::default().fg(C_CMD_KEY)), Span::styled(" Exit Cmd",     Style::default().fg(C_CMD_HNT)),
        ])
    } else {
        Line::from(vec![
            Span::styled("[Up/Down]", Style::default().fg(C_CYAN)), Span::styled(" Select :: ",       Style::default().fg(C_HINT)),
            Span::styled("[Tab]",     Style::default().fg(C_CYAN)), Span::styled(" Cmd Mode :: ",     Style::default().fg(C_HINT)),
            Span::styled("[Enter]",   Style::default().fg(C_CYAN)), Span::styled(" Pane Details :: ", Style::default().fg(C_HINT)),
            Span::styled("[q]",       Style::default().fg(C_CYAN)), Span::styled(" Exit PaneBot",     Style::default().fg(C_HINT)),
        ])
    };
    Paragraph::new(spans).style(Style::default().bg(if cmd_mode { C_CMD_BG } else { C_BG }))
}

// ---------------------------------------------------------------------------
// Playlist
// ---------------------------------------------------------------------------

fn render_playlist_header<'a>(pane: &Pane) -> Paragraph<'a> {
    let vol_str = if pane.muted { "[Vol:Mute]".to_string() } else { format!("[Vol:{:>3}%]", pane.volume) };
    Paragraph::new(Line::from(vec![
        Span::styled("[PaneBot]",                                                              Style::default().fg(C_ORANGE)),
        Span::styled(" :: ",                                                                   Style::default().fg(C_DIM)),
        Span::styled(format!("\"{}\"", pane.name),                                            Style::default().fg(Color::White)),
        Span::styled(" :: ",                                                                   Style::default().fg(C_DIM)),
        Span::styled(format!("[{}]", pane.pane_type),                                         Style::default().fg(C_CYAN)),
        Span::styled(" :: ",                                                                   Style::default().fg(C_DIM)),
        Span::styled(format!("[{}]", pane.status), Style::default().fg(if pane.status == "Playing" { C_ORANGE } else { C_DIM })),
        Span::styled(" :: ",                                                                   Style::default().fg(C_DIM)),
        Span::styled(vol_str,                                                                  Style::default().fg(C_PINK)),
        Span::styled(" ::",                                                                    Style::default().fg(C_DIM)),
    ])).style(Style::default().bg(C_BG))
}

fn render_playlist_row(item: &PlaylistItem, selected: bool, item_cmd: bool) -> ListItem<'static> {
    ListItem::new(Line::from(vec![
        Span::styled(if selected { ">> " } else { "   " },  Style::default().fg(C_ORANGE)),
        Span::styled(format!("{:<3}", item.index),          Style::default().fg(C_PINK)),
        Span::styled(" :: ",                                Style::default().fg(C_DIM)),
        Span::styled(item.title.clone(),                    Style::default().fg(if selected { Color::White } else { C_CYAN })),
        if item_cmd && selected { Span::styled("  [CMD]", Style::default().fg(C_CMD_KEY)) } else { Span::raw("") },
    ])).style(if selected { Style::default().bg(C_CURSOR) } else { Style::default().bg(C_BG) })
}

fn render_playlist_statusbar<'a>(
    item_cmd: bool, move_input: bool, move_buf: &str, add_input: bool, add_buf: &str,
) -> Paragraph<'a> {
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
            Span::styled("[m]",     Style::default().fg(C_CMD_KEY)), Span::styled(" Change Position :: ", Style::default().fg(C_CMD_HNT)),
            Span::styled("[Tab]",   Style::default().fg(C_CMD_KEY)), Span::styled(" Exit Modify",         Style::default().fg(C_CMD_HNT)),
        ])
    } else {
        Line::from(vec![
            Span::styled("[Up/Down]",   Style::default().fg(C_CYAN)), Span::styled(" Select :: ",     Style::default().fg(C_HINT)),
            Span::styled("[Tab]",       Style::default().fg(C_CYAN)), Span::styled(" Modify :: ",     Style::default().fg(C_HINT)),
            Span::styled("[c]",         Style::default().fg(C_CYAN)), Span::styled(" Crop :: ",       Style::default().fg(C_HINT)),
            Span::styled("[n]",         Style::default().fg(C_CYAN)), Span::styled(" Add :: ",        Style::default().fg(C_HINT)),
            Span::styled("[Backspace]", Style::default().fg(C_CYAN)), Span::styled(" Return",         Style::default().fg(C_HINT)),
        ])
    };
    Paragraph::new(spans).style(Style::default().bg(if item_cmd { C_CMD_BG } else { C_BG }))
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
// Screen
// ---------------------------------------------------------------------------

enum Screen { Dashboard, Playlist(usize) }

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() -> Result<(), io::Error> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut panes = startup_sequence(&mut terminal)?;

    if panes.is_empty() {
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        return Ok(());
    }

    let mut cursor   = 0usize;
    let mut cmd_mode = false;
    let mut screen   = Screen::Dashboard;

    let mut pl_items:      Vec<PlaylistItem> = Vec::new();
    let mut pl_cursor      = 0usize;
    let mut pl_item_cmd    = false;
    let mut pl_move_input  = false;
    let mut pl_move_buf    = String::new();
    let mut pl_add_input   = false;
    let mut pl_add_buf     = String::new();
    let mut pl_completions: Vec<String> = Vec::new();
    let mut pl_comp_sel    = 0usize;

    loop {
        for pane in panes.iter_mut() { refresh_pane(pane); }
        if let Screen::Playlist(idx) = &screen {
            pl_items = fetch_playlist(&panes[*idx].socket);
        }

        terminal.draw(|f| {
            let size      = f.size();
            let div_color = if cmd_mode || pl_item_cmd { Color::Rgb(90,55,10) } else { C_DIVIDER };
            let comp_h    = if pl_add_input && !pl_completions.is_empty() { pl_completions.len() as u16 + 1 } else { 0 };

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Min(1),
                    Constraint::Length(comp_h),
                    Constraint::Length(1),
                    Constraint::Length(1),
                ])
                .split(size);

            f.render_widget(Block::default().style(Style::default().bg(C_BG)), size);

            match &screen {
                Screen::Dashboard => {
                    f.render_widget(render_dashboard_header(), chunks[1]);
                    f.render_widget(divider_line(size.width as usize, C_DIVIDER), chunks[2]);
                    let items: Vec<ListItem> = panes.iter().enumerate()
                        .map(|(i, p)| render_dashboard_row(p, i == cursor, cmd_mode && i == cursor))
                        .collect();
                    let mut st = ListState::default(); st.select(Some(cursor));
                    f.render_stateful_widget(List::new(items).style(Style::default().bg(C_BG)), chunks[3], &mut st);
                    f.render_widget(divider_line(size.width as usize, div_color), chunks[5]);
                    f.render_widget(render_dashboard_statusbar(cmd_mode), chunks[6]);
                }
                Screen::Playlist(idx) => {
                    f.render_widget(render_playlist_header(&panes[*idx]), chunks[1]);
                    f.render_widget(divider_line(size.width as usize, C_DIVIDER), chunks[2]);
                    let items: Vec<ListItem> = pl_items.iter().enumerate()
                        .map(|(i, item)| render_playlist_row(item, i == pl_cursor, pl_item_cmd && i == pl_cursor))
                        .collect();
                    let mut st = ListState::default(); st.select(Some(pl_cursor));
                    f.render_stateful_widget(List::new(items).style(Style::default().bg(C_BG)), chunks[3], &mut st);
                    if pl_add_input && !pl_completions.is_empty() {
                        let mut cs = ListState::default(); cs.select(Some(pl_comp_sel));
                        f.render_stateful_widget(render_completions(&pl_completions, pl_comp_sel), chunks[4], &mut cs);
                    }
                    f.render_widget(divider_line(size.width as usize, div_color), chunks[5]);
                    f.render_widget(render_playlist_statusbar(pl_item_cmd, pl_move_input, &pl_move_buf, pl_add_input, &pl_add_buf), chunks[6]);
                }
            }
        })?;

        if !poll(Duration::from_millis(100))? { continue; }

        if let Event::Key(key) = event::read()? {
            match &screen {
                Screen::Dashboard => {
                    let socket  = panes[cursor].socket.clone();
                    let offline = panes[cursor].status == "Offline";
                    if cmd_mode {
                        if offline { if key.code == KeyCode::Tab { cmd_mode = false; } }
                        else {
                            match key.code {
                                KeyCode::Tab        => { cmd_mode = false; }
                                KeyCode::Char(' ')  => { cmd_mpv(&socket, &["cycle","pause"]); }
                                KeyCode::Char('m')  => { cmd_mpv(&socket, &["cycle","mute"]); }
                                KeyCode::Char('n')  => { cmd_mpv(&socket, &["playlist-next"]); }
                                KeyCode::Char('N')  => { cmd_mpv(&socket, &["playlist-prev"]); }
                                KeyCode::Char('=')  => { cmd_mpv(&socket, &["add","volume","5"]); }
                                KeyCode::Char('-')  => { cmd_mpv(&socket, &["add","volume","-5"]); }
                                KeyCode::Right      => { cmd_mpv(&socket, &["seek","10"]); }
                                KeyCode::Left       => { cmd_mpv(&socket, &["seek","-10"]); }
                                KeyCode::Up         => { cmd_mpv(&socket, &["seek","60"]); }
                                KeyCode::Down       => { cmd_mpv(&socket, &["seek","-60"]); }
                                _ => {}
                            }
                        }
                    } else {
                        match key.code {
                            KeyCode::Char('q') => break,
                            KeyCode::Up        => { if cursor > 0 { cursor -= 1; } }
                            KeyCode::Down      => { if cursor < panes.len()-1 { cursor += 1; } }
                            KeyCode::Tab       => { cmd_mode = true; }
                            KeyCode::Enter     => {
                                pl_cursor = 0; pl_item_cmd = false;
                                pl_move_input = false; pl_add_input = false;
                                pl_move_buf.clear(); pl_add_buf.clear(); pl_completions.clear();
                                screen = Screen::Playlist(cursor);
                            }
                            _ => {}
                        }
                    }
                }

                Screen::Playlist(idx) => {
                    let idx    = *idx;
                    let socket = panes[idx].socket.clone();

                    if pl_add_input {
                        let is_stream = matches!(panes[idx].pane_type.as_str(), "http" | "ytdlp" | "rtsp");
                        match key.code {
                            KeyCode::Esc => { pl_add_input = false; pl_add_buf.clear(); pl_completions.clear(); }
                            KeyCode::Enter => {
                                let input = pl_add_buf.trim().to_string();
                                if !input.is_empty() {
                                    if is_stream {
                                        let is_pl = input.ends_with(".m3u") || input.ends_with(".m3u8") || input.ends_with(".pls");
                                        if is_pl { cmd_mpv(&socket, &["loadlist", &input, "append"]); }
                                        else      { cmd_mpv(&socket, &["loadfile", &input, "append"]); }
                                    } else {
                                        for f in expand_input(&input) { cmd_mpv(&socket, &["loadfile", &f, "append"]); }
                                    }
                                }
                                pl_add_input = false; pl_add_buf.clear(); pl_completions.clear();
                            }
                            KeyCode::Tab => {
                                if !is_stream {
                                    if pl_completions.is_empty() { pl_completions = complete_path(&pl_add_buf); pl_comp_sel = 0; }
                                    else { pl_comp_sel = (pl_comp_sel+1) % pl_completions.len(); }
                                    if !pl_completions.is_empty() { pl_add_buf = pl_completions[pl_comp_sel].clone(); }
                                }
                            }
                            KeyCode::BackTab => {
                                if !is_stream && !pl_completions.is_empty() {
                                    pl_comp_sel = if pl_comp_sel == 0 { pl_completions.len()-1 } else { pl_comp_sel-1 };
                                    pl_add_buf = pl_completions[pl_comp_sel].clone();
                                }
                            }
                            KeyCode::Backspace => {
                                pl_add_buf.pop();
                                if !is_stream { pl_completions = complete_path(&pl_add_buf); pl_comp_sel = 0; }
                            }
                            KeyCode::Char(c) => {
                                pl_add_buf.push(c);
                                if !is_stream { pl_completions = complete_path(&pl_add_buf); pl_comp_sel = 0; }
                            }
                            _ => {}
                        }
                    } else if pl_move_input {
                        match key.code {
                            KeyCode::Char(c) if c.is_ascii_digit() => { pl_move_buf.push(c); }
                            KeyCode::Backspace => { pl_move_buf.pop(); }
                            KeyCode::Enter => {
                                if let Ok(dest) = pl_move_buf.parse::<usize>() {
                                    if !pl_items.is_empty() {
                                        let src = pl_items[pl_cursor].index;
                                        cmd_mpv(&socket, &["playlist-move",&src.to_string(),&dest.to_string()]);
                                    }
                                }
                                pl_move_input = false; pl_move_buf.clear();
                            }
                            KeyCode::Esc => { pl_move_input = false; pl_move_buf.clear(); }
                            _ => {}
                        }
                    } else if pl_item_cmd {
                        match key.code {
                            KeyCode::Tab   => { pl_item_cmd = false; }
                            KeyCode::Enter => {
                                if !pl_items.is_empty() {
                                    let ii = pl_items[pl_cursor].index;
                                    cmd_mpv(&socket, &["set_property","playlist-pos",&ii.to_string()]);
                                }
                                pl_item_cmd = false;
                            }
                            KeyCode::Char('r') => {
                                if !pl_items.is_empty() {
                                    let ii = pl_items[pl_cursor].index;
                                    cmd_mpv(&socket, &["playlist-remove",&ii.to_string()]);
                                    if pl_cursor > 0 { pl_cursor -= 1; }
                                }
                                pl_item_cmd = false;
                            }
                            KeyCode::Char('m') => { pl_move_input = true; pl_move_buf.clear(); }
                            _ => {}
                        }
                    } else {
                        match key.code {
                            KeyCode::Up        => { if pl_cursor > 0 { pl_cursor -= 1; } }
                            KeyCode::Down      => { if pl_cursor < pl_items.len().saturating_sub(1) { pl_cursor += 1; } }
                            KeyCode::Tab       => { pl_item_cmd = true; }
                            KeyCode::Char('c') => { cmd_mpv(&socket, &["playlist-clear"]); }
                            KeyCode::Char('n') => { pl_add_input = true; pl_add_buf.clear(); pl_completions.clear(); }
                            KeyCode::Backspace => {
                                screen = Screen::Dashboard;
                                pl_item_cmd = false; pl_move_input = false; pl_add_input = false;
                                pl_move_buf.clear(); pl_add_buf.clear(); pl_completions.clear();
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
