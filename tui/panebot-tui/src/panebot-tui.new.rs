use crossterm::{
    event::{Event, KeyCode, EventStream},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures_util::{SinkExt, StreamExt};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
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
    layouts_dir, home_dir, config_dir, load_hosts, Host,
};

const LOCAL_ADDR:        &str = "ws://127.0.0.1:9090";
const CONNECT_RETRY_MS:  u64  = 500;
const CONNECT_TIMEOUT_S: u64  = 30;

const C_ORANGE:  Color = Color::Rgb(224, 128, 48);
const C_CYAN:    Color = Color::Rgb(60, 160, 160);
const C_DIM:     Color = Color::Rgb(100, 120, 120);
const C_HINT:    Color = Color::Rgb(140, 160, 160);
const C_DIVIDER: Color = Color::Rgb(40, 58, 58);
const C_RED:     Color = Color::Rgb(200, 60, 60);
const C_GREEN:   Color = Color::Rgb(60, 180, 100);
const C_WHITE:   Color = Color::Rgb(220, 220, 220);

type WsSink = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>
    >,
    Message,
>;

// ---------------------------------------------------------------------------
// Pane state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct PaneState {
    mpv_name:     String,
    pane_name:    String,
    online:       bool,
    idle_active:  Option<bool>,
    paused:       Option<bool>,
    muted:        Option<bool>,
    volume:       Option<f64>,
    title:        Option<String>,
    playlist_pos: Option<i64>,
}

impl PaneState {
    fn new(mpv_name: &str, pane_name: &str) -> Self {
        PaneState {
            mpv_name:     mpv_name.to_string(),
            pane_name:    pane_name.to_string(),
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
        if !self.online                     { return ("Offline", C_RED);  }
        if self.idle_active.unwrap_or(true) { return ("Stopped", C_DIM);  }
        if self.paused.unwrap_or(true)      { return ("Paused",  C_HINT); }
        ("Playing", C_GREEN)
    }

    fn volume_label(&self) -> (String, Color) {
        if !self.online                { return ("Offline".to_string(),  C_RED); }
        if self.muted.unwrap_or(false) { return ("Vol:Mute".to_string(), C_DIM); }
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
enum DetailsMode { Normal, Jump, Add, Save, Confirm }

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

struct App {
    pane_order:       Vec<String>,
    panes:            HashMap<String, PaneState>,
    selected:         usize,
    hostname:         String,
    platform:         String,
    layout:           String,
    home:             String,
    owns_daemon:      bool,
    is_remote:        bool,
    show_log:         bool,
    command_mode:     bool,
    show_picker:      bool,
    picker_sel:       usize,
    layouts:          Vec<String>,
    show_details:     bool,
    details_mode:     DetailsMode,
    playlist_sel:     usize,
    playlist_items:   Vec<String>,
    selected_items:   HashSet<usize>,
    status_msg:       Option<String>,
    jump_input:       String,
    add_input:        String,
    passthrough_mode: bool,
}

impl App {
    fn new() -> Self {
        App {
            pane_order:       Vec::new(),
            panes:            HashMap::new(),
            selected:         0,
            hostname:         String::new(),
            platform:         String::new(),
            layout:           String::new(),
            home:             home_dir(),
            owns_daemon:      false,
            is_remote:        false,
            show_log:         false,
            command_mode:     false,
            show_picker:      false,
            picker_sel:       0,
            layouts:          Vec::new(),
            show_details:     false,
            details_mode:     DetailsMode::Normal,
            playlist_sel:     0,
            playlist_items:   Vec::new(),
            selected_items:   HashSet::new(),
            status_msg:       None,
            jump_input:       String::new(),
            add_input:        String::new(),
            passthrough_mode: false,
        }
    }

    fn active_count(&self) -> usize { self.panes.values().filter(|p| p.online).count() }

    fn selected_name(&self) -> Option<String> { self.pane_order.get(self.selected).cloned() }

    fn select_next(&mut self) {
        if !self.pane_order.is_empty() && self.selected + 1 < self.pane_order.len() { self.selected += 1; }
    }
    fn select_prev(&mut self) {
        if !self.pane_order.is_empty() { self.selected = self.selected.saturating_sub(1); }
    }

    fn current_playlist_pos(&self) -> i64 {
        self.selected_name().and_then(|n| self.panes.get(&n)).and_then(|p| p.playlist_pos).unwrap_or(-1)
    }

    fn open_details(&mut self) {
        self.playlist_items.clear();
        self.playlist_sel   = 0;
        self.selected_items.clear();
        self.show_details   = true;
        self.details_mode   = DetailsMode::Normal;
        self.status_msg     = Some("Loading playlist...".to_string());
    }

    fn close_details(&mut self) {
        self.show_details = false;
        self.details_mode = DetailsMode::Normal;
        self.status_msg   = None;
        self.jump_input.clear();
        self.selected_items.clear();
    }

    fn go_log(&mut self)    { self.show_log = true;  }
    fn leave_log(&mut self) { self.show_log = false; }

    fn load_layouts(&mut self) {
        let mut layouts = Vec::new();
        if let Ok(entries) = std::fs::read_dir(layouts_dir()) {
            let mut names: Vec<_> = entries.filter_map(|e| e.ok()).filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                if name.ends_with(".layout") { Some(name.trim_end_matches(".layout").to_string()) } else { None }
            }).collect();
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
                    if !mpv_name.is_empty() {
                        app.pane_order.push(mpv_name.clone());
                        app.panes.insert(mpv_name.clone(), PaneState::new(&mpv_name, &pane_name));
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
        "node:down" => { for ps in app.panes.values_mut() { ps.online = false; } return Some("node:down"); }
        "node:layout" => { if let Some(l) = v["layout"].as_str() { app.layout = l.to_string(); } }
        "node:playlist" => {
            let pane = v["pane"].as_str().unwrap_or("");
            if let Some(sel) = app.pane_order.iter().position(|n| n == pane) {
                if sel == app.selected && app.show_details {
                    if let Some(items) = v["items"].as_array() {
                        app.playlist_items = items.iter().filter_map(|i| {
                            let filename = i["filename"].as_str()?;
                            Some(i["title"].as_str().filter(|t| !t.is_empty()).unwrap_or(filename).to_string())
                        }).collect();
                        app.playlist_sel = app.panes.get(pane).and_then(|p| p.playlist_pos)
                            .filter(|&pos| pos >= 0).map(|pos| pos as usize).unwrap_or(0);
                        app.status_msg = None;
                    }
                }
            }
        }
        "node:playlist-saved" => {
            let path = v["path"].as_str().unwrap_or("unknown");
            app.status_msg = Some(format!("Saved to {}", path));
        }
        _ => {}
    }
    None
}

fn apply_state(ps: &mut PaneState, state: &serde_json::Value) {
    if let Some(v) = state["paused"].as_bool()     { ps.paused       = Some(v); }
    if let Some(v) = state["muted"].as_bool()       { ps.muted        = Some(v); }
    if let Some(v) = state["idle_active"].as_bool() { ps.idle_active  = Some(v); }
    if let Some(v) = state["volume"].as_f64()       { ps.volume       = Some(v); }
    if let Some(v) = state["playlist_pos"].as_i64() { ps.playlist_pos = Some(v); }
    if let Some(v) = state["title"].as_str()        { ps.title        = Some(v.to_string()); }
}

// ---------------------------------------------------------------------------
// Layout helper
// ---------------------------------------------------------------------------

type Term = Terminal<CrosstermBackend<io::Stdout>>;

fn make_chunks(size: Rect) -> std::rc::Rc<[Rect]> {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1), Constraint::Length(1),
        ])
        .split(size)
}

