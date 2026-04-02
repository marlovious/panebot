use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, UnixStream};
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;
use futures_util::{SinkExt, StreamExt};

use panebot_lib::{
    load_config, load_hosts,
    config_dir, layouts_dir, panes_conf, hosts_conf,
    pane_dir, pane_mpv_conf, pane_playlist, pane_scripts, pane_socket,
    Pane,
};

// ---------------------------------------------------------------------------
// Timing constants
// ---------------------------------------------------------------------------

const MONITOR_RETRY_MS:     u64 = 500;   // delay between mpv socket connect attempts
const RESTART_PANE_WAIT_MS: u64 = 3000;  // wait after kill before relaunch
const RESTART_ALL_WAIT_MS:  u64 = 1000;  // wait after killing all before relaunch

// ---------------------------------------------------------------------------
// Default file contents
// ---------------------------------------------------------------------------

const DEFAULT_PANES_CONF: &str = "\
# pb.panes.conf
# [instance_name]                   — drives directory, socket, mpv.conf. set once, never change.
# pane_name = My Pane               — display name in TUI and mpv window title. change freely.
# playlist  = /path/to/file.m3u    — optional. point mpv at an external playlist, directory,
#                                     or stream URL. if omitted, panebot uses the pane's own
#                                     .m3u file in ~/.config/panebot/{instance_name}/.

layout = pb.left.stack

[music]
pane_name = Music

[wide-top]
pane_name = Wide Top

[wide-bottom]
pane_name = Wide Bottom

[standard]
pane_name = Standard
";

const DEFAULT_HOSTS_CONF: &str = "\
# pb.daemon.conf
# mode = local    — bind to 127.0.0.1 only (default, safe)
# mode = remote   — bind to 0.0.0.0, accepts LAN connections
#
# Add remote panebot nodes here.
#
# [my-linux-box]
# address = ws://192.168.1.x:9090

mode = local
";

// ---------------------------------------------------------------------------
// Layout files
//
// Layout files live in layouts_dir() as .layout files.
// Each [pane_name] section has a geometry = WxH+X+Y key.
// On macOS, geometry is passed as --geometry to mpv at launch.
// On Linux, window placement is left to the window manager.
// panebot sets --title=PANE_NAME so the WM can match and place windows.
// ---------------------------------------------------------------------------

const LAYOUT_LEFT_STACK: &str = "\
# panebot layout — pb.left.stack

[music]
geometry = 366x366+0+0

[wide-top]
geometry = 650x366+0+374

[wide-bottom]
geometry = 650x366+0+748

[standard]
geometry = 650x488+0+1122
";

const LAYOUT_RIGHT_STACK: &str = "\
# panebot layout — pb.right.stack

[music]
geometry = 366x366+2574+64

[wide-top]
geometry = 650x366+2290+432

[wide-bottom]
geometry = 650x366+2290+804

[standard]
geometry = 650x488+2290+1180
";

// ---------------------------------------------------------------------------
// Logger
// ---------------------------------------------------------------------------

struct Logger {
    file: std::fs::File,
}

impl Logger {
    fn open() -> std::io::Result<Self> {
        let path = config_dir().join("panebot-daemon.log");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        Ok(Logger { file })
    }

    fn log(&mut self, msg: &str) {
        use std::time::{SystemTime, UNIX_EPOCH};
        let secs  = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let h     = (secs % 86400) / 3600;
        let m     = (secs % 3600) / 60;
        let s     = secs % 60;
        let host: String = hostname().chars().take(10).collect();
        let line  = format!("[{}] [{:02}:{:02}:{:02}] {}\n", host, h, m, s, msg);
        let _     = self.file.write_all(line.as_bytes());
        print!("{}", line);
    }
}

// ---------------------------------------------------------------------------
// Bootstrap
//
// Creates config skeleton if missing. Types are gone — the mpv.conf stub is
// written once with panebot-required options. Users edit freely.
// Returns the loaded Config so main() doesn't need to call load_config() again.
// ---------------------------------------------------------------------------

