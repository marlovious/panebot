use crossterm::{
    event::{self, Event, KeyCode},
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

fn config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/panebot")
}

fn load_panes() -> Vec<Pane> {
    let registry = config_dir().join("panes.conf");
    let content = match std::fs::read_to_string(&registry) {
        Ok(c) => c,
        Err(_) => return mock_panes(),
    };
    let mut panes = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 2 { continue; }
        panes.push(Pane {
            name:      cols[0].to_string(),
            socket:    cols[1].to_string(),
            pane_type: "Video".to_string(),
            status:    "Stopped".to_string(),
            volume:    0,
            muted:     false,
            title:     "\u{2014}".to_string(),
        });
    }
    if panes.is_empty() { mock_panes() } else { panes }
}

struct Pane {
    name: String,
    socket: String,
    pane_type: String,
    status: String,
    volume: i64,
    muted: bool,
    title: String,
}

struct PlaylistItem {
    index: usize,
    title: String,
}

fn mock_panes() -> Vec<Pane> {
    vec![
        Pane { name: "Movies".into(), socket: "~/.config/panebot/movies.sock".into(), pane_type: "Video".into(), status: "Playing".into(), volume: 75, muted: false, title: "The Princess Bride".into() },
        Pane { name: "Music".into(),  socket: "~/.config/panebot/music.sock".into(),  pane_type: "Audio".into(), status: "Stopped".into(), volume: 60, muted: true,  title: "Kind of Blue".into() },
        Pane { name: "Shows".into(),  socket: "~/.config/panebot/shows.sock".into(),  pane_type: "Video".into(), status: "Playing".into(), volume: 40, muted: false, title: "Animaniacs S01E03".into() },
        Pane { name: "Tunes".into(),  socket: "~/.config/panebot/tunes.sock".into(),  pane_type: "Audio".into(), status: "Stopped".into(), volume: 80, muted: true,  title: "\u{2014}".into() },
    ]
}

fn mock_playlist() -> Vec<PlaylistItem> {
    vec![
        PlaylistItem { index: 0, title: "The Princess Bride".into() },
        PlaylistItem { index: 1, title: "Blade Runner 2049".into() },
        PlaylistItem { index: 2, title: "Animaniacs S01E03".into() },
        PlaylistItem { index: 3, title: "The Iron Giant".into() },
        PlaylistItem { index: 4, title: "Spirited Away".into() },
    ]
}

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
    let socket = pane.socket.clone();
    let pause = query_mpv(&socket, "pause").unwrap_or_default();
    pane.status = match pause.as_str() {
        "yes" => "Stopped".to_string(),
        "no"  => "Playing".to_string(),
        _     => "Stopped".to_string(),
    };
    pane.muted = query_mpv(&socket, "mute").map(|v| v == "yes").unwrap_or(false);
    pane.volume = query_mpv(&socket, "volume")
        .and_then(|v| v.parse::<f64>().ok())
        .map(|v| v as i64)
        .unwrap_or(0);
    pane.title = query_mpv(&socket, "media-title")
        .unwrap_or_else(|| "\u{2014}".to_string());
}

fn fetch_playlist(socket: &str) -> Vec<PlaylistItem> {
    let mut stream = match UnixStream::connect(socket) {
        Ok(s) => s,
        Err(_) => return mock_playlist(),
    };
    let cmd = "{\"command\":[\"get_property\",\"playlist\"]}\n";
    if stream.write_all(cmd.as_bytes()).is_err() { return mock_playlist(); }
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let line = match line { Ok(l) => l, Err(_) => continue };
        let v: serde_json::Value = match serde_json::from_str(&line) { Ok(v) => v, Err(_) => continue };
        if v["error"] == "success" {
            if let Some(arr) = v["data"].as_array() {
                return arr.iter().enumerate().map(|(i, item)| {
                    let title = item["title"].as_str()
                        .or_else(|| item["filename"].as_str())
                        .unwrap_or("Unknown")
                        .to_string();
                    PlaylistItem { index: i, title }
                }).collect();
            }
        }
    }
    mock_playlist()
}

