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
use std::collections::{HashMap, VecDeque};
use std::io;
use std::os::unix::process::CommandExt;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

const DAEMON_ADDR: &str = "ws://127.0.0.1:9090";
const LOG_CAPACITY: usize = 500;

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
// Pane state — mirrors PaneState in daemon
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct PaneState {
    name:         String,
    pane_type:    String,
    online:       bool,
    idle_active:  bool,
    paused:       bool,
    muted:        bool,
    volume:       f64,
    title:        String,
    playlist_pos: i64,
}

impl PaneState {
    fn new(name: &str, pane_type: &str) -> Self {
        PaneState {
            name:         name.to_string(),
            pane_type:    pane_type.to_string(),
            online:       false,
            idle_active:  true,
            paused:       true,
            muted:        false,
            volume:       0.0,
            title:        String::new(),
            playlist_pos: -1,
        }
    }

    fn playback_label(&self) -> (&'static str, Color) {
        if !self.online     { return ("Offline", C_RED);  }
        if self.idle_active { return ("Stopped", C_DIM);  }
        if self.paused      { return ("Paused",  C_HINT); }
        ("Playing", C_GREEN)
    }

    fn volume_label(&self) -> (String, Color) {
        if !self.online { return ("Offline".to_string(),        C_RED);  }
        if self.muted   { return ("Vol:Mute".to_string(),       C_DIM);  }
        (format!("Vol:{:3.0}", self.volume),                    C_CYAN)
    }
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct LogLine {
    spans: Vec<(String, Color)>,
}

struct App {
    pane_order:   Vec<String>,           // config order from bootstrap
    panes:        HashMap<String, PaneState>,
    selected:     usize,
    hostname:     String,
    layout:       String,
    log:          VecDeque<LogLine>,
    show_log:     bool,                  // Shift-L toggles full log view
    command_mode: bool,                  // Tab toggles player controls
    show_picker:   bool,                  // W opens layout picker
    picker_sel:    usize,                 // selected layout in picker
    layouts:       Vec<String>,           // available layout names
    show_playlist:  bool,                  // Enter opens playlist detail
    playlist_sel:   usize,                 // selected playlist item
    playlist_items: Vec<String>,          // playlist lines from .m3u
    jump_input:     String,               // numeric input buffer for g jump
}

impl App {
    fn new() -> Self {
        App {
            pane_order:   Vec::new(),
            panes:        HashMap::new(),
            selected:     0,
            hostname:     String::new(),
            layout:       String::new(),
            log:          VecDeque::with_capacity(LOG_CAPACITY),
            show_log:     false,
            command_mode: false,
            show_picker:   false,
            picker_sel:    0,
            layouts:       Vec::new(),
            show_playlist:  false,
            playlist_sel:   0,
            playlist_items: Vec::new(),
            jump_input:     String::new(),
        }
    }

    fn push_log(&mut self, spans: Vec<(String, Color)>) {
        if self.log.len() >= LOG_CAPACITY { self.log.pop_front(); }
        self.log.push_back(LogLine { spans });
    }

    fn active_count(&self) -> usize {
        self.panes.values().filter(|p| p.online).count()
    }

    fn selected_name(&self) -> Option<String> {
        self.pane_order.get(self.selected).cloned()
    }

    fn select_next(&mut self) {
        if !self.pane_order.is_empty() {
            self.selected = (self.selected + 1) % self.pane_order.len();
        }
    }

    fn select_prev(&mut self) {
        if !self.pane_order.is_empty() {
            self.selected = self.selected.saturating_sub(1);
        }
    }
}

// ---------------------------------------------------------------------------
// Log helpers
// ---------------------------------------------------------------------------

fn log_event(app: &mut App, pane: &str, spans: Vec<(String, Color)>) {
    let mut line = vec![
        ("[PaneBot]".to_string(),                        C_ORANGE),
        (" :: ".to_string(),                             C_DIM),
        (format!("{:<12}", pane.to_uppercase()),         C_WHITE),
        (" :: ".to_string(),                             C_DIM),
    ];
    line.extend(spans);
    app.push_log(line);
}

fn log_node(app: &mut App, spans: Vec<(String, Color)>) {
    let mut line = vec![
        ("[PaneBot]".to_string(), C_ORANGE),
        (" :: ".to_string(),      C_DIM),
        ("node".to_string(),      C_DIM),
        (" :: ".to_string(),      C_DIM),
    ];
    line.extend(spans);
    app.push_log(line);
}

// ---------------------------------------------------------------------------
// WS event processing
// ---------------------------------------------------------------------------

fn process_event(app: &mut App, text: &str) -> Option<&'static str> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let event = v["event"].as_str().unwrap_or("");