fn write_if_missing(path: &std::path::PathBuf, content: &str) -> std::io::Result<()> {
    if !path.exists() {
        std::fs::File::create(path)?.write_all(content.as_bytes())?;
    }
    Ok(())
}

fn bootstrap(log: &mut Logger) -> std::io::Result<panebot_lib::Config> {
    log.log("bootstrap :: checking environment");

    std::fs::create_dir_all(config_dir())?;
    std::fs::create_dir_all(layouts_dir())?;

    let fresh = !panes_conf().exists();

    write_if_missing(&panes_conf(),  DEFAULT_PANES_CONF)?;
    write_if_missing(&hosts_conf(),  DEFAULT_HOSTS_CONF)?;
    write_if_missing(&layouts_dir().join("pb.left.stack.layout"),  LAYOUT_LEFT_STACK)?;
    write_if_missing(&layouts_dir().join("pb.right.stack.layout"), LAYOUT_RIGHT_STACK)?;

    // Per-pane setup — dirs, mpv.conf stub, empty playlist.
    // load_config() called once here after defaults are written.
    let cfg = load_config();
    for pane in &cfg.panes {
        std::fs::create_dir_all(pane_dir(&pane.mpv_name))?;
        std::fs::create_dir_all(pane_scripts(&pane.mpv_name))?;

        let mpv_conf = pane_mpv_conf(&pane.mpv_name);
        if !mpv_conf.exists() {
            let mut f = std::fs::File::create(&mpv_conf)?;
            writeln!(f, "# panebot mpv config — {}", pane.mpv_name)?;
            writeln!(f, "# Edit this file to tune mpv for this pane.")?;
            writeln!(f, "# Required options — panebot depends on these.")?;
            writeln!(f, "force-window=yes")?;
            writeln!(f, "idle=yes")?;
            writeln!(f, "really-quiet=yes")?;
            writeln!(f, "pause=yes")?;
            writeln!(f, "volume=100")?;
            writeln!(f, "mute=yes")?;
            writeln!(f, "directory-mode=recursive")?;
            writeln!(f, "scripts-dir={}", pane_scripts(&pane.mpv_name).to_string_lossy())?;
            writeln!(f)?;
            writeln!(f, "# Add your own mpv options below.")?;
            log.log(&format!("bootstrap :: {} :: created mpv.conf", pane.mpv_name));
        }

        let pl = pane_playlist(&pane.mpv_name);
        if !pl.exists() {
            std::fs::write(&pl, "#EXTM3U\n")?;
            log.log(&format!("bootstrap :: {} :: created playlist", pane.mpv_name));
        }
    }

    if fresh {
        log.log("bootstrap :: created default config — edit ~/.config/panebot/pb.panes.conf");
    } else {
        log.log("bootstrap :: config exists");
    }

    Ok(cfg)
}

// ---------------------------------------------------------------------------
// Layout parser
//
// Returns a map of pane name -> LayoutEntry.
// Each entry carries whichever keys were present in the layout file.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct LayoutEntry {
    geometry: Option<String>,
}

fn parse_layout(content: &str) -> HashMap<String, LayoutEntry> {
    let mut map     = HashMap::new();
    let mut current = String::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }

        if line.starts_with('[') && line.ends_with(']') {
            current = line[1..line.len()-1].to_string();
            map.entry(current.clone()).or_insert_with(LayoutEntry::default);
            continue;
        }

        if !current.is_empty() {
            if let Some(eq) = line.find('=') {
                let key   = line[..eq].trim();
                let val   = line[eq+1..].trim().to_string();
                let entry = map.entry(current.clone()).or_insert_with(LayoutEntry::default);
                match key {
                    "geometry" => entry.geometry = Some(val),
                    _          => {}
                }
            }
        }
    }

    map
}