// --- Colors ---

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

fn divider(width: usize, color: Color) -> Paragraph<'static> {
    Paragraph::new(Span::styled(
        "-".repeat(width),
        Style::default().fg(color),
    )).style(Style::default().bg(C_BG))
}

// --- Dashboard widgets ---

fn render_dashboard_header<'a>() -> Paragraph<'a> {
    let spans = Line::from(vec![
        Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
        Span::styled("  ::  Active Panes ::", Style::default().fg(C_DIM)),
    ]);
    Paragraph::new(spans).style(Style::default().bg(C_BG))
}

fn render_dashboard_row(pane: &Pane, selected: bool, cmd_mode: bool) -> ListItem<'static> {
    let arrow = if selected { ">>>" } else { "   " };
    let vol_str = if pane.muted {
        "[Vol:Mute]".to_string()
    } else {
        format!("[Vol:{:>3}%]", pane.volume)
    };
    let vol_color    = if pane.muted { C_DIM } else { C_PINK };
    let status_color = if pane.status == "Playing" { C_ORANGE } else { C_DIM };
    let name_color   = if selected { Color::White } else { C_ORANGE };
    let title_color  = if selected { Color::White } else { C_CYAN };

    let spans = Line::from(vec![
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
        if cmd_mode && selected {
            Span::styled("  [CMD]".to_string(), Style::default().fg(C_CMD_KEY))
        } else {
            Span::raw("")
        },
    ]);

    let style = if selected { Style::default().bg(C_CURSOR) } else { Style::default().bg(C_BG) };
    ListItem::new(spans).style(style)
}

fn render_dashboard_statusbar<'a>(cmd_mode: bool) -> Paragraph<'a> {
    let spans = if cmd_mode {
        Line::from(vec![
            Span::styled("[Space]",       Style::default().fg(C_CMD_KEY)),
            Span::styled(" Play :: ",     Style::default().fg(C_CMD_HNT)),
            Span::styled("[m]",           Style::default().fg(C_CMD_KEY)),
            Span::styled(" Mute :: ",     Style::default().fg(C_CMD_HNT)),
            Span::styled("[Left/Right]",  Style::default().fg(C_CMD_KEY)),
            Span::styled(" Seek 10s :: ", Style::default().fg(C_CMD_HNT)),
            Span::styled("[Up/Down]",     Style::default().fg(C_CMD_KEY)),
            Span::styled(" Seek 1m :: ",  Style::default().fg(C_CMD_HNT)),
            Span::styled("[=/-]",         Style::default().fg(C_CMD_KEY)),
            Span::styled(" Vol :: ",      Style::default().fg(C_CMD_HNT)),
            Span::styled("[n/N]",         Style::default().fg(C_CMD_KEY)),
            Span::styled(" Next/Prev :: ", Style::default().fg(C_CMD_HNT)),
            Span::styled("[Tab]",         Style::default().fg(C_CMD_KEY)),
            Span::styled(" Exit Cmd",     Style::default().fg(C_CMD_HNT)),
        ])
    } else {
        Line::from(vec![
            Span::styled("[Up/Down]", Style::default().fg(C_CYAN)),
            Span::styled(" Select :: ",         Style::default().fg(C_HINT)),
            Span::styled("[Tab]",      Style::default().fg(C_CYAN)),
            Span::styled(" Toggle Cmd Mode :: ", Style::default().fg(C_HINT)),
            Span::styled("[Enter]",    Style::default().fg(C_CYAN)),
            Span::styled(" Pane Details :: ",    Style::default().fg(C_HINT)),
            Span::styled("[q]",        Style::default().fg(C_CYAN)),
            Span::styled(" Exit PaneBot",        Style::default().fg(C_HINT)),
        ])
    };
    let bg = if cmd_mode { C_CMD_BG } else { C_BG };
    Paragraph::new(spans).style(Style::default().bg(bg))
}

