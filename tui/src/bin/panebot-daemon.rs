use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::net::TcpListener;
use tokio::sync::broadcast;
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
# geometry = WxH+X+Y   (set by layout, or manually)\n\
# playlist = /path/to/playlist.m3u\n\
\n\
layout = pb.left.stack\n\
\n\
[music]\n\
type     = video\n\
geometry = 366x366+0+0\n\
playlist = ~/.config/panebot/music/music.m3u\n\
\n\
[wide-top]\n\
type     = video\n\
geometry = 650x366+0+374\n\
playlist = ~/.config/panebot/wide-top/wide-top.m3u\n\
\n\
[wide-bottom]\n\
type     = video\n\
geometry = 650x366+0+748\n\
playlist = ~/.config/panebot/wide-bottom/wide-bottom.m3u\n\
\n\
[standard]\n\
type     = video\n\
geometry = 650x488+0+1122\n\
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

        if let Some(geo) = &pane.geometry {
            writeln!(f)?;
            writeln!(f, "geometry={}", geo)?;
        }
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
// Pane state (shared across tasks)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct PaneState {
    pub name:         String,
    pub online:       bool,
    pub paused:       bool,
    pub volume:       f64,
    pub title:        String,
    pub playlist_pos: i64,
}

impl PaneState {
    fn new(name: &str) -> Self {
        PaneState {
            name:         name.to_string(),
            online:       false,
            paused:       true,
            volume:       0.0,
            title:        String::new(),
            playlist_pos: -1,
        }
    }
}

type SharedState = Arc<Mutex<HashMap<String, PaneState>>>;

// ---------------------------------------------------------------------------
// mpv IPC helpers
// ---------------------------------------------------------------------------

fn mpv_command(socket: &str, args: &[serde_json::Value]) -> Option<serde_json::Value> {
    let mut stream = UnixStream::connect(socket).ok()?;
    stream.set_read_timeout(Some(Duration::from_millis(500))).ok()?;
    let cmd = serde_json::json!({ "command": args });
    let mut line = cmd.to_string();
    line.push('\n');
    stream.write_all(line.as_bytes()).ok()?;
    let reader = BufReader::new(stream);
    for l in reader.lines() {
        let l = l.ok()?;
        let v: serde_json::Value = serde_json::from_str(&l).ok()?;
        if v["error"] == "success" {
            return Some(v["data"].clone());
        }
    }
    None
}

fn mpv_send(socket: &str, args: &[serde_json::Value]) {
    if let Ok(mut stream) = UnixStream::connect(socket) {
        let cmd = serde_json::json!({ "command": args });
        let mut line = cmd.to_string();
        line.push('\n');
        let _ = stream.write_all(line.as_bytes());
    }
}

fn socket_alive(socket: &str) -> bool {
    UnixStream::connect(socket).is_ok()
}

// ---------------------------------------------------------------------------
// Launch mpv
// ---------------------------------------------------------------------------

fn launch_pane(pane: &Pane, log: &mut Logger) {
    let socket   = pane_socket(&pane.name);
    let mpv_conf = pane_mpv_conf(&pane.name);
    let playlist = pane_playlist(&pane.name);

    let mut args: Vec<String> = vec![
        format!("--input-ipc-server={}", socket.to_string_lossy()),
        format!("--title={}", pane.name.to_uppercase()),
        format!("--include={}", mpv_conf.to_string_lossy()),
    ];

    if playlist.exists() && playlist.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        args.push(playlist.to_string_lossy().to_string());
    } else {
        args.push("--idle=yes".to_string());
    }

    match std::process::Command::new("mpv")
        .args(&args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(_)  => log.log(&format!("launch :: {} :: mpv spawned", pane.name)),
        Err(e) => log.log(&format!("launch :: {} :: failed to spawn mpv: {}", pane.name, e)),
    }
}

// ---------------------------------------------------------------------------
// Per-pane monitor task
// ---------------------------------------------------------------------------

