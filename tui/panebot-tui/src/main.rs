use crossterm::{
    event::{Event, KeyCode, EventStream, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures_util::{SinkExt, StreamExt};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, Paragraph},
    Terminal,
};
use std::collections::{HashMap, HashSet};
use std::io;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use panebot_lib::{
    layouts_dir, pane_playlist, read_m3u, write_m3u, m3u_append, m3u_remove, m3u_crop,
    save_playlist, home_dir, config_dir, load_hosts, Host,
};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

const LOCAL_ADDR:        &str  = "ws://127.0.0.1:9090";
const CONNECT_RETRY_MS:  u64   = 500;
const CONNECT_TIMEOUT_S: u64   = 30;

// ---------------------------------------------------------------------------
// Colors
// ---------------------------------------------------------------------------

const C_ORANGE:  Color = Color::Rgb(224, 128, 48);
const C_CYAN:    Color = Color::Rgb(60, 160, 160);
const C_DIM:     Color = Color::Rgb(100, 120, 120);
const C_HINT:    Color = Color::Rgb(140, 160, 160);
const C_DIVIDER: Color = Color::Rgb(40, 58, 58);
const C_RED:     Color = Color::Rgb(200, 60, 60);
const C_GREEN:   Color = Color::Rgb(60, 180, 100);
const C_WHITE:   Color = Color::Rgb(220, 220, 220);

// ---------------------------------------------------------------------------
// WS sink type alias
// ---------------------------------------------------------------------------

type WsSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>
    >,
    Message,
>;

// ---------------------------------------------------------------------------
// Pane state — mirrors PaneState in daemon.
// Kept separate as the TUI does not need serde::Serialize.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct PaneState {
    mpv_name:     String,          // instance name — internal, drives paths
    pane_name:    String,          // display name — TUI and window title
    pane_type:    String,
    online:       bool,
    idle_active:  Option<bool>,
    paused:       Option<bool>,
    muted:        Option<bool>,
    volume:       Option<f64>,
    title:        Option<String>,
    playlist_pos: Option<i64>,
}

impl PaneState {
    fn new(mpv_name: &str, pane_name: &str, pane_type: &str) -> Self {
        PaneState {
            mpv_name:     mpv_name.to_string(),
            pane_name:    pane_name.to_string(),
            pane_type:    pane_type.to_string(),
            online:       false,
            idle_active:  None,
            paused:       None,
            muted:        None,
            volume:       None,
            title:        None,
            playlist_pos: None,
        }
    }

    fn playback_label(&self) -> (&'static str, Color) {
        if !self.online                              { return ("Offline", C_RED);  }
        if self.idle_active.unwrap_or(true)         { return ("Stopped", C_DIM);  }
        if self.paused.unwrap_or(true)              { return ("Paused",  C_HINT); }
        ("Playing", C_GREEN)
    }

    fn volume_label(&self) -> (String, Color) {
        if !self.online                    { return ("Offline".to_string(),  C_RED); }
        if self.muted.unwrap_or(false)     { return ("Vol:Mute".to_string(), C_DIM); }
        match self.volume {
            Some(v) => (format!("Vol:{:3.0}", v), C_CYAN),
            None    => ("Vol:  ?".to_string(),    C_DIM),
        }
    }
}

// ---------------------------------------------------------------------------
// Details screen mode
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum DetailsMode {
    Normal,
    Jump,
    Add,
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

struct App {
    pane_order:     Vec<String>,
    panes:          HashMap<String, PaneState>,
    selected:       usize,
    hostname:       String,
    platform:       String,
    layout:         String,
    home:           String,
    owns_daemon:    bool,
    show_log:       bool,
    command_mode:   bool,
    show_picker:    bool,
    picker_sel:     usize,
    layouts:        Vec<String>,
    show_details:   bool,
    details_mode:   DetailsMode,
    playlist_sel:   usize,
    playlist_items: Vec<String>,
    selected_items: HashSet<usize>,
    status_msg:     Option<String>,
    jump_input:     String,
    add_input:      String,
}

impl App {
    fn new() -> Self {
        App {
            pane_order:     Vec::new(),
            panes:          HashMap::new(),
            selected:       0,
            hostname:       String::new(),
            platform:       String::new(),
            layout:         String::new(),
            home:           home_dir(),
            owns_daemon:    false,
            show_log:       false,
            command_mode:   false,
            show_picker:    false,
            picker_sel:     0,
            layouts:        Vec::new(),
            show_details:   false,
            details_mode:   DetailsMode::Normal,
            playlist_sel:   0,
            playlist_items: Vec::new(),
            selected_items: HashSet::new(),
            status_msg:     None,
            jump_input:     String::new(),
            add_input:      String::new(),
        }
    }

    fn active_count(&self) -> usize {
        self.panes.values().filter(|p| p.online).count()
    }

    fn selected_name(&self) -> Option<String> {
        self.pane_order.get(self.selected).cloned()
    }

    fn select_next(&mut self) {
        if !self.pane_order.is_empty() && self.selected + 1 < self.pane_order.len() {
            self.selected += 1;
        }
    }

    fn select_prev(&mut self) {
        if !self.pane_order.is_empty() {
            self.selected = self.selected.saturating_sub(1);
        }
    }

    fn current_playlist_pos(&self) -> i64 {
        self.selected_name()
            .and_then(|n| self.panes.get(&n))
            .and_then(|p| p.playlist_pos)
            .unwrap_or(-1)
    }

    fn sel_is_playing(&self) -> bool {
        let pos = self.current_playlist_pos();
        pos >= 0 && pos as usize == self.playlist_sel
    }

    fn load_layouts(&mut self) {
        let mut layouts = Vec::new();
        if let Ok(entries) = std::fs::read_dir(layouts_dir()) {
            let mut names: Vec<_> = entries
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    if name.ends_with(".layout") {
                        Some(name.trim_end_matches(".layout").to_string())
                    } else {
                        None
                    }
                })
                .collect();
            names.sort();
            layouts = names;
        }
        self.layouts = layouts;
    }
}

// ---------------------------------------------------------------------------
// WS event processing
// ---------------------------------------------------------------------------

