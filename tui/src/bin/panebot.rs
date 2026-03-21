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
use std::path::PathBuf;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

const DAEMON_ADDR: &str  = "ws://127.0.0.1:9090";
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
        if !self.online { return ("Offline".to_string(), C_RED); }
        if self.muted   { return ("Vol:Mute".to_string(), C_DIM); }
        (format!("Vol:{:3.0}", self.volume), C_CYAN)
    }
}

// ---------------------------------------------------------------------------
// Details screen mode
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum DetailsMode {
    Normal,
    Command,
    Jump,    // typing a number to jump to
    Add,     // typing a path to add
    Send,    // pane picker overlay for S
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct LogLine {
    spans: Vec<(String, Color)>,
}

struct App {
    pane_order:    Vec<String>,
    panes:         HashMap<String, PaneState>,
    selected:      usize,
    hostname:      String,
    layout:        String,
    log:           VecDeque<LogLine>,
    show_log:      bool,
    command_mode:  bool,         // dashboard CMD mode
    show_picker:   bool,         // layout picker overlay
    picker_sel:    usize,
    layouts:       Vec<String>,

    // Details screen
    show_details:   bool,
    details_mode:   DetailsMode,
    playlist_sel:   usize,
    playlist_items: Vec<String>,  // raw paths from .m3u
    status_msg:     Option<String>, // transient footer message (errors/blocks)

    // Jump mode input
    jump_input:     String,

    // Add mode input + completion cache
    add_input:      String,
    add_completions: Vec<String>,
    add_comp_sel:   usize,

    // Send picker
    send_picker_sel: usize,
}

impl App {
    fn new() -> Self {
        App {
            pane_order:      Vec::new(),
            panes:           HashMap::new(),
            selected:        0,
            hostname:        String::new(),
            layout:          String::new(),
            log:             VecDeque::with_capacity(LOG_CAPACITY),
            show_log:        false,
            command_mode:    false,
            show_picker:     false,
            picker_sel:      0,
            layouts:         Vec::new(),
            show_details:    false,
            details_mode:    DetailsMode::Normal,
            playlist_sel:    0,
            playlist_items:  Vec::new(),
            status_msg:      None,
            jump_input:      String::new(),
            add_input:       String::new(),
            add_completions: Vec::new(),
            add_comp_sel:    0,
            send_picker_sel: 0,
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
        if !self.pane_order.is_empty() && self.selected + 1 < self.pane_order.len() {
            self.selected += 1;
        }
    }

    fn select_prev(&mut self) {
        if !self.pane_order.is_empty() {
            self.selected = self.selected.saturating_sub(1);
        }
    }

    // Current playlist position for the selected pane (-1 if unknown)
    fn current_playlist_pos(&self) -> i64 {
        self.selected_name()
            .and_then(|n| self.panes.get(&n))
            .map(|p| p.playlist_pos)
            .unwrap_or(-1)
    }

    // True if selected playlist item is currently playing
    fn sel_is_playing(&self) -> bool {
        let pos = self.current_playlist_pos();
        pos >= 0 && pos as usize == self.playlist_sel
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
            let pane        = v["pane"].as_str().unwrap_or("");
            let was_offline = app.panes.get(pane).map(|p| !p.online).unwrap_or(false);
            if let Some(ps) = app.panes.get_mut(pane) {
                ps.online = true;
                if let Some(state) = v.get("state") { apply_state(ps, state); }
            }
            if was_offline {
                log_event(app, pane, vec![("Online".to_string(), C_GREEN)]);
            }
        }

        "offline" => {
            let pane       = v["pane"].as_str().unwrap_or("");
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
                    let title = v["value"].as_str().unwrap_or("-").to_string();
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
// Rendering — dashboard
// ---------------------------------------------------------------------------

type Term = Terminal<CrosstermBackend<io::Stdout>>;

fn render(terminal: &mut Term, app: &App) -> io::Result<()> {
    terminal.draw(|f| {
        let size        = f.size();
        let _pane_count = app.pane_order.len().max(1) as u16;

        // Fixed layout: header | div | content | div | footer
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

                let title_str = ps.map(|p| if p.title.is_empty() { "-".to_string() } else { p.title.clone() }).unwrap_or_else(|| "-".to_string());
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
                Span::styled("[up/dn]",   Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(" 10s",      Style::default()),
                Span::styled(" :: ",      Style::default()),
                Span::styled("[lt/rt]",   Style::default().add_modifier(Modifier::BOLD)),
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
                Span::styled("[up/dn]",  Style::default().fg(C_DIM)),
                Span::styled(" Select",  Style::default().fg(C_DIM)),
                Span::styled(" :: ",     Style::default().fg(C_ORANGE)),
                Span::styled("[Tab]",    Style::default().fg(C_DIM)),
                Span::styled(" Command", Style::default().fg(C_DIM)),
                Span::styled(" :: ",     Style::default().fg(C_ORANGE)),
                Span::styled("[L]",      Style::default().fg(C_DIM)),
                Span::styled(" Log",     Style::default().fg(C_DIM)),
                Span::styled(" :: ",     Style::default().fg(C_ORANGE)),
                Span::styled("[W]",      Style::default().fg(C_DIM)),
                Span::styled(" Layout",  Style::default().fg(C_DIM)),
                Span::styled(" :: ",     Style::default().fg(C_ORANGE)),
                Span::styled("[q]",      Style::default().fg(C_DIM)),
                Span::styled(" Quit",    Style::default().fg(C_DIM)),
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
// Rendering — startup
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
                Constraint::Length(1),  // header
                Constraint::Length(1),  // divider
                Constraint::Min(1),     // list
                Constraint::Length(1),  // divider
                Constraint::Length(1),  // footer
            ])
            .split(size);

        // -- Header — same style as dashboard --
        let sep = Span::styled(" :: ", Style::default().fg(C_DIM));
        let (pb_label, pb_color) = ps.map(|p| p.playback_label()).unwrap_or(("Offline", C_RED));
        let (vol_str, vol_color) = ps.map(|p| p.volume_label()).unwrap_or_else(|| ("Offline".to_string(), C_RED));
        f.render_widget(Paragraph::new(Line::from(vec![
            Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
            sep.clone(),
            Span::styled(format!("\"{}\"", pane_name.to_uppercase()), Style::default().fg(C_WHITE).add_modifier(Modifier::BOLD)),
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
            Span::styled(
                format!("{} items", app.playlist_items.len()),
                Style::default().fg(C_DIM),
            ),
        ])), chunks[0]);

        // -- Divider --
        f.render_widget(divider(size.width as usize), chunks[1]);

        // -- Playlist list --
        let list_height = chunks[2].height as usize;
        // Scroll so selected item stays visible
        let scroll_off = if app.playlist_sel >= list_height {
            app.playlist_sel + 1 - list_height
        } else {
            0
        };

        let items: Vec<ListItem> = app.playlist_items.iter().enumerate()
            .skip(scroll_off)
            .take(list_height)
            .map(|(i, entry)| {
                let is_current = i as i64 == current_pos;
                let is_sel     = i == app.playlist_sel;

                let display_str = { let t = entry.trim_end_matches('/'); t.split('/').last().unwrap_or(entry.as_str()).to_string() }; let display = display_str.as_str();

                let cursor = if is_sel {
                    Span::styled(">> ", Style::default().fg(C_ORANGE))
                } else {
                    Span::raw("   ")
                };

                let now_marker = if is_current {
                    Span::styled("* ", Style::default().fg(C_GREEN))
                } else {
                    Span::raw("  ")
                };

                let idx_color = if is_current { C_ORANGE } else { C_DIM };
                let txt_color = if is_current { C_CYAN   } else { C_HINT };

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

        // -- Send pane picker overlay --
        if app.details_mode == DetailsMode::Send {
            let other_panes: Vec<&String> = app.pane_order.iter()
                .filter(|n| Some(*n) != app.selected_name().as_ref())
                .collect();
            let picker_items: Vec<ListItem> = other_panes.iter().enumerate().map(|(i, name)| {
                let is_sel = i == app.send_picker_sel;
                let item = ListItem::new(Line::from(vec![
                    Span::raw(if is_sel { ">> " } else { "   " }),
                    Span::styled(name.to_uppercase(), Style::default()
                        .fg(if is_sel { C_CYAN } else { C_HINT })
                        .add_modifier(if is_sel { Modifier::BOLD } else { Modifier::empty() })
                    ),
                ]));
                if is_sel { item.style(Style::default().bg(Color::Rgb(20, 40, 40))) } else { item }
            }).collect();

            let picker_height = (other_panes.len() as u16 + 2).min(chunks[2].height);
            let picker_area = ratatui::layout::Rect {
                x:      chunks[2].x + 2,
                y:      chunks[2].y,
                width:  24,
                height: picker_height,
            };
            f.render_widget(ratatui::widgets::Clear, picker_area);
            f.render_widget(
                List::new(picker_items)
                    .block(ratatui::widgets::Block::default()
                        .borders(ratatui::widgets::Borders::ALL)
                        .border_style(Style::default().fg(C_ORANGE))
                        .title(" Send to ")),
                picker_area,
            );
        }

        // -- Divider --
        f.render_widget(divider(size.width as usize), chunks[3]);

        // -- Footer --
        let footer = match &app.details_mode {

            DetailsMode::Jump => Line::from(vec![
                Span::styled("Jump to #: ", Style::default().fg(C_HINT)),
                Span::styled(app.jump_input.clone(), Style::default().fg(C_CYAN).add_modifier(Modifier::BOLD)),
                Span::styled("  [Enter] Go  [Esc] Cancel", Style::default().fg(C_DIM)),
            ]),

            DetailsMode::Add => {
                let match_hint = if app.add_completions.is_empty() {
                    String::new()
                } else {
                    format!(" [{} match{}]",
                        app.add_completions.len(),
                        if app.add_completions.len() == 1 { "" } else { "es" })
                };
                Line::from(vec![
                    Span::styled("Add: ", Style::default().fg(C_HINT)),
                    Span::styled(app.add_input.clone(), Style::default().fg(C_WHITE).add_modifier(Modifier::BOLD)),
                    Span::styled(match_hint, Style::default().fg(C_DIM)),
                ])
            },

            DetailsMode::Send => Line::from(vec![
                Span::styled("[up/dn] Select pane", Style::default().fg(C_DIM)),
                Span::styled(" :: ", Style::default().fg(C_ORANGE)),
                Span::styled("[Enter] Send", Style::default().fg(C_DIM)),
                Span::styled(" :: ", Style::default().fg(C_ORANGE)),
                Span::styled("[Esc] Cancel", Style::default().fg(C_DIM)),
            ]),

            DetailsMode::Command => {
                // Status message overrides hints if set
                if let Some(msg) = &app.status_msg {
                    Line::from(vec![
                        Span::styled(msg.clone(), Style::default().fg(C_RED).add_modifier(Modifier::BOLD)),
                    ])
                } else {
                    Line::from(vec![
                        Span::styled("[CMD]",   Style::default().fg(C_ORANGE).add_modifier(Modifier::BOLD)),
                        Span::styled(" :: ",    Style::default().fg(C_DIM)),
                        Span::styled("[Enter]", Style::default().fg(C_DIM)),
                        Span::styled(" Play",   Style::default().fg(C_DIM)),
                        Span::styled(" :: ",    Style::default().fg(C_ORANGE)),
                        Span::styled("[R]",     Style::default().fg(C_DIM)),
                        Span::styled(" Remove", Style::default().fg(C_DIM)),
                        Span::styled(" :: ",    Style::default().fg(C_ORANGE)),
                        Span::styled("[S]",     Style::default().fg(C_DIM)),
                        Span::styled(" Send",   Style::default().fg(C_DIM)),
                        Span::styled(" :: ",    Style::default().fg(C_ORANGE)),
                        Span::styled("[Tab]",   Style::default().fg(C_DIM)),
                        Span::styled(" Normal", Style::default().fg(C_DIM)),
                    ])
                }
            },

            DetailsMode::Normal => {
                if let Some(msg) = &app.status_msg {
                    Line::from(vec![
                        Span::styled(msg.clone(), Style::default().fg(C_RED).add_modifier(Modifier::BOLD)),
                    ])
                } else {
                    Line::from(vec![
                        Span::styled("[up/dn]", Style::default().fg(C_DIM)),
                        Span::styled(" Select", Style::default().fg(C_DIM)),
                        Span::styled(" :: ",    Style::default().fg(C_ORANGE)),
                        Span::styled("[j]",     Style::default().fg(C_DIM)),
                        Span::styled(" Jump",   Style::default().fg(C_DIM)),
                        Span::styled(" :: ",    Style::default().fg(C_ORANGE)),
                        Span::styled("[J]",     Style::default().fg(C_DIM)),
                        Span::styled(" End",    Style::default().fg(C_DIM)),
                        Span::styled(" :: ",    Style::default().fg(C_ORANGE)),
                        Span::styled("[n]",     Style::default().fg(C_DIM)),
                        Span::styled(" Add",    Style::default().fg(C_DIM)),
                        Span::styled(" :: ",    Style::default().fg(C_ORANGE)),
                        Span::styled("[C]",     Style::default().fg(C_DIM)),
                        Span::styled(" Crop",   Style::default().fg(C_DIM)),
                        Span::styled(" :: ",    Style::default().fg(C_ORANGE)),
                        Span::styled("[Tab]",   Style::default().fg(C_DIM)),
                        Span::styled(" Cmd",    Style::default().fg(C_DIM)),
                        Span::styled(" :: ",    Style::default().fg(C_ORANGE)),
                        Span::styled("[Esc]",   Style::default().fg(C_DIM)),
                        Span::styled(" Back",   Style::default().fg(C_DIM)),
                    ])
                }
            },
        };

        let footer_widget = if app.details_mode == DetailsMode::Command {
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
// Playlist helpers — all operate on the .m3u file directly
// ---------------------------------------------------------------------------

// Resolve config dir the same way lib.rs does, without importing it.
// The TUI binary does not link lib.rs directly.
fn config_dir() -> PathBuf {
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

fn pane_playlist_path(pane_name: &str) -> PathBuf {
    config_dir()
        .join(pane_name.to_lowercase())
        .join(format!("{}.m3u", pane_name.to_lowercase()))
}

// Recursively walk a directory, returning all file paths sorted.
fn walk_dir(dir: &str) -> Vec<String> {
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
fn read_m3u(pane_name: &str) -> Vec<String> {
    let path = pane_playlist_path(pane_name);
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
        let _ = write_m3u(pane_name, &expanded);
    }

    expanded
}

// Write items back to .m3u, preserving the #EXTM3U header.
fn write_m3u(pane_name: &str, items: &[String]) -> io::Result<()> {
    let path = pane_playlist_path(pane_name);
    let mut out = String::from("#EXTM3U\n");
    for item in items {
        out.push_str(item);
        out.push('\n');
    }
    std::fs::write(&path, out)
}

// Append one entry, returns the updated list.
fn m3u_append(pane_name: &str, entry: &str) -> io::Result<Vec<String>> {
    let mut items = read_m3u(pane_name);
    items.push(entry.trim().to_string());
    write_m3u(pane_name, &items)?;
    Ok(items)
}

// Remove entry at index. Refuses if it is the currently-playing position.
// Returns Ok(Some(items)) on success, Ok(None) if blocked (playing item).
fn m3u_remove(pane_name: &str, idx: usize, current_pos: i64) -> io::Result<Option<Vec<String>>> {
    if current_pos >= 0 && current_pos as usize == idx {
        return Ok(None); // blocked
    }
    let mut items = read_m3u(pane_name);
    if idx < items.len() {
        items.remove(idx);
        write_m3u(pane_name, &items)?;
    }
    Ok(Some(items))
}

// Crop: keep only the currently-playing item (by current_pos).
// If nothing is playing (current_pos < 0) this is a no-op.
fn m3u_crop(pane_name: &str, current_pos: i64) -> io::Result<Option<Vec<String>>> {
    if current_pos < 0 {
        return Ok(None); // nothing playing, refuse
    }
    let items = read_m3u(pane_name);
    let idx = current_pos as usize;
    if idx >= items.len() {
        return Ok(None);
    }
    let kept = vec![items[idx].clone()];
    write_m3u(pane_name, &kept)?;
    Ok(Some(kept))
}

// ---------------------------------------------------------------------------
// Path completion for add mode
// ---------------------------------------------------------------------------

// Build completions for the current input string.
// Expands ~ and lists directory contents that start with the typed prefix.
// For recursive expansion: if input ends with '/' we list that directory.
fn build_completions(input: &str) -> Vec<String> {
    let expanded = if input.starts_with('~') {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        input.replacen('~', &home, 1)
    } else {
        input.to_string()
    };

    // Split into dir + prefix
    let (dir, prefix) = if expanded.ends_with('/') {
        (expanded.as_str(), "")
    } else {
        match expanded.rfind('/') {
            Some(i) => (&expanded[..=i], &expanded[i+1..]),
            None    => ("./", expanded.as_str()),
        }
    };

    let read_dir = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return Vec::new(),
    };

    let mut results: Vec<String> = read_dir
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.starts_with(prefix) { return None; }
            let mut full = format!("{}{}", dir, name);
            if e.path().is_dir() { full.push('/'); }
            // Re-collapse home dir back to ~
            let home = std::env::var("HOME").unwrap_or_default();
            if !home.is_empty() && full.starts_with(&home) {
                full = full.replacen(&home, "~", 1);
            }
            Some(full)
        })
        .collect();

    results.sort();

    // For media files filter to common extensions + dirs
    results.retain(|r| {
        r.ends_with('/') ||
        r.ends_with(".mp4") || r.ends_with(".mkv") || r.ends_with(".avi") ||
        r.ends_with(".mov") || r.ends_with(".webm") || r.ends_with(".mp3") ||
        r.ends_with(".flac") || r.ends_with(".m4a") || r.ends_with(".ogg") ||
        r.ends_with(".m3u") || r.ends_with(".m3u8") ||
        r.ends_with(".ts")  || r.ends_with(".wmv")
    });

    results
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

// Reload mpv's in-memory playlist from the .m3u file.
async fn reload_playlist_cmd(ws_tx: &mut WsSink, pane: &str) {
    let path = pane_playlist_path(pane);
    send_cmd(ws_tx, pane, "loadlist",
        serde_json::json!([path.to_string_lossy(), "replace"])).await;
}

// ---------------------------------------------------------------------------
// Spawn daemon sibling binary — macOS only.
// On Linux the daemon runs as a systemd service; we never spawn it.
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn spawn_daemon() -> io::Result<()> {
    use std::os::unix::process::CommandExt;
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

    render_startup(terminal, "Connecting to daemon...")?;

    let ws = match connect_async(DAEMON_ADDR).await {
        Ok((ws, _)) => ws,

        #[cfg(target_os = "macos")]
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

        #[cfg(not(target_os = "macos"))]
        Err(_) => {
            render_startup(terminal, "Daemon not running. Start panebot-daemon service.")?;
            tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
            return Ok(());
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
                            if app.show_details {
                                render_details(terminal, &app)?;
                            } else {
                                render(terminal, &app)?;
                            }
                            // TODO: reconnect loop for multi-node resilience
                            break;
                        }
                        if app.show_details {
                            render_details(terminal, &app)?;
                        } else {
                            render(terminal, &app)?;
                        }
                    }
                    Some(Err(_)) | None => {
                        log_node(&mut app, vec![("connection lost".to_string(), C_RED)]);
                        render(&mut *terminal, &app)?;
                        // TODO: reconnect loop
                        break;
                    }
                    _ => {}
                }
            }

            key = key_events.next() => {
                match key {
                    Some(Ok(Event::Key(k))) => {

                        // q — quit always (unless typing in add mode)
                        if k.code == KeyCode::Char('q') && app.details_mode != DetailsMode::Add {
                            break;
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
                                        render_details(terminal, &app)?;
                                    }
                                    KeyCode::Backspace => {
                                        app.jump_input.pop();
                                        render_details(terminal, &app)?;
                                    }
                                    KeyCode::Enter => {
                                        if let Ok(idx) = app.jump_input.parse::<usize>() {
                                            let max = app.playlist_items.len().saturating_sub(1);
                                            app.playlist_sel = idx.min(max);
                                        }
                                        app.jump_input.clear();
                                        app.details_mode = DetailsMode::Normal;
                                        render_details(terminal, &app)?;
                                    }
                                    KeyCode::Esc => {
                                        app.jump_input.clear();
                                        app.details_mode = DetailsMode::Normal;
                                        render_details(terminal, &app)?;
                                    }
                                    _ => {}
                                }
                                continue;
                            }

                            // -- Add mode --
                            if app.details_mode == DetailsMode::Add {
                                match k.code {
                                    KeyCode::Char(c) => {
                                        app.add_input.push(c);
                                        app.add_completions = build_completions(&app.add_input);
                                        app.add_comp_sel    = 0;
                                        render_details(terminal, &app)?;
                                    }
                                    KeyCode::Backspace => {
                                        app.add_input.pop();
                                        app.add_completions = build_completions(&app.add_input);
                                        app.add_comp_sel    = 0;
                                        render_details(terminal, &app)?;
                                    }
                                    KeyCode::Tab => {
                                        // Accept current completion or cycle
                                        if !app.add_completions.is_empty() {
                                            let comp = app.add_completions[app.add_comp_sel].clone();
                                            app.add_input       = comp;
                                            app.add_completions = build_completions(&app.add_input);
                                            app.add_comp_sel    = 0;
                                            render_details(terminal, &app)?;
                                        }
                                    }
                                    KeyCode::Down => {
                                        if app.add_comp_sel + 1 < app.add_completions.len() {
                                            app.add_comp_sel += 1;
                                            render_details(terminal, &app)?;
                                        }
                                    }
                                    KeyCode::Up => {
                                        app.add_comp_sel = app.add_comp_sel.saturating_sub(1);
                                        render_details(terminal, &app)?;
                                    }
                                    KeyCode::Enter => {
                                        let entry = app.add_input.trim().to_string();
                                        if !entry.is_empty() {
                                            if let Some(pane) = app.selected_name() {
                                                // Expand ~ before writing
                                                let expanded = if entry.starts_with('~') {
                                                    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
                                                    entry.replacen('~', &home, 1)
                                                } else {
                                                    entry.clone()
                                                };
                                                match m3u_append(&pane, &expanded) {
                                                    Ok(items) => {
                                                        app.playlist_items = items;
                                                        reload_playlist_cmd(&mut ws_tx, &pane).await;
                                                    }
                                                    Err(e) => {
                                                        app.status_msg = Some(format!("Add failed: {}", e));
                                                    }
                                                }
                                            }
                                        }
                                        app.add_input.clear();
                                        app.add_completions.clear();
                                        app.details_mode = DetailsMode::Normal;
                                        render_details(terminal, &app)?;
                                    }
                                    KeyCode::Esc => {
                                        app.add_input.clear();
                                        app.add_completions.clear();
                                        app.details_mode = DetailsMode::Normal;
                                        render_details(terminal, &app)?;
                                    }
                                    _ => {}
                                }
                                continue;
                            }

                            // -- Send picker mode --
                            if app.details_mode == DetailsMode::Send {
                                let other_panes: Vec<String> = app.pane_order.iter()
                                    .filter(|n| Some(*n) != app.selected_name().as_ref())
                                    .cloned()
                                    .collect();
                                match k.code {
                                    KeyCode::Char('k') | KeyCode::Up => {
                                        app.send_picker_sel = app.send_picker_sel.saturating_sub(1);
                                        render_details(terminal, &app)?;
                                    }
                                    KeyCode::Char('j') | KeyCode::Down => {
                                        if app.send_picker_sel + 1 < other_panes.len() {
                                            app.send_picker_sel += 1;
                                        }
                                        render_details(terminal, &app)?;
                                    }
                                    KeyCode::Enter => {
                                        if let Some(target) = other_panes.get(app.send_picker_sel) {
                                            let target = target.clone();
                                            if let Some(src_pane) = app.selected_name() {
                                                let idx         = app.playlist_sel;
                                                let current_pos = app.current_playlist_pos();
                                                // Block if playing
                                                if current_pos >= 0 && current_pos as usize == idx {
                                                    app.status_msg   = Some("Cannot move playing item".to_string());
                                                    app.details_mode = DetailsMode::Normal;
                                                    render_details(terminal, &app)?;
                                                    continue;
                                                }
                                                if let Some(entry) = app.playlist_items.get(idx).cloned() {
                                                    // Append to target
                                                    let _ = m3u_append(&target, &entry);
                                                    reload_playlist_cmd(&mut ws_tx, &target).await;
                                                    // Remove from source
                                                    match m3u_remove(&src_pane, idx, current_pos) {
                                                        Ok(Some(items)) => {
                                                            app.playlist_items = items;
                                                            if app.playlist_sel > 0 && app.playlist_sel >= app.playlist_items.len() {
                                                                app.playlist_sel -= 1;
                                                            }
                                                            reload_playlist_cmd(&mut ws_tx, &src_pane).await;
                                                        }
                                                        _ => {}
                                                    }
                                                }
                                            }
                                        }
                                        app.details_mode    = DetailsMode::Normal;
                                        app.send_picker_sel = 0;
                                        render_details(terminal, &app)?;
                                    }
                                    KeyCode::Esc => {
                                        app.details_mode    = DetailsMode::Normal;
                                        app.send_picker_sel = 0;
                                        render_details(terminal, &app)?;
                                    }
                                    _ => {}
                                }
                                continue;
                            }

                            // -- Command mode --
                            if app.details_mode == DetailsMode::Command {
                                match k.code {
                                    KeyCode::Tab => {
                                        app.details_mode = DetailsMode::Normal;
                                        app.status_msg   = None;
                                        render_details(terminal, &app)?;
                                    }
                                    KeyCode::Esc => {
                                        app.details_mode = DetailsMode::Normal;
                                        app.status_msg   = None;
                                        render_details(terminal, &app)?;
                                    }
                                    KeyCode::Enter => {
                                        // Play selected item (start from idle if needed)
                                        if let Some(pane) = app.selected_name() {
                                            let ps = app.panes.get(&pane);
                                            if ps.map(|p| p.idle_active).unwrap_or(true) {
                                                // Idle — reload list then play index
                                                reload_playlist_cmd(&mut ws_tx, &pane).await;
                                                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                                            }
                                            send_cmd(&mut ws_tx, &pane, "playlist-play-index",
                                                serde_json::json!([app.playlist_sel])).await;
                                            send_cmd(&mut ws_tx, &pane, "set_property",
                                                serde_json::json!(["pause", false])).await;
                                        }
                                        app.status_msg = None;
                                        render_details(terminal, &app)?;
                                    }
                                    KeyCode::Char('R') => {
                                        if app.sel_is_playing() {
                                            app.status_msg = Some("Cannot remove playing item".to_string());
                                            render_details(terminal, &app)?;
                                        } else if let Some(pane) = app.selected_name() {
                                            let current_pos = app.current_playlist_pos();
                                            match m3u_remove(&pane, app.playlist_sel, current_pos) {
                                                Ok(Some(items)) => {
                                                    app.playlist_items = items;
                                                    if app.playlist_sel > 0 && app.playlist_sel >= app.playlist_items.len() {
                                                        app.playlist_sel -= 1;
                                                    }
                                                    reload_playlist_cmd(&mut ws_tx, &pane).await;
                                                    app.status_msg = None;
                                                }
                                                Ok(None) => {
                                                    app.status_msg = Some("Cannot remove playing item".to_string());
                                                }
                                                Err(e) => {
                                                    app.status_msg = Some(format!("Remove failed: {}", e));
                                                }
                                            }
                                            render_details(terminal, &app)?;
                                        }
                                    }
                                    KeyCode::Char('S') => {
                                        if app.sel_is_playing() {
                                            app.status_msg = Some("Cannot move playing item".to_string());
                                            render_details(terminal, &app)?;
                                        } else if app.pane_order.len() > 1 {
                                            app.details_mode    = DetailsMode::Send;
                                            app.send_picker_sel = 0;
                                            app.status_msg      = None;
                                            render_details(terminal, &app)?;
                                        }
                                    }
                                    _ => {}
                                }
                                continue;
                            }

                            // -- Normal mode --
                            match k.code {
                                KeyCode::Esc => {
                                    app.show_details  = false;
                                    app.details_mode  = DetailsMode::Normal;
                                    app.status_msg    = None;
                                    app.jump_input.clear();
                                    render(terminal, &app)?;
                                }
                                KeyCode::Tab => {
                                    app.details_mode = DetailsMode::Command;
                                    app.status_msg   = None;
                                    render_details(terminal, &app)?;
                                }
                                KeyCode::Up | KeyCode::Char('k') => {
                                    app.playlist_sel = app.playlist_sel.saturating_sub(1);
                                    app.status_msg   = None;
                                    render_details(terminal, &app)?;
                                }
                                KeyCode::Down | KeyCode::Char('j') => {
                                    if app.playlist_sel + 1 < app.playlist_items.len() {
                                        app.playlist_sel += 1;
                                    }
                                    app.status_msg = None;
                                    render_details(terminal, &app)?;
                                }
                                KeyCode::Char('J') => {
                                    // Jump to end
                                    app.playlist_sel = app.playlist_items.len().saturating_sub(1);
                                    app.status_msg   = None;
                                    render_details(terminal, &app)?;
                                }
                                KeyCode::Char('j') => {
                                    // This arm is unreachable because Down/'j' above catches it —
                                    // 'j' for jump is intentionally on lowercase, handled above.
                                    // Jump prompt is on 'g' below instead to avoid conflict.
                                }
                                KeyCode::Char('g') => {
                                    // Jump prompt (g to avoid clash with j=down)
                                    app.details_mode = DetailsMode::Jump;
                                    app.jump_input.clear();
                                    render_details(terminal, &app)?;
                                }
                                KeyCode::Char('n') => {
                                    // Add new item
                                    app.details_mode    = DetailsMode::Add;
                                    app.add_input.clear();
                                    app.add_completions = Vec::new();
                                    app.add_comp_sel    = 0;
                                    app.status_msg      = None;
                                    render_details(terminal, &app)?;
                                }
                                KeyCode::Char('C') => {
                                    // Crop — keep only playing item
                                    if let Some(pane) = app.selected_name() {
                                        let current_pos = app.current_playlist_pos();
                                        match m3u_crop(&pane, current_pos) {
                                            Ok(Some(items)) => {
                                                app.playlist_items = items;
                                                app.playlist_sel   = 0;
                                                reload_playlist_cmd(&mut ws_tx, &pane).await;
                                                app.status_msg = None;
                                            }
                                            Ok(None) => {
                                                app.status_msg = Some("Nothing playing to crop around".to_string());
                                            }
                                            Err(e) => {
                                                app.status_msg = Some(format!("Crop failed: {}", e));
                                            }
                                        }
                                        render_details(terminal, &app)?;
                                    }
                                }
                                _ => {}
                            }
                            continue;
                        }

                        // ================================================================
                        // DASHBOARD
                        // ================================================================

                        // Layout picker
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

                        // W — open layout picker
                        if k.code == KeyCode::Char('W') {
                            app.picker_sel = app.layouts.iter().position(|l| l == &app.layout).unwrap_or(0);
                            app.show_picker = true;
                            render(terminal, &app)?;
                            continue;
                        }

                        // Enter — open details screen for selected pane
                        if k.code == KeyCode::Enter {
                            if let Some(pane) = app.selected_name() {
                                app.playlist_items = read_m3u(&pane);
                                app.playlist_sel   = app.panes.get(&pane)
                                    .map(|p| p.playlist_pos.max(0) as usize)
                                    .unwrap_or(0);
                                app.show_details   = true;
                                app.details_mode   = DetailsMode::Normal;
                                app.status_msg     = None;
                                render_details(terminal, &app)?;
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
                                KeyCode::Char('j') | KeyCode::Down => { app.select_next(); render(terminal, &app)?; continue; }
                                KeyCode::Char('k') | KeyCode::Up   => { app.select_prev(); render(terminal, &app)?; continue; }
                                _ => {}
                            }
                        }

                        // Player controls — command mode only
                        if app.command_mode {
                            if let Some(pane) = app.selected_name() {
                                match k.code {
                                    KeyCode::Char(' ') => { send_cmd(&mut ws_tx, &pane, "cycle",         serde_json::json!(["pause"])).await; }
                                    KeyCode::Char('m') => { send_cmd(&mut ws_tx, &pane, "cycle",         serde_json::json!(["mute"])).await; }
                                    KeyCode::Char('j') | KeyCode::Down  => { send_cmd(&mut ws_tx, &pane, "seek", serde_json::json!([-10, "relative"])).await; }
                                    KeyCode::Char('k') | KeyCode::Up    => { send_cmd(&mut ws_tx, &pane, "seek", serde_json::json!([10,  "relative"])).await; }
                                    KeyCode::Char('h') | KeyCode::Left  => { send_cmd(&mut ws_tx, &pane, "seek", serde_json::json!([-60, "relative"])).await; }
                                    KeyCode::Char('l') | KeyCode::Right => { send_cmd(&mut ws_tx, &pane, "seek", serde_json::json!([60,  "relative"])).await; }
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