// ---------------------------------------------------------------------------
// Footer helpers
// ---------------------------------------------------------------------------

fn sep_orange() -> Span<'static> { Span::styled(" :: ", Style::default().fg(C_ORANGE)) }
fn sep_dim()    -> Span<'static> { Span::styled(" :: ", Style::default().fg(C_DIM))    }
fn fdim(s: &str) -> Span<'static> { Span::styled(s.to_string(), Style::default().fg(C_DIM)) }

fn hint(key: &str, label: &str) -> Vec<Span<'static>> {
    vec![sep_orange(), fdim(key), fdim(&format!(" {}", label))]
}

fn footer_dashboard_normal() -> Line<'static> {
    let mut s = vec![fdim("[j/k]"), fdim(" Nav")];
    for (k, l) in &[("[l/Rt]","Detail"),("[r/R]","Restart"),("[S]","Solo"),("[M]","Mute\u{2205}"),("[P]","Pause\u{2205}"),("[W]","Layout"),("[C]","Connect"),("[h/Lt]","Log"),("[q]","Quit")] {
        s.extend(hint(k, l));
    }
    Line::from(s)
}

fn footer_dashboard_cmd() -> Line<'static> {
    let mut s = vec![Span::styled("[CMD]", Style::default().add_modifier(Modifier::BOLD)), sep_dim()];
    for (k, l) in &[("[Space]","Pause"),("[m]","Mute"),("[Enter]","Next"),("[h/l]","\u{b1}5s"),("[j/k]","\u{b1}60s"),("[9/0]","Vol"),("[f]","Full"),("[v]","Adv"),("[Tab]","Close")] {
        s.push(Span::styled(k.to_string(), Style::default().add_modifier(Modifier::BOLD)));
        s.push(Span::styled(format!(" {} ", l), Style::default()));
    }
    Line::from(s)
}

fn footer_details_normal() -> Line<'static> {
    let mut s = vec![fdim("[j/k]"), fdim(" Nav")];
    for (k, l) in &[("[Spc]","Mark"),("[Enter]","Play"),("[n]","Queue"),("[D]","Del"),("[M]","Move"),("[C]","Crop"),("[A]","Add"),("[S]","Save"),("[G]","Goto"),("[h/Lt]","Back")] {
        s.extend(hint(k, l));
    }
    Line::from(s)
}

fn footer_details_cmd() -> Line<'static> {
    let mut s = vec![Span::styled("[CMD]", Style::default().fg(C_ORANGE).add_modifier(Modifier::BOLD)), sep_orange()];
    for (k, l) in &[("[Space]","Pause"),("[m]","Mute"),("[Enter]","Next"),("[h/l]","\u{b1}5s"),("[j/k]","\u{b1}60s"),("[9/0]","Vol"),("[Tab]","Close")] {
        s.push(fdim(k)); s.push(fdim(&format!(" {}", l))); s.push(sep_orange());
    }
    s.pop();
    Line::from(s)
}

// ---------------------------------------------------------------------------
// Rendering — host picker
// ---------------------------------------------------------------------------

fn render_host_picker(terminal: &mut Term, hosts: &[Host], sel: usize) -> io::Result<()> {
    terminal.draw(|f| {
        let size   = f.size();
        let chunks = make_chunks(size);

        f.render_widget(Paragraph::new(Line::from(vec![
            Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)), sep_dim(),
            Span::styled("Select node to connect", Style::default().fg(C_HINT)),
        ])), chunks[0]);
        f.render_widget(divider(size.width as usize), chunks[1]);

        let items: Vec<ListItem> = hosts.iter().enumerate().map(|(i, host)| {
            let is_sel = i == sel;
            let cursor = if is_sel { Span::styled(">> ", Style::default().fg(C_ORANGE)) } else { Span::raw("   ") };
            let item = ListItem::new(Line::from(vec![
                cursor,
                Span::styled(host.label.clone(), Style::default().fg(if is_sel { C_WHITE } else { C_HINT }).add_modifier(if is_sel { Modifier::BOLD } else { Modifier::empty() })),
                sep_dim(),
                Span::styled(host.address.clone(), Style::default().fg(C_DIM)),
            ]));
            if is_sel { item.style(Style::default().bg(Color::Rgb(20, 40, 40))) } else { item }
        }).collect();

        f.render_widget(List::new(items), chunks[2]);
        f.render_widget(divider(size.width as usize), chunks[3]);
        f.render_widget(Paragraph::new(Line::from(vec![fdim("[j/k] Select"), sep_orange(), fdim("[Enter] Connect"), sep_orange(), fdim("[q] Quit")])), chunks[4]);
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Rendering — dashboard (+ log)
// ---------------------------------------------------------------------------

fn render(terminal: &mut Term, app: &App) -> io::Result<()> {
    terminal.draw(|f| {
        let size   = f.size();
        let chunks = make_chunks(size);

        f.render_widget(Paragraph::new(Line::from(vec![
            Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)), sep_dim(),
            Span::styled(format!("Active Panes: {}/{}", app.active_count(), app.pane_order.len()), Style::default().fg(C_HINT)),
            sep_dim(),
            Span::styled(format!("Layout: {}", app.layout), Style::default().fg(C_CYAN)),
            sep_dim(),
            Span::styled(format!("{} [{}]", app.hostname, app.platform), Style::default().fg(C_DIM)),
        ])), chunks[0]);
        f.render_widget(divider(size.width as usize), chunks[1]);

        if app.show_log {
            render_log_body(f, chunks[2], app);
        } else {
            render_pane_list(f, chunks[2], app);
            if app.show_picker { render_layout_picker(f, chunks[2], app); }
        }

        f.render_widget(divider(size.width as usize), chunks[3]);

        let footer = if app.command_mode { footer_dashboard_cmd() } else { footer_dashboard_normal() };
        let fw = if app.command_mode {
            Paragraph::new(footer).style(Style::default().bg(Color::Rgb(80, 40, 10)).fg(Color::Rgb(160, 100, 40)))
        } else { Paragraph::new(footer) };
        f.render_widget(fw, chunks[4]);
    })?;
    Ok(())
}

