//! GG LAN client model: persistent configuration, agent/coordinator child
//! process management, and the background poller threads that feed the UI.

use crate::http;
use anyhow::{Context, Result};
use gg_lan_core::crypto::{b64, StaticKeypair};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::net::{TcpListener, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------- config

/// A network entry in the client (persisted).
#[derive(Serialize, Deserialize, Clone)]
pub struct NetCfg {
    pub name: String,
    /// Coordinator control-plane address, e.g. "203.0.113.7:7100".
    pub server: String,
    pub code: String,
    /// "mesh" (direct P2P + relay fallback) or "hub" (all traffic relayed).
    pub mode: String,
    /// This client also hosts the coordinator process.
    pub local: bool,
    /// Auto-connect on launch.
    pub enabled: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct GuiConfig {
    pub nickname: String,
    /// Optional explicit agent binary path (blank = auto-detect).
    pub agent_path: String,
    /// Optional explicit coordinator binary path (blank = auto-detect).
    pub server_path: String,
    pub networks: Vec<NetCfg>,
}

impl Default for GuiConfig {
    fn default() -> Self {
        Self {
            nickname: default_nickname(),
            agent_path: String::new(),
            server_path: String::new(),
            networks: Vec::new(),
        }
    }
}

fn default_nickname() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "player".into())
}

pub fn config_dir() -> PathBuf {
    let base = dirs::config_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("gg-lan")
}

pub fn load_config() -> (GuiConfig, PathBuf) {
    let path = config_dir().join("gui.json");
    let cfg = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    (cfg, path)
}

pub fn save_config(cfg: &GuiConfig, path: &Path) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(s) = serde_json::to_string_pretty(cfg) {
        let _ = std::fs::write(path, s);
    }
}

/// Create the shared node identity file if it does not exist yet, so that
/// several agents can safely start in parallel from it.
pub fn ensure_identity(dir: &Path) -> PathBuf {
    let path = dir.join("identity.txt");
    if !path.exists() {
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let kp = StaticKeypair::generate();
        let id = rand::rngs::OsRng.next_u64();
        let doc = serde_json::json!({
            "node_id": id,
            "secret": b64(&kp.secret_bytes()),
        });
        let _ = std::fs::write(&path, doc.to_string());
    }
    path
}