fn load_layout(name: &str, log: &mut Logger) -> Option<HashMap<String, LayoutEntry>> {
    let path = layouts_dir().join(format!("{}.layout", name));
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let map = parse_layout(&content);
            log.log(&format!("layout :: loaded {} ({} entries)", name, map.len()));
            Some(map)
        }
        Err(e) => {
            log.log(&format!("layout :: failed to load {}: {}", name, e));
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Launch / kill mpv
//
// volume and mute live in mpv.conf — not passed as launch args.
// geometry: passed as --geometry=WxH+X+Y on macOS only.
// On Linux, panebot sets --title= and hands off to the window manager.
// ---------------------------------------------------------------------------

fn launch_pane(pane: &Pane, geometry: Option<&str>, log: &mut Logger) {
    let socket   = pane_socket(&pane.mpv_name);
    let mpv_conf = pane_mpv_conf(&pane.mpv_name);
    let playlist = pane.playlist.as_deref()
        .map(|s| s.to_string())
        .unwrap_or_else(|| pane_playlist(&pane.mpv_name).to_string_lossy().to_string());

    let mut args: Vec<String> = vec![
        format!("--input-ipc-server={}", socket.to_string_lossy()),
        format!("--title={}", pane.pane_name.to_uppercase()),
        format!("--include={}", mpv_conf.to_string_lossy()),
        format!("--playlist={}", playlist),
    ];

    #[cfg(target_os = "macos")]
    if let Some(geo) = geometry {
        args.push(format!("--geometry={}", geo));
        log.log(&format!("launch :: {} :: geometry={}", pane.mpv_name, geo));
    }

    #[cfg(not(target_os = "macos"))]
    let _ = geometry;

    let mut cmd = std::process::Command::new("mpv");
    cmd.args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    match cmd.spawn() {
        Ok(_)  => log.log(&format!("launch :: {} :: mpv spawned", pane.mpv_name)),
        Err(e) => log.log(&format!("launch :: {} :: failed to spawn mpv: {}", pane.mpv_name, e)),
    }
}

async fn kill_pane_async(mpv_name: &str, log: &mut Logger) {
    let socket = pane_socket(mpv_name).to_string_lossy().to_string();
    match UnixStream::connect(&socket).await {
        Ok(mut stream) => {
            let cmd = serde_json::json!({"command":["quit"]}).to_string() + "\n";
            let _ = stream.write_all(cmd.as_bytes()).await;
            log.log(&format!("kill :: {} :: quit sent", mpv_name));
        }
        Err(_) => {
            log.log(&format!("kill :: {} :: socket not found, already dead", mpv_name));
        }
    }
}

// ---------------------------------------------------------------------------
// Pane state
//
// Properties are Option — unknown until mpv reports them via observe_property.
// Avoids showing wrong state in the window before the first push arrives.
// online is bool — set directly by the daemon, not derived from mpv.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct PaneState {
    pub mpv_name:     String,
    pub pane_name:    String,
    pub online:       bool,
    pub idle_active:  Option<bool>,
    pub paused:       Option<bool>,
    pub muted:        Option<bool>,
    pub volume:       Option<f64>,
    pub title:        Option<String>,
    pub playlist_pos: Option<i64>,
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
}

type SharedState  = Arc<Mutex<HashMap<String, PaneState>>>;
type PaneCommands = Arc<Mutex<HashMap<String, mpsc::Sender<String>>>>;
type SharedPanes  = Arc<Vec<Pane>>;
type SharedLayout = Arc<Mutex<String>>;
type ShutdownTx   = Arc<tokio::sync::Notify>;

// ---------------------------------------------------------------------------
// mpv IPC — async per-pane monitor
// ---------------------------------------------------------------------------