fn process_event(app: &mut App, text: &str) -> Option<&'static str> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let event = v["event"].as_str().unwrap_or("");

    match event {

        // node:snapshot — fires on connect and reconnect, replaces bootstrap_complete.
        // Pane online state comes from subsequent online/offline events.
        "node:snapshot" => {
            app.hostname = v["hostname"].as_str().unwrap_or("").to_string();
            app.platform = v["platform"].as_str().unwrap_or("").to_string();
            app.layout   = v["layout"].as_str().unwrap_or("").to_string();
            app.home     = v["home"].as_str().unwrap_or("").to_string();
            app.load_layouts();

            if let Some(panes) = v["panes"].as_array() {
                for p in panes {
                    let mpv_name  = p["name"].as_str().unwrap_or("").to_string();
                    let pane_name = p["pane_name"].as_str().unwrap_or(mpv_name.as_str()).to_string();
                    let ptype     = p["pane_type"].as_str().unwrap_or("video").to_string();
                    if !mpv_name.is_empty() {
                        app.pane_order.push(mpv_name.clone());
                        app.panes.insert(mpv_name.clone(), PaneState::new(&mpv_name, &pane_name, &ptype));
                    }
                }
            }
        }

        "online" => {
            let pane = v["pane"].as_str().unwrap_or("");
            if let Some(ps) = app.panes.get_mut(pane) {
                ps.online = true;
                if let Some(state) = v.get("state") { apply_state(ps, state); }
            }
        }

        "offline" => {
            let pane = v["pane"].as_str().unwrap_or("");
            if let Some(ps) = app.panes.get_mut(pane) { ps.online = false; }
        }

        "property-change" => {
            let pane = v["pane"].as_str().unwrap_or("");
            let prop = v["property"].as_str().unwrap_or("");
            if let Some(ps) = app.panes.get_mut(pane) {
                match prop {
                    "pause"        => { ps.paused       = v["value"].as_bool(); }
                    "volume"       => { ps.volume       = v["value"].as_f64(); }
                    "media-title"  => { ps.title        = v["value"].as_str().map(|s| s.to_string()); }
                    "playlist-pos" => { ps.playlist_pos = v["value"].as_i64(); }
                    "mute"         => { ps.muted        = v["value"].as_bool(); }
                    "idle-active"  => { ps.idle_active  = v["value"].as_bool(); }
                    _ => {}
                }
            }
        }

        "node:down" => {
            for ps in app.panes.values_mut() { ps.online = false; }
            return Some("node:down");
        }

        "node:layout" => {
            if let Some(layout) = v["layout"].as_str() {
                app.layout = layout.to_string();
            }
        }

        _ => {}
    }

    None
}

fn apply_state(ps: &mut PaneState, state: &serde_json::Value) {
    if let Some(v) = state["paused"].as_bool()      { ps.paused       = Some(v); }
    if let Some(v) = state["muted"].as_bool()        { ps.muted        = Some(v); }
    if let Some(v) = state["idle_active"].as_bool()  { ps.idle_active  = Some(v); }
    if let Some(v) = state["volume"].as_f64()        { ps.volume       = Some(v); }
    if let Some(v) = state["playlist_pos"].as_i64()  { ps.playlist_pos = Some(v); }
    if let Some(v) = state["title"].as_str()         { ps.title        = Some(v.to_string()); }
}

// ---------------------------------------------------------------------------
// Rendering — host picker screen
// ---------------------------------------------------------------------------

type Term = Terminal<CrosstermBackend<io::Stdout>>;

fn render_host_picker(terminal: &mut Term, hosts: &[Host], sel: usize) -> io::Result<()> {
    terminal.draw(|f| {
        let size = f.size();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(size);

        f.render_widget(Paragraph::new(Line::from(vec![
            Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
            Span::styled(" :: ",     Style::default().fg(C_DIM)),
            Span::styled("Select node to connect", Style::default().fg(C_HINT)),
        ])), chunks[0]);

        f.render_widget(divider(size.width as usize), chunks[1]);

        let items: Vec<ListItem> = hosts.iter().enumerate().map(|(i, host)| {
            let is_sel = i == sel;
            let cursor = if is_sel { Span::styled(">> ", Style::default().fg(C_ORANGE)) } else { Span::raw("   ") };
            let item = ListItem::new(Line::from(vec![
                cursor,
                Span::styled(host.label.clone(), Style::default()
                    .fg(if is_sel { C_WHITE } else { C_HINT })
                    .add_modifier(if is_sel { Modifier::BOLD } else { Modifier::empty() })
                ),
                Span::styled(" :: ", Style::default().fg(C_DIM)),
                Span::styled(host.address.clone(), Style::default().fg(C_DIM)),
            ]));
            if is_sel { item.style(Style::default().bg(Color::Rgb(20, 40, 40))) } else { item }
        }).collect();

        f.render_widget(List::new(items), chunks[2]);
        f.render_widget(divider(size.width as usize), chunks[3]);
        f.render_widget(Paragraph::new(Line::from(vec![
            Span::styled("[up/dn]", Style::default().fg(C_DIM)),
            Span::styled(" Select",  Style::default().fg(C_DIM)),
            Span::styled(" :: ",     Style::default().fg(C_ORANGE)),
            Span::styled("[Enter]",  Style::default().fg(C_DIM)),
            Span::styled(" Connect", Style::default().fg(C_DIM)),
            Span::styled(" :: ",     Style::default().fg(C_ORANGE)),
            Span::styled("[q]",      Style::default().fg(C_DIM)),
            Span::styled(" Quit",    Style::default().fg(C_DIM)),
        ])), chunks[4]);
    })?;
    Ok(())
}

