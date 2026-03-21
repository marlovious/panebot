use std::collections::HashMap;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, UnixStream};
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;
use futures_util::{SinkExt, StreamExt};

use panebot::{
    load_config, load_types,
    config_dir, layouts_dir, panes_conf, types_conf, scripts_lib,
    pane_dir, pane_mpv_conf, pane_playlist, pane_scripts, pane_socket,
    Pane, PaneType,
};

// ---------------------------------------------------------------------------
// Default file contents
// ---------------------------------------------------------------------------

const DEFAULT_PANES_CONF: &str = "\
# pb.panes.conf\n\
# [panename]\n\
# type     = video | audio | ytube | rtsp | http\n\
# playlist = /path/to/playlist.m3u\n\
\n\
layout = pb.left.stack\n\
\n\
[music]\n\
type     = video\n\
playlist = ~/.config/panebot/music/music.m3u\n\
\n\
[wide-top]\n\
type     = video\n\
playlist = ~/.config/panebot/wide-top/wide-top.m3u\n\
\n\
[wide-bottom]\n\
type     = video\n\
playlist = ~/.config/panebot/wide-bottom/wide-bottom.m3u\n\
\n\
[standard]\n\
type     = video\n\
playlist = ~/.config/panebot/standard/standard.m3u\n";

const DEFAULT_TYPES_CONF: &str = "\
# pb.types.conf\n\
# Define pane types and their default mpv options.\n\
# Stamped into pane mpv.conf on first creation only.\n\
\n\
[video]\n\
really-quiet=yes\n\
pause=yes\n\
force-window=yes\n\
\n\
[audio]\n\
really-quiet=yes\n\
pause=yes\n\
force-window=yes\n\
vid=no\n\
\n\
[ytube]\n\
really-quiet=yes\n\
pause=yes\n\
force-window=yes\n\
ytdl-format=bestvideo+bestaudio\n\
\n\
[rtsp]\n\
really-quiet=yes\n\
pause=yes\n\
force-window=yes\n\
rtsp-transport=tcp\n\
\n\
[http]\n\
really-quiet=yes\n\
pause=yes\n\
force-window=yes\n";

// Layout files — simple INI: [panename] sections with geometry = WxH+X+Y
// The geometry value is passed directly to mpv --geometry at launch.
// On Linux/Sway: geometry is ignored — Sway handles placement via window title.
// Future: add [sway] sections when Sway layout support is built.

const LAYOUT_LEFT_STACK: &str = "\
# panebot layout — pb.left.stack\n\
\n\
[music]\n\
geometry = 366x366+0+0\n\
\n\
[wide-top]\n\
geometry = 650x366+0+374\n\
\n\
[wide-bottom]\n\
geometry = 650x366+0+748\n\
\n\
[standard]\n\
geometry = 650x488+0+1122\n";

const LAYOUT_RIGHT_STACK: &str = "\
# panebot layout — pb.right.stack\n\
\n\
[music]\n\
geometry = 366x366+2574+64\n\
\n\
[wide-top]\n\
geometry = 650x366+2290+432\n\
\n\
[wide-bottom]\n\
geometry = 650x366+2290+804\n\
\n\
[standard]\n\
geometry = 650x488+2290+1180\n";

const LAYOUT_TOP_ROW: &str = "\
# panebot layout — pb.top.row\n\
\n\
[music]\n\
geometry = 366x366+0+0\n\
\n\
[wide]\n\
geometry = 650x366+374+0\n\
\n\
[standard]\n\
geometry = 650x488+1032+0\n";

const LAYOUT_SPLIT: &str = "\
# panebot layout — pb.split\n\
# Split layout features a 16:9 editor pane with lua scripts to cut and export clips.\n\
\n\
[scope]\n\
geometry = 926x386+0+68\n\
\n\
[wide-editor]\n\
geometry = 790x568+2060+490\n\
\n\
[standard]\n\
geometry = 2014x1074+6+482\n\
\n\
[wide]\n\
geometry = 858x484+2038+1074\n";


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
        let ts   = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let line = format!("[{}] {}\n", ts, msg);
        let _    = self.file.write_all(line.as_bytes());
        print!("{}", line);
    }
}