// --- Playlist widgets ---

fn render_playlist_header<'a>(pane: &Pane) -> Paragraph<'a> {
    let vol_str = if pane.muted {
        "[Vol:Mute]".to_string()
    } else {
        format!("[Vol:{:>3}%]", pane.volume)
    };
    let spans = Line::from(vec![
        Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
        Span::styled(" :: ", Style::default().fg(C_DIM)),
        Span::styled(format!("\"{}\"", pane.name), Style::default().fg(Color::White)),
        Span::styled(" :: ", Style::default().fg(C_DIM)),
        Span::styled(format!("[{}]", pane.pane_type), Style::default().fg(C_CYAN)),
        Span::styled(" :: ", Style::default().fg(C_DIM)),
        Span::styled(format!("[{}]", pane.status),
            Style::default().fg(if pane.status == "Playing" { C_ORANGE } else { C_DIM })),
        Span::styled(" :: ", Style::default().fg(C_DIM)),
        Span::styled(vol_str, Style::default().fg(C_PINK)),
        Span::styled(" ::", Style::default().fg(C_DIM)),
    ]);
    Paragraph::new(spans).style(Style::default().bg(C_BG))
}

fn render_playlist_row(item: &PlaylistItem, selected: bool, item_cmd: bool) -> ListItem<'static> {
    let arrow = if selected { ">>" } else { "  " };
    let spans = Line::from(vec![
        Span::styled(format!("{} ", arrow),        Style::default().fg(C_ORANGE)),
        Span::styled(format!("{:<3}", item.index), Style::default().fg(C_PINK)),
        Span::styled(" :: ",                       Style::default().fg(C_DIM)),
        Span::styled(item.title.clone(),           Style::default().fg(if selected { Color::White } else { C_CYAN })),
        if item_cmd && selected {
            Span::styled("  [CMD]".to_string(), Style::default().fg(C_CMD_KEY))
        } else {
            Span::raw("")
        },
    ]);
    let style = if selected { Style::default().bg(C_CURSOR) } else { Style::default().bg(C_BG) };
    ListItem::new(spans).style(style)
}

fn render_playlist_statusbar<'a>(item_cmd: bool, move_input: bool, move_buf: &str) -> Paragraph<'a> {
    if move_input {
        let spans = Line::from(vec![
            Span::styled("Move to position: ", Style::default().fg(C_HINT)),
            Span::styled(move_buf.to_string(), Style::default().fg(Color::White)),
            Span::styled("_", Style::default().fg(C_ORANGE)),
        ]);
        return Paragraph::new(spans).style(Style::default().bg(C_CMD_BG));
    }

    let spans = if item_cmd {
        Line::from(vec![
            Span::styled("[Enter]",  Style::default().fg(C_CMD_KEY)),
            Span::styled(" Play Now :: ",      Style::default().fg(C_CMD_HNT)),
            Span::styled("[r]",      Style::default().fg(C_CMD_KEY)),
            Span::styled(" Remove :: ",        Style::default().fg(C_CMD_HNT)),
            Span::styled("[m]",      Style::default().fg(C_CMD_KEY)),
            Span::styled(" Change Position :: ", Style::default().fg(C_CMD_HNT)),
            Span::styled("[Tab]",    Style::default().fg(C_CMD_KEY)),
            Span::styled(" Exit Modify",       Style::default().fg(C_CMD_HNT)),
        ])
    } else {
        Line::from(vec![
            Span::styled("[Up/Down]",   Style::default().fg(C_CYAN)),
            Span::styled(" Select :: ",      Style::default().fg(C_HINT)),
            Span::styled("[Tab]",       Style::default().fg(C_CYAN)),
            Span::styled(" Modify Item :: ", Style::default().fg(C_HINT)),
            Span::styled("[c]",         Style::default().fg(C_CYAN)),
            Span::styled(" Crop :: ",        Style::default().fg(C_HINT)),
            Span::styled("[n]",         Style::default().fg(C_CYAN)),
            Span::styled(" Add Item :: ",    Style::default().fg(C_HINT)),
            Span::styled("[Backspace]", Style::default().fg(C_CYAN)),
            Span::styled(" Return",          Style::default().fg(C_HINT)),
        ])
    };
    let bg = if item_cmd { C_CMD_BG } else { C_BG };
    Paragraph::new(spans).style(Style::default().bg(bg))
}

