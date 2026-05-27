//! pbsend — send files or URLs to a PaneBot pane
//!
//! pbsend file.mkv                    send to local daemon, pick pane
//! pbsend @nodename file.mkv          send to named remote node
//! pbsend --nodes                     interactive: pick host, pick pane, enter URL
//! pbsend --pane=music file.mkv       skip pane picker
//! pbsend --mode=replace file.mkv     replace instead of append-play

use std::{
    env,
    io::{self, Write},
    net::TcpStream,
    time::Duration,
};

use native_tls::TlsConnector;
use serde::Deserialize;
use serde_json::{json, Value};
use tungstenite::Message;

// ── ANSI helpers ────────────────────────────────────────────────────────────
macro_rules! orange { ($s:expr) => { format!("\x1b[33m{}\x1b[0m", $s) } }
macro_rules! cyan   { ($s:expr) => { format!("\x1b[36m{}\x1b[0m", $s) } }
macro_rules! red    { ($s:expr) => { format!("\x1b[31m{}\x1b[0m", $s) } }
macro_rules! white  { ($s:expr) => { format!("\x1b[37m{}\x1b[0m", $s) } }
macro_rules! sep    { ()        => { orange!(" :: ") } }
macro_rules! clr    { ()        => { print!("\x1b[2J\x1b[H") } }

// ── Types ────────────────────────────────────────────────────────────────────
#[derive(Deserialize, Clone)]
struct Pane {
    name:      String,
    pane_name: String,
}

#[derive(Deserialize, Clone)]
struct Host {
    label:   String,
    address: String,
}

#[derive(Deserialize)]
struct Snapshot {
    #[serde(default)] panes:       Vec<Pane>,
    #[serde(default)] known_hosts: Vec<Host>,
}

type Ws = tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<TcpStream>>;

// ── WebSocket ────────────────────────────────────────────────────────────────
fn connect(addr: &str) -> anyhow::Result<Ws> {
    let tls = TlsConnector::builder().danger_accept_invalid_certs(true).build()?;
    let url = url::Url::parse(addr)?;
    let tcp = TcpStream::connect_timeout(
        &format!("{}:{}", url.host_str().unwrap_or("127.0.0.1"), url.port().unwrap_or(9090)).parse()?,
        Duration::from_secs(5),
    )?;
    tcp.set_read_timeout(Some(Duration::from_secs(10)))?;
    Ok(tungstenite::client_tls_with_config(addr, tcp, None, Some(tungstenite::Connector::NativeTls(tls)))?.0)
}

fn snapshot(ws: &mut Ws) -> anyhow::Result<Snapshot> {
    loop {
        if let Message::Text(t) = ws.read()? {
            let v: Value = serde_json::from_str(&t)?;
            if v["event"] == "node:snapshot" {
                return Ok(serde_json::from_value(v)?);
            }
        }
    }
}

// ── Pickers ──────────────────────────────────────────────────────────────────
fn pick_pane(panes: &[Pane]) -> anyhow::Result<&Pane> {
    for (i, p) in panes.iter().enumerate() {
        println!("  {}  {}", white!(format!("{}.", i + 1)), cyan!(p.pane_name.clone()));
    }
    println!();
    print!("{} ", white!(format!("Select [1-{}]:", panes.len())));
    io::stdout().flush()?;
    let mut buf = String::new();
    io::stdin().read_line(&mut buf)?;
    let n: usize = buf.trim().parse().unwrap_or(0);
    if n < 1 || n > panes.len() {
        eprintln!("{}{}{}", red!("[PaneBot]"), sep!(), red!("invalid"));
        std::process::exit(1);
    }
    Ok(&panes[n - 1])
}

fn pick_host<'a>(hosts: &'a [(String, String)]) -> anyhow::Result<&'a str> {
    for (i, (label, addr)) in hosts.iter().enumerate() {
        println!("  {}  {}  {}", white!(format!("{}.", i + 1)), cyan!(label), white!(addr));
    }
    println!();
    print!("{} ", white!(format!("Select [1-{}]:", hosts.len())));
    io::stdout().flush()?;
    let mut buf = String::new();
    io::stdin().read_line(&mut buf)?;
    let n: usize = buf.trim().parse().unwrap_or(0);
    if n < 1 || n > hosts.len() {
        eprintln!("{}{}{}", red!("[PaneBot]"), sep!(), red!("invalid"));
        std::process::exit(1);
    }
    Ok(&hosts[n - 1].1)
}

// ── Helpers ──────────────────────────────────────────────────────────────────
fn tilde(path: &str) -> String {
    env::var("HOME").ok()
        .filter(|h| path.starts_with(h.as_str()))
        .map(|h| format!("~{}", &path[h.len()..]))
        .unwrap_or_else(|| path.into())
}

fn resolve_path(file: &str, cwd: &std::path::Path) -> String {
    if file.starts_with('/') || file.starts_with("http") {
        file.into()
    } else {
        cwd.join(file).to_string_lossy().into_owned()
    }
}

