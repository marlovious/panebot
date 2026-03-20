use crossterm::{
    event::{Event, KeyCode, EventStream},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use futures_util::StreamExt;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, List, ListItem, Paragraph},
    Terminal,
};
use std::io;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

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

// ---------------------------------------------------------------------------
// Startup screen
// ---------------------------------------------------------------------------

struct StartupEntry {
    name:   String,
    status: String,
}

fn divider(width: usize, color: Color) -> Paragraph<'static> {
    Paragraph::new(Span::styled(
        "-".repeat(width),
        Style::default().fg(color),
    ))
}

fn render_startup(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    status:   &str,
    entries:  &[StartupEntry],
    complete: bool,
) -> io::Result<()> {
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
            ])
            .split(size);

        let title_line = if complete {
            Line::from(vec![
                Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
                Span::styled(" :: Startup Complete :: Press ", Style::default().fg(C_HINT)),
                Span::styled("[Enter]", Style::default().fg(C_CYAN)),
            ])
        } else {
            Line::from(vec![
                Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
                Span::styled(" :: ", Style::default().fg(C_DIM)),
                Span::styled(status.to_string(), Style::default().fg(C_HINT)),
                Span::styled(" ::", Style::default().fg(C_DIM)),
            ])
        };
        f.render_widget(Paragraph::new(title_line), chunks[1]);
        f.render_widget(divider(size.width as usize, C_DIVIDER), chunks[2]);

        let max_name = entries.iter().map(|e| e.name.len() + 2).max().unwrap_or(8);
        let items: Vec<ListItem> = entries.iter().map(|e| {
            let status_color = match e.status.as_str() {
                "Online"  => C_GREEN,
                "Offline" => C_RED,
                _         => C_DIM,
            };
            let padded = format!(
                "{:<width$}",
                format!("\"{}\"", e.name.to_uppercase()),
                width = max_name
            );
            ListItem::new(Line::from(vec![
                Span::styled("[PaneBot]", Style::default().fg(C_ORANGE)),
                Span::styled(" :: ",     Style::default().fg(C_DIM)),
                Span::styled(padded,     Style::default().fg(Color::White)),
                Span::styled(" :: ",     Style::default().fg(C_DIM)),
                Span::styled(format!("[{}]", e.status), Style::default().fg(status_color)),
            ]))
        }).collect();

        f.render_widget(List::new(items), chunks[3]);
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Startup sequence
// ---------------------------------------------------------------------------

async fn startup_sequence(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> io::Result<bool> {

    let (ws, _) = loop {
        render_startup(terminal, "Waiting for daemon...", &[], false)?;
        match connect_async("ws://127.0.0.1:9090").await {
            Ok(conn) => break conn,
            Err(_)   => tokio::time::sleep(std::time::Duration::from_millis(500)).await,
        }
    };

    render_startup(terminal, "Connected :: Waiting for bootstrap", &[], false)?;

    let (_, mut ws_rx) = ws.split();

    let entries = loop {
        match ws_rx.next().await {
            Some(Ok(Message::Text(text))) => {
                let v: serde_json::Value = match serde_json::from_str(&text) {
                    Ok(v)  => v,
                    Err(_) => continue,
                };
                if v["event"] == "bootstrap_complete" {
                    let entries = v["panes"]
                        .as_array()
                        .unwrap_or(&vec![])
                        .iter()
                        .map(|p| StartupEntry {
                            name:   p["name"].as_str().unwrap_or("").to_string(),
                            status: p["status"].as_str().unwrap_or("Offline").to_string(),
                        })
                        .collect::<Vec<_>>();
                    break entries;
                }
            }
            Some(Err(_)) | None => {
                render_startup(terminal, "Daemon disconnected", &[], false)?;
                tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                return Ok(false);
            }
            _ => {}
        }
    };

    render_startup(terminal, "", &entries, true)?;

    let mut events = EventStream::new();
    loop {
        match events.next().await {
            Some(Ok(Event::Key(k))) => match k.code {
                KeyCode::Enter | KeyCode::Char(' ') => return Ok(true),
                KeyCode::Char('q')                  => return Ok(false),
                _ => {}
            },
            Some(Err(e)) => return Err(e),
            None         => return Ok(false),
            _            => {}
        }
    }
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

    let proceed = startup_sequence(&mut terminal).await?;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;

    if !proceed { return Ok(()); }

    // Dashboard goes here next

    Ok(())
}