fn render(terminal: &mut Term, app: &App) -> io::Result<()> {
    terminal.draw(|f| {
        let size = f.size();

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(size);

        f.render_widget(Paragraph::new(Line::from(vec![
            Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
            Span::styled(" :: ",     Style::default().fg(C_DIM)),
            Span::styled(
                format!("Active Panes: {}/{}", app.active_count(), app.pane_order.len()),
                Style::default().fg(C_HINT),
            ),
            Span::styled(" :: ",     Style::default().fg(C_DIM)),
            Span::styled(
                format!("Layout: {}", app.layout),
                Style::default().fg(C_CYAN),
            ),
            Span::styled(" :: ",     Style::default().fg(C_DIM)),
            Span::styled(
                format!("{} [{}]", app.hostname, app.platform),
                Style::default().fg(C_DIM),
            ),
        ])), chunks[0]);

        f.render_widget(divider(size.width as usize), chunks[1]);

        if app.show_log {
            let log_height = chunks[2].height as usize;
            let log_path   = config_dir().join("panebot-daemon.log");
            let log_lines: Vec<ListItem> = std::fs::read_to_string(&log_path)
                .unwrap_or_default()
                .lines()
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .take(log_height)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(|line| ListItem::new(Span::styled(line, Style::default().fg(C_DIM))))
                .collect();
            f.render_widget(List::new(log_lines), chunks[2]);
        } else {
            let max_name = app.panes.values().map(|p| p.pane_name.len() + 2).max().unwrap_or(8);
            let max_type = app.panes.values().map(|p| p.pane_type.len()).max().unwrap_or(5);

            let items: Vec<ListItem> = app.pane_order.iter().enumerate().map(|(i, name)| {
                let ps     = app.panes.get(name);
                let is_sel = i == app.selected;
                let sep    = Span::styled(" :: ", Style::default().fg(C_DIM));

                let cursor = if is_sel {
                    Span::styled(":: ", Style::default().fg(C_ORANGE))
                } else {
                    Span::raw("   ")
                };

                let name_span = Span::styled(
                    format!("{:<width$}", format!("\"{}\"", ps.map(|p| p.pane_name.to_uppercase()).unwrap_or_else(|| name.to_uppercase())), width = max_name),
                    Style::default().fg(C_WHITE).add_modifier(if is_sel { Modifier::BOLD } else { Modifier::empty() }),
                );

                let type_span = Span::styled(
                    format!("[{:<width$}]", ps.map(|p| p.pane_type.to_uppercase()).unwrap_or_else(|| "?".to_string()), width = max_type),
                    Style::default().fg(C_DIM),
                );

                let (pb_label, pb_color) = ps.map(|p| p.playback_label()).unwrap_or(("Offline", C_RED));
                let pb_span = Span::styled(format!("[{:7}]", pb_label), Style::default().fg(pb_color));

                let (vol_str, vol_color) = ps.map(|p| p.volume_label()).unwrap_or_else(|| ("Offline".to_string(), C_RED));
                let vol_span = Span::styled(format!("[{:8}]", vol_str), Style::default().fg(vol_color));

                let title_str = ps.map(|p| p.title.as_deref().filter(|s| !s.is_empty()).unwrap_or("-").to_string()).unwrap_or_else(|| "-".to_string());
                let title_span = Span::styled(title_str, Style::default().fg(C_CYAN));

                let cmd_badge = if is_sel && app.command_mode {
                    Span::styled(" [CMD]", Style::default().fg(C_ORANGE))
                } else {
                    Span::raw("")
                };

                let item = ListItem::new(Line::from(vec![
                    cursor, name_span, sep.clone(),
                    type_span, sep.clone(),
                    pb_span, sep.clone(),
                    vol_span, sep,
                    title_span, cmd_badge,
                ]));
                if is_sel { item.style(Style::default().bg(Color::Rgb(20, 40, 40))) } else { item }
            }).collect();

            f.render_widget(List::new(items), chunks[2]);
        }

        if app.show_picker {
            let picker_items: Vec<ListItem> = app.layouts.iter().enumerate().map(|(i, name)| {
                let is_sel = i == app.picker_sel;
                let item = ListItem::new(Line::from(vec![
                    Span::raw(if is_sel { ">> " } else { "   " }),
                    Span::styled(name.clone(), Style::default()
                        .fg(if is_sel { C_CYAN } else { C_HINT })
                        .add_modifier(if is_sel { Modifier::BOLD } else { Modifier::empty() })
                    ),
                    if name == &app.layout {
                        Span::styled(" *", Style::default().fg(C_ORANGE))
                    } else {
                        Span::raw("")
                    },
                ]));
                if is_sel { item.style(Style::default().bg(Color::Rgb(20, 40, 40))) } else { item }
            }).collect();

            let picker_height = app.layouts.len() as u16 + 2;
            let picker_area = ratatui::layout::Rect {
                x:      chunks[2].x + 2,
                y:      chunks[2].y,
                width:  30,
                height: picker_height.min(chunks[2].height),
            };
            f.render_widget(ratatui::widgets::Clear, picker_area);
            f.render_widget(
                List::new(picker_items)
                    .block(ratatui::widgets::Block::default()
                        .borders(ratatui::widgets::Borders::ALL)
                        .border_style(Style::default().fg(C_ORANGE))
                        .title(" Layout ")),
                picker_area,
            );
        }

        f.render_widget(divider(size.width as usize), chunks[3]);

        let footer = if app.command_mode {
            Line::from(vec![
                Span::styled("[CMD]",    Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(" :: ",     Style::default()),
                Span::styled("[Space]",  Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(" Pause",   Style::default()),
                Span::styled(" :: ",     Style::default()),
                Span::styled("[m]",      Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(" Mute",    Style::default()),
                Span::styled(" :: ",     Style::default()),
                Span::styled("[↵]",     Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(" Next",    Style::default()),
                Span::styled(" :: ",     Style::default()),
                Span::styled("[h/l]",    Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(" ±5s",     Style::default()),
                Span::styled(" :: ",     Style::default()),
                Span::styled("[j/k]",    Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(" ±60s",    Style::default()),
                Span::styled(" :: ",     Style::default()),
                Span::styled("[9/0]",    Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(" Vol",     Style::default()),
                Span::styled(" :: ",     Style::default()),
                Span::styled("[f]",      Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(" Full",    Style::default()),
                Span::styled(" :: ",     Style::default()),
                Span::styled("[v]",      Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(" Adv",     Style::default()),
                Span::styled(" :: ",     Style::default()),
                Span::styled("[Tab]",    Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(" Close",   Style::default()),
            ])
        } else {
            Line::from(vec![
                Span::styled("[j/k]",   Style::default().fg(C_DIM)),
                Span::styled(" Nav",    Style::default().fg(C_DIM)),
                Span::styled(" :: ",    Style::default().fg(C_ORANGE)),
                Span::styled("[↵]",    Style::default().fg(C_DIM)),
                Span::styled(" Detail", Style::default().fg(C_DIM)),
                Span::styled(" :: ",    Style::default().fg(C_ORANGE)),
                Span::styled("[r/R]",   Style::default().fg(C_DIM)),
                Span::styled(" Restart",Style::default().fg(C_DIM)),
                Span::styled(" :: ",    Style::default().fg(C_ORANGE)),
                Span::styled("[S]",     Style::default().fg(C_DIM)),
                Span::styled(" Solo",   Style::default().fg(C_DIM)),
                Span::styled(" :: ",    Style::default().fg(C_ORANGE)),
                Span::styled("[M]",     Style::default().fg(C_DIM)),
                Span::styled(" Mute∅",  Style::default().fg(C_DIM)),
                Span::styled(" :: ",    Style::default().fg(C_ORANGE)),
                Span::styled("[X/P]",   Style::default().fg(C_DIM)),
                Span::styled(" Stp/Ply",Style::default().fg(C_DIM)),
                Span::styled(" :: ",    Style::default().fg(C_ORANGE)),
                Span::styled("[W]",     Style::default().fg(C_DIM)),
                Span::styled(" Layout", Style::default().fg(C_DIM)),
                Span::styled(" :: ",    Style::default().fg(C_ORANGE)),
                Span::styled("[L]",     Style::default().fg(C_DIM)),
                Span::styled(" Log",    Style::default().fg(C_DIM)),
                Span::styled(" :: ",    Style::default().fg(C_ORANGE)),
                Span::styled("[q]",     Style::default().fg(C_DIM)),
                Span::styled(" Quit",   Style::default().fg(C_DIM)),
            ])
        };

        let footer_widget = if app.command_mode {
            Paragraph::new(footer)
                .style(Style::default().bg(Color::Rgb(80, 40, 10)).fg(Color::Rgb(160, 100, 40)))
        } else {
            Paragraph::new(footer)
        };
        f.render_widget(footer_widget, chunks[4]);

    })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Rendering — startup status line
// ---------------------------------------------------------------------------

fn render_startup(terminal: &mut Term, status: &str) -> io::Result<()> {
    terminal.draw(|f| {
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
                Span::styled(" :: ",     Style::default().fg(C_DIM)),
                Span::styled(status,     Style::default().fg(C_HINT)),
                Span::styled(" ::",      Style::default().fg(C_DIM)),
            ])),
            f.size(),
        );
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Rendering — details screen
// ---------------------------------------------------------------------------

fn render_details(terminal: &mut Term, app: &App) -> io::Result<()> {
    let pane_name = match app.pane_order.get(app.selected) {
        Some(n) => n.clone(),
        None    => return Ok(()),
    };
    let ps          = app.panes.get(&pane_name);
    let current_pos = app.current_playlist_pos();

    terminal.draw(|f| {
        let size = f.size();

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(size);

        let sep = Span::styled(" :: ", Style::default().fg(C_DIM));
        let (pb_label, pb_color) = ps.map(|p| p.playback_label()).unwrap_or(("Offline", C_RED));
        let (vol_str, vol_color) = ps.map(|p| p.volume_label()).unwrap_or_else(|| ("Offline".to_string(), C_RED));
        f.render_widget(Paragraph::new(Line::from(vec![
            Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
            sep.clone(),
            Span::styled(format!("\"{}\"", ps.map(|p| p.pane_name.to_uppercase()).unwrap_or_else(|| pane_name.to_uppercase())), Style::default().fg(C_WHITE).add_modifier(Modifier::BOLD)),
            sep.clone(),
            Span::styled(
                format!("[{}]", ps.map(|p| p.pane_type.to_uppercase()).unwrap_or_else(|| "?".to_string())),
                Style::default().fg(C_DIM),
            ),
            sep.clone(),
            Span::styled(format!("[{:7}]", pb_label), Style::default().fg(pb_color)),
            sep.clone(),
            Span::styled(format!("[{:8}]", vol_str), Style::default().fg(vol_color)),
            sep.clone(),
            Span::styled(format!("{} items", app.playlist_items.len()), Style::default().fg(C_DIM)),
        ])), chunks[0]);

        f.render_widget(divider(size.width as usize), chunks[1]);

        let list_height = chunks[2].height as usize;
        let scroll_off  = if app.playlist_sel >= list_height {
            app.playlist_sel + 1 - list_height
        } else {
            0
        };

        let items: Vec<ListItem> = app.playlist_items.iter().enumerate()
            .skip(scroll_off)
            .take(list_height)
            .map(|(i, entry)| {
                let is_current  = i as i64 == current_pos;
                let is_sel      = i == app.playlist_sel;
                let is_marked   = app.selected_items.contains(&i);
                let display     = {
                    let t = entry.trim_end_matches('/');
                    t.split('/').last().unwrap_or(entry.as_str())
                };

                let cursor = if is_sel {
                    Span::styled(">> ", Style::default().fg(C_ORANGE))
                } else {
                    Span::raw("   ")
                };

                let now_marker = if is_current {
                    Span::styled("* ", Style::default().fg(C_GREEN))
                } else if is_marked {
                    Span::styled("• ", Style::default().fg(C_ORANGE))
                } else {
                    Span::raw("  ")
                };

                let idx_color = if is_current { C_ORANGE } else if is_marked { C_ORANGE } else { C_DIM };
                let txt_color = if is_current { C_CYAN } else if is_marked { C_WHITE } else { C_HINT };

                let item = ListItem::new(Line::from(vec![
                    cursor,
                    now_marker,
                    Span::styled(format!("{:<4}", i), Style::default().fg(idx_color)),
                    Span::styled(" :: ", Style::default().fg(C_DIM)),
                    Span::styled(display.to_string(), Style::default().fg(txt_color)),
                ]));
                if is_sel { item.style(Style::default().bg(Color::Rgb(20, 40, 40))) } else { item }
            })
            .collect();

        f.render_widget(List::new(items), chunks[2]);

        f.render_widget(divider(size.width as usize), chunks[3]);

        let footer = match &app.details_mode {

            DetailsMode::Jump => Line::from(vec![
                Span::styled("Jump to #: ", Style::default().fg(C_HINT)),
                Span::styled(app.jump_input.clone(), Style::default().fg(C_CYAN).add_modifier(Modifier::BOLD)),
                Span::styled("  [Enter] Go  [Esc] Cancel", Style::default().fg(C_DIM)),
            ]),

            DetailsMode::Add => Line::from(vec![
                Span::styled("Add: ", Style::default().fg(C_HINT)),
                Span::styled(app.add_input.clone(), Style::default().fg(C_WHITE).add_modifier(Modifier::BOLD)),
                Span::styled("  [Enter] Add  [Esc] Cancel", Style::default().fg(C_DIM)),
            ]),

            DetailsMode::Normal => {
                if let Some(msg) = &app.status_msg {
                    Line::from(vec![
                        Span::styled(msg.clone(), Style::default().fg(C_RED).add_modifier(Modifier::BOLD)),
                    ])
                } else if app.command_mode {
                    Line::from(vec![
                        Span::styled("[CMD]",    Style::default().fg(C_ORANGE).add_modifier(Modifier::BOLD)),
                        Span::styled(" :: ",     Style::default().fg(C_DIM)),
                        Span::styled("[Space]",  Style::default().fg(C_DIM)),
                        Span::styled(" Pause",   Style::default().fg(C_DIM)),
                        Span::styled(" :: ",     Style::default().fg(C_ORANGE)),
                        Span::styled("[m]",      Style::default().fg(C_DIM)),
                        Span::styled(" Mute",    Style::default().fg(C_DIM)),
                        Span::styled(" :: ",     Style::default().fg(C_ORANGE)),
                        Span::styled("[Enter]",  Style::default().fg(C_DIM)),
                        Span::styled(" Next",    Style::default().fg(C_DIM)),
                        Span::styled(" :: ",     Style::default().fg(C_ORANGE)),
                        Span::styled("[h/l]",    Style::default().fg(C_DIM)),
                        Span::styled(" ±5s",     Style::default().fg(C_DIM)),
                        Span::styled(" :: ",     Style::default().fg(C_ORANGE)),
                        Span::styled("[j/k]",    Style::default().fg(C_DIM)),
                        Span::styled(" ±60s",    Style::default().fg(C_DIM)),
                        Span::styled(" :: ",     Style::default().fg(C_ORANGE)),
                        Span::styled("[9/0]",    Style::default().fg(C_DIM)),
                        Span::styled(" Vol",     Style::default().fg(C_DIM)),
                        Span::styled(" :: ",     Style::default().fg(C_ORANGE)),
                        Span::styled("[Tab]",    Style::default().fg(C_DIM)),
                        Span::styled(" Close",   Style::default().fg(C_DIM)),
                    ])
                } else {
                    Line::from(vec![
                        Span::styled("[j/k]",   Style::default().fg(C_DIM)),
                        Span::styled(" Nav",    Style::default().fg(C_DIM)),
                        Span::styled(" :: ",    Style::default().fg(C_ORANGE)),
                        Span::styled("[Spc]",   Style::default().fg(C_DIM)),
                        Span::styled(" Mark",   Style::default().fg(C_DIM)),
                        Span::styled(" :: ",    Style::default().fg(C_ORANGE)),
                        Span::styled("[D]",     Style::default().fg(C_DIM)),
                        Span::styled(" Del",    Style::default().fg(C_DIM)),
                        Span::styled(" :: ",    Style::default().fg(C_ORANGE)),
                        Span::styled("[M]",     Style::default().fg(C_DIM)),
                        Span::styled(" Move",   Style::default().fg(C_DIM)),
                        Span::styled(" :: ",    Style::default().fg(C_ORANGE)),
                        Span::styled("[C]",     Style::default().fg(C_DIM)),
                        Span::styled(" Crop",   Style::default().fg(C_DIM)),
                        Span::styled(" :: ",    Style::default().fg(C_ORANGE)),
                        Span::styled("[A]",     Style::default().fg(C_DIM)),
                        Span::styled(" Add",    Style::default().fg(C_DIM)),
                        Span::styled(" :: ",    Style::default().fg(C_ORANGE)),
                        Span::styled("[S]",     Style::default().fg(C_DIM)),
                        Span::styled(" Save",   Style::default().fg(C_DIM)),
                        Span::styled(" :: ",    Style::default().fg(C_ORANGE)),
                        Span::styled("[G]",     Style::default().fg(C_DIM)),
                        Span::styled(" Goto",   Style::default().fg(C_DIM)),
                        Span::styled(" :: ",    Style::default().fg(C_ORANGE)),
                        Span::styled("[S-Spc]", Style::default().fg(C_DIM)),
                        Span::styled(" Play",   Style::default().fg(C_DIM)),
                        Span::styled(" :: ",    Style::default().fg(C_ORANGE)),
                        Span::styled("[↵]",    Style::default().fg(C_DIM)),
                        Span::styled(" Back",   Style::default().fg(C_DIM)),
                    ])
                }
            },
        };

        let footer_widget = if app.command_mode {
            Paragraph::new(footer)
                .style(Style::default().bg(Color::Rgb(80, 40, 10)).fg(Color::Rgb(160, 100, 40)))
        } else {
            Paragraph::new(footer)
        };
        f.render_widget(footer_widget, chunks[4]);

    })?;
    Ok(())
}

fn divider(width: usize) -> Paragraph<'static> {
    Paragraph::new(Span::styled(
        "-".repeat(width),
        Style::default().fg(C_DIVIDER),
    ))
}

// ---------------------------------------------------------------------------
// Send commands to daemon
// ---------------------------------------------------------------------------

async fn send_cmd(ws_tx: &mut WsSink, pane: &str, cmd: &str, args: serde_json::Value) {
    let _ = ws_tx.send(Message::Text(serde_json::json!({
        "command": cmd,
        "pane":    pane,
        "args":    args,
    }).to_string())).await;
}

async fn send_node_cmd(ws_tx: &mut WsSink, cmd: &str, params: serde_json::Value) {
    let mut msg    = params;
    msg["command"] = serde_json::Value::String(cmd.to_string());
    let _ = ws_tx.send(Message::Text(msg.to_string())).await;
}

async fn reload_playlist_cmd(ws_tx: &mut WsSink, pane: &str) {
    let path = pane_playlist(pane);
    send_cmd(ws_tx, pane, "loadlist",
        serde_json::json!([path.to_string_lossy(), "replace"])).await;
}

async fn cmd_stop_all(ws_tx: &mut WsSink, pane_order: &[String]) {
    for pane in pane_order {
        send_cmd(ws_tx, pane, "stop", serde_json::json!([])).await;
    }
}

async fn cmd_start_all(ws_tx: &mut WsSink, pane_order: &[String]) {
    for pane in pane_order {
        send_cmd(ws_tx, pane, "set_property", serde_json::json!(["pause", false])).await;
    }
}

async fn cmd_solo(ws_tx: &mut WsSink, solo: &str, pane_order: &[String]) {
    for pane in pane_order {
        if pane == solo {
            send_cmd(ws_tx, pane, "set_property", serde_json::json!(["mute",  false])).await;
            send_cmd(ws_tx, pane, "set_property", serde_json::json!(["pause", false])).await;
        } else {
            send_cmd(ws_tx, pane, "set_property", serde_json::json!(["mute", true])).await;
        }
    }
}

async fn cmd_mute_others(ws_tx: &mut WsSink, keep: &str, pane_order: &[String]) {
    for pane in pane_order {
        if pane != keep {
            send_cmd(ws_tx, pane, "set_property", serde_json::json!(["mute", true])).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Spawn daemon sibling binary.
//
// Platform agnostic — if panebot-daemon exists next to the TUI binary,
// spawn it. Works on macOS, Linux without systemd, or any manual deployment.
// Returns Ok(()) if spawned, Err if binary not found or spawn fails.
// ---------------------------------------------------------------------------

fn spawn_daemon() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false; };
    let Some(dir) = exe.parent() else { return false; };
    let daemon_path = dir.join("panebot-daemon");

    if !daemon_path.exists() { return false; }

    let mut cmd = std::process::Command::new(&daemon_path);
    cmd.stdin(std::process::Stdio::null())
       .stdout(std::process::Stdio::null())
       .stderr(std::process::Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    cmd.spawn().is_ok()
}

// ---------------------------------------------------------------------------
// WebSocket connection — connect with retry.
//
// Local address: try to spawn the daemon binary if present, then wait
//   indefinitely — the daemon may just be starting up.
// Remote address: retry with countdown, give up after CONNECT_TIMEOUT_S.
//
// The spawn attempt is best-effort — if no binary is present (e.g. systemd
// manages the daemon) we skip it and just wait for the connection.
// ---------------------------------------------------------------------------

async fn connect_ws(
    terminal: &mut Term,
    addr:     &str,
) -> io::Result<(tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, bool)> {

    render_startup(terminal, &format!("Connecting to {}...", addr))?;

    // Fast path — daemon already running
    if let Ok((ws, _)) = connect_async(addr).await {
        return Ok((ws, false));
    }

    let is_local = addr == LOCAL_ADDR;

    if is_local {
        let spawned = spawn_daemon();
        if spawned {
            render_startup(terminal, "Starting daemon...")?;
        } else {
            render_startup(terminal, "Waiting for daemon...")?;
        }
        loop {
            render_startup(terminal, "Waiting for daemon...")?;
            tokio::time::sleep(std::time::Duration::from_millis(CONNECT_RETRY_MS)).await;
            if let Ok((ws, _)) = connect_async(addr).await {
                return Ok((ws, spawned));
            }
        }
    }

    // Remote address — retry with countdown, give up after timeout
    let retries = (CONNECT_TIMEOUT_S * 1000 / CONNECT_RETRY_MS) as u32;
    for attempt in 0..retries {
        let remaining = CONNECT_TIMEOUT_S.saturating_sub(attempt as u64 * CONNECT_RETRY_MS / 1000);
        render_startup(terminal, &format!("Waiting for {} ({}s)...", addr, remaining))?;
        tokio::time::sleep(std::time::Duration::from_millis(CONNECT_RETRY_MS)).await;
        if let Ok((ws, _)) = connect_async(addr).await {
            return Ok((ws, false));
        }
    }

    Err(io::Error::new(io::ErrorKind::TimedOut,
        format!("Could not connect to {} after {}s", addr, CONNECT_TIMEOUT_S)))
}

// ---------------------------------------------------------------------------
// Host resolution
//
// 0 hosts configured → connect to localhost
// 1 host configured  → connect to it automatically
// 2+ hosts           → show picker, user selects
// ---------------------------------------------------------------------------

async fn resolve_daemon_addr(terminal: &mut Term) -> io::Result<Option<String>> {
    let hosts = load_hosts();

    match hosts.len() {
        0 => Ok(Some(LOCAL_ADDR.to_string())),
        1 => Ok(Some(hosts[0].address.clone())),
        _ => {
            let mut sel        = 0usize;
            let mut key_events = EventStream::new();
            render_host_picker(terminal, &hosts, sel)?;

            loop {
                if let Some(Ok(Event::Key(k))) = key_events.next().await {
                    match k.code {
                        KeyCode::Char('q') => return Ok(None),
                        KeyCode::Char('k') | KeyCode::Up => {
                            sel = sel.saturating_sub(1);
                            render_host_picker(terminal, &hosts, sel)?;
                        }
                        KeyCode::Char('j') | KeyCode::Down => {
                            if sel + 1 < hosts.len() { sel += 1; }
                            render_host_picker(terminal, &hosts, sel)?;
                        }
                        KeyCode::Enter => {
                            return Ok(Some(hosts[sel].address.clone()));
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Key action — returned from handle_key to tell run() what to do
// ---------------------------------------------------------------------------

enum KeyAction {
    Quit,
    Render,
    RenderDetails,
    Nothing,
}

// ---------------------------------------------------------------------------
// Key handler — all key logic lives here, run() just dispatches
// ---------------------------------------------------------------------------

async fn handle_key(
    app:    &mut App,
    k:      crossterm::event::KeyEvent,
    ws_tx:  &mut WsSink,
) -> io::Result<KeyAction> {

    if k.code == KeyCode::Char('q') && app.details_mode != DetailsMode::Add {
        return Ok(KeyAction::Quit);
    }

    // ================================================================
    // DETAILS SCREEN
    // ================================================================
    if app.show_details {

        // -- Jump mode --
        if app.details_mode == DetailsMode::Jump {
            match k.code {
                KeyCode::Char(c) if c.is_ascii_digit() => {
                    app.jump_input.push(c);
                }
                KeyCode::Backspace => {
                    app.jump_input.pop();
                }
                KeyCode::Enter => {
                    if let Ok(idx) = app.jump_input.parse::<usize>() {
                        let max = app.playlist_items.len().saturating_sub(1);
                        app.playlist_sel = idx.min(max);
                    }
                    app.jump_input.clear();
                    app.details_mode = DetailsMode::Normal;
                }
                KeyCode::Esc => {
                    app.jump_input.clear();
                    app.details_mode = DetailsMode::Normal;
                }
                _ => return Ok(KeyAction::Nothing),
            }
            return Ok(KeyAction::RenderDetails);
        }

        // -- Add mode --
        if app.details_mode == DetailsMode::Add {
            match k.code {
                KeyCode::Char(c) => {
                    app.add_input.push(c);
                }
                KeyCode::Backspace => {
                    app.add_input.pop();
                }
                KeyCode::Enter => {
                    let entry = app.add_input.trim().to_string();
                    if !entry.is_empty() {
                        if let Some(pane) = app.selected_name() {
                            let expanded = if entry.starts_with('~') {
                                entry.replacen('~', &app.home, 1)
                            } else {
                                entry.clone()
                            };
                            match m3u_append(&pane, &expanded) {
                                Ok(items) => {
                                    app.playlist_items = items;
                                    reload_playlist_cmd(ws_tx, &pane).await;
                                }
                                Err(e) => {
                                    app.status_msg = Some(format!("Add failed: {}", e));
                                }
                            }
                        }
                    }
                    app.add_input.clear();
                    app.details_mode = DetailsMode::Normal;
                }
                KeyCode::Esc => {
                    app.add_input.clear();
                    app.details_mode = DetailsMode::Normal;
                }
                _ => return Ok(KeyAction::Nothing),
            }
            return Ok(KeyAction::RenderDetails);
        }

        // -- Command mode (global, works in details too) --
        if app.command_mode {
            if let Some(pane) = app.selected_name() {
                match k.code {
                    KeyCode::Char(' ')              => { send_cmd(ws_tx, &pane, "cycle",     serde_json::json!(["pause"])).await; }
                    KeyCode::Char('m')              => { send_cmd(ws_tx, &pane, "cycle",     serde_json::json!(["mute"])).await; }
                    KeyCode::Enter                  => { send_cmd(ws_tx, &pane, "keypress",  serde_json::json!(["ENTER"])).await; }
                    KeyCode::Char('f')              => { send_cmd(ws_tx, &pane, "cycle",     serde_json::json!(["fullscreen"])).await; }
                    KeyCode::Left  | KeyCode::Char('h') => { send_cmd(ws_tx, &pane, "seek", serde_json::json!([-5,  "relative"])).await; }
                    KeyCode::Right | KeyCode::Char('l') => { send_cmd(ws_tx, &pane, "seek", serde_json::json!([5,   "relative"])).await; }
                    KeyCode::Up    | KeyCode::Char('k') => { send_cmd(ws_tx, &pane, "seek", serde_json::json!([60,  "relative"])).await; }
                    KeyCode::Down  | KeyCode::Char('j') => { send_cmd(ws_tx, &pane, "seek", serde_json::json!([-60, "relative"])).await; }
                    KeyCode::Char('0')              => { send_cmd(ws_tx, &pane, "add",      serde_json::json!(["volume",  5])).await; }
                    KeyCode::Char('9')              => { send_cmd(ws_tx, &pane, "add",      serde_json::json!(["volume", -5])).await; }
                    KeyCode::Char('v')              => { /* TODO: advanced passthrough */ }
                    KeyCode::Tab                    => {
                        app.command_mode = false;
                        app.status_msg   = None;
                    }
                    _ => return Ok(KeyAction::Nothing),
                }
                return Ok(KeyAction::RenderDetails);
            }
        }

        // -- Normal mode (details) --
        match k.code {
            // Close details
            KeyCode::Enter if !k.modifiers.contains(KeyModifiers::SHIFT) => {
                app.show_details = false;
                app.details_mode = DetailsMode::Normal;
                app.status_msg   = None;
                app.jump_input.clear();
                app.selected_items.clear();
                return Ok(KeyAction::Render);
            }

            // Play selected item
            KeyCode::Char(' ') if k.modifiers.contains(KeyModifiers::SHIFT) => {
                if let Some(pane) = app.selected_name() {
                    let ps = app.panes.get(&pane);
                    if ps.map(|p| p.idle_active.unwrap_or(true)).unwrap_or(true) {
                        reload_playlist_cmd(ws_tx, &pane).await;
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    }
                    send_cmd(ws_tx, &pane, "playlist-play-index",
                        serde_json::json!([app.playlist_sel])).await;
                    send_cmd(ws_tx, &pane, "set_property",
                        serde_json::json!(["pause", false])).await;
                    app.status_msg = None;
                }
            }

            // Toggle command mode
            KeyCode::Tab => {
                app.command_mode = !app.command_mode;
                app.status_msg   = None;
            }

            // Navigation
            KeyCode::Up | KeyCode::Char('k') => {
                app.playlist_sel = app.playlist_sel.saturating_sub(1);
                app.status_msg   = None;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if app.playlist_sel + 1 < app.playlist_items.len() {
                    app.playlist_sel += 1;
                }
                app.status_msg = None;
            }
            KeyCode::Char('[') => {
                let page = 10usize;
                app.playlist_sel = app.playlist_sel.saturating_sub(page);
                app.status_msg   = None;
            }
            KeyCode::Char(']') => {
                let page = 10usize;
                let max  = app.playlist_items.len().saturating_sub(1);
                app.playlist_sel = (app.playlist_sel + page).min(max);
                app.status_msg   = None;
            }
            KeyCode::Char('\\') => {
                app.playlist_sel = app.playlist_items.len().saturating_sub(1);
                app.status_msg   = None;
            }

            // Toggle selection on current item
            KeyCode::Char(' ') => {
                let idx = app.playlist_sel;
                if app.selected_items.contains(&idx) {
                    app.selected_items.remove(&idx);
                } else {
                    app.selected_items.insert(idx);
                }
                // Advance cursor after marking
                if app.playlist_sel + 1 < app.playlist_items.len() {
                    app.playlist_sel += 1;
                }
                app.status_msg = None;
            }

            // Clear selection
            KeyCode::Esc => {
                if !app.selected_items.is_empty() {
                    app.selected_items.clear();
                    app.status_msg = None;
                } else {
                    return Ok(KeyAction::Nothing);
                }
            }

            // Go to item number
            KeyCode::Char('G') => {
                app.details_mode = DetailsMode::Jump;
                app.jump_input.clear();
            }

            // Add item
            KeyCode::Char('A') => {
                app.details_mode = DetailsMode::Add;
                app.add_input.clear();
                app.status_msg   = None;
            }

            // Delete — multi-aware
            KeyCode::Char('D') => {
                if let Some(pane) = app.selected_name() {
                    let current_pos = app.current_playlist_pos();
                    let targets: Vec<usize> = if app.selected_items.is_empty() {
                        vec![app.playlist_sel]
                    } else {
                        let mut v: Vec<usize> = app.selected_items.iter().cloned().collect();
                        v.sort_unstable();
                        v
                    };

                    // Check none of the targets are currently playing
                    if targets.iter().any(|&i| current_pos >= 0 && i as i64 == current_pos) {
                        app.status_msg = Some("Cannot remove playing item".to_string());
                    } else {
                        let mut items = app.playlist_items.clone();
                        // Remove in reverse order to preserve indices
                        for &idx in targets.iter().rev() {
                            if idx < items.len() { items.remove(idx); }
                        }
                        match write_m3u(&pane, &items) {
                            Ok(_) => {
                                app.playlist_items = items;
                                app.selected_items.clear();
                                app.playlist_sel = app.playlist_sel.min(
                                    app.playlist_items.len().saturating_sub(1)
                                );
                                reload_playlist_cmd(ws_tx, &pane).await;
                                app.status_msg = None;
                            }
                            Err(e) => { app.status_msg = Some(format!("Delete failed: {}", e)); }
                        }
                    }
                }
            }

            // Move selected items to cursor position — multi-aware
            KeyCode::Char('M') => {
                if let Some(pane) = app.selected_name() {
                    if app.selected_items.is_empty() {
                        app.status_msg = Some("Mark items with Space first".to_string());
                    } else {
                        let dest = app.playlist_sel;
                        let mut targets: Vec<usize> = app.selected_items.iter().cloned().collect();
                        targets.sort_unstable();

                        // Build new list: extract marked items, insert at dest
                        let marked: Vec<String> = targets.iter()
                            .filter_map(|&i| app.playlist_items.get(i).cloned())
                            .collect();
                        let mut rest: Vec<String> = app.playlist_items.iter().enumerate()
                            .filter(|(i, _)| !app.selected_items.contains(i))
                            .map(|(_, s)| s.clone())
                            .collect();

                        // Adjust dest for removed items before it
                        let removed_before = targets.iter().filter(|&&i| i < dest).count();
                        let insert_at = dest.saturating_sub(removed_before).min(rest.len());
                        for (j, item) in marked.into_iter().enumerate() {
                            rest.insert(insert_at + j, item);
                        }

                        match write_m3u(&pane, &rest) {
                            Ok(_) => {
                                app.playlist_items = rest;
                                app.selected_items.clear();
                                app.playlist_sel = insert_at;
                                reload_playlist_cmd(ws_tx, &pane).await;
                                app.status_msg = None;
                            }
                            Err(e) => { app.status_msg = Some(format!("Move failed: {}", e)); }
                        }
                    }
                }
            }

            // Crop — multi-aware: if items selected keep those, else keep playing item
            KeyCode::Char('C') => {
                if let Some(pane) = app.selected_name() {
                    if app.selected_items.is_empty() {
                        let current_pos = app.current_playlist_pos();
                        match m3u_crop(&pane, current_pos) {
                            Ok(Some(items)) => {
                                app.playlist_items = items;
                                app.playlist_sel   = 0;
                                reload_playlist_cmd(ws_tx, &pane).await;
                                app.status_msg = None;
                            }
                            Ok(None) => { app.status_msg = Some("Nothing playing to crop around".to_string()); }
                            Err(e)   => { app.status_msg = Some(format!("Crop failed: {}", e)); }
                        }
                    } else {
                        let mut targets: Vec<usize> = app.selected_items.iter().cloned().collect();
                        targets.sort_unstable();
                        let kept: Vec<String> = targets.iter()
                            .filter_map(|&i| app.playlist_items.get(i).cloned())
                            .collect();
                        match write_m3u(&pane, &kept) {
                            Ok(_) => {
                                app.playlist_items = kept;
                                app.selected_items.clear();
                                app.playlist_sel = 0;
                                reload_playlist_cmd(ws_tx, &pane).await;
                                app.status_msg = None;
                            }
                            Err(e) => { app.status_msg = Some(format!("Crop failed: {}", e)); }
                        }
                    }
                }
            }

            // Save playlist
            KeyCode::Char('S') => {
                if let Some(pane) = app.selected_name() {
                    match save_playlist(&pane) {
                        Ok(n) => {
                            app.playlist_items = read_m3u(&pane);
                            app.status_msg = Some(format!("Saved {} items", n));
                        }
                        Err(e) => {
                            app.status_msg = Some(format!("Save failed: {}", e));
                        }
                    }
                }
            }

            _ => return Ok(KeyAction::Nothing),
        }
        return Ok(KeyAction::RenderDetails);
    }

    // ================================================================
    // DASHBOARD
    // ================================================================

    if app.show_picker {
        match k.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if app.picker_sel + 1 < app.layouts.len() {
                    app.picker_sel += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                app.picker_sel = app.picker_sel.saturating_sub(1);
            }
            KeyCode::Enter => {
                if let Some(layout) = app.layouts.get(app.picker_sel) {
                    let layout = layout.clone();
                    send_node_cmd(ws_tx, "panebot:layout",
                        serde_json::json!({"layout_name": layout})).await;
                    app.show_picker = false;
                }
            }
            KeyCode::Esc | KeyCode::Char('W') => {
                app.show_picker = false;
            }
            _ => return Ok(KeyAction::Nothing),
        }
        return Ok(KeyAction::Render);
    }

    if k.code == KeyCode::Char('W') {
        app.picker_sel  = app.layouts.iter().position(|l| l == &app.layout).unwrap_or(0);
        app.show_picker = true;
        return Ok(KeyAction::Render);
    }

    if k.code == KeyCode::Enter {
        if let Some(pane) = app.selected_name() {
            app.playlist_items = read_m3u(&pane);
            app.playlist_sel   = app.panes.get(&pane)
                .map(|p| p.playlist_pos.unwrap_or(0).max(0) as usize)
                .unwrap_or(0);
            app.selected_items.clear();
            app.show_details   = true;
            app.details_mode   = DetailsMode::Normal;
            app.status_msg     = None;
            return Ok(KeyAction::RenderDetails);
        }
        return Ok(KeyAction::Nothing);
    }

    if k.code == KeyCode::Tab {
        app.command_mode = !app.command_mode;
        return Ok(KeyAction::Render);
    }

    let shift_l = k.code == KeyCode::Char('L') ||
        (k.code == KeyCode::Char('l') && k.modifiers.contains(KeyModifiers::SHIFT));
    if shift_l {
        app.show_log = !app.show_log;
        return Ok(KeyAction::Render);
    }

    if !app.command_mode {
        match k.code {
            KeyCode::Char('j') | KeyCode::Down => { app.select_next(); return Ok(KeyAction::Render); }
            KeyCode::Char('k') | KeyCode::Up   => { app.select_prev(); return Ok(KeyAction::Render); }
            // Orchestration
            KeyCode::Char('r') => {
                if let Some(pane) = app.selected_name() {
                    send_node_cmd(ws_tx, "panebot:restart-pane", serde_json::json!({"pane": pane})).await;
                }
                return Ok(KeyAction::Render);
            }
            KeyCode::Char('R') => {
                send_node_cmd(ws_tx, "panebot:restart-all", serde_json::json!({})).await;
                return Ok(KeyAction::Render);
            }
            KeyCode::Char('S') => {
                if let Some(pane) = app.selected_name() {
                    let pane_order = app.pane_order.clone();
                    cmd_solo(ws_tx, &pane, &pane_order).await;
                }
                return Ok(KeyAction::Render);
            }
            KeyCode::Char('M') => {
                if let Some(pane) = app.selected_name() {
                    let pane_order = app.pane_order.clone();
                    cmd_mute_others(ws_tx, &pane, &pane_order).await;
                }
                return Ok(KeyAction::Render);
            }
            KeyCode::Char('X') => {
                let pane_order = app.pane_order.clone();
                cmd_stop_all(ws_tx, &pane_order).await;
                return Ok(KeyAction::Render);
            }
            KeyCode::Char('P') => {
                let pane_order = app.pane_order.clone();
                cmd_start_all(ws_tx, &pane_order).await;
                return Ok(KeyAction::Render);
            }
            _ => {}
        }
    }

    if app.command_mode {
        if let Some(pane) = app.selected_name() {
            match k.code {
                KeyCode::Char(' ')              => { send_cmd(ws_tx, &pane, "cycle",         serde_json::json!(["pause"])).await; }
                KeyCode::Char('m')              => { send_cmd(ws_tx, &pane, "cycle",         serde_json::json!(["mute"])).await; }
                KeyCode::Enter                  => { send_cmd(ws_tx, &pane, "keypress",      serde_json::json!(["ENTER"])).await; }
                KeyCode::Char('f')              => { send_cmd(ws_tx, &pane, "cycle",         serde_json::json!(["fullscreen"])).await; }
                KeyCode::Left  | KeyCode::Char('h') => { send_cmd(ws_tx, &pane, "seek",     serde_json::json!([-5,  "relative"])).await; }
                KeyCode::Right | KeyCode::Char('l') => { send_cmd(ws_tx, &pane, "seek",     serde_json::json!([5,   "relative"])).await; }
                KeyCode::Up    | KeyCode::Char('k') => { send_cmd(ws_tx, &pane, "seek",     serde_json::json!([60,  "relative"])).await; }
                KeyCode::Down  | KeyCode::Char('j') => { send_cmd(ws_tx, &pane, "seek",     serde_json::json!([-60, "relative"])).await; }
                KeyCode::Char('0')              => { send_cmd(ws_tx, &pane, "add",          serde_json::json!(["volume",  5])).await; }
                KeyCode::Char('9')              => { send_cmd(ws_tx, &pane, "add",          serde_json::json!(["volume", -5])).await; }
                KeyCode::Char('v')              => { /* TODO: advanced passthrough mode */ }
                _ => return Ok(KeyAction::Nothing),
            }
            return Ok(KeyAction::Render);
        }
    }

    Ok(KeyAction::Nothing)
}

// ---------------------------------------------------------------------------
// Main event loop
//
// App state is created once and preserved across reconnects — log history,
// pane selection, and layout name survive a daemon restart.
// Pane online states are reset on reconnect since we'll get fresh events.
// ---------------------------------------------------------------------------

async fn run(terminal: &mut Term, addr: &str) -> io::Result<()> {
    let mut app        = App::new();
    let mut key_events = EventStream::new();

    'reconnect: loop {

        let ws = match connect_ws(terminal, addr).await {
            Ok((ws, spawned)) => {
                if spawned { app.owns_daemon = true; }
                ws
            }
            Err(_) => {
                tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
                continue 'reconnect;
            }
        };

        // Clear pane state on reconnect — node:snapshot will repopulate fresh.
        app.pane_order.clear();
        app.panes.clear();
        app.selected = 0;

        let (mut ws_tx, mut ws_rx) = ws.split();

        loop {
            tokio::select! {

                msg = ws_rx.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            let signal = process_event(&mut app, &text);
                            if app.show_details { render_details(terminal, &app)?; }
                            else                { render(terminal, &app)?;         }
                            if signal == Some("node:down") {
                                tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
                                continue 'reconnect;
                            }
                        }
                        Some(Err(_)) | None => {
                            tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
                            continue 'reconnect;
                        }
                        _ => {}
                    }
                }

                key = key_events.next() => {
                    match key {
                        Some(Ok(Event::Key(k))) => {
                            match handle_key(&mut app, k, &mut ws_tx).await? {
                                KeyAction::Quit => {
                                    if app.owns_daemon {
                                        send_node_cmd(&mut ws_tx, "panebot:shutdown",
                                            serde_json::json!({})).await;
                                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                                    }
                                    break 'reconnect;
                                }
                                KeyAction::Render        => render(terminal, &app)?,
                                KeyAction::RenderDetails => {
                                    if app.show_details { render_details(terminal, &app)?; }
                                    else                { render(terminal, &app)?;         }
                                }
                                KeyAction::Nothing => {}
                            }
                        }
                        Some(Err(e)) => return Err(e),
                        None         => break 'reconnect,
                        _            => {}
                    }
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend      = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = async {
        let addr = match resolve_daemon_addr(&mut terminal).await? {
            Some(a) => a,
            None    => return Ok(()),
        };
        run(&mut terminal, &addr).await
    }.await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    result
}