fn render_log_body(f: &mut ratatui::Frame, area: Rect, _app: &App) {
    let log_path  = config_dir().join("panebot-daemon.log");
    let log_lines: Vec<ListItem> = std::fs::read_to_string(&log_path)
        .unwrap_or_default().lines().map(|l| l.to_string()).collect::<Vec<_>>()
        .into_iter().rev().take(area.height as usize).collect::<Vec<_>>()
        .into_iter().rev()
        .map(|line| {
            if let Some(rest) = line.strip_prefix('[') {
                if let Some(mid) = rest.find("] [") {
                    let ts   = &rest[..mid];
                    let tail = &rest[mid + 3..];
                    if let Some(end) = tail.find(']') {
                        return ListItem::new(Line::from(vec![
                            Span::styled(format!("[{}] ", ts),        Style::default().fg(C_DIM)),
                            Span::styled(format!("[{}]", &tail[..end]), Style::default().fg(C_HINT)),
                            Span::styled(tail[end + 1..].to_string(), Style::default().fg(C_DIM)),
                        ]));
                    }
                }
            }
            ListItem::new(Span::styled(line, Style::default().fg(C_DIM)))
        }).collect();
    f.render_widget(List::new(log_lines), area);
}

fn render_pane_list(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let max_name = app.panes.values().map(|p| p.pane_name.len() + 2).max().unwrap_or(8);

    let items: Vec<ListItem> = app.pane_order.iter().enumerate().map(|(i, name)| {
        let ps     = app.panes.get(name);
        let is_sel = i == app.selected;

        let cursor    = if is_sel { Span::styled(":: ", Style::default().fg(C_ORANGE)) } else { Span::raw("   ") };
        let name_span = Span::styled(
            format!("{:<width$}", format!("\"{}\"", ps.map(|p| p.pane_name.to_uppercase()).unwrap_or_else(|| name.to_uppercase())), width = max_name),
            Style::default().fg(C_WHITE).add_modifier(if is_sel { Modifier::BOLD } else { Modifier::empty() }),
        );
        let (pb_label, pb_color) = ps.map(|p| p.playback_label()).unwrap_or(("Offline", C_RED));
        let (vol_str, vol_color) = ps.map(|p| p.volume_label()).unwrap_or_else(|| ("Offline".to_string(), C_RED));
        let title_str = ps.map(|p| p.title.as_deref().filter(|s| !s.is_empty()).unwrap_or("-").to_string()).unwrap_or_else(|| "-".to_string());
        let cmd_badge = if is_sel && app.command_mode { Span::styled(" [CMD]", Style::default().fg(C_ORANGE)) } else { Span::raw("") };

        let item = ListItem::new(Line::from(vec![
            cursor, name_span, sep_dim(),
            Span::styled(format!("[{:7}]", pb_label), Style::default().fg(pb_color)), sep_dim(),
            Span::styled(format!("[{:8}]", vol_str),  Style::default().fg(vol_color)), sep_dim(),
            Span::styled(title_str, Style::default().fg(C_CYAN)), cmd_badge,
        ]));
        if is_sel { item.style(Style::default().bg(Color::Rgb(20, 40, 40))) } else { item }
    }).collect();

    f.render_widget(List::new(items), area);
}

fn render_layout_picker(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let picker_items: Vec<ListItem> = app.layouts.iter().enumerate().map(|(i, name)| {
        let is_sel = i == app.picker_sel;
        let item = ListItem::new(Line::from(vec![
            Span::raw(if is_sel { ">> " } else { "   " }),
            Span::styled(name.clone(), Style::default().fg(if is_sel { C_CYAN } else { C_HINT }).add_modifier(if is_sel { Modifier::BOLD } else { Modifier::empty() })),
            if name == &app.layout { Span::styled(" *", Style::default().fg(C_ORANGE)) } else { Span::raw("") },
        ]));
        if is_sel { item.style(Style::default().bg(Color::Rgb(20, 40, 40))) } else { item }
    }).collect();

    let picker_area = Rect { x: area.x + 2, y: area.y, width: 30, height: (app.layouts.len() as u16 + 2).min(area.height) };
    f.render_widget(ratatui::widgets::Clear, picker_area);
    f.render_widget(
        List::new(picker_items).block(ratatui::widgets::Block::default()
            .borders(ratatui::widgets::Borders::ALL)
            .border_style(Style::default().fg(C_ORANGE))
            .title(" Layout ")),
        picker_area,
    );
}

// ---------------------------------------------------------------------------
// Rendering — startup status
// ---------------------------------------------------------------------------

