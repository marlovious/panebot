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

fn mock_panes() -> Vec<Pane> {
    vec![
        Pane { name: "Movies".into(), socket: "~/.config/panebot/movies.sock".into(), pane_type: "Video".into(), status: "Playing".into(), volume: 75, muted: false, title: "The Princess Bride".into() },
        Pane { name: "Music".into(),  socket: "~/.config/panebot/music.sock".into(),  pane_type: "Audio".into(), status: "Stopped".into(), volume: 60, muted: true,  title: "Kind of Blue".into() },
        Pane { name: "Shows".into(),  socket: "~/.config/panebot/shows.sock".into(),  pane_type: "Video".into(), status: "Playing".into(), volume: 40, muted: false, title: "Animaniacs S01E03".into() },
        Pane { name: "Tunes".into(),  socket: "~/.config/panebot/tunes.sock".into(),  pane_type: "Audio".into(), status: "Stopped".into(), volume: 80, muted: true,  title: "\u{2014}".into() },
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

fn render_header<'a>() -> Paragraph<'a> {
    let spans = Line::from(vec![
        Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
        Span::styled("  ::  Active Panes ::", Style::default().fg(C_DIM)),
    ]);
    Paragraph::new(spans).style(Style::default().bg(C_BG))
}

fn render_row(pane: &Pane, selected: bool, cmd_mode: bool) -> ListItem<'static> {
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
        Span::styled(" :: ".to_string(),                               Style::default().fg(C_DIM)),
        Span::styled(format!("[{:<5}]", pane.pane_type),               Style::default().fg(C_CYAN)),
        Span::styled(" :: ".to_string(),                               Style::default().fg(C_DIM)),
        Span::styled(format!("[{:<7}]", pane.status),                  Style::default().fg(status_color)),
        Span::styled(" :: ".to_string(),                               Style::default().fg(C_DIM)),
        Span::styled(format!("{:<10}", vol_str),                       Style::default().fg(vol_color)),
        Span::styled(" :: ".to_string(),                               Style::default().fg(C_DIM)),
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

fn render_statusbar<'a>(cmd_mode: bool) -> Paragraph<'a> {
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

fn divider(width: usize, color: Color) -> Paragraph<'static> {
    Paragraph::new(Span::styled(
        "-".repeat(width),
        Style::default().fg(color),
    )).style(Style::default().bg(C_BG))
}

fn main() -> Result<(), io::Error> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut panes = load_panes();
    let mut cursor: usize = 0;
    let mut cmd_mode = false;

    loop {
        for pane in panes.iter_mut() {
            refresh_pane(pane);
        }

        terminal.draw(|f| {
            let size = f.size();

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1), // blank top padding
                    Constraint::Length(1), // header
                    Constraint::Length(1), // top divider
                    Constraint::Min(1),    // pane list
                    Constraint::Length(1), // bottom divider
                    Constraint::Length(1), // statusbar
                ])
                .split(size);

            let div_color = if cmd_mode { Color::Rgb(90, 55, 10) } else { C_DIVIDER };

            f.render_widget(Block::default().style(Style::default().bg(C_BG)), size);
            f.render_widget(render_header(), chunks[1]);
            f.render_widget(divider(size.width as usize, C_DIVIDER), chunks[2]);

            let items: Vec<ListItem> = panes
                .iter()
                .enumerate()
                .map(|(i, p)| render_row(p, i == cursor, cmd_mode && i == cursor))
                .collect();

            let list = List::new(items).style(Style::default().bg(C_BG));
            let mut state = ListState::default();
            state.select(Some(cursor));
            f.render_stateful_widget(list, chunks[3], &mut state);

            f.render_widget(divider(size.width as usize, div_color), chunks[4]);
            f.render_widget(render_statusbar(cmd_mode), chunks[5]);
        })?;

        if let Event::Key(key) = event::read()? {
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
                    _ => {}
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}