// ---------------------------------------------------------------------------
// Bootstrap
// ---------------------------------------------------------------------------

fn write_if_missing(path: &std::path::PathBuf, content: &str) -> std::io::Result<()> {
    if !path.exists() {
        std::fs::File::create(path)?.write_all(content.as_bytes())?;
    }
    Ok(())
}

fn bootstrap(log: &mut Logger) -> std::io::Result<()> {
    log.log("bootstrap :: checking environment");

    std::fs::create_dir_all(config_dir())?;
    std::fs::create_dir_all(layouts_dir())?;
    std::fs::create_dir_all(scripts_lib())?;

    let created = !panes_conf().exists();

    write_if_missing(&panes_conf(),  DEFAULT_PANES_CONF)?;
    write_if_missing(&types_conf(),  DEFAULT_TYPES_CONF)?;
    write_if_missing(&layouts_dir().join("pb.left.stack.layout"),  LAYOUT_LEFT_STACK)?;
    write_if_missing(&layouts_dir().join("pb.right.stack.layout"), LAYOUT_RIGHT_STACK)?;
    write_if_missing(&layouts_dir().join("pb.top.row.layout"),     LAYOUT_TOP_ROW)?;
    write_if_missing(&layouts_dir().join("pb.split.layout"),       LAYOUT_SPLIT)?;

    if created {
        log.log("bootstrap :: created default config");
    } else {
        log.log("bootstrap :: config exists, skipping defaults");
    }

    Ok(())
}

fn ensure_pane_files(pane: &Pane, types: &HashMap<String, PaneType>, log: &mut Logger) -> std::io::Result<()> {
    std::fs::create_dir_all(pane_dir(&pane.name))?;
    std::fs::create_dir_all(pane_scripts(&pane.name))?;

    let mpv_conf = pane_mpv_conf(&pane.name);
    if !mpv_conf.exists() {
        log.log(&format!("ensure_pane :: {} :: creating mpv.conf [{}]", pane.name, pane.pane_type));
        let mut f = std::fs::File::create(&mpv_conf)?;
        writeln!(f, "# panebot mpv config — {} [{}]", pane.name, pane.pane_type)?;
        writeln!(f, "# Edit this file to tune mpv for this pane.")?;
        writeln!(f)?;

        if let Some(pt) = types.get(&pane.pane_type) {
            for opt in &pt.options {
                writeln!(f, "{}", opt)?;
            }
            for script in &pt.scripts {
                let src  = scripts_lib().join(script);
                let dest = pane_scripts(&pane.name).join(script);
                if src.exists() && !dest.exists() {
                    std::fs::copy(&src, &dest)?;
                    log.log(&format!("ensure_pane :: {} :: copied script {}", pane.name, script));
                }
            }
            if !pt.scripts.is_empty() {
                writeln!(f)?;
                writeln!(f, "scripts-dir={}", pane_scripts(&pane.name).to_string_lossy())?;
            }
        }

        // macOS: initial geometry written here when layout parser is implemented
        // Linux/Sway: geometry not written — Sway rules handle placement via
        //             window title matching on pane.name.to_uppercase()
    } else {
        log.log(&format!("ensure_pane :: {} :: mpv.conf exists", pane.name));
    }

    if !pane_playlist(&pane.name).exists() {
        std::fs::File::create(pane_playlist(&pane.name))?;
        log.log(&format!("ensure_pane :: {} :: created empty playlist", pane.name));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Layout parser
// ---------------------------------------------------------------------------

fn parse_layout(content: &str) -> HashMap<String, String> {
    let mut map      = HashMap::new();
    let mut current  = String::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }

        if line.starts_with('[') && line.ends_with(']') {
            current = line[1..line.len()-1].to_string();
            continue;
        }

        if !current.is_empty() {
            if let Some(eq) = line.find('=') {
                let key = line[..eq].trim();
                let val = line[eq+1..].trim().to_string();
                if key == "geometry" {
                    map.insert(current.clone(), val);
                }
            }
        }
    }

    map
}