async fn monitor_pane(
    pane:  Pane,
    state: SharedState,
    tx:    broadcast::Sender<String>,
    cmds:  PaneCommands,
) {
    let socket_path = pane_socket(&pane.mpv_name).to_string_lossy().to_string();

    loop {
        // Try to connect. If it fails, mark offline and wait before retrying.
        let stream = match UnixStream::connect(&socket_path).await {
            Ok(s) => s,
            Err(_) => {
                {
                    let mut s = state.lock().unwrap();
                    let ps = s.entry(pane.mpv_name.clone())
                        .or_insert_with(|| PaneState::new(&pane.mpv_name, &pane.pane_name));
                    if ps.online {
                        ps.online = false;
                        let _ = tx.send(serde_json::json!({
                            "pane":  pane.mpv_name,
                            "event": "offline"
                        }).to_string());
                    }
                }
                tokio::time::sleep(Duration::from_millis(MONITOR_RETRY_MS)).await;
                continue;
            }
        };

        // Fresh channel on each reconnect — fixes consumed cmd_rx bug
        let (cmd_tx, cmd_rx) = mpsc::channel::<String>(32);
        cmds.lock().unwrap().insert(pane.mpv_name.clone(), cmd_tx);

        let (read_half, mut write_half) = tokio::io::split(stream);
        let mut reader = BufReader::new(read_half);

        let subs = [
            serde_json::json!({"command":["observe_property",1,"pause"]}),
            serde_json::json!({"command":["observe_property",2,"volume"]}),
            serde_json::json!({"command":["observe_property",3,"media-title"]}),
            serde_json::json!({"command":["observe_property",4,"playlist-pos"]}),
            serde_json::json!({"command":["observe_property",5,"mute"]}),
            serde_json::json!({"command":["observe_property",6,"idle-active"]}),
        ];

        let mut sub_ok = true;
        for sub in &subs {
            let mut line = sub.to_string();
            line.push('\n');
            if write_half.write_all(line.as_bytes()).await.is_err() {
                sub_ok = false;
                break;
            }
        }
        if !sub_ok {
            tokio::time::sleep(Duration::from_millis(MONITOR_RETRY_MS)).await;
            continue;
        }

        {
            let mut s = state.lock().unwrap();
            let ps = s.entry(pane.mpv_name.clone())
                .or_insert_with(|| PaneState::new(&pane.mpv_name, &pane.pane_name));
            ps.online = true;
            let _ = tx.send(serde_json::json!({
                "pane":  pane.mpv_name,
                "event": "online",
                "state": &*ps,
            }).to_string());
        }

        let mpv_name          = pane.mpv_name.clone();
        let pane_display_name = pane.pane_name.clone();
        let state_r           = state.clone();
        let tx_r              = tx.clone();

        let read_task = tokio::spawn(async move {
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }

                let v: serde_json::Value = match serde_json::from_str(line.trim()) {
                    Ok(v)  => v,
                    Err(_) => continue,
                };

                if v["event"] == "property-change" {
                    let prop  = v["name"].as_str().unwrap_or("").to_string();
                    let mut s = state_r.lock().unwrap();
                    let ps    = s.entry(mpv_name.clone())
                        .or_insert_with(|| PaneState::new(&mpv_name, &pane_display_name));

                    match prop.as_str() {
                        "pause"        => { ps.paused       = v["data"].as_bool(); }
                        "volume"       => { ps.volume       = v["data"].as_f64(); }
                        "media-title"  => { ps.title        = v["data"].as_str().map(|s| s.to_string()); }
                        "playlist-pos" => { ps.playlist_pos = v["data"].as_i64(); }
                        "mute"         => { ps.muted        = v["data"].as_bool(); }
                        "idle-active"  => { ps.idle_active  = v["data"].as_bool(); }
                        _ => continue,
                    }

                    let _ = tx_r.send(serde_json::json!({
                        "pane":     mpv_name,
                        "event":    "property-change",
                        "property": prop,
                        "value":    v["data"],
                    }).to_string());
                } else if v["error"] == "success" && v["data"].is_array() {
                    // Response to get_property playlist — broadcast as node:playlist
                    let _ = tx_r.send(serde_json::json!({
                        "event": "node:playlist",
                        "pane":  mpv_name,
                        "items": v["data"],
                    }).to_string());
                }
            }
        });

        let write_task = tokio::spawn(async move {
            let mut cmd_rx = cmd_rx;
            while let Some(mut cmd) = cmd_rx.recv().await {
                cmd.push('\n');
                if write_half.write_all(cmd.as_bytes()).await.is_err() {
                    break;
                }
            }
        });

        tokio::select! {
            _ = read_task  => {}
            _ = write_task => {}
        }

        {
            let mut s = state.lock().unwrap();
            if let Some(ps) = s.get_mut(&pane.mpv_name) {
                ps.online = false;
            }
            let _ = tx.send(serde_json::json!({
                "pane":  pane.mpv_name,
                "event": "offline"
            }).to_string());
        }

        tokio::time::sleep(Duration::from_millis(MONITOR_RETRY_MS)).await;
    }
}

