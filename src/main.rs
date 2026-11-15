mod app;
mod command;
mod details;
mod search;
mod service;
mod ui;

use std::io;

use app::{App, Effect};
use crossterm::{
    event::{Event, EventStream},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures_util::StreamExt;
use mpv_targets::RemoteClient;
use ratatui::{Terminal, backend::CrosstermBackend};
use service::{
    ServiceUpdate, load_channels, load_endpoints, load_playlist, run_connection,
    run_dashboard_action, run_details_action,
};
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl TerminalSession {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Self { terminal })
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
        let _ = self.terminal.show_cursor();
    }
}

struct Connection {
    generation: u64,
    cancel: watch::Sender<bool>,
    task: JoinHandle<()>,
    client: Option<RemoteClient>,
}

impl Connection {
    async fn close(self) {
        let _ = self.cancel.send(true);
        if let Some(client) = self.client {
            client.close().await;
        }
        let _ = self.task.await;
    }
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let explicit_url =
        explicit_url().map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    let endpoints = load_endpoints(explicit_url)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let mut app = App::new(endpoints);
    let mut terminal = TerminalSession::enter()?;
    let mut events = EventStream::new();
    let (updates_tx, mut updates_rx) = mpsc::unbounded_channel();
    let mut connection: Option<Connection> = None;
    let mut next_generation = 1_u64;
    let mut redraw = true;

    loop {
        if redraw {
            terminal.terminal.draw(|frame| ui::render(frame, &app))?;
        }
        redraw = true;
        tokio::select! {
            event = events.next() => match event {
                Some(Ok(Event::Key(key))) => {
                    app.details_page = terminal.terminal.size()?.height.saturating_sub(4) as usize;
                    if let Some(effect) = app.handle_key(key) {
                        match effect {
                            Effect::Connect(endpoint) => {
                                if let Some(active) = connection.take() { active.close().await; }
                                let generation = next_generation;
                                next_generation += 1;
                                let (cancel, cancel_rx) = watch::channel(false);
                                let sender = updates_tx.clone();
                                let task = tokio::spawn(run_connection(generation, endpoint, sender, cancel_rx));
                                connection = Some(Connection { generation, cancel, task, client: None });
                            }
                            Effect::CancelConnection => {
                                if let Some(active) = connection.take() { active.close().await; }
                            }
                            Effect::Quit => break,
                            Effect::Run(action) => {
                                if let Some(active) = connection.as_ref()
                                    && let Some(client) = active.client.clone()
                                {
                                    tokio::spawn(run_dashboard_action(
                                        active.generation,
                                        client,
                                        action,
                                        updates_tx.clone(),
                                    ));
                                } else {
                                    app.complete_action(Err("Service is not connected".into()));
                                }
                            }
                            Effect::LoadChannels => {
                                if let Some(active) = connection.as_ref()
                                    && let Some(client) = active.client.clone()
                                {
                                    tokio::spawn(load_channels(
                                        active.generation,
                                        client,
                                        updates_tx.clone(),
                                    ));
                                } else {
                                    app.channels_loaded(Err("Service is not connected".into()));
                                }
                            }
                            Effect::LoadPlaylist { target } => {
                                if let Some(active) = connection.as_ref()
                                    && let Some(client) = active.client.clone() {
                                    tokio::spawn(load_playlist(active.generation, client, target, updates_tx.clone()));
                                } else {
                                    app.playlist_loaded(&target, Err("Service is not connected".into()));
                                }
                            }
                            Effect::RunDetails { target, action } => {
                                if let Some(active) = connection.as_ref()
                                    && let Some(client) = active.client.clone() {
                                    tokio::spawn(run_details_action(active.generation, client, target, action, updates_tx.clone()));
                                } else {
                                    app.details_completed(&target, false, Err("Service is not connected".into()));
                                }
                            }
                        }
                    }
                }
                Some(Ok(Event::Resize(_, _))) => terminal.terminal.clear()?,
                Some(Ok(Event::Paste(value))) => app.handle_paste(value),
                Some(Ok(_)) => redraw = false,
                Some(Err(error)) => return Err(error),
                None => break,
            },
            update = updates_rx.recv() => if let Some(update) = update {
                let active_generation = connection.as_ref().map(|active| active.generation);
                match update {
                    ServiceUpdate::Connected { generation, endpoint, client, snapshot }
                        if active_generation == Some(generation) => {
                            if let Some(active) = connection.as_mut() { active.client = Some(client); }
                            app.connected(endpoint, snapshot);
                        }
                    ServiceUpdate::Snapshot { generation, snapshot }
                        if active_generation == Some(generation) => {
                            if let Some(target)=app.apply_snapshot(snapshot)
                                && let Some(client)=connection.as_ref().and_then(|a|a.client.clone()) {
                                tokio::spawn(load_playlist(generation,client,target,updates_tx.clone()));
                            }
                        },
                    ServiceUpdate::Retrying { generation, endpoint, error }
                        if active_generation == Some(generation) => {
                            if let Some(active) = connection.as_mut() { active.client = None; }
                            app.retrying(endpoint, error);
                        }
                    ServiceUpdate::ActionCompleted { generation, result }
                        if active_generation == Some(generation) => app.complete_action(result),
                    ServiceUpdate::ChannelsLoaded { generation, result }
                        if active_generation == Some(generation) => app.channels_loaded(result),
                    ServiceUpdate::PlaylistLoaded { generation, target, result }
                        if active_generation == Some(generation) => {
                            if let Some(target) = app.playlist_loaded(&target, result)
                                && let Some(client) = connection.as_ref().and_then(|a| a.client.clone()) {
                                tokio::spawn(load_playlist(generation, client, target, updates_tx.clone()));
                            }
                        }
                    ServiceUpdate::DetailsCompleted { generation, target, refresh, result }
                        if active_generation == Some(generation) => {
                            if let Some(target) = app.details_completed(&target, refresh, result)
                                && let Some(client) = connection.as_ref().and_then(|a| a.client.clone()) {
                                tokio::spawn(load_playlist(generation, client, target, updates_tx.clone()));
                            }
                        }
                    _ => redraw = false,
                }
            }
        }
    }

    if let Some(active) = connection {
        active.close().await;
    }
    Ok(())
}

fn explicit_url() -> Result<Option<String>, String> {
    let mut args = std::env::args().skip(1);
    match (args.next(), args.next(), args.next()) {
        (None, None, None) => Ok(None),
        (Some(flag), Some(url), None) if flag == "--url" => Ok(Some(url)),
        _ => Err("usage: panebot [--url wss://host:port]".into()),
    }
}