fn load_layout(name: &str, log: &mut Logger) -> Option<HashMap<String, String>> {
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

fn apply_layout(layout_map: &HashMap<String, String>, panes: &[Pane], log: &mut Logger) {
    for pane in panes {
        if let Some(geo) = layout_map.get(&pane.name) {
            if cfg!(target_os = "macos") {
                apply_geometry_macos(&pane.name, geo, log);
            } else {
                log.log(&format!("layout :: {} :: skipping geometry on linux (Sway handles placement)", pane.name));
            }
        } else {
            log.log(&format!("layout :: no entry for pane {}", pane.name));
        }
    }
}

// On macOS: write geometry to mpv.conf and restart the pane.
// mpv respects --geometry=WxH+X+Y at launch.
fn apply_geometry_macos(pane_name: &str, geometry: &str, log: &mut Logger) {
    let mpv_conf = pane_mpv_conf(pane_name);

    // Read existing conf, replace or append geometry line
    let content = std::fs::read_to_string(&mpv_conf).unwrap_or_default();
    let mut lines: Vec<String> = content.lines()
        .filter(|l| !l.trim().starts_with("geometry="))
        .map(|l| l.to_string())
        .collect();
    lines.push(format!("geometry={}", geometry));

    if let Err(e) = std::fs::write(&mpv_conf, lines.join("\n") + "\n") {
        log.log(&format!("layout.macos :: {} :: failed to write mpv.conf: {}", pane_name, e));
        return;
    }

    log.log(&format!("layout.macos :: {} :: geometry={}", pane_name, geometry));
    // Pane will pick up geometry on next restart — caller handles restart if needed
}

// ---------------------------------------------------------------------------
// Launch / kill mpv
// ---------------------------------------------------------------------------

fn launch_pane(pane: &Pane, log: &mut Logger) {
    let socket   = pane_socket(&pane.name);
    let mpv_conf = pane_mpv_conf(&pane.name);
    let playlist = pane_playlist(&pane.name);

    let mut args: Vec<String> = vec![
        format!("--input-ipc-server={}", socket.to_string_lossy()),
        format!("--title={}", pane.name.to_uppercase()),
        format!("--include={}", mpv_conf.to_string_lossy()),
        "--volume=100".to_string(),
        "--mute=yes".to_string(),
        "--pause=yes".to_string(),
    ];

    if playlist.exists() && playlist.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        args.push(playlist.to_string_lossy().to_string());
    } else {
        args.push("--idle=yes".to_string());
    }

    match std::process::Command::new("mpv")
        .args(&args)
        .process_group(0)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(_)  => log.log(&format!("launch :: {} :: mpv spawned", pane.name)),
        Err(e) => log.log(&format!("launch :: {} :: failed to spawn mpv: {}", pane.name, e)),
    }
}

async fn kill_pane_async(pane_name: &str, log: &mut Logger) {
    let socket = pane_socket(pane_name).to_string_lossy().to_string();
    match UnixStream::connect(&socket).await {
        Ok(mut stream) => {
            let cmd = serde_json::json!({"command":["quit"]}).to_string() + "\n";
            let _ = stream.write_all(cmd.as_bytes()).await;
            log.log(&format!("kill :: {} :: quit sent", pane_name));
        }
        Err(_) => {
            log.log(&format!("kill :: {} :: socket not found, already dead", pane_name));
        }
    }
}

// ---------------------------------------------------------------------------
// Pane state
//
// Tracks all observable mpv properties for a pane.
// idle_active: true  = stopped (no file playing, mpv is idle)
// idle_active: false = a file is loaded (may be paused or playing)
// paused:      true  = explicitly paused
// muted:       true  = muted via mpv mute property
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct PaneState {
    pub name:         String,
    pub pane_type:    String,
    pub online:       bool,
    pub idle_active:  bool,
    pub paused:       bool,
    pub muted:        bool,
    pub volume:       f64,
    pub title:        String,
    pub playlist_pos: i64,
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
}

