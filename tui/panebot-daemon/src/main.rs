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
    load_config,
    config_dir, layouts_dir, panes_conf, hosts_conf,
    pane_dir, pane_mpv_conf, pane_playlist, pane_scripts, pane_socket,
    Pane,
};

// ---------------------------------------------------------------------------
// Timing constants
// ---------------------------------------------------------------------------

const MONITOR_RETRY_MS:     u64 = 500;   // delay between mpv socket connect attempts
#[cfg(target_os = "linux")]
const SWAY_SETTLE_MS:       u64 = 500;   // wait for mpv window before swaymsg
const RESTART_PANE_WAIT_MS: u64 = 3000;  // wait after kill before relaunch
const RESTART_ALL_WAIT_MS:  u64 = 300;   // wait after killing all before relaunch

// ---------------------------------------------------------------------------
// Default file contents
// ---------------------------------------------------------------------------

const DEFAULT_PANES_CONF: &str = "\
# pb.panes.conf
# [panename]
# type     = video | audio | ytube | rtsp | http
# playlist = /path/to/playlist.m3u

layout = pb.left.stack

[music]
type     = video
playlist = ~/.config/panebot/music/music.m3u

[wide-top]
type     = video
playlist = ~/.config/panebot/wide-top/wide-top.m3u

[wide-bottom]
type     = video
playlist = ~/.config/panebot/wide-bottom/wide-bottom.m3u

[standard]
type     = video
playlist = ~/.config/panebot/standard/standard.m3u
";

const DEFAULT_HOSTS_CONF: &str = "\
# pb.daemon.conf
# Add remote panebot nodes here.
# The TUI connects to the listed host(s) at startup.
# If empty, the TUI connects to localhost.
#
# [my-linux-box]
# address = ws://192.168.1.x:9090
";

// ---------------------------------------------------------------------------
// Layout files
//
// macOS layouts: [panename] sections with geometry = WxH+X+Y
//   Passed as --geometry arg to mpv at launch. Never written to mpv.conf.
//
// Sway layouts: [panename] sections with swaymsg = <criteria> <commands>
//   Executed via `swaymsg` after pane launch on Linux.
//   mpv sets window title to PANENAME (uppercase) — Sway matches on that.
//   geometry key is ignored on Linux.
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

const LAYOUT_SWAY_LEFT_STACK: &str = "\
# panebot layout — pb.sway.left.stack
# Sway floating layout, left stack.
# Windows matched by mpv --title=PANENAME (uppercase).