pub fn new_join_code() -> String {
    let mut b = [0u8; 6];
    rand::rngs::OsRng.fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// ---------------------------------------------------------------- events

/// Events pushed to the UI by background threads.
pub enum Event {
    /// Agent /status snapshot (network index, json).
    Status(usize, serde_json::Value),
    /// A line of agent or coordinator stderr.
    Log(usize, String),
    /// Ping result for a peer (network index, ip, text).
    Ping(usize, String, String),
}

// ---------------------------------------------------------------- processes

/// One running agent, bound to a network entry.
pub struct AgentProc {
    pub child: Child,
    pub status_port: u16,
    #[allow(dead_code)]
    pub frame_port: u16,
    pub stop: Arc<AtomicBool>,
}

impl AgentProc {
    pub fn is_running(&mut self) -> bool {
        self.child.try_wait().map(|w| w.is_none()).unwrap_or(false)
    }
}

/// One locally-hosted coordinator.
pub struct ServerProc {
    pub child: Child,
}

pub struct NetEntry {
    pub cfg: NetCfg,
    pub agent: Option<AgentProc>,
    pub server: Option<ServerProc>,
    /// Last /status JSON (pretty subset used by the UI).
    pub status: Option<serde_json::Value>,
    pub last_ok: Option<Instant>,
    pub log: VecDeque<String>,
    notified_crash: bool,
}

impl NetEntry {
    pub fn new(cfg: NetCfg) -> Self {
        Self {
            cfg,
            agent: None,
            server: None,
            status: None,
            last_ok: None,
            log: VecDeque::new(),
            notified_crash: false,
        }
    }

    /// Coarse connection state derived from the runtime bits.
    pub fn state(&mut self) -> NetState {
        match &mut self.agent {
            None => NetState::Off,
            Some(a) => {
                if !a.is_running() {
                    return NetState::Dead;
                }
                let net_up = self
                    .status
                    .as_ref()
                    .and_then(|s| s.get("net"))
                    .map(|n| !n.is_null())
                    .unwrap_or(false);
                let fresh = self
                    .last_ok
                    .map(|t| t.elapsed() < Duration::from_secs(5))
                    .unwrap_or(false);
                if net_up && fresh {
                    NetState::Connected
                } else if fresh {
                    NetState::Waiting
                } else {
                    NetState::Starting
                }
            }
        }
    }

    pub fn push_log(&mut self, line: String) {
        self.log.push_back(line);
        while self.log.len() > 400 {
            self.log.pop_front();
        }
    }

    /// True exactly once per agent crash (for toast notifications).
    pub fn take_crash(&mut self) -> bool {
        let Some(a) = self.agent.as_mut() else {
            return false;
        };
        if !a.is_running() && !self.notified_crash {
            self.notified_crash = true;
            return true;
        }
        false
    }

    pub fn reset_crash_notice(&mut self) {
        self.notified_crash = false;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NetState {
    Off,
    Starting,
    Waiting,
    Connected,
    Dead,
}

impl NetState {
    pub fn label(&self) -> &'static str {
        match self {
            NetState::Off => "Offline",
            NetState::Starting => "Starting",
            NetState::Waiting => "Waiting",
            NetState::Connected => "Connected",
            NetState::Dead => "Crashed",
        }
    }
}

// ---------------------------------------------------------------- spawn helpers

fn exe_name(base: &str) -> String {
    if cfg!(windows) {
        format!("{base}.exe")
    } else {
        base.to_string()
    }
}

/// Where the sibling binaries live (portable zip layout: all in one dir).
fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Resolve a sidecar binary: explicit override > exe dir > PATH.
pub fn resolve_binary(explicit: &str, base: &str) -> String {
    if !explicit.is_empty() && Path::new(&explicit).exists() {
        return explicit.to_string();
    }
    let sidecar = exe_dir().join(exe_name(base));
    if sidecar.exists() {
        return sidecar.to_string_lossy().into_owned();
    }
    exe_name(base) // fall back to PATH
}

#[cfg(windows)]
fn hide_console(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_console(_cmd: &mut Command) {}

fn pick_free_tcp_port() -> Option<u16> {
    TcpListener::bind("127.0.0.1:0")
        .ok()?
        .local_addr()
        .ok()?
        .port()
        .into()
}

/// A port that is free on both TCP and UDP (for the local coordinator).
fn pick_free_tcp_udp_port() -> Option<u16> {
    for _ in 0..64 {
        let p = pick_free_tcp_port()?;
        if UdpSocket::bind(("0.0.0.0", p)).is_ok() {
            return Some(p);
        }
    }
    None
}

/// Start the agent for network `idx` (spawned process + poller + log threads).
pub fn start_agent(
    cfg: &GuiConfig,
    idx: usize,
    net: &NetCfg,
    identity: &Path,
    tx: Sender<Event>,
) -> Result<AgentProc> {
    let status_port = pick_free_tcp_port().context("no free status port")?;
    let frame_port = pick_free_tcp_port().context("no free frame port")?;
    let bin = resolve_binary(&cfg.agent_path, "gg-lan-agent");
    let mut cmd = Command::new(bin);
    cmd.args([
        "--server",
        &net.server,
        "--code",
        &net.code,
        "--name",
        &cfg.nickname,
        "--status",
        &format!("127.0.0.1:{status_port}"),
        "--frame-port",
        &frame_port.to_string(),
        "--identity",
        &identity.to_string_lossy(),
    ]);
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    hide_console(&mut cmd);
    let mut child = cmd.spawn().context("spawn gg-lan-agent")?;

    let _ = tx.send(Event::Log(
        idx,
        format!(
            "agent started (status 127.0.0.1:{status_port}, frame bridge 127.0.0.1:{frame_port})"
        ),
    ));

    // stderr → log lines
    if let Some(err) = child.stderr.take() {
        let tx2 = tx.clone();
        std::thread::spawn(move || {
            let r = BufReader::new(err);
            for line in r.lines().map_while(Result::ok) {
                if tx2.send(Event::Log(idx, line)).is_err() {
                    break;
                }
            }
        });
    }

    // status poller
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop2 = Arc::clone(&stop);
        let tx2 = tx.clone();
        std::thread::spawn(move || {
            while !stop2.load(Ordering::Relaxed) {
                if let Ok(v) = http::get(status_port, "/status", Duration::from_secs(2))
                    .and_then(|b| serde_json::from_str::<serde_json::Value>(&b).map_err(Into::into))
                {
                    if tx2.send(Event::Status(idx, v)).is_err() {
                        break;
                    }
                    // agent status refreshed successfully
                }
                std::thread::sleep(Duration::from_millis(1000));
            }
        });
    }

    Ok(AgentProc {
        child,
        status_port,
        frame_port,
        stop,
    })
}

/// Fire-and-forget synthetic ping through a running agent.
pub fn fire_ping(idx: usize, status_port: u16, ip: String, tx: Sender<Event>) {
    std::thread::spawn(move || {
        let text = match http::get(
            status_port,
            &format!("/ping?ip={ip}"),
            Duration::from_secs(4),
        ) {
            Ok(body) => match serde_json::from_str::<serde_json::Value>(&body) {
                Ok(v) if v.get("ok").and_then(|o| o.as_bool()) == Some(true) => format!(
                    "{} ms",
                    v.get("rtt_ms").and_then(|r| r.as_u64()).unwrap_or(0)
                ),
                Ok(v) => format!(
                    "failed: {}",
                    v.get("error").and_then(|e| e.as_str()).unwrap_or("?")
                ),
                Err(_) => "bad response".into(),
            },
            Err(e) => format!("error: {e}"),
        };
        let _ = tx.send(Event::Ping(idx, ip, text));
    });
}

/// Start a local coordinator for a hosted network. Returns (proc, bound addr).
pub fn start_local_server(
    cfg: &GuiConfig,
    idx: usize,
    net: &NetCfg,
    cfg_dir: &Path,
    tx: Sender<Event>,
) -> Result<(ServerProc, String)> {
    let port = pick_free_tcp_udp_port().context("no free server port")?;
    let status_port = pick_free_tcp_port().context("no free status port")?;
    let bin = resolve_binary(&cfg.server_path, "gg-lan-server");
    let state_file = cfg_dir.join(format!("server-{}.key", slug(&net.name)));
    let mut cmd = Command::new(bin);
    cmd.args([
        "--ctrl",
        &format!("0.0.0.0:{port}"),
        "--udp",
        &format!("0.0.0.0:{port}"),
        "--status",
        &format!("127.0.0.1:{status_port}"),
        "--net-name",
        &net.name,
        "--join-code",
        &net.code,
        "--mode",
        &net.mode,
        "--state",
        &state_file.to_string_lossy(),
    ]);
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    hide_console(&mut cmd);
    let mut child = cmd.spawn().context("spawn gg-lan-server")?;

    if let Some(err) = child.stderr.take() {
        let tx2 = tx.clone();
        std::thread::spawn(move || {
            let r = BufReader::new(err);
            for line in r.lines().map_while(Result::ok) {
                if tx2.send(Event::Log(idx, line)).is_err() {
                    break;
                }
            }
        });
    }

    Ok((ServerProc { child }, format!("127.0.0.1:{port}")))
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars().take(24) {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        }
    }
    if out.is_empty() {
        "net".into()
    } else {
        out
    }
}

/// Best-effort "this machine's LAN address" for sharing hosted networks.
pub fn lan_address(port: u16) -> Option<String> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?; // no packets sent; picks the default route
    let ip = s.local_addr().ok()?.ip().to_string();
    Some(format!("{ip}:{port}"))
}

// ---------------------------------------------------------------- app state glue

/// Handle for the UI: the receiving end of the poller threads.
/// The `tx` half is cheap to clone into background threads.
pub struct EventBus {
    pub tx: Sender<Event>,
    pub rx: Receiver<Event>,
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self { tx, rx }
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}