fn render_startup(terminal: &mut Term, status: &str) -> io::Result<()> {
    terminal.draw(|f| {
        f.render_widget(Paragraph::new(Line::from(vec![
            Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)), sep_dim(),
            Span::styled(status, Style::default().fg(C_HINT)),
        ])), f.size());
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Rendering — details screen
// ---------------------------------------------------------------------------

fn render_details(terminal: &mut Term, app: &App) -> io::Result<()> {
    let pane_name = match app.pane_order.get(app.selected) { Some(n) => n.clone(), None => return Ok(()) };
    let ps          = app.panes.get(&pane_name);
    let current_pos = app.current_playlist_pos();

    terminal.draw(|f| {
        let size   = f.size();
        let chunks = make_chunks(size);

        let (pb_label, pb_color) = ps.map(|p| p.playback_label()).unwrap_or(("Offline", C_RED));
        let (vol_str, vol_color) = ps.map(|p| p.volume_label()).unwrap_or_else(|| ("Offline".to_string(), C_RED));
        f.render_widget(Paragraph::new(Line::from(vec![
            Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)), sep_dim(),
            Span::styled(format!("\"{}\"", ps.map(|p| p.pane_name.to_uppercase()).unwrap_or_else(|| pane_name.to_uppercase())), Style::default().fg(C_WHITE).add_modifier(Modifier::BOLD)),
            sep_dim(),
            Span::styled(format!("[{:7}]", pb_label), Style::default().fg(pb_color)), sep_dim(),
            Span::styled(format!("[{:8}]", vol_str),  Style::default().fg(vol_color)), sep_dim(),
            Span::styled(format!("{} items", app.playlist_items.len()), Style::default().fg(C_DIM)),
        ])), chunks[0]);
        f.render_widget(divider(size.width as usize), chunks[1]);

        let list_height = chunks[2].height as usize;
        let scroll_off  = if app.playlist_sel >= list_height { app.playlist_sel + 1 - list_height } else { 0 };

        let items: Vec<ListItem> = app.playlist_items.iter().enumerate()
            .skip(scroll_off).take(list_height)
            .map(|(i, entry)| {
                let is_current = i as i64 == current_pos;
                let is_sel     = i == app.playlist_sel;
                let is_marked  = app.selected_items.contains(&i);
                let cursor     = if is_sel { Span::styled(">> ", Style::default().fg(C_ORANGE)) } else { Span::raw("   ") };
                let marker     = if is_current { Span::styled("* ",  Style::default().fg(C_GREEN))  }
                                 else if is_marked  { Span::styled("• ", Style::default().fg(C_ORANGE)) }
                                 else { Span::raw("  ") };
                let idx_color  = if is_current || is_marked { C_ORANGE } else { C_DIM };
                let txt_color  = if is_current { C_CYAN } else if is_marked { C_WHITE } else { C_HINT };
                let item = ListItem::new(Line::from(vec![
                    cursor, marker,
                    Span::styled(format!("{:<4}", i), Style::default().fg(idx_color)),
                    Span::styled(" :: ", Style::default().fg(C_DIM)),
                    Span::styled(entry.clone(), Style::default().fg(txt_color)),
                ]));
                if is_sel { item.style(Style::default().bg(Color::Rgb(20, 40, 40))) } else { item }
            }).collect();

        f.render_widget(List::new(items), chunks[2]);
        f.render_widget(divider(size.width as usize), chunks[3]);

        let footer = match &app.details_mode {
            DetailsMode::Jump    => Line::from(vec![
                Span::styled("Jump to #: ", Style::default().fg(C_HINT)),
                Span::styled(app.jump_input.clone(), Style::default().fg(C_CYAN).add_modifier(Modifier::BOLD)),
                fdim("  [Enter] Go  [Esc] Cancel"),
            ]),
            DetailsMode::Confirm => Line::from(vec![
                Span::styled("Play Now [Enter]", Style::default().fg(C_CYAN).add_modifier(Modifier::BOLD)),
                Span::styled("  ::  ", Style::default().fg(C_ORANGE)),
                Span::styled("Queue Next [n]", Style::default().fg(C_CYAN).add_modifier(Modifier::BOLD)),
                Span::styled("  ::  ", Style::default().fg(C_ORANGE)),
                fdim("Cancel [Esc]"),
            ]),
            DetailsMode::Add     => Line::from(vec![
                Span::styled("Add: ", Style::default().fg(C_HINT)),
                Span::styled(app.add_input.clone(), Style::default().fg(C_WHITE).add_modifier(Modifier::BOLD)),
                fdim("  [Enter] Add  [Esc] Cancel"),
            ]),
            DetailsMode::Save    => Line::from(vec![
                Span::styled("Save as: ", Style::default().fg(C_HINT)),
                Span::styled(app.add_input.clone(), Style::default().fg(C_WHITE).add_modifier(Modifier::BOLD)),
                fdim(".m3u  [Enter] Save  [Esc] Cancel"),
            ]),
            DetailsMode::Normal  => {
                if let Some(msg) = &app.status_msg {
                    Line::from(Span::styled(msg.clone(), Style::default().fg(C_RED).add_modifier(Modifier::BOLD)))
                } else if app.command_mode { footer_details_cmd() } else { footer_details_normal() }
            }
        };

        let fw = if app.command_mode {
            Paragraph::new(footer).style(Style::default().bg(Color::Rgb(80, 40, 10)).fg(Color::Rgb(160, 100, 40)))
        } else { Paragraph::new(footer) };
        f.render_widget(fw, chunks[4]);
    })?;
    Ok(())
}

fn divider(width: usize) -> Paragraph<'static> {
    Paragraph::new(Span::styled("-".repeat(width), Style::default().fg(C_DIVIDER)))
}

// ---------------------------------------------------------------------------
// mpv key name mapper
// ---------------------------------------------------------------------------

fn mpv_key_name(code: KeyCode) -> &'static str {
    match code {
        KeyCode::Char(' ') => "SPACE",  KeyCode::Enter     => "ENTER",  KeyCode::Esc      => "ESC",
        KeyCode::Backspace => "BS",     KeyCode::Delete    => "DEL",    KeyCode::Tab      => "TAB",
        KeyCode::Up        => "UP",     KeyCode::Down      => "DOWN",   KeyCode::Left     => "LEFT",
        KeyCode::Right     => "RIGHT",  KeyCode::PageUp    => "PGUP",   KeyCode::PageDown => "PGDWN",
        KeyCode::Home      => "HOME",   KeyCode::End       => "END",
        KeyCode::F(1)=>"F1", KeyCode::F(2)=>"F2", KeyCode::F(3)=>"F3",  KeyCode::F(4)=>"F4",
        KeyCode::F(5)=>"F5", KeyCode::F(6)=>"F6", KeyCode::F(7)=>"F7",  KeyCode::F(8)=>"F8",
        KeyCode::F(9)=>"F9", KeyCode::F(10)=>"F10",
        KeyCode::Char(c) => match c {
            'a'=>"a",'b'=>"b",'c'=>"c",'d'=>"d",'e'=>"e",'f'=>"f",'g'=>"g",'h'=>"h",
            'i'=>"i",'j'=>"j",'k'=>"k",'l'=>"l",'m'=>"m",'n'=>"n",'o'=>"o",'p'=>"p",
            'q'=>"q",'r'=>"r",'s'=>"s",'t'=>"t",'u'=>"u",'w'=>"w",'x'=>"x",'y'=>"y",
            'z'=>"z",'0'=>"0",'1'=>"1",'2'=>"2",'3'=>"3",'4'=>"4",
            '5'=>"5",'6'=>"6",'7'=>"7",'8'=>"8",'9'=>"9", _=>"",
        },
        _ => "",
    }
}