type SharedState  = Arc<Mutex<HashMap<String, PaneState>>>;
type PaneCommands = Arc<Mutex<HashMap<String, mpsc::Sender<String>>>>;
type SharedPanes  = Arc<Vec<Pane>>;
type ShutdownTx   = Arc<tokio::sync::Notify>;

// ---------------------------------------------------------------------------
// mpv IPC — async per-pane monitor
// ---------------------------------------------------------------------------

async fn socket_alive(socket: &str) -> bool {
    UnixStream::connect(socket).await.is_ok()
}

async fn monitor_pane(
    pane:  Pane,
    state: SharedState,
    tx:    broadcast::Sender<String>,
    cmds:  PaneCommands,
) {
    let socket_path = pane_socket(&pane.name).to_string_lossy().to_string();

    loop {
        if !socket_alive(&socket_path).await {
            {
                let mut s = state.lock().unwrap();
                let ps = s.entry(pane.name.clone())
                    .or_insert_with(|| PaneState::new(&pane.name, &pane.pane_type));
                if ps.online {
                    ps.online = false;
                    let _ = tx.send(serde_json::json!({
                        "pane":  pane.name,
                        "event": "offline"
                    }).to_string());
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            continue;
        }

        let stream = match UnixStream::connect(&socket_path).await {
            Ok(s)  => s,
            Err(_) => { tokio::time::sleep(Duration::from_millis(200)).await; continue; }
        };

        // Fresh channel on each reconnect — fixes consumed cmd_rx bug
        let (cmd_tx, cmd_rx) = mpsc::channel::<String>(32);
        cmds.lock().unwrap().insert(pane.name.clone(), cmd_tx);

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
            tokio::time::sleep(Duration::from_millis(200)).await;
            continue;
        }

        {
            let mut s = state.lock().unwrap();
            let ps = s.entry(pane.name.clone())
                .or_insert_with(|| PaneState::new(&pane.name, &pane.pane_type));
            ps.online = true;
            let _ = tx.send(serde_json::json!({
                "pane":  pane.name,
                "event": "online"
            }).to_string());
        }

        let pane_name = pane.name.clone();
        let pane_type = pane.pane_type.clone();
        let state_r   = state.clone();
        let tx_r      = tx.clone();

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
                    let ps    = s.entry(pane_name.clone())
                        .or_insert_with(|| PaneState::new(&pane_name, &pane_type));

                    match prop.as_str() {
                        "pause"        => { ps.paused      = v["data"].as_bool().unwrap_or(true); }
                        "volume"       => { ps.volume       = v["data"].as_f64().unwrap_or(0.0); }
                        "media-title"  => { ps.title        = v["data"].as_str().unwrap_or("").to_string(); }
                        "playlist-pos" => { ps.playlist_pos = v["data"].as_i64().unwrap_or(-1); }
                        "mute"         => { ps.muted        = v["data"].as_bool().unwrap_or(false); }
                        "idle-active"  => { ps.idle_active  = v["data"].as_bool().unwrap_or(true); }
                        _ => continue,
                    }

                    let _ = tx_r.send(serde_json::json!({
                        "pane":     pane_name,
                        "event":    "property-change",
                        "property": prop,
                        "value":    v["data"],
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
            if let Some(ps) = s.get_mut(&pane.name) {
                ps.online = false;
            }
            let _ = tx.send(serde_json::json!({
                "pane":  pane.name,
                "event": "offline"
            }).to_string());
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

// ---------------------------------------------------------------------------
// WebSocket handler
// ---------------------------------------------------------------------------

async fn handle_ws(
    stream:    tokio::net::TcpStream,
    state:     SharedState,
    tx:        broadcast::Sender<String>,
    bootstrap: String,
    cmds:      PaneCommands,
    panes:     SharedPanes,
    shutdown:  ShutdownTx,
) {
    let ws = match accept_async(stream).await {
        Ok(ws) => ws,
        Err(_) => return,
    };

    let (mut sink, mut source) = ws.split();
    let mut rx = tx.subscribe();

    let _ = sink.send(Message::Text(bootstrap)).await;

    let state_msgs: Vec<String> = {
        let s = state.lock().unwrap();
        s.values().map(|ps| serde_json::json!({
            "pane":  ps.name,
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
                        handle_command(&text, &state, &cmds, &panes, &tx, &shutdown).await;
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
        handle_node_command(&cmd, &v, state, cmds, panes, tx, shutdown).await;
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
        // "show-text" -- OSD, enable when needed
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

        "panebot:node-status" => {
            let s     = state.lock().unwrap();
            let ps: Vec<&PaneState> = s.values().collect();
            let _ = tx.send(serde_json::json!({
                "event": "node:status",
                "panes": ps,
            }).to_string());
        }

        "panebot:stop-all" => {
            let senders: Vec<_> = cmds.lock().unwrap().values().cloned().collect();
            let cmd = serde_json::json!({"command":["stop"]}).to_string();
            for s in senders { let _ = s.send(cmd.clone()).await; }
        }

        "panebot:start-all" => {
            let senders: Vec<_> = cmds.lock().unwrap().values().cloned().collect();
            let cmd = serde_json::json!({"command":["set_property","pause",false]}).to_string();
            for s in senders { let _ = s.send(cmd.clone()).await; }
        }

        "panebot:restart-all" => {
            let mut log = Logger::open().unwrap();
            for pane in panes.iter() {
                kill_pane_async(&pane.name, &mut log).await;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
            for pane in panes.iter() {
                launch_pane(pane, &mut log);
            }
            let _ = tx.send(serde_json::json!({"event":"node:restart-all"}).to_string());
        }

        "panebot:restart-pane" => {
            let pane_name = match v["pane"].as_str() { Some(n) => n.to_string(), None => return };
            if let Some(pane) = panes.iter().find(|p| p.name == pane_name) {
                let mut log = Logger::open().unwrap();
                kill_pane_async(&pane.name, &mut log).await;
                tokio::time::sleep(Duration::from_millis(3000)).await;
                launch_pane(pane, &mut log);
                let _ = tx.send(serde_json::json!({
                    "event": "node:restart-pane",
                    "pane":  pane_name,
                }).to_string());
            }
        }

        "panebot:reload-playlist" => {
            let pane_name = match v["pane"].as_str() { Some(n) => n.to_string(), None => return };
            let playlist  = pane_playlist(&pane_name);
            let sender    = cmds.lock().unwrap().get(&pane_name).cloned();
            if let Some(s) = sender {
                let cmd = serde_json::json!({
                    "command": ["loadlist", playlist.to_string_lossy(), "replace"]
                }).to_string();
                let _ = s.send(cmd).await;
            }
        }

        "panebot:solo" => {
            let solo    = match v["pane"].as_str() { Some(n) => n.to_string(), None => return };
            let senders = cmds.lock().unwrap().clone();
            for (name, s) in &senders {
                if *name == solo {
                    let _ = s.send(serde_json::json!({"command":["set_property","mute",false]}).to_string()).await;
                    let _ = s.send(serde_json::json!({"command":["set_property","pause",false]}).to_string()).await;
                } else {
                    let _ = s.send(serde_json::json!({"command":["set_property","mute",true]}).to_string()).await;
                }
            }
        }

        "panebot:mute-others" => {
            let keep    = match v["pane"].as_str() { Some(n) => n.to_string(), None => return };
            let senders = cmds.lock().unwrap().clone();
            for (name, s) in &senders {
                if *name != keep {
                    let _ = s.send(serde_json::json!({"command":["set_property","mute",true]}).to_string()).await;
                }
            }
        }

        "panebot:swap-volume" => {
            let pane_a = match v["pane_a"].as_str() { Some(n) => n.to_string(), None => return };
            let pane_b = match v["pane_b"].as_str() { Some(n) => n.to_string(), None => return };
            let (vol_a, vol_b) = {
                let s  = state.lock().unwrap();
                let va = s.get(&pane_a).map(|p| p.volume).unwrap_or(0.0);
                let vb = s.get(&pane_b).map(|p| p.volume).unwrap_or(0.0);
                (va, vb)
            };
            let senders = cmds.lock().unwrap().clone();
            if let Some(s) = senders.get(&pane_a) {
                let _ = s.send(serde_json::json!({"command":["set_property","volume",vol_b]}).to_string()).await;
            }
            if let Some(s) = senders.get(&pane_b) {
                let _ = s.send(serde_json::json!({"command":["set_property","volume",vol_a]}).to_string()).await;
            }
        }

        "panebot:set-volume-all" => {
            let vol     = match v["volume"].as_f64() { Some(v) => v, None => return };
            let senders: Vec<_> = cmds.lock().unwrap().values().cloned().collect();
            let cmd     = serde_json::json!({"command":["set_property","volume",vol]}).to_string();
            for s in senders { let _ = s.send(cmd.clone()).await; }
        }

        "panebot:layout" => {
            let layout_name = match v["layout_name"].as_str() { Some(n) => n.to_string(), None => return };
            let mut log = Logger::open().unwrap();
            if let Some(layout_map) = load_layout(&layout_name, &mut log) {
                apply_layout(&layout_map, panes, &mut log);

                // Snapshot state before killing so we can restore after relaunch
                let snapshots: Vec<(String, f64, bool, bool)> = {
                    let s = state.lock().unwrap();
                    panes.iter().map(|pane| {
                        let ps = s.get(&pane.name);
                        let volume = ps.map(|p| p.volume).unwrap_or(100.0);
                        let muted  = ps.map(|p| p.muted).unwrap_or(false);
                        let paused = ps.map(|p| p.paused).unwrap_or(true);
                        (pane.name.clone(), volume, muted, paused)
                    }).collect()
                };

                log.log(&format!("layout :: restarting {} panes", panes.len()));
                for pane in panes.iter() {
                    kill_pane_async(&pane.name, &mut log).await;
                }

                for pane in panes.iter() {
                    launch_pane(pane, &mut log);
                }

                // Poll each pane socket until alive (max 5s), then restore state
                for (pane_name, volume, muted, _paused) in &snapshots {
                    let socket = pane_socket(pane_name).to_string_lossy().to_string();
                    let mut alive = false;
                    for _ in 0..50 {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        if socket_alive(&socket).await {
                            alive = true;
                            break;
                        }
                    }
                    if alive {
                        let sender = cmds.lock().unwrap().get(pane_name).cloned();
                        if let Some(s) = sender {
                            let _ = s.send(serde_json::json!({"command":["set_property","volume",volume]}).to_string()).await;
                            let _ = s.send(serde_json::json!({"command":["set_property","mute",muted]}).to_string()).await;
                            let _ = s.send(serde_json::json!({"command":["set_property","pause",true]}).to_string()).await;
                            log.log(&format!("layout :: {} :: state restored (vol={:.0} mute={})", pane_name, volume, muted));
                        }
                    } else {
                        log.log(&format!("layout :: {} :: socket did not come up in time", pane_name));
                    }
                }

                let _ = tx.send(serde_json::json!({
                    "event":  "node:layout",
                    "layout": layout_name,
                }).to_string());
            }
        }

        "panebot:reload-config" => {
            // TODO: requires panes to be Arc<Mutex<Vec<Pane>>> for hot-add/remove
            let _ = tx.send(serde_json::json!({
                "event":  "node:reload-config",
                "status": "not-yet-implemented",
                "reason": "requires mutable shared pane list"
            }).to_string());
        }

        "panebot:shutdown" => {
            let _ = tx.send(serde_json::json!({
                "event":  "node:down",
                "reason": "admin"
            }).to_string());
            tokio::time::sleep(Duration::from_millis(200)).await;
            shutdown.notify_one();
        }

        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Platform helpers
// ---------------------------------------------------------------------------

fn hostname() -> String {
    if let Ok(h) = std::fs::read_to_string("/etc/hostname") {
        let h = h.trim().to_string();
        if !h.is_empty() { return h; }
    }
    if let Ok(h) = std::env::var("HOSTNAME") {
        if !h.is_empty() { return h; }
    }
    if let Ok(h) = std::env::var("HOST") {
        if !h.is_empty() { return h; }
    }
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

    bootstrap(&mut log).expect("Bootstrap failed");

    let cfg   = load_config();
    let types = load_types();

    log.log(&format!("config :: layout = {}", cfg.layout));

    for pane in &cfg.panes {
        if let Err(e) = ensure_pane_files(pane, &types, &mut log) {
            log.log(&format!("ensure_pane :: {} :: error: {}", pane.name, e));
        }
    }

    for pane in &cfg.panes {
        let socket = pane_socket(&pane.name).to_string_lossy().to_string();
        if socket_alive(&socket).await {
            log.log(&format!("launch :: {} :: already running", pane.name));
        } else {
            launch_pane(pane, &mut log);
        }
    }

    // Apply layout — writes geometry to mpv.conf on macOS and restarts panes.
    // On Linux geometry is skipped; Sway handles placement via window title.
    // On macOS only restart panes that were freshly spawned — never touch
    // already-running panes on daemon restart.
    if let Some(layout_map) = load_layout(&cfg.layout, &mut log) {
        apply_layout(&layout_map, &cfg.panes, &mut log);
        if cfg!(target_os = "macos") {
            for pane in &cfg.panes {
                let socket = pane_socket(&pane.name).to_string_lossy().to_string();
                if layout_map.contains_key(&pane.name) && !socket_alive(&socket).await {
                    kill_pane_async(&pane.name, &mut log).await;
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    launch_pane(pane, &mut log);
                }
            }
        }
    }

    let bootstrap_panes: Vec<serde_json::Value> = {
        let mut out = Vec::new();
        for pane in &cfg.panes {
            let socket = pane_socket(&pane.name).to_string_lossy().to_string();
            let status = if socket_alive(&socket).await { "Online" } else { "Offline" };
            log.log(&format!("status :: {} :: {}", pane.name, status));
            out.push(serde_json::json!({
                "name":      pane.name,
                "pane_type": pane.pane_type,
                "status":    status,
            }));
        }
        out
    };

    let bootstrap_complete = serde_json::json!({
        "event":    "bootstrap_complete",
        "hostname": hostname(),
        "platform": platform(),
        "layout":   cfg.layout,
        "panes":    bootstrap_panes,
    }).to_string();

    log.log("panebot-daemon :: bootstrap complete, listening on ws://0.0.0.0:9090");

    let panes:    SharedPanes  = Arc::new(cfg.panes);
    let state:    SharedState  = Arc::new(Mutex::new(HashMap::new()));
    let cmds:     PaneCommands = Arc::new(Mutex::new(HashMap::new()));
    let shutdown: ShutdownTx   = Arc::new(tokio::sync::Notify::new());
    let (tx, _rx)              = broadcast::channel::<String>(256);

    for pane in panes.iter().cloned() {
        let s = state.clone();
        let t = tx.clone();
        let c = cmds.clone();
        tokio::spawn(async move {
            monitor_pane(pane, s, t, c).await;
        });
    }

    let listener = TcpListener::bind("0.0.0.0:9090").await
        .expect("Failed to bind port 9090");

    loop {
        tokio::select! {
            _ = shutdown.notified() => {
                log.log("panebot-daemon :: shutdown by admin");
                std::process::exit(0);
            }
            result = listener.accept() => {
                let (stream, _) = match result {
                    Ok(s)  => s,
                    Err(_) => continue,
                };
                let s  = state.clone();
                let t  = tx.clone();
                let bc = bootstrap_complete.clone();
                let c  = cmds.clone();
                let p  = panes.clone();
                let sd = shutdown.clone();
                tokio::spawn(async move {
                    handle_ws(stream, s, t, bc, c, p, sd).await;
                });
            }
        }
    }
}