// ---------------------------------------------------------------------------
// WebSocket handler
// ---------------------------------------------------------------------------

async fn handle_ws(
    stream:   tokio::net::TcpStream,
    state:    SharedState,
    tx:       broadcast::Sender<String>,
    snapshot: String,
    cmds:     PaneCommands,
    panes:    SharedPanes,
    shutdown: ShutdownTx,
    layout:   SharedLayout,
) {
    let ws = match accept_async(stream).await {
        Ok(ws) => ws,
        Err(_) => return,
    };

    let (mut sink, mut source) = ws.split();
    let mut rx = tx.subscribe();

    let _ = sink.send(Message::Text(snapshot)).await;

    let state_msgs: Vec<String> = {
        let s = state.lock().unwrap();
        s.values().map(|ps| serde_json::json!({
            "pane":  ps.mpv_name,
            "event": if ps.online { "online" } else { "offline" },
            "state": ps,
        }).to_string()).collect()
    };
    for msg in state_msgs {
        let _ = sink.send(Message::Text(msg)).await;
    }

    loop {
        tokio::select! {
            msg = rx.recv() => {
                match msg {
                    Ok(m)  => { let _ = sink.send(Message::Text(m)).await; }
                    Err(_) => break,
                }
            }
            msg = source.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        handle_command(&text, &state, &cmds, &panes, &tx, &shutdown, &layout).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Command handler
// ---------------------------------------------------------------------------

async fn handle_command(
    text:     &str,
    state:    &SharedState,
    cmds:     &PaneCommands,
    panes:    &SharedPanes,
    tx:       &broadcast::Sender<String>,
    shutdown: &ShutdownTx,
    layout:   &SharedLayout,
) {
    let v: serde_json::Value = match serde_json::from_str(text) {
        Ok(v)  => v,
        Err(_) => return,
    };

    let cmd = match v["command"].as_str() {
        Some(c) => c.to_string(),
        None    => return,
    };

    if cmd.starts_with("panebot:") {
        handle_node_command(&cmd, &v, state, cmds, panes, tx, shutdown, layout).await;
        return;
    }

    let pane_name = match v["pane"].as_str() {
        Some(n) => n.to_string(),
        None    => return,
    };

    let args = match v["args"].as_array() {
        Some(a) => a.clone(),
        None    => return,
    };

    let allowed = matches!(
        cmd.as_str(),
        "set_property"       |
        "cycle"              |
        "add"                |
        "stop"               |
        "quit"               |
        "seek"               |
        "revert-seek"        |
        "playlist-next"      |
        "playlist-prev"      |
        "playlist-play-index"|
        "playlist-remove"    |
        "playlist-move"      |
        "playlist-shuffle"   |
        "playlist-unshuffle" |
        "playlist-clear"     |
        "loadfile"           |
        "loadlist"           |
        "keypress"           |
        "keydown"            |
        "keyup"
    );

    if !allowed { return; }

    let mut mpv_cmd = vec![serde_json::Value::String(cmd)];
    mpv_cmd.extend(args);
    let cmd_str = serde_json::json!({ "command": mpv_cmd }).to_string();

    let sender = cmds.lock().unwrap().get(&pane_name).cloned();
    if let Some(s) = sender {
        let _ = s.send(cmd_str).await;
    }
}

// ---------------------------------------------------------------------------
// Node command handler
// ---------------------------------------------------------------------------

async fn handle_node_command(
    cmd:      &str,
    v:        &serde_json::Value,
    state:    &SharedState,
    cmds:     &PaneCommands,
    panes:    &SharedPanes,
    tx:       &broadcast::Sender<String>,
    shutdown: &ShutdownTx,
    layout:   &SharedLayout,
) {
    match cmd {

        "panebot:node-info" => {
            let s     = state.lock().unwrap();
            let ps: Vec<&PaneState> = s.values().collect();
            let _ = tx.send(serde_json::json!({
                "event":    "node:info",
                "hostname": hostname(),
                "platform": platform(),
                "panes":    ps,
            }).to_string());
        }

        "panebot:restart-all" => {
            let mut log      = Logger::open().unwrap();
            let layout_name  = layout.lock().unwrap().clone();
            let layout_map   = load_layout(&layout_name, &mut log);
            for pane in panes.iter() {
                kill_pane_async(&pane.mpv_name, &mut log).await;
            }
            tokio::time::sleep(Duration::from_millis(RESTART_ALL_WAIT_MS)).await;
            for pane in panes.iter() {
                let geo = layout_map.as_ref()
                    .and_then(|m| m.get(&pane.mpv_name))
                    .and_then(|e| e.geometry.as_deref());
                launch_pane(pane, geo, &mut log);
            }
            let _ = tx.send(serde_json::json!({"event":"node:restart-all"}).to_string());
        }

        "panebot:restart-pane" => {
            let pane_name = match v["pane"].as_str() { Some(n) => n.to_string(), None => return };
            if let Some(pane) = panes.iter().find(|p| p.mpv_name == pane_name) {
                let mut log     = Logger::open().unwrap();
                let layout_name = layout.lock().unwrap().clone();
                let layout_map  = load_layout(&layout_name, &mut log);
                kill_pane_async(&pane.mpv_name, &mut log).await;
                tokio::time::sleep(Duration::from_millis(RESTART_PANE_WAIT_MS)).await;
                let geo = layout_map.as_ref()
                    .and_then(|m| m.get(&pane.mpv_name))
                    .and_then(|e| e.geometry.as_deref());
                launch_pane(pane, geo, &mut log);
                let _ = tx.send(serde_json::json!({
                    "event": "node:restart-pane",
                    "pane":  pane_name,
                }).to_string());
            }
        }

        "panebot:playlist-get" => {
            let pane_name = match v["pane"].as_str() { Some(n) => n.to_string(), None => return };
            let sender = cmds.lock().unwrap().get(&pane_name).cloned();
            if let Some(s) = sender {
                let cmd = serde_json::json!({"command":["get_property","playlist"]}).to_string();
                let _ = s.send(cmd).await;
            }
        }

        "panebot:playlist-save" => {
            // Save mpv's current playlist to a user-specified file.
            // Expects: { pane, path }
            let pane_name = match v["pane"].as_str() { Some(n) => n.to_string(), None => return };
            let save_path = match v["path"].as_str() { Some(p) => p.to_string(), None => return };
            let sender    = cmds.lock().unwrap().get(&pane_name).cloned();
            if let Some(s) = sender {
                let cmd = serde_json::json!({
                    "command": ["playlist-save", save_path]
                }).to_string();
                let _ = s.send(cmd).await;
                let _ = tx.send(serde_json::json!({
                    "event": "node:playlist-saved",
                    "pane":  pane_name,
                    "path":  save_path,
                }).to_string());
            }
        }

        "panebot:layout" => {
            let layout_name = match v["layout_name"].as_str() { Some(n) => n.to_string(), None => return };
            let mut log = Logger::open().unwrap();
            if let Some(layout_map) = load_layout(&layout_name, &mut log) {
                *layout.lock().unwrap() = layout_name.clone();

                log.log(&format!("layout :: switching to {} — restarting {} panes", layout_name, panes.len()));

                for pane in panes.iter() {
                    kill_pane_async(&pane.mpv_name, &mut log).await;
                }

                tokio::time::sleep(Duration::from_millis(RESTART_ALL_WAIT_MS)).await;

                for pane in panes.iter() {
                    let geo = layout_map.get(&pane.mpv_name)
                        .and_then(|e| e.geometry.as_deref());
                    launch_pane(pane, geo, &mut log);
                }

                let _ = tx.send(serde_json::json!({
                    "event":  "node:layout",
                    "layout": layout_name,
                }).to_string());
            }
        }

        "panebot:shutdown" => {
            let _ = tx.send(serde_json::json!({
                "event":  "node:down",
                "reason": "admin"
            }).to_string());
            tokio::time::sleep(Duration::from_millis(MONITOR_RETRY_MS)).await;
            shutdown.notify_one();
        }

        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Platform helpers
// ---------------------------------------------------------------------------

fn hostname() -> String {
    // /etc/hostname is Linux-specific. Use std::env or gethostname via std on all platforms.
    #[cfg(target_os = "linux")]
    if let Ok(h) = std::fs::read_to_string("/etc/hostname") {
        let h = h.trim().to_string();
        if !h.is_empty() { return h; }
    }

    // HOSTNAME env var works on most unix shells
    if let Ok(h) = std::env::var("HOSTNAME") {
        let h = h.trim().to_string();
        if !h.is_empty() { return h; }
    }

    // Fallback — hostname binary, works on macOS and Linux
    if let Ok(out) = std::process::Command::new("hostname").output() {
        let h = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !h.is_empty() { return h; }
    }

    "unknown".to_string()
}

fn platform() -> &'static str {
    if      cfg!(target_os = "macos") { "macos" }
    else if cfg!(target_os = "linux") { "linux" }
    else                               { "unknown" }
}

// Returns the local LAN IP if available, otherwise 127.0.0.1.
// Uses a UDP socket trick — no data is sent.
fn local_ip() -> String {
    use std::net::UdpSocket;
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| { s.connect("8.8.8.8:80")?; s.local_addr() })
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".to_string())
}

// ---------------------------------------------------------------------------
// Signal handling — handles SIGINT (Ctrl-C) and SIGTERM (systemd stop).
// ---------------------------------------------------------------------------

async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate())
            .expect("Failed to register SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
        }
    }
    #[cfg(not(unix))]
    { let _ = tokio::signal::ctrl_c().await; }
}