// ---------------------------------------------------------------------------
// Send commands
// ---------------------------------------------------------------------------

async fn send_cmd(ws_tx: &mut WsSink, pane: &str, cmd: &str, args: serde_json::Value) {
    let _ = ws_tx.send(Message::Text(serde_json::json!({"command":cmd,"pane":pane,"args":args}).to_string())).await;
}

async fn send_node_cmd(ws_tx: &mut WsSink, cmd: &str, params: serde_json::Value) {
    let mut msg = params; msg["command"] = serde_json::Value::String(cmd.to_string());
    let _ = ws_tx.send(Message::Text(msg.to_string())).await;
}

async fn cmd_pause_toggle_all(ws_tx: &mut WsSink, panes: &HashMap<String, PaneState>, pane_order: &[String]) {
    let any_playing = panes.values().any(|p| p.online && !p.idle_active.unwrap_or(true) && !p.paused.unwrap_or(true));
    for pane in pane_order {
        if any_playing { send_cmd(ws_tx, pane, "set_property", serde_json::json!(["pause", true])).await; }
        else {
            send_cmd(ws_tx, pane, "set_property", serde_json::json!(["mute",  true])).await;
            send_cmd(ws_tx, pane, "set_property", serde_json::json!(["pause", false])).await;
        }
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
        if pane != keep { send_cmd(ws_tx, pane, "set_property", serde_json::json!(["mute", true])).await; }
    }
}

// ---------------------------------------------------------------------------
// Spawn daemon
// ---------------------------------------------------------------------------

fn spawn_daemon() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false; };
    let Some(dir) = exe.parent() else { return false; };
    let daemon_path = dir.join("panebot-daemon");
    if !daemon_path.exists() { return false; }
    let mut cmd = std::process::Command::new(&daemon_path);
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(unix)] { use std::os::unix::process::CommandExt; cmd.process_group(0); }
    cmd.spawn().is_ok()
}

// ---------------------------------------------------------------------------
// WebSocket connection
// ---------------------------------------------------------------------------

async fn connect_ws(terminal: &mut Term, addr: &str) -> io::Result<(tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, bool)> {
    render_startup(terminal, &format!("Connecting to {}...", addr))?;
    if let Ok((ws, _)) = connect_async(addr).await { return Ok((ws, false)); }

    if addr == LOCAL_ADDR {
        let spawned    = spawn_daemon();
        let mut kevs   = EventStream::new();
        let msg        = if spawned { "Starting daemon... [q] to cancel" } else { "Waiting for daemon... [q] to cancel" };
        loop {
            render_startup(terminal, msg)?;
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_millis(CONNECT_RETRY_MS)) => {
                    if let Ok((ws, _)) = connect_async(addr).await { return Ok((ws, spawned)); }
                }
                key = kevs.next() => {
                    if let Some(Ok(Event::Key(k))) = key {
                        if k.code == KeyCode::Char('q') { return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled")); }
                    }
                }
            }
        }
    }

    let retries = (CONNECT_TIMEOUT_S * 1000 / CONNECT_RETRY_MS) as u32;
    for attempt in 0..retries {
        let remaining = CONNECT_TIMEOUT_S.saturating_sub(attempt as u64 * CONNECT_RETRY_MS / 1000);
        render_startup(terminal, &format!("Waiting for {} ({}s)...", addr, remaining))?;
        tokio::time::sleep(std::time::Duration::from_millis(CONNECT_RETRY_MS)).await;
        if let Ok((ws, _)) = connect_async(addr).await { return Ok((ws, false)); }
    }

    Err(io::Error::new(io::ErrorKind::TimedOut, format!("Could not connect to {} after {}s", addr, CONNECT_TIMEOUT_S)))
}

// ---------------------------------------------------------------------------
// Host resolution
// ---------------------------------------------------------------------------