    match event {

        "bootstrap_complete" => {
            app.hostname = v["hostname"].as_str().unwrap_or("").to_string();
            app.layout   = v["layout"].as_str().unwrap_or("").to_string();
            // Seed known layouts — hardcoded for now, could come from daemon later
            app.layouts  = vec![
                "pb.left.stack".to_string(),
                "pb.right.stack".to_string(),
                "pb.top.row".to_string(),
                "pb.split".to_string(),
            ];

            if let Some(panes) = v["panes"].as_array() {
                for p in panes {
                    let name   = p["name"].as_str().unwrap_or("").to_string();
                    let ptype  = p["pane_type"].as_str().unwrap_or("video").to_string();
                    let online = p["status"].as_str() == Some("Online");
                    if !name.is_empty() && !app.pane_order.contains(&name) {
                        app.pane_order.push(name.clone());
                        let mut ps = PaneState::new(&name, &ptype);
                        ps.online = online;
                        app.panes.insert(name.clone(), ps);
                        log_event(app, &name, vec![(
                            if online { "Online".to_string()  } else { "Offline".to_string() },
                            if online { C_GREEN               } else { C_RED                  },
                        )]);
                    }
                }
            }
        }

        "online" => {
            let pane         = v["pane"].as_str().unwrap_or("");
            let was_offline  = app.panes.get(pane).map(|p| !p.online).unwrap_or(false);
            if let Some(ps) = app.panes.get_mut(pane) {
                ps.online = true;
                if let Some(state) = v.get("state") { apply_state(ps, state); }
            }
            if was_offline {
                log_event(app, pane, vec![("Online".to_string(), C_GREEN)]);
            }
        }

        "offline" => {
            let pane      = v["pane"].as_str().unwrap_or("");
            let was_online = app.panes.get(pane).map(|p| p.online).unwrap_or(false);
            if let Some(ps) = app.panes.get_mut(pane) { ps.online = false; }
            if was_online {
                log_event(app, pane, vec![("Offline".to_string(), C_RED)]);
            }
        }

        "property-change" => {
            let pane = v["pane"].as_str().unwrap_or("");
            let prop = v["property"].as_str().unwrap_or("");
            if let Some(ps) = app.panes.get_mut(pane) {
                match prop {
                    "pause"        => { ps.paused       = v["value"].as_bool().unwrap_or(true); }
                    "volume"       => { ps.volume        = v["value"].as_f64().unwrap_or(0.0); }
                    "media-title"  => { ps.title         = v["value"].as_str().unwrap_or("").to_string(); }
                    "playlist-pos" => { ps.playlist_pos  = v["value"].as_i64().unwrap_or(-1); }
                    "mute"         => { ps.muted         = v["value"].as_bool().unwrap_or(false); }
                    "idle-active"  => { ps.idle_active   = v["value"].as_bool().unwrap_or(true); }
                    _ => {}
                }
            }
            match prop {
                "volume" => {
                    let vol = v["value"].as_f64().unwrap_or(0.0);
                    log_event(app, pane, vec![
                        ("volume".to_string(), C_DIM),
                        (" :: ".to_string(),   C_DIM),
                        (format!("{:.0}", vol), C_CYAN),
                    ]);
                }
                "media-title" => {
                    let title = v["value"].as_str().unwrap_or("–").to_string();
                    log_event(app, pane, vec![
                        ("title".to_string(), C_DIM),
                        (" :: ".to_string(),  C_DIM),
                        (title,               C_CYAN),
                    ]);
                }
                "pause" => {
                    let paused = v["value"].as_bool().unwrap_or(true);
                    log_event(app, pane, vec![(
                        if paused { "Paused".to_string()  } else { "Playing".to_string() },
                        if paused { C_HINT               } else { C_GREEN               },
                    )]);
                }
                "mute" => {
                    let muted = v["value"].as_bool().unwrap_or(false);
                    log_event(app, pane, vec![(
                        if muted { "Muted".to_string() } else { "Unmuted".to_string() },
                        C_DIM,
                    )]);
                }
                _ => {}
            }
        }

        "node:down" => {
            let reason = v["reason"].as_str().unwrap_or("unknown");
            log_node(app, vec![
                ("node:down".to_string(), C_RED),
                (" :: ".to_string(),      C_DIM),
                (reason.to_string(),      C_ORANGE),
            ]);
            for ps in app.panes.values_mut() { ps.online = false; }
            return Some("node:down");
        }

        "node:layout" => {
            let layout = v["layout"].as_str().unwrap_or("unknown");
            app.layout = layout.to_string();
            log_node(app, vec![
                ("layout".to_string(), C_DIM),
                (" :: ".to_string(),   C_DIM),
                (layout.to_string(),   C_CYAN),
            ]);
        }

        "node:restart-pane" => {
            let pane = v["pane"].as_str().unwrap_or("unknown");
            log_event(app, pane, vec![("restarting".to_string(), C_ORANGE)]);
        }

        "node:restart-all" => {
            log_node(app, vec![("restart-all".to_string(), C_ORANGE)]);
        }

        _ => {}
    }