[music]
swaymsg = [title=\"MUSIC\"] floating enable, resize set 366 366, move position 0 0

[wide-top]
swaymsg = [title=\"WIDE-TOP\"] floating enable, resize set 650 366, move position 0 374

[wide-bottom]
swaymsg = [title=\"WIDE-BOTTOM\"] floating enable, resize set 650 366, move position 0 748

[standard]
swaymsg = [title=\"STANDARD\"] floating enable, resize set 650 488, move position 0 1122
";

const LAYOUT_SWAY_RIGHT_STACK: &str = "\
# panebot layout — pb.sway.right.stack
# Sway floating layout, right stack.
# Windows matched by mpv --title=PANENAME (uppercase).

[music]
swaymsg = [title=\"MUSIC\"] floating enable, resize set 366 366, move position 2574 64

[wide-top]
swaymsg = [title=\"WIDE-TOP\"] floating enable, resize set 650 366, move position 2290 432

[wide-bottom]
swaymsg = [title=\"WIDE-BOTTOM\"] floating enable, resize set 650 366, move position 2290 804

[standard]
swaymsg = [title=\"STANDARD\"] floating enable, resize set 650 488, move position 2290 1180
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
    write_if_missing(&layouts_dir().join("pb.left.stack.layout"),       LAYOUT_LEFT_STACK)?;
    write_if_missing(&layouts_dir().join("pb.right.stack.layout"),      LAYOUT_RIGHT_STACK)?;
    write_if_missing(&layouts_dir().join("pb.sway.left.stack.layout"),  LAYOUT_SWAY_LEFT_STACK)?;
    write_if_missing(&layouts_dir().join("pb.sway.right.stack.layout"), LAYOUT_SWAY_RIGHT_STACK)?;

    // Per-pane setup — dirs, mpv.conf stub, empty playlist.
    // load_config() called once here after defaults are written.
    let cfg = load_config();
    for pane in &cfg.panes {
        std::fs::create_dir_all(pane_dir(&pane.name))?;
        std::fs::create_dir_all(pane_scripts(&pane.name))?;

        let mpv_conf = pane_mpv_conf(&pane.name);
        if !mpv_conf.exists() {
            let mut f = std::fs::File::create(&mpv_conf)?;
            writeln!(f, "# panebot mpv config — {} [{}]", pane.name, pane.pane_type)?;
            writeln!(f, "# Edit this file to tune mpv for this pane.")?;
            writeln!(f, "# Required options — panebot depends on these.")?;
            writeln!(f, "force-window=yes")?;
            writeln!(f, "really-quiet=yes")?;
            writeln!(f, "pause=yes")?;
            writeln!(f, "volume=100")?;
            writeln!(f, "mute=yes")?;
            writeln!(f, "scripts-dir={}", pane_scripts(&pane.name).to_string_lossy())?;
            writeln!(f)?;
            writeln!(f, "# Add your own mpv options below.")?;
            log.log(&format!("bootstrap :: {} :: created mpv.conf", pane.name));
        }

        let pl = pane_playlist(&pane.name);
        if !pl.exists() {
            std::fs::File::create(&pl)?;
            log.log(&format!("bootstrap :: {} :: created empty playlist", pane.name));
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
    swaymsg:  Option<String>,
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
                    "swaymsg"  => entry.swaymsg  = Some(val),
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
// volume and mute now live in mpv.conf — not passed as launch args.
// geometry: passed as --geometry=WxH+X+Y on macOS only, at launch time.
//           Ignored on Linux — Sway positions via swaymsg after launch.
// ---------------------------------------------------------------------------

fn launch_pane(pane: &Pane, geometry: Option<&str>, log: &mut Logger) {
    let socket   = pane_socket(&pane.name);
    let mpv_conf = pane_mpv_conf(&pane.name);
    let playlist = pane_playlist(&pane.name);

    let mut args: Vec<String> = vec![
        format!("--input-ipc-server={}", socket.to_string_lossy()),
        format!("--title={}", pane.name.to_uppercase()),
        format!("--include={}", mpv_conf.to_string_lossy()),
    ];

    #[cfg(target_os = "macos")]
    if let Some(geo) = geometry {
        args.push(format!("--geometry={}", geo));
        log.log(&format!("launch :: {} :: geometry={}", pane.name, geo));
    }

    #[cfg(not(target_os = "macos"))]
    let _ = geometry;

    if playlist.exists() && playlist.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        args.push(playlist.to_string_lossy().to_string());
    } else {
        args.push("--idle=yes".to_string());
    }

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
        Ok(_)  => log.log(&format!("launch :: {} :: mpv spawned", pane.name)),
        Err(e) => log.log(&format!("launch :: {} :: failed to spawn mpv: {}", pane.name, e)),
    }
}

#[cfg(target_os = "linux")]
fn apply_swaymsg(pane_name: &str, msg: &str, log: &mut Logger) {
    match std::process::Command::new("swaymsg").arg(msg).output() {
        Ok(out) => {
            if out.status.success() {
                log.log(&format!("sway :: {} :: positioned", pane_name));
            } else {
                let err = String::from_utf8_lossy(&out.stderr);
                log.log(&format!("sway :: {} :: swaymsg failed: {}", pane_name, err.trim()));
            }
        }
        Err(e) => log.log(&format!("sway :: {} :: could not run swaymsg: {}", pane_name, e)),
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
    let socket_path = pane_socket(&pane.name).to_string_lossy().to_string();

    loop {
        // Try to connect. If it fails, mark offline and wait before retrying.
        let stream = match UnixStream::connect(&socket_path).await {
            Ok(s) => s,
            Err(_) => {
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
                tokio::time::sleep(Duration::from_millis(MONITOR_RETRY_MS)).await;
                continue;
            }
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
            tokio::time::sleep(Duration::from_millis(MONITOR_RETRY_MS)).await;
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
                        "pause"        => { ps.paused       = v["data"].as_bool().unwrap_or(true); }
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

        "panebot:node-status" => {
            let s     = state.lock().unwrap();
            let ps: Vec<&PaneState> = s.values().collect();
            let _ = tx.send(serde_json::json!({
                "event": "node:status",
                "panes": ps,
            }).to_string());
        }

        "panebot:restart-all" => {
            let mut log      = Logger::open().unwrap();
            let layout_name  = layout.lock().unwrap().clone();
            let layout_map   = load_layout(&layout_name, &mut log);
            for pane in panes.iter() {
                kill_pane_async(&pane.name, &mut log).await;
            }
            tokio::time::sleep(Duration::from_millis(RESTART_ALL_WAIT_MS)).await;
            for pane in panes.iter() {
                let geo = layout_map.as_ref()
                    .and_then(|m| m.get(&pane.name))
                    .and_then(|e| e.geometry.as_deref());
                launch_pane(pane, geo, &mut log);
            }
            #[cfg(target_os = "linux")]
            if let Some(ref lmap) = layout_map {
                tokio::time::sleep(Duration::from_millis(SWAY_SETTLE_MS)).await;
                for pane in panes.iter() {
                    if let Some(entry) = lmap.get(&pane.name) {
                        if let Some(msg) = &entry.swaymsg {
                            apply_swaymsg(&pane.name, msg, &mut log);
                        }
                    }
                }
            }
            let _ = tx.send(serde_json::json!({"event":"node:restart-all"}).to_string());
        }

        "panebot:restart-pane" => {
            let pane_name = match v["pane"].as_str() { Some(n) => n.to_string(), None => return };
            if let Some(pane) = panes.iter().find(|p| p.name == pane_name) {
                let mut log     = Logger::open().unwrap();
                let layout_name = layout.lock().unwrap().clone();
                let layout_map  = load_layout(&layout_name, &mut log);
                kill_pane_async(&pane.name, &mut log).await;
                tokio::time::sleep(Duration::from_millis(RESTART_PANE_WAIT_MS)).await;
                let geo = layout_map.as_ref()
                    .and_then(|m| m.get(&pane.name))
                    .and_then(|e| e.geometry.as_deref());
                launch_pane(pane, geo, &mut log);
                #[cfg(target_os = "linux")]
                if let Some(ref lmap) = layout_map {
                    tokio::time::sleep(Duration::from_millis(SWAY_SETTLE_MS)).await;
                    if let Some(entry) = lmap.get(&pane.name) {
                        if let Some(msg) = &entry.swaymsg {
                            apply_swaymsg(&pane.name, msg, &mut log);
                        }
                    }
                }
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

        "panebot:layout" => {
            let layout_name = match v["layout_name"].as_str() { Some(n) => n.to_string(), None => return };
            let mut log = Logger::open().unwrap();
            if let Some(layout_map) = load_layout(&layout_name, &mut log) {
                *layout.lock().unwrap() = layout_name.clone();

                log.log(&format!("layout :: switching to {} — restarting {} panes", layout_name, panes.len()));

                for pane in panes.iter() {
                    kill_pane_async(&pane.name, &mut log).await;
                }

                tokio::time::sleep(Duration::from_millis(RESTART_ALL_WAIT_MS)).await;

                for pane in panes.iter() {
                    let geo = layout_map.get(&pane.name)
                        .and_then(|e| e.geometry.as_deref());
                    launch_pane(pane, geo, &mut log);
                }

                #[cfg(target_os = "linux")]
                {
                    tokio::time::sleep(Duration::from_millis(SWAY_SETTLE_MS)).await;
                    for pane in panes.iter() {
                        if let Some(entry) = layout_map.get(&pane.name) {
                            if let Some(msg) = &entry.swaymsg {
                                apply_swaymsg(&pane.name, msg, &mut log);
                            }
                        }
                    }
                }

                let _ = tx.send(serde_json::json!({
                    "event":  "node:layout",
                    "layout": layout_name,
                }).to_string());
            }
        }

        "panebot:reload-config" => {
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

    // Load layout first so geometry is available at launch time
    let layout_map = load_layout(&cfg.layout, &mut log);

    // Launch only panes that are not already running
    let mut freshly_launched: Vec<String> = Vec::new();
    for pane in &cfg.panes {
        let socket_path = pane_socket(&pane.name);
        let already_running = UnixStream::connect(&socket_path).await.is_ok();
        if already_running {
            log.log(&format!("launch :: {} :: already running", pane.name));
        } else {
            let geo = layout_map.as_ref()
                .and_then(|m| m.get(&pane.name))
                .and_then(|e| e.geometry.as_deref());
            launch_pane(pane, geo, &mut log);
            freshly_launched.push(pane.name.clone());
        }
    }

    // On Linux/Sway: position freshly launched panes via swaymsg
    #[cfg(target_os = "linux")]
    if let Some(ref lmap) = layout_map {
        if !freshly_launched.is_empty() {
            tokio::time::sleep(Duration::from_millis(SWAY_SETTLE_MS)).await;
            for pane in &cfg.panes {
                if freshly_launched.contains(&pane.name) {
                    if let Some(entry) = lmap.get(&pane.name) {
                        if let Some(msg) = &entry.swaymsg {
                            apply_swaymsg(&pane.name, msg, &mut log);
                        }
                    }
                }
            }
        }
    }

    let node_snapshot = serde_json::json!({
        "event":    "node:snapshot",
        "hostname": hostname(),
        "platform": platform(),
        "layout":   cfg.layout,
        "panes":    cfg.panes.iter().map(|p| serde_json::json!({
            "name":      p.name,
            "pane_type": p.pane_type,
        })).collect::<Vec<_>>(),
    }).to_string();

    log.log("panebot-daemon :: listening on ws://0.0.0.0:9090");

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

    let listener = TcpListener::bind("0.0.0.0:9090").await
        .expect("Failed to bind port 9090");

    let cleanup_panes = panes.clone();
    let do_cleanup = move |log: &mut Logger| {
        log.log("panebot-daemon :: cleaning up stale socket files");
        for pane in cleanup_panes.iter() {
            let path = pane_socket(&pane.name);
            if path.exists() {
                // Only remove if nothing is listening — mpv may still be running
                let alive = std::os::unix::net::UnixStream::connect(&path).is_ok();
                if !alive {
                    let _ = std::fs::remove_file(&path);
                    log.log(&format!("cleanup :: {} :: stale socket removed", pane.name));
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