fn send(ws: &mut Ws, pane: &str, path: &str, mode: &str) -> anyhow::Result<()> {
    ws.send(Message::Text(
        json!({"command": "loadfile", "pane": pane, "args": [path, mode]}).to_string()
    ))?;
    Ok(())
}

// ── Main ─────────────────────────────────────────────────────────────────────
fn main() -> anyhow::Result<()> {
    let local_addr = env::var("PANEBOT_HOST")
        .unwrap_or_else(|_| "wss://127.0.0.1:9090".into());

    let mut pane_arg  = String::new();
    let mut mode      = "append-play".to_string();
    let mut files     = Vec::<String>::new();
    let mut node_arg  = Option::<String>::None;
    let mut nodes_flag = false;

    for arg in env::args().skip(1) {
        match arg.as_str() {
            "--nodes"                  => nodes_flag = true,
            a if a.starts_with('@')    => node_arg   = Some(a[1..].into()),
            a if a.starts_with("--pane=") => pane_arg = a[7..].into(),
            a if a.starts_with("--mode=") => mode     = a[7..].into(),
            a                          => files.push(a.into()),
        }
    }

    // Always connect local first — need known_hosts
    let mut local_ws = connect(&local_addr)?;
    let local_snap   = snapshot(&mut local_ws)?;

    let mut hosts: Vec<(String, String)> = vec![("local".into(), local_addr.clone())];
    for h in &local_snap.known_hosts {
        hosts.push((h.label.clone(), h.address.clone()));
    }

    // ── --nodes mode ────────────────────────────────────────────────────────
    if nodes_flag {
        clr!();
        println!("{}{}{}", orange!("[PaneBot]"), sep!(), orange!("Select Host"));
        println!();
        let addr = pick_host(&hosts)?.to_string();

        let (panes, mut ws) = if addr == local_addr {
            (local_snap.panes, local_ws)
        } else {
            let mut rws = connect(&addr)?;
            let snap = snapshot(&mut rws)?;
            (snap.panes, rws)
        };

        if panes.is_empty() {
            eprintln!("{}{}{}", red!("[PaneBot]"), sep!(), red!("no panes"));
            std::process::exit(1);
        }

        clr!();
        println!("{}{}{}", orange!("[PaneBot]"), sep!(), orange!("Select Pane"));
        println!();
        let pane = pick_pane(&panes)?.name.clone();

        print!("{}{}{} ", orange!("[PaneBot]"), sep!(), white!("URL or path: "));
        io::stdout().flush()?;
        let mut buf = String::new();
        io::stdin().read_line(&mut buf)?;
        let path = buf.trim();
        if !path.is_empty() {
            send(&mut ws, &pane, path, &mode)?;
        }
        ws.close(None).ok();
        return Ok(());
    }

    // ── Fast path ────────────────────────────────────────────────────────────
    if files.is_empty() {
        eprintln!("usage: pbsend [@node] [--pane=name] [--mode=replace|append|append-play] file_or_url ...");
        eprintln!("       pbsend --nodes");
        std::process::exit(1);
    }

    let target_addr = match node_arg.as_deref() {
        None => local_addr.clone(),
        Some(name) => match hosts.iter().find(|(l, _)| l.to_lowercase() == name.to_lowercase()) {
            Some(h) => h.1.clone(),
            None => {
                eprintln!("{}{}{}", red!("[PaneBot]"), sep!(), red!(format!("unknown node: {}", name)));
                std::process::exit(1);
            }
        }
    };

    let (panes, mut ws) = if target_addr == local_addr {
        (local_snap.panes, local_ws)
    } else {
        let mut rws = connect(&target_addr)?;
        let snap = snapshot(&mut rws)?;
        (snap.panes, rws)
    };

    if panes.is_empty() {
        eprintln!("{}{}{}", red!("[PaneBot]"), sep!(), red!("no panes"));
        std::process::exit(1);
    }

    let pane = if !pane_arg.is_empty() {
        pane_arg
    } else if panes.len() == 1 {
        panes[0].name.clone()
    } else {
        clr!();
        let display: Vec<String> = files.iter().map(|f| cyan!(tilde(f))).collect();
        println!("{}{}{} {}", orange!("[PaneBot]"), sep!(), display.join(&sep!()), orange!(">>"));
        println!();
        let p = pick_pane(&panes)?;
        let display: Vec<String> = files.iter().map(|f| tilde(f)).collect();
        clr!();
        println!("{}{}{} {} {}", orange!("[PaneBot]"), sep!(), display.join(", "), orange!(">>"), cyan!(p.pane_name.clone()));
        p.name.clone()
    };

    let cwd = env::current_dir()?;
    for file in &files {
        send(&mut ws, &pane, &resolve_path(file, &cwd), &mode)?;
    }
    ws.close(None).ok();
    Ok(())
}