async fn resolve_daemon_addr(terminal: &mut Term) -> io::Result<Option<String>> {
    let mut hosts = load_hosts();
    if hosts.is_empty() { return Ok(Some(LOCAL_ADDR.to_string())); }
    hosts.insert(0, Host { label: "local".to_string(), address: LOCAL_ADDR.to_string() });

    let mut sel  = 0usize;
    let mut kevs = EventStream::new();
    render_host_picker(terminal, &hosts, sel)?;

    loop {
        if let Some(Ok(Event::Key(k))) = kevs.next().await {
            match k.code {
                KeyCode::Char('q')                 => return Ok(None),
                KeyCode::Char('k') | KeyCode::Up   => { sel = sel.saturating_sub(1); render_host_picker(terminal, &hosts, sel)?; }
                KeyCode::Char('j') | KeyCode::Down => { if sel + 1 < hosts.len() { sel += 1; } render_host_picker(terminal, &hosts, sel)?; }
                KeyCode::Enter                     => return Ok(Some(hosts[sel].address.clone())),
                _ => {}
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Key action
// ---------------------------------------------------------------------------

enum KeyAction { Quit, Render, RenderDetails, Reconnect, Nothing }

// ---------------------------------------------------------------------------
// Details key handler
// ---------------------------------------------------------------------------

async fn handle_details_keys(app: &mut App, k: crossterm::event::KeyEvent, ws_tx: &mut WsSink) -> io::Result<KeyAction> {

    if app.details_mode == DetailsMode::Jump {
        match k.code {
            KeyCode::Char(c) if c.is_ascii_digit() => { app.jump_input.push(c); }
            KeyCode::Backspace => { app.jump_input.pop(); }
            KeyCode::Enter => {
                if let Ok(idx) = app.jump_input.parse::<usize>() {
                    app.playlist_sel = idx.min(app.playlist_items.len().saturating_sub(1));
                }
                app.jump_input.clear(); app.details_mode = DetailsMode::Normal;
            }
            KeyCode::Esc => { app.jump_input.clear(); app.details_mode = DetailsMode::Normal; }
            _ => return Ok(KeyAction::Nothing),
        }
        return Ok(KeyAction::RenderDetails);
    }

    if app.details_mode == DetailsMode::Confirm {
        match k.code {
            KeyCode::Enter => {
                if let Some(pane) = app.selected_name() {
                    send_cmd(ws_tx, &pane, "playlist-play-index", serde_json::json!([app.playlist_sel])).await;
                    send_cmd(ws_tx, &pane, "set_property", serde_json::json!(["pause", false])).await;
                }
                app.details_mode = DetailsMode::Normal; app.status_msg = None;
            }
            KeyCode::Char('n') => {
                if let Some(pane) = app.selected_name() {
                    let current_pos = app.current_playlist_pos();
                    let insert_at = if current_pos >= 0 { (current_pos as usize + 1).min(app.playlist_items.len()) } else { 0 };
                    let sel = app.playlist_sel;
                    if sel != insert_at && sel + 1 != insert_at {
                        send_cmd(ws_tx, &pane, "playlist-move", serde_json::json!([sel, insert_at])).await;
                        send_node_cmd(ws_tx, "panebot:playlist-get", serde_json::json!({"pane": pane})).await;
                        app.status_msg = Some("Queued next".to_string());
                    }
                }
                app.details_mode = DetailsMode::Normal;
            }
            KeyCode::Esc => { app.details_mode = DetailsMode::Normal; app.status_msg = None; }
            _ => return Ok(KeyAction::Nothing),
        }
        return Ok(KeyAction::RenderDetails);
    }

    if app.details_mode == DetailsMode::Save {
        match k.code {
            KeyCode::Char(c)   => { app.add_input.push(c); }
            KeyCode::Backspace => { app.add_input.pop(); }
            KeyCode::Enter => {
                let name = app.add_input.trim().to_string();
                if !name.is_empty() {
                    if let Some(pane) = app.selected_name() {
                        send_node_cmd(ws_tx, "panebot:playlist-save", serde_json::json!({"pane": pane, "path": format!("{}/{}.m3u", app.home, name)})).await;
                    }
                }
                app.add_input.clear(); app.details_mode = DetailsMode::Normal;
            }
            KeyCode::Esc => { app.add_input.clear(); app.details_mode = DetailsMode::Normal; }
            _ => return Ok(KeyAction::Nothing),
        }
        return Ok(KeyAction::RenderDetails);
    }

    if app.details_mode == DetailsMode::Add {
        match k.code {
            KeyCode::Char(c)   => { app.add_input.push(c); }
            KeyCode::Backspace => { app.add_input.pop(); }
            KeyCode::Enter => {
                let entry = app.add_input.trim().to_string();
                if !entry.is_empty() {
                    if let Some(pane) = app.selected_name() {
                        let expanded = if entry.starts_with('~') { entry.replacen('~', &app.home, 1) } else { entry };
                        send_cmd(ws_tx, &pane, "loadfile", serde_json::json!([expanded, "append"])).await;
                        send_node_cmd(ws_tx, "panebot:playlist-get", serde_json::json!({"pane": pane})).await;
                    }
                }
                app.add_input.clear(); app.details_mode = DetailsMode::Normal;
            }
            KeyCode::Esc => { app.add_input.clear(); app.details_mode = DetailsMode::Normal; }
            _ => return Ok(KeyAction::Nothing),
        }
        return Ok(KeyAction::RenderDetails);
    }

    // Command mode in details
    if app.command_mode {
        if let Some(pane) = app.selected_name() {
            match k.code {
                KeyCode::Char(' ')                  => { send_cmd(ws_tx, &pane, "cycle",    serde_json::json!(["pause"])).await; }
                KeyCode::Char('m')                  => { send_cmd(ws_tx, &pane, "cycle",    serde_json::json!(["mute"])).await; }
                KeyCode::Enter                      => { send_cmd(ws_tx, &pane, "keypress", serde_json::json!(["ENTER"])).await; }
                KeyCode::Char('f')                  => { send_cmd(ws_tx, &pane, "cycle",    serde_json::json!(["fullscreen"])).await; }
                KeyCode::Left  | KeyCode::Char('h') => { send_cmd(ws_tx, &pane, "seek",     serde_json::json!([-5,  "relative"])).await; }
                KeyCode::Right | KeyCode::Char('l') => { send_cmd(ws_tx, &pane, "seek",     serde_json::json!([5,   "relative"])).await; }
                KeyCode::Up    | KeyCode::Char('k') => { send_cmd(ws_tx, &pane, "seek",     serde_json::json!([60,  "relative"])).await; }
                KeyCode::Down  | KeyCode::Char('j') => { send_cmd(ws_tx, &pane, "seek",     serde_json::json!([-60, "relative"])).await; }
                KeyCode::Char('0')                  => { send_cmd(ws_tx, &pane, "add",      serde_json::json!(["volume",  5])).await; }
                KeyCode::Char('9')                  => { send_cmd(ws_tx, &pane, "add",      serde_json::json!(["volume", -5])).await; }
                KeyCode::Char('v')                  => { app.passthrough_mode = true; }
                KeyCode::Tab                        => { app.command_mode = false; app.status_msg = None; }
                _ => return Ok(KeyAction::Nothing),
            }
            return Ok(KeyAction::RenderDetails);
        }
    }

    // Normal mode
    if (k.code == KeyCode::Left || k.code == KeyCode::Char('h')) && !app.command_mode {
        app.close_details();
        return Ok(KeyAction::Render);
    }

    match k.code {
        KeyCode::Enter     => { app.details_mode = DetailsMode::Confirm; app.status_msg = None; }
        KeyCode::Tab       => { app.command_mode = !app.command_mode; app.status_msg = None; }
        KeyCode::Up    | KeyCode::Char('k') => { app.playlist_sel = app.playlist_sel.saturating_sub(1); app.status_msg = None; }
        KeyCode::Down  | KeyCode::Char('j') => { if app.playlist_sel + 1 < app.playlist_items.len() { app.playlist_sel += 1; } app.status_msg = None; }
        KeyCode::Char('J') => { app.playlist_sel = app.playlist_items.len().saturating_sub(1); app.status_msg = None; }
        KeyCode::Char('K') => { app.playlist_sel = 0; app.status_msg = None; }
        KeyCode::Char('[') => { app.playlist_sel = app.playlist_sel.saturating_sub(10); app.status_msg = None; }
        KeyCode::Char(']') => { app.playlist_sel = (app.playlist_sel + 10).min(app.playlist_items.len().saturating_sub(1)); app.status_msg = None; }
        KeyCode::Char(' ') => {
            let idx = app.playlist_sel;
            if app.selected_items.contains(&idx) { app.selected_items.remove(&idx); } else { app.selected_items.insert(idx); }
            if app.playlist_sel + 1 < app.playlist_items.len() { app.playlist_sel += 1; }
            app.status_msg = None;
        }
        KeyCode::Esc => {
            if !app.selected_items.is_empty() { app.selected_items.clear(); app.status_msg = None; }
            else { return Ok(KeyAction::Nothing); }
        }
        KeyCode::Char('G') => { app.details_mode = DetailsMode::Jump; app.jump_input.clear(); }
        KeyCode::Char('A') => { app.details_mode = DetailsMode::Add;  app.add_input.clear();  app.status_msg = None; }
        KeyCode::Char('S') => { app.details_mode = DetailsMode::Save; app.add_input.clear();  app.status_msg = None; }
        KeyCode::Char('D') => {
            if let Some(pane) = app.selected_name() {
                let current_pos = app.current_playlist_pos();
                let targets: Vec<usize> = if app.selected_items.is_empty() { vec![app.playlist_sel] } else {
                    let mut v: Vec<usize> = app.selected_items.iter().cloned().collect(); v.sort_unstable(); v
                };
                if targets.iter().any(|&i| current_pos >= 0 && i as i64 == current_pos) {
                    app.status_msg = Some("Cannot remove playing item".to_string());
                } else {
                    for &idx in targets.iter().rev() {
                        send_cmd(ws_tx, &pane, "playlist-remove", serde_json::json!([idx])).await;
                    }
                    app.selected_items.clear();
                    app.playlist_sel = app.playlist_sel.min(app.playlist_items.len().saturating_sub(targets.len()).saturating_sub(1));
                    send_node_cmd(ws_tx, "panebot:playlist-get", serde_json::json!({"pane": pane})).await;
                    app.status_msg = None;
                }
            }
        }
        KeyCode::Char('M') => {
            if let Some(pane) = app.selected_name() {
                if app.selected_items.is_empty() {
                    app.status_msg = Some("Mark items with Space first".to_string());
                } else {
                    let dest = app.playlist_sel;
                    let mut targets: Vec<usize> = app.selected_items.iter().cloned().collect(); targets.sort_unstable();
                    let mut adj = dest;
                    for &idx in &targets {
                        send_cmd(ws_tx, &pane, "playlist-move", serde_json::json!([idx, adj])).await;
                        if idx < adj { adj = adj.saturating_sub(1); }
                    }
                    app.selected_items.clear(); app.playlist_sel = dest;
                    send_node_cmd(ws_tx, "panebot:playlist-get", serde_json::json!({"pane": pane})).await;
                    app.status_msg = None;
                }
            }
        }
        KeyCode::Char('C') => {
            if let Some(pane) = app.selected_name() {
                let keep: Vec<usize> = if !app.selected_items.is_empty() {
                    let mut v: Vec<usize> = app.selected_items.iter().cloned().collect(); v.sort_unstable(); v
                } else {
                    let pos = app.current_playlist_pos();
                    if pos < 0 { app.status_msg = Some("Nothing playing to crop around".to_string()); return Ok(KeyAction::RenderDetails); }
                    vec![pos as usize]
                };
                let mut to_remove: Vec<usize> = (0..app.playlist_items.len()).filter(|i| !keep.contains(i)).collect();
                to_remove.sort_unstable();
                for &idx in to_remove.iter().rev() { send_cmd(ws_tx, &pane, "playlist-remove", serde_json::json!([idx])).await; }
                app.selected_items.clear(); app.playlist_sel = 0;
                send_node_cmd(ws_tx, "panebot:playlist-get", serde_json::json!({"pane": pane})).await;
                app.status_msg = None;
            }
        }
        _ => return Ok(KeyAction::Nothing),
    }
    Ok(KeyAction::RenderDetails)
}

// ---------------------------------------------------------------------------
// Dashboard key handler
// ---------------------------------------------------------------------------

async fn handle_dashboard_keys(app: &mut App, k: crossterm::event::KeyEvent, ws_tx: &mut WsSink) -> io::Result<KeyAction> {

    if app.show_picker {
        match k.code {
            KeyCode::Char('j') | KeyCode::Down => { if app.picker_sel + 1 < app.layouts.len() { app.picker_sel += 1; } }
            KeyCode::Char('k') | KeyCode::Up   => { app.picker_sel = app.picker_sel.saturating_sub(1); }
            KeyCode::Enter => {
                if let Some(layout) = app.layouts.get(app.picker_sel).cloned() {
                    send_node_cmd(ws_tx, "panebot:layout", serde_json::json!({"layout_name": layout})).await;
                    app.show_picker = false;
                }
            }
            KeyCode::Esc | KeyCode::Char('W') => { app.show_picker = false; }
            _ => return Ok(KeyAction::Nothing),
        }
        return Ok(KeyAction::Render);
    }

    if k.code == KeyCode::Char('W') && !app.is_remote {
        app.picker_sel  = app.layouts.iter().position(|l| l == &app.layout).unwrap_or(0);
        app.show_picker = true;
        return Ok(KeyAction::Render);
    }

    if k.code == KeyCode::Char('C') { return Ok(KeyAction::Reconnect); }

    if !app.command_mode && (k.code == KeyCode::Left || k.code == KeyCode::Char('h')) {
        app.go_log(); return Ok(KeyAction::Render);
    }

    if !app.command_mode && (k.code == KeyCode::Right || k.code == KeyCode::Char('l')) {
        if app.selected_name().is_some() {
            app.open_details();
            if let Some(pane) = app.selected_name() {
                send_node_cmd(ws_tx, "panebot:playlist-get", serde_json::json!({"pane": pane})).await;
            }
            return Ok(KeyAction::RenderDetails);
        }
        return Ok(KeyAction::Nothing);
    }

    if k.code == KeyCode::Tab { app.command_mode = !app.command_mode; return Ok(KeyAction::Render); }

    if !app.command_mode {
        match k.code {
            KeyCode::Char('j') | KeyCode::Down => { app.select_next(); return Ok(KeyAction::Render); }
            KeyCode::Char('k') | KeyCode::Up   => { app.select_prev(); return Ok(KeyAction::Render); }
            KeyCode::Char('r') => {
                if let Some(pane) = app.selected_name() { send_node_cmd(ws_tx, "panebot:restart-pane", serde_json::json!({"pane": pane})).await; }
                return Ok(KeyAction::Render);
            }
            KeyCode::Char('R') => { send_node_cmd(ws_tx, "panebot:restart-all", serde_json::json!({})).await; return Ok(KeyAction::Render); }
            KeyCode::Char('S') => {
                if let Some(pane) = app.selected_name() { let po = app.pane_order.clone(); cmd_solo(ws_tx, &pane, &po).await; }
                return Ok(KeyAction::Render);
            }
            KeyCode::Char('M') => {
                if let Some(pane) = app.selected_name() { let po = app.pane_order.clone(); cmd_mute_others(ws_tx, &pane, &po).await; }
                return Ok(KeyAction::Render);
            }
            KeyCode::Char('P') => {
                let po = app.pane_order.clone(); cmd_pause_toggle_all(ws_tx, &app.panes, &po).await;
                return Ok(KeyAction::Render);
            }
            _ => {}
        }
    }

    if app.command_mode {
        if let Some(pane) = app.selected_name() {
            match k.code {
                KeyCode::Char(' ')                  => { send_cmd(ws_tx, &pane, "cycle",    serde_json::json!(["pause"])).await; }
                KeyCode::Char('m')                  => { send_cmd(ws_tx, &pane, "cycle",    serde_json::json!(["mute"])).await; }
                KeyCode::Enter                      => { send_cmd(ws_tx, &pane, "keypress", serde_json::json!(["ENTER"])).await; }
                KeyCode::Char('f')                  => { send_cmd(ws_tx, &pane, "cycle",    serde_json::json!(["fullscreen"])).await; }
                KeyCode::Left  | KeyCode::Char('h') => { send_cmd(ws_tx, &pane, "seek",     serde_json::json!([-5,  "relative"])).await; }
                KeyCode::Right | KeyCode::Char('l') => { send_cmd(ws_tx, &pane, "seek",     serde_json::json!([5,   "relative"])).await; }
                KeyCode::Up    | KeyCode::Char('k') => { send_cmd(ws_tx, &pane, "seek",     serde_json::json!([60,  "relative"])).await; }
                KeyCode::Down  | KeyCode::Char('j') => { send_cmd(ws_tx, &pane, "seek",     serde_json::json!([-60, "relative"])).await; }
                KeyCode::Char('0')                  => { send_cmd(ws_tx, &pane, "add",      serde_json::json!(["volume",  5])).await; }
                KeyCode::Char('9')                  => { send_cmd(ws_tx, &pane, "add",      serde_json::json!(["volume", -5])).await; }
                KeyCode::Char('v')                  => { app.passthrough_mode = true; }
                _ => return Ok(KeyAction::Nothing),
            }
            return Ok(KeyAction::Render);
        }
    }

    Ok(KeyAction::Nothing)
}

// ---------------------------------------------------------------------------
// Main key handler
// ---------------------------------------------------------------------------

async fn handle_key(app: &mut App, k: crossterm::event::KeyEvent, ws_tx: &mut WsSink) -> io::Result<KeyAction> {

    if k.code == KeyCode::Char('q') && !app.show_picker {
        if app.show_details { app.close_details(); return Ok(KeyAction::Render); }
        if app.show_log     { app.leave_log();     return Ok(KeyAction::Render); }
        return Ok(KeyAction::Quit);
    }

    if app.passthrough_mode {
        if k.code == KeyCode::Char('v') { app.passthrough_mode = false; return Ok(KeyAction::Render); }
        if let Some(pane) = app.selected_name() {
            let key_str = mpv_key_name(k.code);
            if !key_str.is_empty() { send_cmd(ws_tx, &pane, "keypress", serde_json::json!([key_str])).await; }
        }
        return Ok(KeyAction::Nothing);
    }

    if app.show_log && !app.show_details {
        if k.code == KeyCode::Right || k.code == KeyCode::Char('l') ||
           k.code == KeyCode::Char('j') || k.code == KeyCode::Down {
            app.leave_log(); return Ok(KeyAction::Render);
        }
        return Ok(KeyAction::Nothing);
    }

    if app.show_details { return handle_details_keys(app, k, ws_tx).await; }

    handle_dashboard_keys(app, k, ws_tx).await
}

// ---------------------------------------------------------------------------
// Main event loop
// ---------------------------------------------------------------------------

async fn run(terminal: &mut Term) -> io::Result<()> {
    let mut app  = App::new();
    let mut kevs = EventStream::new();

    'outer: loop {
        let addr = match resolve_daemon_addr(terminal).await? { Some(a) => a, None => return Ok(()) };

        'reconnect: loop {
            let ws = match connect_ws(terminal, &addr).await {
                Ok((ws, spawned)) => { if spawned { app.owns_daemon = true; } app.is_remote = addr != LOCAL_ADDR; ws }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => break 'reconnect,
                Err(_) => { tokio::time::sleep(std::time::Duration::from_millis(2000)).await; continue 'reconnect; }
            };

            app.pane_order.clear(); app.panes.clear(); app.selected = 0;
            let (mut ws_tx, mut ws_rx) = ws.split();

            loop {
                tokio::select! {
                    msg = ws_rx.next() => {
                        match msg {
                            Some(Ok(Message::Text(text))) => {
                                let signal = process_event(&mut app, &text);
                                if app.show_details { render_details(terminal, &app)?; } else { render(terminal, &app)?; }
                                if signal == Some("node:down") {
                                    tokio::time::sleep(std::time::Duration::from_millis(2000)).await;
                                    continue 'reconnect;
                                }
                            }
                            Some(Err(_)) | None => { tokio::time::sleep(std::time::Duration::from_millis(1000)).await; continue 'reconnect; }
                            _ => {}
                        }
                    }
                    key = kevs.next() => {
                        match key {
                            Some(Ok(Event::Key(k))) => {
                                match handle_key(&mut app, k, &mut ws_tx).await? {
                                    KeyAction::Quit => {
                                        if app.owns_daemon {
                                            send_node_cmd(&mut ws_tx, "panebot:shutdown", serde_json::json!({})).await;
                                            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                                        }
                                        break 'outer;
                                    }
                                    KeyAction::Reconnect     => break 'reconnect,
                                    KeyAction::Render        => render(terminal, &app)?,
                                    KeyAction::RenderDetails => {
                                        if app.show_details { render_details(terminal, &app)?; } else { render(terminal, &app)?; }
                                    }
                                    KeyAction::Nothing => {}
                                }
                            }
                            Some(Err(e)) => return Err(e),
                            None         => break 'outer,
                            _            => {}
                        }
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
    let result       = run(&mut terminal).await;
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    result
}
