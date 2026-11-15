use std::{env, fs, net::SocketAddr, path::PathBuf, time::Duration};

use mpv_targets::{ClientOptions, DaemonConfig, DisconnectReason, RemoteClient, Snapshot};
use serde::Deserialize;
use tokio::sync::{mpsc::UnboundedSender, watch};

use crate::app::{CreateMode, DashboardAction, LifecycleAction};
use crate::{command, details};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Endpoint {
    pub label: String,
    pub url: String,
}

impl Endpoint {
    pub fn new(label: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            url: url.into(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct NodeBook {
    #[serde(default)]
    nodes: Vec<NodeEntry>,
}

#[derive(Debug, Deserialize)]
struct NodeEntry {
    id: String,
    url: String,
}

#[derive(Clone)]
pub enum ServiceUpdate {
    Connected {
        generation: u64,
        endpoint: Endpoint,
        client: RemoteClient,
        snapshot: Snapshot,
    },
    Snapshot {
        generation: u64,
        snapshot: Snapshot,
    },
    Retrying {
        generation: u64,
        endpoint: Endpoint,
        error: String,
    },
    ActionCompleted {
        generation: u64,
        result: Result<String, String>,
    },
    ChannelsLoaded {
        generation: u64,
        result: Result<Vec<String>, String>,
    },
    PlaylistLoaded {
        generation: u64,
        target: String,
        result: Result<Vec<mpv_targets::PlaylistItem>, String>,
    },
    DetailsCompleted {
        generation: u64,
        target: String,
        refresh: bool,
        result: Result<String, String>,
    },
}

pub async fn load_playlist(
    generation: u64,
    client: RemoteClient,
    target: String,
    updates: UnboundedSender<ServiceUpdate>,
) {
    let result = client
        .target(target.clone())
        .playlist()
        .await
        .map_err(|e| e.to_string());
    let _ = updates.send(ServiceUpdate::PlaylistLoaded {
        generation,
        target,
        result,
    });
}

pub async fn run_details_action(
    generation: u64,
    client: RemoteClient,
    target: String,
    action: details::Action,
    updates: UnboundedSender<ServiceUpdate>,
) {
    let refresh = !matches!(
        action,
        details::Action::Loop(_) | details::Action::Repeat(_) | details::Action::Save { .. }
    );
    let result = execute_details_action(&client, &target, action).await;
    let _ = updates.send(ServiceUpdate::DetailsCompleted {
        generation,
        target,
        refresh,
        result,
    });
}

async fn execute_details_action(
    client: &RemoteClient,
    target: &str,
    action: details::Action,
) -> Result<String, String> {
    use details::Action;
    use mpv_targets::{LoadMode, LoopState};
    let t = client.target(target.to_owned());
    let message =
        match action {
            Action::Play(i) => {
                t.playlist_play(i as u32).await.map_err(|e| e.to_string())?;
                t.play().await.map_err(|e| e.to_string())?;
                "Playing now".into()
            }
            Action::MoveNext(i) => {
                let items = t.playlist().await.map_err(|e| e.to_string())?;
                let current = items.iter().position(|x| x.current);
                let destination = current.map_or(0, |c| c + 1);
                move_block(&t, items, vec![i], destination).await?;
                if current.is_none() {
                    "Moved to front".into()
                } else {
                    "Moved to next".into()
                }
            }
            Action::Move {
                indices,
                destination,
            } => {
                let items = t.playlist().await.map_err(|e| e.to_string())?;
                move_block(&t, items, indices, destination).await?;
                "Queue reordered".into()
            }
            Action::Remove {
                mut indices,
                kept_playing,
            } => {
                let n = indices.len();
                indices.sort_unstable_by(|a, b| b.cmp(a));
                for i in indices {
                    t.playlist_remove(i as u32)
                        .await
                        .map_err(|e| e.to_string())?;
                }
                if kept_playing {
                    format!("Removed {n}; kept playing item")
                } else {
                    format!("Removed {n}")
                }
            }
            Action::CropBefore(current) => {
                for i in (0..current).rev() {
                    t.playlist_remove(i as u32)
                        .await
                        .map_err(|e| e.to_string())?;
                }
                format!("Removed {current}")
            }
            Action::CropAfter(current) => {
                let count = t.playlist().await.map_err(|e| e.to_string())?.len();
                let n = count.saturating_sub(current + 1);
                for i in ((current + 1)..count).rev() {
                    t.playlist_remove(i as u32)
                        .await
                        .map_err(|e| e.to_string())?;
                }
                format!("Removed {n}")
            }
            Action::Shuffle => {
                t.shuffle().await.map_err(|e| e.to_string())?;
                "Queue shuffled".into()
            }
            Action::Load { source, replace } => {
                t.load(
                    source,
                    if replace {
                        LoadMode::Replace
                    } else {
                        LoadMode::Append
                    },
                )
                .await
                .map_err(|e| e.to_string())?;
                if replace {
                    "Queue replaced".into()
                } else {
                    "Added to queue".into()
                }
            }
            Action::Save { channel, overwrite } => {
                if !overwrite {
                    let catalog = client.list_channels().await.map_err(|e| e.to_string())?;
                    if catalog.channels.iter().any(|c| {
                        mpv_targets::channel_display_name(c).eq_ignore_ascii_case(&channel)
                    }) {
                        return Err(format!("__OVERWRITE__{channel}"));
                    }
                }
                let saved = t
                    .save_channel(channel, overwrite)
                    .await
                    .map_err(|e| e.to_string())?;
                format!("Saved {} items to {}", saved.items, saved.channel)
            }
            Action::Loop(on) => {
                t.set_loop(if on { LoopState::On } else { LoopState::Off })
                    .await
                    .map_err(|e| e.to_string())?;
                format!("Item Loop {}", if on { "On" } else { "Off" })
            }
            Action::Repeat(on) => {
                t.set_repeat(if on { LoopState::On } else { LoopState::Off })
                    .await
                    .map_err(|e| e.to_string())?;
                format!("Queue Repeat {}", if on { "On" } else { "Off" })
            }
        };
    Ok(message)
}

async fn move_block(
    t: &mpv_targets::TargetClient,
    items: Vec<mpv_targets::PlaylistItem>,
    indices: Vec<usize>,
    destination: usize,
) -> Result<(), String> {
    let ids: Vec<u64> = indices
        .iter()
        .filter_map(|i| items.get(*i).map(|x| x.id))
        .collect();
    let mut desired: Vec<u64> = items
        .iter()
        .map(|x| x.id)
        .filter(|id| !ids.contains(id))
        .collect();
    let at = if destination == items.len() {
        desired.len()
    } else {
        let dest_id = items
            .get(destination)
            .map(|x| x.id)
            .ok_or("Invalid move destination")?;
        desired
            .iter()
            .position(|id| *id == dest_id)
            .ok_or("Invalid move destination")?
    };
    for (id_offset, id) in ids.into_iter().enumerate() {
        desired.insert(at + id_offset, id)
    }
    let mut live: Vec<u64> = items.iter().map(|x| x.id).collect();
    for wanted in 0..desired.len() {
        if live[wanted] == desired[wanted] {
            continue;
        }
        let from = live.iter().position(|id| *id == desired[wanted]).unwrap();
        t.playlist_move(from as u32, wanted as u32)
            .await
            .map_err(|e| e.to_string())?;
        let id = live.remove(from);
        live.insert(wanted, id);
    }
    Ok(())
}

pub async fn load_channels(
    generation: u64,
    client: RemoteClient,
    updates: UnboundedSender<ServiceUpdate>,
) {
    let result = client
        .list_channels()
        .await
        .map(|catalog| catalog.channels)
        .map_err(|error| error.to_string());
    let _ = updates.send(ServiceUpdate::ChannelsLoaded { generation, result });
}

pub async fn run_dashboard_action(
    generation: u64,
    client: RemoteClient,
    action: DashboardAction,
    updates: UnboundedSender<ServiceUpdate>,
) {
    let result = execute_dashboard_action(&client, action)
        .await
        .map_err(|error| error.to_string());
    let _ = updates.send(ServiceUpdate::ActionCompleted { generation, result });
}

async fn execute_dashboard_action(
    client: &RemoteClient,
    action: DashboardAction,
) -> Result<String, String> {
    match action {
        DashboardAction::Identify => {
            let snapshot = client.snapshot().await.ok_or("Service is not connected")?;
            let mut count = 0;
            for target in snapshot.targets.iter().filter(|target| target.online) {
                client
                    .target(&target.name)
                    .show_text(
                        format!(r"${{osd-ass-cc/0}}{{\\fs50}}{}", target.name.to_uppercase()),
                        4000,
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                count += 1;
            }
            Ok(format!("Identified {count} targets"))
        }
        DashboardAction::Lifecycle { target, action } => {
            let target_client = client.target(target.clone());
            match action {
                LifecycleAction::Enable => {
                    target_client.enable().await.map_err(|e| e.to_string())?;
                }
                LifecycleAction::EnableAndStart => {
                    target_client.enable().await.map_err(|e| e.to_string())?;
                    target_client.start().await.map_err(|e| e.to_string())?;
                }
                LifecycleAction::Start => {
                    target_client.start().await.map_err(|e| e.to_string())?;
                }
                LifecycleAction::Restart => {
                    target_client.restart().await.map_err(|e| e.to_string())?;
                }
                LifecycleAction::Stop => {
                    target_client
                        .stop_target()
                        .await
                        .map_err(|e| e.to_string())?;
                }
                LifecycleAction::Disable => {
                    target_client.disable().await.map_err(|e| e.to_string())?;
                }
            }
            Ok(format!("{} {}", action.label(), target.to_uppercase()))
        }
        DashboardAction::Solo { selected, others } => {
            let selected_client = client.target(selected.clone());
            selected_client.unmute().await.map_err(|e| e.to_string())?;
            selected_client.play().await.map_err(|e| e.to_string())?;
            selected_client
                .set_fullscreen(true)
                .await
                .map_err(|e| e.to_string())?;
            for target in others {
                client
                    .target(target)
                    .mute()
                    .await
                    .map_err(|e| e.to_string())?;
            }
            Ok(format!("Solo {}", selected.to_uppercase()))
        }
        DashboardAction::ToggleAll { targets, play } => {
            for target in targets {
                if play {
                    client.target(target).play().await
                } else {
                    client.target(target).pause().await
                }
                .map_err(|e| e.to_string())?;
            }
            Ok(format!(
                "{} all targets",
                if play { "Played" } else { "Paused" }
            ))
        }
        DashboardAction::Command { target, action } => {
            let target_client = client.target(target.clone());
            match action {
                command::Action::TogglePlay => target_client.toggle_play().await,
                command::Action::ToggleMute => target_client.toggle_mute().await,
                command::Action::CycleAudio => target_client.cycle_audio().await,
                command::Action::CycleSubtitle => target_client.cycle_subtitle().await,
                command::Action::Keypress(key) => target_client.keypress(key).await,
                command::Action::ToggleFullscreen => target_client.fullscreen().await,
                command::Action::Seek(seconds) => target_client.seek(seconds).await,
                command::Action::AdjustVolume(delta) => target_client.adjust_volume(delta).await,
            }
            .map_err(|error| error.to_string())?;
            Ok(format!("Command {}", target.to_uppercase()))
        }
        DashboardAction::Rename {
            target,
            replacement,
        } => {
            let renamed = client
                .target(target)
                .rename(replacement)
                .await
                .map_err(|error| error.to_string())?;
            Ok(format!(
                "Renamed {} to {}",
                renamed.from.to_uppercase(),
                renamed.to.to_uppercase()
            ))
        }
        DashboardAction::Remove { target } => {
            let removed = client
                .target(target)
                .remove_target()
                .await
                .map_err(|error| error.to_string())?;
            Ok(format!("Removed {}", removed.removed.to_uppercase()))
        }
        DashboardAction::Duplicate {
            source,
            target,
            mode,
        } => {
            let target_client = client.target(target.clone());
            target_client
                .add(Some(source.clone()), mode != CreateMode::Create)
                .await
                .map_err(|error| error.to_string())?;
            if mode == CreateMode::Start {
                target_client
                    .start()
                    .await
                    .map_err(|error| error.to_string())?;
            }
            Ok(format!(
                "{} {} from {}",
                mode.label(),
                target.to_uppercase(),
                source.to_uppercase()
            ))
        }
        DashboardAction::Create {
            target,
            channel,
            play_on_start,
            mute_on_start,
            mode,
        } => {
            let target_client = client.target(target.clone());
            target_client
                .add_base(mode != CreateMode::Create, !play_on_start, mute_on_start)
                .await
                .map_err(|error| error.to_string())?;
            if let Some(channel) = channel {
                target_client
                    .set_channel(Some(channel), false)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            if mode == CreateMode::Start {
                target_client
                    .start()
                    .await
                    .map_err(|error| error.to_string())?;
            }
            Ok(format!("{} {}", mode.label(), target.to_uppercase()))
        }
        DashboardAction::SetChannel {
            target,
            channel,
            restart,
        } => {
            client
                .target(target.clone())
                .set_channel(Some(channel.clone()), restart)
                .await
                .map_err(|error| error.to_string())?;
            Ok(format!(
                "Set channel {} to {}",
                target.to_uppercase(),
                mpv_targets::channel_display_name(&channel)
            ))
        }
    }
}

pub fn load_endpoints(explicit_url: Option<String>) -> Result<Vec<Endpoint>, String> {
    let root = config_root()?;
    let config_path = root.join("mpv-targets.toml");
    let mut endpoints = Vec::new();
    if config_path.exists() {
        let source = fs::read_to_string(&config_path).map_err(|error| {
            format!(
                "cannot read local service configuration {}: {error}",
                config_path.display()
            )
        })?;
        let config = DaemonConfig::parse(&source).map_err(|error| {
            format!(
                "invalid local service configuration {}: {error}",
                config_path.display()
            )
        })?;
        endpoints.push(Endpoint::new("local", daemon_url(&config)?));
    }
    let node_book_path = root.join("nodes.toml");
    if node_book_path.exists() {
        let source = fs::read_to_string(&node_book_path).map_err(|error| {
            format!(
                "cannot read node book {}: {error}",
                node_book_path.display()
            )
        })?;
        let book: NodeBook = toml::from_str(&source)
            .map_err(|error| format!("invalid node book {}: {error}", node_book_path.display()))?;
        endpoints.extend(
            book.nodes
                .into_iter()
                .map(|node| Endpoint::new(node.id, node.url)),
        );
    }

    if let Some(url) = explicit_url
        && !endpoints.iter().any(|endpoint| endpoint.url == url)
    {
        endpoints.push(Endpoint::new("direct", url));
    }
    if endpoints.is_empty() {
        return Err(format!(
            "no service endpoints configured in {} and no --url was supplied",
            root.display()
        ));
    }
    Ok(endpoints)
}

pub async fn run_connection(
    generation: u64,
    endpoint: Endpoint,
    updates: UnboundedSender<ServiceUpdate>,
    mut cancel: watch::Receiver<bool>,
) {
    loop {
        let connect = RemoteClient::connect(ClientOptions::new(endpoint.url.clone()));
        let client = tokio::select! {
            _ = cancelled(&mut cancel) => return,
            result = connect => match result {
                Ok(client) => client,
                Err(error) => {
                    if updates.send(ServiceUpdate::Retrying {
                        generation,
                        endpoint: endpoint.clone(),
                        error: error.to_string(),
                    }).is_err() {
                        return;
                    }
                    if retry_delay(&mut cancel).await { return; }
                    continue;
                }
            }
        };

        let snapshot = client
            .snapshot()
            .await
            .expect("RemoteClient::connect guarantees an initial snapshot");

        if updates
            .send(ServiceUpdate::Connected {
                generation,
                endpoint: endpoint.clone(),
                client: client.clone(),
                snapshot,
            })
            .is_err()
        {
            client.close().await;
            return;
        }

        let mut snapshots = client.subscribe_snapshots();
        let disconnect = loop {
            tokio::select! {
                _ = cancelled(&mut cancel) => {
                    client.close().await;
                    return;
                }
                changed = snapshots.changed() => {
                    if changed.is_err() { break DisconnectReason::Closed; }
                    let snapshot = snapshots.borrow().clone();
                    if snapshot.is_some_and(|snapshot| {
                        updates.send(ServiceUpdate::Snapshot { generation, snapshot }).is_err()
                    }) {
                        client.close().await;
                        return;
                    }
                }
                reason = client.wait_disconnected() => break reason,
            }
        };

        let error = match disconnect {
            DisconnectReason::Closed => "service closed the connection".to_owned(),
            DisconnectReason::Failed(error) => error.to_string(),
        };
        if updates
            .send(ServiceUpdate::Retrying {
                generation,
                endpoint: endpoint.clone(),
                error,
            })
            .is_err()
        {
            return;
        }
        if retry_delay(&mut cancel).await {
            return;
        }
    }
}

async fn cancelled(cancel: &mut watch::Receiver<bool>) {
    loop {
        if *cancel.borrow() || cancel.changed().await.is_err() {
            return;
        }
    }
}

async fn retry_delay(cancel: &mut watch::Receiver<bool>) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(2)) => false,
        _ = cancelled(cancel) => true,
    }
}

fn config_root() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(path).join("mpv-targets"));
    }
    let home = env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join(".config/mpv-targets"))
}

fn daemon_url(config: &DaemonConfig) -> Result<String, String> {
    let mut address: SocketAddr = config
        .node
        .listen
        .parse()
        .map_err(|_| "local service has an invalid listen address".to_owned())?;
    if address.ip().is_unspecified() {
        address.set_ip(if address.is_ipv4() {
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
        } else {
            std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
        });
    }
    Ok(format!("wss://{address}"))
}