    None
}

fn apply_state(ps: &mut PaneState, state: &serde_json::Value) {
    if let Some(v) = state["paused"].as_bool()      { ps.paused       = v; }
    if let Some(v) = state["muted"].as_bool()       { ps.muted        = v; }
    if let Some(v) = state["idle_active"].as_bool() { ps.idle_active  = v; }
    if let Some(v) = state["volume"].as_f64()       { ps.volume       = v; }
    if let Some(v) = state["playlist_pos"].as_i64() { ps.playlist_pos = v; }
    if let Some(v) = state["title"].as_str()        { ps.title        = v.to_string(); }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

type Term = Terminal<CrosstermBackend<io::Stdout>>;

fn render(terminal: &mut Term, app: &App) -> io::Result<()> {
    terminal.draw(|f| {
        let size         = f.size();
        let _pane_count  = app.pane_order.len().max(1) as u16;

        // Fixed layout: header | div | content | div | footer
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),       // header
                Constraint::Length(1),       // divider
                Constraint::Min(1),          // content
                Constraint::Length(1),       // divider
                Constraint::Length(1),       // footer
            ])
            .split(size);

        // -- Header --
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
            Span::styled(app.hostname.clone(), Style::default().fg(C_DIM)),
        ])), chunks[0]);

        // -- Divider --
        f.render_widget(divider(size.width as usize), chunks[1]);

        // -- Content: log view or pane rows --
        if app.show_log {
            let log_height = chunks[2].height as usize;
            let log_items: Vec<ListItem> = app.log.iter()
                .rev()
                .take(log_height)
                .rev()
                .map(|line| ListItem::new(Line::from(
                    line.spans.iter()
                        .map(|(t, c)| Span::styled(t.clone(), Style::default().fg(*c)))
                        .collect::<Vec<_>>()
                )))
                .collect();
            f.render_widget(List::new(log_items), chunks[2]);
        } else {
            let max_name = app.pane_order.iter().map(|n| n.len() + 2).max().unwrap_or(8);
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
                    format!("{:<width$}", format!("\"{}\"", name.to_uppercase()), width = max_name),
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

                let title_str = ps.map(|p| if p.title.is_empty() { "–".to_string() } else { p.title.clone() }).unwrap_or_else(|| "–".to_string());
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

        // -- Layout picker overlay --
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

            // Render picker in top-left of content area
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

        // -- Divider --
        f.render_widget(divider(size.width as usize), chunks[3]);

        // -- Footer --
        let footer = if app.command_mode {
            Line::from(vec![
                Span::styled("[CMD]",     Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(" :: ",      Style::default()),
                Span::styled("[Space]",   Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(" Play",     Style::default()),
                Span::styled(" :: ",      Style::default()),
                Span::styled("[m]",       Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(" Mute",     Style::default()),
                Span::styled(" :: ",      Style::default()),
                Span::styled("[up/dn]",    Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(" 10s",      Style::default()),
                Span::styled(" :: ",      Style::default()),
                Span::styled("[lt/rt]",    Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(" 1m",       Style::default()),
                Span::styled(" :: ",      Style::default()),
                Span::styled("[=/–]",     Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(" Vol",      Style::default()),
                Span::styled(" :: ",      Style::default()),
                Span::styled("[n/N]",     Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(" Skip",     Style::default()),
                Span::styled(" :: ",      Style::default()),
                Span::styled("[f]",       Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(" Full",     Style::default()),
                Span::styled(" :: ",      Style::default()),
                Span::styled("[R]",       Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(" Relaunch", Style::default()),
                Span::styled(" :: ",      Style::default()),
                Span::styled("[Tab]",     Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(" Close",    Style::default()),
            ])
        } else {
            Line::from(vec![
                Span::styled("[j/k]",    Style::default().fg(C_DIM)),
                Span::styled(" Select",  Style::default().fg(C_DIM)),
                Span::styled(" :: ",     Style::default().fg(C_ORANGE)),
                Span::styled("[Tab]",    Style::default().fg(C_DIM)),
                Span::styled(" Command", Style::default().fg(C_DIM)),
                Span::styled(" :: ",     Style::default().fg(C_ORANGE)),
                Span::styled("[L]",      Style::default().fg(C_DIM)),
                Span::styled(" Log",     Style::default().fg(C_DIM)),
                Span::styled(" :: ",     Style::default().fg(C_ORANGE)),
                Span::styled("[q]",      Style::default().fg(C_DIM)),
                Span::styled(" Quit",    Style::default().fg(C_DIM)),
                Span::styled(" :: ",     Style::default().fg(C_ORANGE)),
                Span::styled("[W]",      Style::default().fg(C_DIM)),
                Span::styled(" Layout",  Style::default().fg(C_DIM)),
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

fn divider(width: usize) -> Paragraph<'static> {
    Paragraph::new(Span::styled(
        "─".repeat(width),
        Style::default().fg(C_DIVIDER),
    ))
}

// ---------------------------------------------------------------------------
// Playlist helpers
// ---------------------------------------------------------------------------

fn load_playlist(pane_name: &str) -> Vec<String> {
    // Read the .m3u from the standard pane location
    // Path mirrors pane_playlist() in lib.rs
    let home    = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let path    = format!("{}/.config/panebot/{}/{}.m3u", home, pane_name.to_lowercase(), pane_name.to_lowercase());
    match std::fs::read_to_string(&path) {
        Ok(content) => content.lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .map(|l| l.trim().to_string())
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn render_playlist(terminal: &mut Term, app: &App) -> io::Result<()> {
    let pane_name = match app.pane_order.get(app.selected) {
        Some(n) => n.clone(),
        None    => return Ok(()),
    };
    let ps = app.panes.get(&pane_name);

    terminal.draw(|f| {
        let size = f.size();

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),  // header
                Constraint::Length(1),  // divider
                Constraint::Min(1),     // playlist items
                Constraint::Length(1),  // divider
                Constraint::Length(1),  // footer
            ])
            .split(size);

        // -- Header --
        let (pb_label, pb_color) = ps.map(|p| p.playback_label()).unwrap_or(("Offline", C_RED));
        let (vol_str, vol_color) = ps.map(|p| p.volume_label()).unwrap_or_else(|| ("Offline".to_string(), C_RED));
        let sep = Span::styled(" :: ", Style::default().fg(C_DIM));
        f.render_widget(Paragraph::new(Line::from(vec![
            Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
            sep.clone(),
            Span::styled(format!("\"{}\"", pane_name.to_uppercase()), Style::default().fg(C_WHITE)),
            sep.clone(),
            Span::styled(
                format!("[{}]", ps.map(|p| p.pane_type.to_uppercase()).unwrap_or_else(|| "?".to_string())),
                Style::default().fg(C_DIM),
            ),
            sep.clone(),
            Span::styled(format!("[{}]", pb_label), Style::default().fg(pb_color)),
            sep.clone(),
            Span::styled(format!("[{}]", vol_str), Style::default().fg(vol_color)),
        ])), chunks[0]);

        // -- Divider --
        f.render_widget(divider(size.width as usize), chunks[1]);

        // -- Playlist items --
        let current_pos = ps.map(|p| p.playlist_pos).unwrap_or(-1);
        let items: Vec<ListItem> = app.playlist_items.iter().enumerate().map(|(i, entry)| {
            let is_current = i as i64 == current_pos;
            let is_sel     = i == app.playlist_sel;

            // Trim to just filename for display
            let display = entry.split('/').last().unwrap_or(entry);

            let cursor = if is_sel {
                Span::styled(">> ", Style::default().fg(C_ORANGE))
            } else {
                Span::raw("   ")
            };

            let idx_color = if is_current { C_ORANGE } else { C_DIM };
            let txt_color = if is_current { C_CYAN   } else { C_HINT };

            let item = ListItem::new(Line::from(vec![
                cursor,
                Span::styled(format!("{:<3}", i), Style::default().fg(idx_color)),
                Span::styled(" :: ", Style::default().fg(C_DIM)),
                Span::styled(display.to_string(), Style::default().fg(txt_color)),
            ]));
            if is_sel { item.style(Style::default().bg(Color::Rgb(20, 40, 40))) } else { item }
        }).collect();

        f.render_widget(List::new(items), chunks[2]);

        // -- Divider --
        f.render_widget(divider(size.width as usize), chunks[3]);

        // -- Footer --
        let footer = if app.command_mode {
            Line::from(vec![
                Span::styled("[CMD]",   Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(" :: ",    Style::default()),
                Span::styled("[Space]", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(" Play",   Style::default()),
                Span::styled(" :: ",    Style::default()),
                Span::styled("[r]",     Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(" Remove", Style::default()),
                Span::styled(" :: ",    Style::default()),
                Span::styled("[j]",     Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(" Jump",   Style::default()),
                Span::styled(" :: ",    Style::default()),
                Span::styled("[n/N]",   Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(" Skip",   Style::default()),
                Span::styled(" :: ",    Style::default()),
                Span::styled("[Tab]",   Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(" Close",  Style::default()),
            ])
        } else if !app.jump_input.is_empty() {
            Line::from(vec![
                Span::styled("Jump to: ", Style::default().fg(C_HINT)),
                Span::styled(app.jump_input.trim_start_matches('_').to_string(), Style::default().fg(C_CYAN).add_modifier(Modifier::BOLD)),
                Span::styled("  [Enter] Go  [Esc] Cancel", Style::default().fg(C_DIM)),
            ])
        } else {
            Line::from(vec![
                Span::styled("[up/dn]", Style::default().fg(C_DIM)),
                Span::styled(" Select", Style::default().fg(C_DIM)),
                Span::styled(" :: ",    Style::default().fg(C_ORANGE)),
                Span::styled("[Tab]",   Style::default().fg(C_DIM)),
                Span::styled(" Command",Style::default().fg(C_DIM)),
                Span::styled(" :: ",    Style::default().fg(C_ORANGE)),
                Span::styled("[Esc]",   Style::default().fg(C_DIM)),
                Span::styled(" Back",   Style::default().fg(C_DIM)),
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
// Startup screen — shown before WS connects
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

// ---------------------------------------------------------------------------
// Spawn daemon sibling binary
// ---------------------------------------------------------------------------

fn spawn_daemon() -> io::Result<()> {
    let daemon_path = std::env::current_exe()?
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "could not determine binary dir"))?
        .join("panebot-daemon");

    std::process::Command::new(&daemon_path)
        .process_group(0)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Main event loop
// ---------------------------------------------------------------------------

async fn run(terminal: &mut Term) -> io::Result<()> {

    // Connect or spawn daemon
    render_startup(terminal, "Connecting to daemon...")?;

    let ws = match connect_async(DAEMON_ADDR).await {
        Ok((ws, _)) => ws,
        Err(_) => {
            render_startup(terminal, "Starting daemon...")?;
            if let Err(e) = spawn_daemon() {
                render_startup(terminal, &format!("Failed to start daemon: {}", e))?;
                tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
                return Ok(());
            }
            loop {
                render_startup(terminal, "Waiting for daemon...")?;
                match connect_async(DAEMON_ADDR).await {
                    Ok((ws, _)) => break ws,
                    Err(_)      => tokio::time::sleep(std::time::Duration::from_millis(500)).await,
                }
            }
        }
    };

    let (mut ws_tx, mut ws_rx) = ws.split();
    let mut app        = App::new();
    let mut key_events = EventStream::new();

    loop {
        tokio::select! {

            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if process_event(&mut app, &text) == Some("node:down") {
                            render(terminal, &app)?;
                            // TODO: reconnect loop for multi-node resilience
                            break;
                        }
                        if app.show_playlist {
                            render_playlist(terminal, &app)?;
                        } else {
                            render(terminal, &app)?;
                        }
                    }
                    Some(Err(_)) | None => {
                        log_node(&mut app, vec![("connection lost".to_string(), C_RED)]);
                        render(terminal, &app)?;
                        // TODO: reconnect loop
                        break;
                    }
                    _ => {}
                }
            }

            key = key_events.next() => {
                match key {
                    Some(Ok(Event::Key(k))) => {

                        // q — quit always
                        if k.code == KeyCode::Char('q') { break; }

                        // Playlist screen
                        if app.show_playlist {
                            match k.code {
                                KeyCode::Esc if app.jump_input.is_empty() => {
                                    app.show_playlist = false;
                                    app.command_mode  = false;
                                    render(terminal, &app)?;
                                }
                                KeyCode::Tab => {
                                    app.command_mode = !app.command_mode;
                                    render_playlist(terminal, &app)?;
                                }
                                KeyCode::Char('j') | KeyCode::Down if !app.command_mode => {
                                    if app.playlist_sel + 1 < app.playlist_items.len() {
                                        app.playlist_sel += 1;
                                    }
                                    render_playlist(terminal, &app)?;
                                }
                                KeyCode::Char('k') | KeyCode::Up if !app.command_mode => {
                                    app.playlist_sel = app.playlist_sel.saturating_sub(1);
                                    render_playlist(terminal, &app)?;
                                }
                                KeyCode::Char(' ') if app.command_mode => {
                                    if let Some(pane) = app.selected_name() {
                                        send_cmd(&mut ws_tx, &pane, "playlist-play-index",
                                            serde_json::json!([app.playlist_sel])).await;
                                    }
                                }
                                KeyCode::Char('r') if app.command_mode => {
                                    if let Some(pane) = app.selected_name() {
                                        send_cmd(&mut ws_tx, &pane, "playlist-remove",
                                            serde_json::json!([app.playlist_sel])).await;
                                        if app.playlist_sel < app.playlist_items.len() {
                                            app.playlist_items.remove(app.playlist_sel);
                                            if app.playlist_sel > 0 && app.playlist_sel >= app.playlist_items.len() {
                                                app.playlist_sel -= 1;
                                            }
                                        }
                                        render_playlist(terminal, &app)?;
                                    }
                                }
                                KeyCode::Char('n') if app.command_mode => {
                                    if let Some(pane) = app.selected_name() {
                                        send_cmd(&mut ws_tx, &pane, "playlist-next",
                                            serde_json::json!([])).await;
                                    }
                                }
                                KeyCode::Char('N') if app.command_mode => {
                                    if let Some(pane) = app.selected_name() {
                                        send_cmd(&mut ws_tx, &pane, "playlist-prev",
                                            serde_json::json!([])).await;
                                    }
                                }
                                // j in command mode starts jump prompt
                                KeyCode::Char('j') if app.command_mode && app.jump_input.is_empty() => {
                                    app.jump_input.push('_'); // sentinel to indicate jump mode
                                    render_playlist(terminal, &app)?;
                                }
                                // collect digits while in jump mode
                                KeyCode::Char(c) if c.is_ascii_digit() && !app.jump_input.is_empty() => {
                                    if app.jump_input == "_" { app.jump_input = String::new(); }
                                    app.jump_input.push(c);
                                    render_playlist(terminal, &app)?;
                                }
                                KeyCode::Backspace if !app.jump_input.is_empty() => {
                                    app.jump_input.pop();
                                    render_playlist(terminal, &app)?;
                                }
                                KeyCode::Enter if !app.jump_input.is_empty() => {
                                    let input = app.jump_input.trim_start_matches('_').to_string();
                                    if let Ok(idx) = input.parse::<usize>() {
                                        let max = app.playlist_items.len().saturating_sub(1);
                                        app.playlist_sel = idx.min(max);
                                    }
                                    app.jump_input.clear();
                                    render_playlist(terminal, &app)?;
                                }
                                KeyCode::Esc if !app.jump_input.is_empty() => {
                                    app.jump_input.clear();
                                    render_playlist(terminal, &app)?;
                                }
                                _ => {}
                            }
                            continue;
                        }

                        // Layout picker — W opens, j/k navigate, Enter applies, Esc closes
                        if app.show_picker {
                            match k.code {
                                KeyCode::Char('j') | KeyCode::Down => {
                                    if app.picker_sel + 1 < app.layouts.len() {
                                        app.picker_sel += 1;
                                    }
                                    render(terminal, &app)?;
                                }
                                KeyCode::Char('k') | KeyCode::Up => {
                                    app.picker_sel = app.picker_sel.saturating_sub(1);
                                    render(terminal, &app)?;
                                }
                                KeyCode::Enter => {
                                    if let Some(layout) = app.layouts.get(app.picker_sel) {
                                        let layout = layout.clone();
                                        send_node_cmd(&mut ws_tx, "panebot:layout",
                                            serde_json::json!({"layout_name": layout})).await;
                                        app.show_picker = false;
                                        render(terminal, &app)?;
                                    }
                                }
                                KeyCode::Esc | KeyCode::Char('W') => {
                                    app.show_picker = false;
                                    render(terminal, &app)?;
                                }
                                _ => {}
                            }
                            continue;
                        }

                        // W opens layout picker
                        if k.code == KeyCode::Char('W') {
                            // Set picker selection to current layout
                            app.picker_sel = app.layouts.iter().position(|l| l == &app.layout).unwrap_or(0);
                            app.show_picker = true;
                            render(terminal, &app)?;
                            continue;
                        }

                        // Enter opens playlist detail
                        if k.code == KeyCode::Enter {
                            if let Some(pane) = app.selected_name() {
                                app.playlist_items = load_playlist(&pane);
                                app.playlist_sel   = app.panes.get(&pane)
                                    .map(|p| p.playlist_pos.max(0) as usize)
                                    .unwrap_or(0);
                                app.show_playlist  = true;
                                app.command_mode   = false;
                                render_playlist(terminal, &app)?;
                            }
                            continue;
                        }

                        // Tab — toggle command mode
                        if k.code == KeyCode::Tab {
                            app.command_mode = !app.command_mode;
                            render(terminal, &app)?;
                            continue;
                        }

                        // Shift-L — toggle log view
                        let shift_l = k.code == KeyCode::Char('L') ||
                            (k.code == KeyCode::Char('l') && k.modifiers.contains(KeyModifiers::SHIFT));
                        if shift_l {
                            app.show_log = !app.show_log;
                            render(terminal, &app)?;
                            continue;
                        }

                        // Navigation — normal mode only
                        if !app.command_mode {
                            match k.code {
                                KeyCode::Char('j') | KeyCode::Down  => { app.select_next(); render(terminal, &app)?; continue; }
                                KeyCode::Char('k') | KeyCode::Up    => { app.select_prev(); render(terminal, &app)?; continue; }
                                _ => {}
                            }
                        }

                        // Player controls — command mode only
                        if app.command_mode {
                            if let Some(pane) = app.selected_name() {
                                match k.code {
                                    KeyCode::Char(' ') => { send_cmd(&mut ws_tx, &pane, "cycle",         serde_json::json!(["pause"])).await; }
                                    KeyCode::Char('m') => { send_cmd(&mut ws_tx, &pane, "cycle",         serde_json::json!(["mute"])).await; }
                                    KeyCode::Char('j') => { send_cmd(&mut ws_tx, &pane, "seek",          serde_json::json!([-10, "relative"])).await; }
                                    KeyCode::Char('k') => { send_cmd(&mut ws_tx, &pane, "seek",          serde_json::json!([10,  "relative"])).await; }
                                    KeyCode::Char('h') => { send_cmd(&mut ws_tx, &pane, "seek",          serde_json::json!([-60, "relative"])).await; }
                                    KeyCode::Char('l') => { send_cmd(&mut ws_tx, &pane, "seek",          serde_json::json!([60,  "relative"])).await; }
                                    KeyCode::Char('=') => { send_cmd(&mut ws_tx, &pane, "add",           serde_json::json!(["volume",  5])).await; }
                                    KeyCode::Char('-') => { send_cmd(&mut ws_tx, &pane, "add",           serde_json::json!(["volume", -5.0])).await; }
                                    KeyCode::Char('n') => { send_cmd(&mut ws_tx, &pane, "playlist-next", serde_json::json!([])).await; }
                                    KeyCode::Char('N') => { send_cmd(&mut ws_tx, &pane, "playlist-prev", serde_json::json!([])).await; }
                                    KeyCode::Char('f') => { send_cmd(&mut ws_tx, &pane, "cycle",         serde_json::json!(["fullscreen"])).await; }
                                    KeyCode::Char('R') => { send_node_cmd(&mut ws_tx, "panebot:restart-pane", serde_json::json!({"pane": pane})).await; }
                                    _ => {}
                                }
                            }
                        }
                    }
                    Some(Err(e)) => return Err(e),
                    None         => break,
                    _            => {}
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

    let result = run(&mut terminal).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    result
}