// ---------------------------------------------------------------------------
// Daemon mode — read from pb.daemon.conf
// ---------------------------------------------------------------------------

fn load_daemon_mode() -> &'static str {
    let content = std::fs::read_to_string(hosts_conf()).unwrap_or_default();
    for line in content.lines() {
        let line = line.trim();
        if let Some(eq) = line.find('=') {
            let key = line[..eq].trim();
            let val = line[eq+1..].trim();
            if key == "mode" && val == "remote" { return "remote"; }
        }
    }
    "local"
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    std::fs::create_dir_all(config_dir()).expect("Failed to create config dir");
    let mut log = Logger::open().expect("Failed to open log file");

    log.log("panebot-daemon :: starting");
    log.log(&format!("platform :: {}", platform()));
    log.log(&format!("hostname :: {}", hostname()));

    let cfg = bootstrap(&mut log).expect("Bootstrap failed");
    log.log(&format!("config :: layout = {}", cfg.layout));
    log.log(&format!("config :: {} panes", cfg.panes.len()));

    let daemon_mode = load_daemon_mode();
    let bind_addr   = if daemon_mode == "remote" { "0.0.0.0:9090" } else { "127.0.0.1:9090" };
    let display_ip  = if daemon_mode == "remote" { local_ip() } else { "127.0.0.1".to_string() };
    log.log(&format!("mode :: {} :: {}", daemon_mode, display_ip));

    // Load layout first so geometry is available at launch time
    let layout_map = load_layout(&cfg.layout, &mut log);

    // Clean stale sockets before launch — if mpv crashed without removing its
    // socket, UnixStream::connect() succeeds against the dead file and we'd
    // think the pane is already running when it isn't.
    for pane in &cfg.panes {
        let socket_path = pane_socket(&pane.mpv_name);
        if socket_path.exists() {
            let alive = UnixStream::connect(&socket_path).await.is_ok();
            if !alive {
                let _ = std::fs::remove_file(&socket_path);
                log.log(&format!("bootstrap :: {} :: stale socket removed", pane.mpv_name));
            }
        }
    }

    // Launch only panes that are not already running
    for pane in &cfg.panes {
        let socket_path = pane_socket(&pane.mpv_name);
        let already_running = UnixStream::connect(&socket_path).await.is_ok();
        if already_running {
            log.log(&format!("launch :: {} :: already running", pane.mpv_name));
        } else {
            let geo = layout_map.as_ref()
                .and_then(|m| m.get(&pane.mpv_name))
                .and_then(|e| e.geometry.as_deref());
            launch_pane(pane, geo, &mut log);
        }
    }

    let known_hosts = load_hosts();

    let node_snapshot = serde_json::json!({
        "event":    "node:snapshot",
        "hostname": hostname(),
        "platform": platform(),
        "ip":       display_ip,
        "layout":   cfg.layout,
        "home":     cfg.home,
        "panes":    cfg.panes.iter().map(|p| serde_json::json!({
            "name":      p.mpv_name,
            "pane_name": p.pane_name,
        })).collect::<Vec<_>>(),
        "known_hosts": known_hosts.iter().map(|h| serde_json::json!({
            "label":   h.label,
            "address": h.address,
        })).collect::<Vec<_>>(),
    }).to_string();

    log.log(&format!("panebot-daemon :: listening on ws://{}", bind_addr));

    let current_layout = cfg.layout.clone();
    let panes:    SharedPanes  = Arc::new(cfg.panes);
    let state:    SharedState  = Arc::new(Mutex::new(HashMap::new()));
    let cmds:     PaneCommands = Arc::new(Mutex::new(HashMap::new()));
    let shutdown: ShutdownTx   = Arc::new(tokio::sync::Notify::new());
    let layout:   SharedLayout = Arc::new(Mutex::new(current_layout));
    let (tx, _rx)              = broadcast::channel::<String>(256);

    for pane in panes.iter().cloned() {
        let s = state.clone();
        let t = tx.clone();
        let c = cmds.clone();
        tokio::spawn(async move {
            monitor_pane(pane, s, t, c).await;
        });
    }

    let listener = TcpListener::bind(bind_addr).await
        .expect("Failed to bind port 9090");

    let cleanup_panes = panes.clone();
    let do_cleanup = move |log: &mut Logger| {
        log.log("panebot-daemon :: cleaning up stale socket files");
        for pane in cleanup_panes.iter() {
            let path = pane_socket(&pane.mpv_name);
            if path.exists() {
                #[cfg(unix)]
                let alive = std::os::unix::net::UnixStream::connect(&path).is_ok();
                #[cfg(not(unix))]
                let alive = false;
                if !alive {
                    let _ = std::fs::remove_file(&path);
                    log.log(&format!("cleanup :: {} :: stale socket removed", pane.mpv_name));
                }
            }
        }
    };

    loop {
        tokio::select! {
            _ = shutdown.notified() => {
                log.log("panebot-daemon :: shutdown by admin");
                do_cleanup(&mut log);
                std::process::exit(0);
            }
            _ = wait_for_signal() => {
                log.log("panebot-daemon :: caught signal");
                do_cleanup(&mut log);
                std::process::exit(0);
            }
            result = listener.accept() => {
                let (stream, _) = match result {
                    Ok(s)  => s,
                    Err(_) => continue,
                };
                let s  = state.clone();
                let t  = tx.clone();
                let sn = node_snapshot.clone();
                let c  = cmds.clone();
                let p  = panes.clone();
                let sd = shutdown.clone();
                let la = layout.clone();
                tokio::spawn(async move {
                    handle_ws(stream, s, t, sn, c, p, sd, la).await;
                });
            }
        }
    }
}