async fn monitor_pane(
    pane:  Pane,
    state: SharedState,
    tx:    broadcast::Sender<String>,
) {
    let socket = pane_socket(&pane.name).to_string_lossy().to_string();

    loop {
        if !socket_alive(&socket) {
            {
                let mut s = state.lock().unwrap();
                let ps = s.entry(pane.name.clone()).or_insert_with(|| PaneState::new(&pane.name));
                if ps.online {
                    ps.online = false;
                    let event = serde_json::json!({
                        "pane":  pane.name,
                        "event": "offline"
                    });
                    let _ = tx.send(event.to_string());
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            continue;
        }

        let mut stream = match UnixStream::connect(&socket) {
            Ok(s)  => s,
            Err(_) => { tokio::time::sleep(Duration::from_millis(200)).await; continue; }
        };

        let subs = [
            serde_json::json!({"command":["observe_property",1,"pause"]}),
            serde_json::json!({"command":["observe_property",2,"volume"]}),
            serde_json::json!({"command":["observe_property",3,"media-title"]}),
            serde_json::json!({"command":["observe_property",4,"playlist-pos"]}),
        ];
        let mut sub_ok = true;
        for sub in &subs {
            let mut line = sub.to_string();
            line.push('\n');
            if stream.write_all(line.as_bytes()).is_err() {
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
            let ps = s.entry(pane.name.clone()).or_insert_with(|| PaneState::new(&pane.name));
            ps.online = true;
            let event = serde_json::json!({
                "pane":  pane.name,
                "event": "online"
            });
            let _ = tx.send(event.to_string());
        }

        let reader = BufReader::new(stream);
        for line in reader.lines() {
            let line = match line {
                Ok(l)  => l,
                Err(_) => break,
            };

            let v: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v)  => v,
                Err(_) => continue,
            };

            if v["event"] == "property-change" {
                let prop  = v["name"].as_str().unwrap_or("");
                let mut s = state.lock().unwrap();
                let ps    = s.entry(pane.name.clone()).or_insert_with(|| PaneState::new(&pane.name));

                match prop {
                    "pause"        => { ps.paused       = v["data"].as_bool().unwrap_or(true); }
                    "volume"       => { ps.volume        = v["data"].as_f64().unwrap_or(0.0); }
                    "media-title"  => { ps.title         = v["data"].as_str().unwrap_or("").to_string(); }
                    "playlist-pos" => { ps.playlist_pos  = v["data"].as_i64().unwrap_or(-1); }
                    _ => continue,
                }

                let event = serde_json::json!({
                    "pane":     pane.name,
                    "event":    "property-change",
                    "property": prop,
                    "value":    v["data"],
                });
                let _ = tx.send(event.to_string());
            }
        }

        {
            let mut s = state.lock().unwrap();
            if let Some(ps) = s.get_mut(&pane.name) {
                ps.online = false;
            }
            let event = serde_json::json!({
                "pane":  pane.name,
                "event": "offline"
            });
            let _ = tx.send(event.to_string());
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
) {
    let ws = match accept_async(stream).await {
        Ok(ws) => ws,
        Err(_) => return,
    };

    let (mut sink, mut source) = ws.split();
    let mut rx = tx.subscribe();

    // bootstrap_complete is always the first message to any connecting client
    let _ = sink.send(Message::Text(bootstrap)).await;

    // Follow with a snapshot of current live state
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
                        handle_command(&text, &state);
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

fn handle_command(text: &str, _state: &SharedState) {
    let v: serde_json::Value = match serde_json::from_str(text) {
        Ok(v)  => v,
        Err(_) => return,
    };

    let pane_name = match v["pane"].as_str() {
        Some(n) => n.to_string(),
        None    => return,
    };

    let socket = pane_socket(&pane_name).to_string_lossy().to_string();
    if !socket_alive(&socket) { return; }

    let cmd = match v["command"].as_array() {
        Some(c) => c.clone(),
        None    => return,
    };

    let allowed = matches!(
        cmd.first().and_then(|v| v.as_str()).unwrap_or(""),
        "keypress"        |
        "set_property"    |
        "playlist-next"   |
        "playlist-prev"   |
        "playlist-remove" |
        "playlist-move"   |
        "playlist-clear"  |
        "loadfile"        |
        "seek"            |
        "stop"            |
        "quit"
    );

    if allowed {
        mpv_send(&socket, &cmd);
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let mut log = Logger::open().expect("Failed to open log file");

    log.log("panebot-daemon :: starting");

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
        if socket_alive(&socket) {
            log.log(&format!("launch :: {} :: already running", pane.name));
        } else {
            launch_pane(pane, &mut log);
        }
    }

    // Snapshot whatever is alive right now — honest status, no settle
    let bootstrap_panes: Vec<serde_json::Value> = cfg.panes.iter().map(|pane| {
        let socket = pane_socket(&pane.name).to_string_lossy().to_string();
        let status = if socket_alive(&socket) { "Online" } else { "Offline" };
        log.log(&format!("status :: {} :: {}", pane.name, status));
        serde_json::json!({ "name": pane.name, "status": status })
    }).collect();

    let bootstrap_complete = serde_json::json!({
        "event": "bootstrap_complete",
        "panes": bootstrap_panes,
    }).to_string();

    log.log("panebot-daemon :: bootstrap complete, listening on ws://0.0.0.0:9090");

    let state: SharedState = Arc::new(Mutex::new(HashMap::new()));
    let (tx, _rx) = broadcast::channel::<String>(256);

    for pane in cfg.panes {
        let s = state.clone();
        let t = tx.clone();
        tokio::spawn(async move {
            monitor_pane(pane, s, t).await;
        });
    }

    let listener = TcpListener::bind("0.0.0.0:9090").await
        .expect("Failed to bind port 9090");

    loop {
        let (stream, _) = match listener.accept().await {
            Ok(s)  => s,
            Err(_) => continue,
        };
        let s  = state.clone();
        let t  = tx.clone();
        let bc = bootstrap_complete.clone();
        tokio::spawn(async move {
            handle_ws(stream, s, t, bc).await;
        });
    }
}