// --- Screen ---

enum Screen {
    Dashboard,
    Playlist(usize),
}

// --- Main ---

fn main() -> Result<(), io::Error> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut panes       = load_panes();
    let mut cursor      = 0usize;
    let mut cmd_mode    = false;
    let mut screen      = Screen::Dashboard;

    let mut pl_items:      Vec<PlaylistItem> = Vec::new();
    let mut pl_cursor      = 0usize;
    let mut pl_item_cmd    = false;
    let mut pl_move_input  = false; // true when typing a position number
    let mut pl_move_buf    = String::new();

    loop {
        for pane in panes.iter_mut() {
            refresh_pane(pane);
        }
        if let Screen::Playlist(idx) = &screen {
            pl_items = fetch_playlist(&panes[*idx].socket);
        }

        terminal.draw(|f| {
            let size = f.size();
            let div_color = if cmd_mode || pl_item_cmd { Color::Rgb(90, 55, 10) } else { C_DIVIDER };

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Min(1),
                    Constraint::Length(1),
                    Constraint::Length(1),
                ])
                .split(size);

            f.render_widget(Block::default().style(Style::default().bg(C_BG)), size);

            match &screen {
                Screen::Dashboard => {
                    f.render_widget(render_dashboard_header(), chunks[1]);
                    f.render_widget(divider(size.width as usize, C_DIVIDER), chunks[2]);

                    let items: Vec<ListItem> = panes.iter().enumerate()
                        .map(|(i, p)| render_dashboard_row(p, i == cursor, cmd_mode && i == cursor))
                        .collect();
                    let mut state = ListState::default();
                    state.select(Some(cursor));
                    f.render_stateful_widget(List::new(items).style(Style::default().bg(C_BG)), chunks[3], &mut state);

                    f.render_widget(divider(size.width as usize, div_color), chunks[4]);
                    f.render_widget(render_dashboard_statusbar(cmd_mode), chunks[5]);
                }

                Screen::Playlist(idx) => {
                    f.render_widget(render_playlist_header(&panes[*idx]), chunks[1]);
                    f.render_widget(divider(size.width as usize, C_DIVIDER), chunks[2]);

                    let items: Vec<ListItem> = pl_items.iter().enumerate()
                        .map(|(i, item)| render_playlist_row(item, i == pl_cursor, pl_item_cmd && i == pl_cursor))
                        .collect();
                    let mut state = ListState::default();
                    state.select(Some(pl_cursor));
                    f.render_stateful_widget(List::new(items).style(Style::default().bg(C_BG)), chunks[3], &mut state);

                    f.render_widget(divider(size.width as usize, div_color), chunks[4]);
                    f.render_widget(render_playlist_statusbar(pl_item_cmd, pl_move_input, &pl_move_buf), chunks[5]);
                }
            }
        })?;

        if let Event::Key(key) = event::read()? {
            match &screen {
                Screen::Dashboard => {
                    let socket = panes[cursor].socket.clone();
                    if cmd_mode {
                        match key.code {
                            KeyCode::Tab        => { cmd_mode = false; }
                            KeyCode::Char(' ')  => { cmd_mpv(&socket, &["cycle", "pause"]); }
                            KeyCode::Char('m')  => { cmd_mpv(&socket, &["cycle", "mute"]); }
                            KeyCode::Char('n')  => { cmd_mpv(&socket, &["playlist-next"]); }
                            KeyCode::Char('N')  => { cmd_mpv(&socket, &["playlist-prev"]); }
                            KeyCode::Char('=')  => { cmd_mpv(&socket, &["add", "volume", "5"]); }
                            KeyCode::Char('-')  => { cmd_mpv(&socket, &["add", "volume", "-5"]); }
                            KeyCode::Right      => { cmd_mpv(&socket, &["seek", "10"]); }
                            KeyCode::Left       => { cmd_mpv(&socket, &["seek", "-10"]); }
                            KeyCode::Up         => { cmd_mpv(&socket, &["seek", "60"]); }
                            KeyCode::Down       => { cmd_mpv(&socket, &["seek", "-60"]); }
                            _ => {}
                        }
                    } else {
                        match key.code {
                            KeyCode::Char('q') => break,
                            KeyCode::Up        => { if cursor > 0 { cursor -= 1; } }
                            KeyCode::Down      => { if cursor < panes.len() - 1 { cursor += 1; } }
                            KeyCode::Tab       => { cmd_mode = true; }
                            KeyCode::Enter     => {
                                pl_cursor    = 0;
                                pl_item_cmd  = false;
                                pl_move_input = false;
                                pl_move_buf.clear();
                                screen = Screen::Playlist(cursor);
                            }
                            _ => {}
                        }
                    }
                }

                Screen::Playlist(idx) => {
                    let idx = *idx;
                    let socket = panes[idx].socket.clone();

                    if pl_item_cmd {
                        if pl_move_input {
                            match key.code {
                                KeyCode::Char(c) if c.is_ascii_digit() => { pl_move_buf.push(c); }
                                KeyCode::Backspace => { pl_move_buf.pop(); }
                                KeyCode::Enter => {
                                    if let Ok(dest) = pl_move_buf.parse::<usize>() {
                                        if !pl_items.is_empty() {
                                            let src = pl_items[pl_cursor].index;
                                            cmd_mpv(&socket, &["playlist-move", &src.to_string(), &dest.to_string()]);
                                        }
                                    }
                                    pl_move_input = false;
                                    pl_move_buf.clear();
                                }
                                KeyCode::Esc => {
                                    pl_move_input = false;
                                    pl_move_buf.clear();
                                }
                                _ => {}
                            }
                        } else {
                            match key.code {
                                KeyCode::Tab   => { pl_item_cmd = false; }
                                KeyCode::Enter => {
                                    if !pl_items.is_empty() {
                                        let item_idx = pl_items[pl_cursor].index;
                                        cmd_mpv(&socket, &["set_property", "playlist-pos", &item_idx.to_string()]);
                                    }
                                    pl_item_cmd = false;
                                }
                                KeyCode::Char('r') => {
                                    if !pl_items.is_empty() {
                                        let item_idx = pl_items[pl_cursor].index;
                                        cmd_mpv(&socket, &["playlist-remove", &item_idx.to_string()]);
                                        if pl_cursor > 0 { pl_cursor -= 1; }
                                    }
                                    pl_item_cmd = false;
                                }
                                KeyCode::Char('m') => {
                                    pl_move_input = true;
                                    pl_move_buf.clear();
                                }
                                _ => {}
                            }
                        }
                    } else {
                        match key.code {
                            KeyCode::Up        => { if pl_cursor > 0 { pl_cursor -= 1; } }
                            KeyCode::Down      => { if pl_cursor < pl_items.len().saturating_sub(1) { pl_cursor += 1; } }
                            KeyCode::Tab       => { pl_item_cmd = true; }
                            KeyCode::Char('c') => { cmd_mpv(&socket, &["playlist-clear"]); }
                            KeyCode::Char('n') => { /* add item input — TBD */ }
                            KeyCode::Backspace => {
                                screen = Screen::Dashboard;
                                pl_item_cmd   = false;
                                pl_move_input = false;
                                pl_move_buf.clear();
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
