//! GG LAN coordinator/relay server.
//!
//! Control plane: TCP + X25519 static-static AEAD sessions.
//! Data plane: UDP — endpoint discovery (server pings), punch brokering, relay.

use anyhow::{anyhow, Context, Result};
use gg_lan_core::control::*;
use gg_lan_core::crypto::{b64, control_keys, unb64, StaticKeypair};
use gg_lan_core::wire::Packet;
use gg_lan_core::{ip_str, mac_str, virtual_mac, Mac, NetId, NodeId};
use rand::RngCore;
use serde_json::json;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU16, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{mpsc, Mutex};

/// Per-network state.
pub struct Net {
    pub id: NetId,
    pub name: String,
    pub mode: NetMode,
    pub subnet_base: u32,
    pub join_code: String,
    pub allocator: gg_lan_core::alloc::HostAllocator,
    pub peers: HashMap<NodeId, Peer>,
}

pub struct Peer {
    pub name: String,
    pub static_pub: [u8; 32],
    pub host: u32,
    pub mac: Mac,
    pub endpoints: Vec<SocketAddr>,
    /// Serialized SrvMsg (plaintext JSON) queue to this agent's control conn.
    pub tx: mpsc::Sender<Vec<u8>>,
}

#[derive(Default)]
pub struct Analytics {
    pub relayed_packets: AtomicU64,
    pub relayed_bytes: AtomicU64,
    pub udp_pings: AtomicU64,
    pub joined_total: AtomicU64,
}

pub struct ServerState {
    pub server_key: StaticKeypair,
    pub nets: Mutex<HashMap<NetId, Net>>,
    /// (net, node) → verified main UDP endpoint.
    pub udp_endpoints: Mutex<HashMap<(NetId, NodeId), SocketAddr>>,
    /// (net, node) → udp token issued at welcome.
    pub udp_tokens: Mutex<HashMap<(NetId, NodeId), u64>>,
    pub udp_port: AtomicU16,
    pub analytics: Analytics,
}

impl ServerState {
    async fn roster(net: &Net, exclude: Option<NodeId>) -> Vec<PeerInfo> {
        let mut peers: Vec<PeerInfo> = net
            .peers
            .iter()
            .filter(|(id, _)| Some(**id) != exclude)
            .map(|(id, p)| PeerInfo {
                id: *id,
                name: p.name.clone(),
                mac: mac_str(&p.mac),
                ip: ip_str(net.allocator.ip_of(p.host)),
                static_pub: b64(&p.static_pub),
                endpoints: p.endpoints.iter().map(|e| e.to_string()).collect(),
            })
            .collect();
        peers.sort_by_key(|p| p.id);
        peers
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let get = |k: &str, d: &str| -> String {
        let mut v = d.to_string();
        let mut it = args.iter().peekable();
        while let Some(a) = it.next() {
            if a == k {
                if let Some(n) = it.peek() {
                    v = (*n).clone();
                }
            }
        }
        v
    };

    let ctrl_addr = get("--ctrl", "0.0.0.0:7100");
    let udp_addr = get("--udp", "0.0.0.0:7100");
    let status_addr = get("--status", "127.0.0.1:7200");
    let net_name = get("--net-name", "gg-lan");
    let mode = if get("--mode", "mesh") == "hub" {
        NetMode::Hub
    } else {
        NetMode::Mesh
    };
    let subnet = get("--subnet", "");
    let join_code = get("--join-code", "");
    let state_file = get("--state", "");

    let state = Arc::new(ServerState {
        server_key: load_or_create_key(&state_file)?,
        nets: Mutex::new(HashMap::new()),
        udp_endpoints: Mutex::new(HashMap::new()),
        udp_tokens: Mutex::new(HashMap::new()),
        udp_port: AtomicU16::new(0),
        analytics: Analytics::default(),
    });

    {
        let base = if subnet.is_empty() {
            gg_lan_core::alloc::pick_subnet(&[]).context("no free subnet")?
        } else {
            let (b, p) = gg_lan_core::parse_cidr(&subnet).ok_or_else(|| anyhow!("bad --subnet"))?;
            if p != 16 {
                return Err(anyhow!("only /16 subnets are supported in v1"));
            }
            b
        };
        let code = if join_code.is_empty() {
            let mut b = [0u8; 6];
            rand::rngs::OsRng.fill_bytes(&mut b);
            b.iter().map(|x| format!("{:02x}", x)).collect::<String>()
        } else {
            join_code.clone()
        };
        let net = Net {
            id: 1,
            name: net_name,
            mode: mode.clone(),
            subnet_base: base,
            join_code: code,
            allocator: gg_lan_core::alloc::HostAllocator::new(&gg_lan_core::format_cidr(base, 16))
                .ok_or_else(|| anyhow!("allocator"))?,
            peers: HashMap::new(),
        };
        eprintln!(
            "network '{}' mode {:?} subnet {} join-code {}",
            net.name,
            net.mode,
            gg_lan_core::format_cidr(net.subnet_base, 16),
            net.join_code
        );
        state.nets.lock().await.insert(1, net);
    }

    let tcp = TcpListener::bind(&ctrl_addr).await?;
    let udp = Arc::new(UdpSocket::bind(&udp_addr).await?);
    let udp_port = udp.local_addr()?.port();
    state.udp_port.store(udp_port, Ordering::Relaxed);
    eprintln!(
        "gg-lan-server listening tcp://{ctrl_addr} udp://{udp_addr} status http://{status_addr}"
    );

    // status http server
    {
        let state = Arc::clone(&state);
        let addr: SocketAddr = status_addr.parse()?;
        std::thread::spawn(move || {
            let srv = match tiny_http::Server::http(addr) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("status bind failed: {e}");
                    return;
                }
            };
            for req in srv.incoming_requests() {
                if req.url() == "/status" || req.url() == "/" {
                    let body = status_json(&state);
                    let resp = tiny_http::Response::from_string(body).with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"application/json"[..],
                        )
                        .unwrap(),
                    );
                    let _ = req.respond(resp);
                } else {
                    let _ = req.respond(
                        tiny_http::Response::from_string("not found").with_status_code(404),
                    );
                }
            }
        });
    }

    spawn_udp_task(Arc::clone(&state), Arc::clone(&udp));

    loop {
        let (sock, peer_addr) = tcp.accept().await?;
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(e) = handle_agent(state, sock, peer_addr).await {
                eprintln!("[ctrl {peer_addr}] closed: {e:#}");
            }
        });
    }
}

fn status_json(state: &ServerState) -> String {
    let rt = tokio::runtime::Handle::try_current();
    let nets_json: Vec<serde_json::Value> = rt
        .map(|rt| {
            rt.block_on(async {
                let nets = state.nets.lock().await;
                nets.values()
                    .map(|net| {
                        let peers: Vec<serde_json::Value> = net
                            .peers
                            .values()
                            .map(|p| {
                                json!({
                                    "name": p.name,
                                    "ip": ip_str(net.allocator.ip_of(p.host)),
                                    "mac": mac_str(&p.mac),
                                    "endpoints": p.endpoints.iter().map(|e| e.to_string()).collect::<Vec<_>>(),
                                })
                            })
                            .collect();
                        json!({
                            "id": net.id,
                            "name": net.name,
                            "mode": format!("{:?}", net.mode),
                            "subnet": gg_lan_core::format_cidr(net.subnet_base, 16),
                            "peers": peers,
                        })
                    })
                    .collect()
            })
        })
        .unwrap_or_default();
    json!({
        "product": "gg-lan-server",
        "version": env!("CARGO_PKG_VERSION"),
        "networks": nets_json,
        "relay": {
            "packets": state.analytics.relayed_packets.load(Ordering::Relaxed),
            "bytes": state.analytics.relayed_bytes.load(Ordering::Relaxed),
        },
        "udp_pings": state.analytics.udp_pings.load(Ordering::Relaxed),
        "joined_total": state.analytics.joined_total.load(Ordering::Relaxed),
    })
    .to_string()
}

fn load_or_create_key(path: &str) -> Result<StaticKeypair> {
    if !path.is_empty() {
        if let Ok(s) = std::fs::read_to_string(path) {
            let bytes: [u8; 32] = unb64(s.trim())?
                .try_into()
                .map_err(|_| anyhow!("bad key file"))?;
            return Ok(StaticKeypair::from_bytes(bytes));
        }
        let kp = StaticKeypair::generate();
        std::fs::write(path, b64(&kp.secret_bytes()))?;
        return Ok(kp);
    }
    Ok(StaticKeypair::generate())
}

fn spawn_udp_task(state: Arc<ServerState>, udp: Arc<UdpSocket>) {
    tokio::spawn(async move {
        let mut buf = vec![0u8; 70_000];
        loop {
            let (n, src) = match udp.recv_from(&mut buf).await {
                Ok(v) => v,
                Err(_) => continue,
            };
            let pkt = match Packet::decode(&buf[..n]) {
                Ok(p) => p,
                Err(_) => continue,
            };
            match pkt {
                Packet::ServerPing {
                    pong,
                    net,
                    node,
                    token,
                } => {
                    if pong {
                        continue;
                    }
                    let ok = {
                        let toks = state.udp_tokens.lock().await;
                        toks.get(&(net, node)) == Some(&token)
                    };
                    if !ok {
                        continue;
                    }
                    state.analytics.udp_pings.fetch_add(1, Ordering::Relaxed);
                    state.udp_endpoints.lock().await.insert((net, node), src);
                    update_peer_endpoints(&state, net, node, vec![src], false).await;
                    let reply = Packet::ServerPing {
                        pong: true,
                        net,
                        node,
                        token,
                    }
                    .encode();
                    let _ = udp.send_to(&reply, src).await;
                }
                Packet::Relay {
                    net,
                    from,
                    to,
                    inner,
                } => {
                    let target = {
                        let eps = state.udp_endpoints.lock().await;
                        let src_ok = eps.get(&(net, from)) == Some(&src);
                        if src_ok {
                            eps.get(&(net, to)).copied()
                        } else {
                            None
                        }
                    };
                    if let Some(dst) = target {
                        state
                            .analytics
                            .relayed_packets
                            .fetch_add(1, Ordering::Relaxed);
                        state
                            .analytics
                            .relayed_bytes
                            .fetch_add(inner.len() as u64, Ordering::Relaxed);
                        let _ = udp.send_to(&inner, dst).await;
                    }
                }
                _ => {}
            }
        }
    });
}

async fn update_peer_endpoints(
    state: &ServerState,
    net_id: NetId,
    node: NodeId,
    eps: Vec<SocketAddr>,
    force: bool,
) {
    let mut changed = false;
    {
        let mut nets = state.nets.lock().await;
        if let Some(net) = nets.get_mut(&net_id) {
            if let Some(p) = net.peers.get_mut(&node) {
                for e in eps {
                    if !p.endpoints.contains(&e) {
                        p.endpoints.push(e);
                        if p.endpoints.len() > 8 {
                            p.endpoints.remove(0);
                        }
                        changed = true;
                    }
                }
            }
        }
    }
    if changed || force {
        broadcast_roster(state, net_id).await;
    }
}

/// Push a roster snapshot (+ punch orders in mesh mode) to every member.
async fn broadcast_roster(state: &ServerState, net_id: NetId) {
    // Gather serialized messages per recipient while holding the lock briefly.
    let mut sends: Vec<(mpsc::Sender<Vec<u8>>, Vec<u8>)> = Vec::new();
    let mut punch_orders: Vec<(mpsc::Sender<Vec<u8>>, Vec<u8>)> = Vec::new();
    {
        let nets = state.nets.lock().await;
        let Some(net) = nets.get(&net_id) else { return };
        for (pid, p) in net.peers.iter() {
            let upd = SrvMsg::PeersUpdate {
                peers: ServerState::roster(net, Some(*pid)).await,
                removed: Vec::new(),
            };
            sends.push((p.tx.clone(), serde_json::to_vec(&upd).expect("json")));
        }
        if matches!(net.mode, NetMode::Mesh) {
            let ids: Vec<NodeId> = net.peers.keys().copied().collect();
            for &a in &ids {
                for &b in &ids {
                    if a == b {
                        continue;
                    }
                    let Some(pa) = net.peers.get(&a) else {
                        continue;
                    };
                    let Some(pb) = net.peers.get(&b) else {
                        continue;
                    };
                    let token = pair_token(a, b, net_id);
                    let order = SrvMsg::PunchOrder {
                        peer: b,
                        token,
                        endpoints: pb.endpoints.iter().map(|e| e.to_string()).collect(),
                    };
                    punch_orders.push((pa.tx.clone(), serde_json::to_vec(&order).expect("json")));
                }
            }
        }
    }
    for (tx, msg) in sends.into_iter().chain(punch_orders) {
        let _ = tx.try_send(msg);
    }
}

fn pair_token(a: NodeId, b: NodeId, net: NetId) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (a.min(b), a.max(b), net, "punch-v1").hash(&mut h);
    h.finish()
}

async fn handle_agent(
    state: Arc<ServerState>,
    mut sock: TcpStream,
    peer_addr: SocketAddr,
) -> Result<()> {
    sock.set_nodelay(true).ok();
    let line = read_json_line(&mut sock).await?;
    let hello: Hello = serde_json::from_slice(&line).context("bad hello")?;
    if hello.v != 1 {
        let rep = HelloReply {
            ok: false,
            reason: Some("version".into()),
            server_static: None,
        };
        let mut out = serde_json::to_vec(&rep)?;
        out.push(b'\n');
        sock.write_all(&out).await?;
        return Err(anyhow!("unsupported version"));
    }
    let agent_static = parse_static_pub(&hello.static_pub)?;

    let (net_id, host, mac) = {
        let mut nets = state.nets.lock().await;
        let mut found: Option<(NetId, u32, Mac)> = None;
        for net in nets.values_mut() {
            if net.join_code != hello.join_code {
                continue;
            }
            if net.peers.contains_key(&hello.node_id) {
                continue;
            }
            let Some(host) = net.allocator.assign(hello.node_id) else {
                continue;
            };
            let mac = virtual_mac(net.id, hello.node_id);
            found = Some((net.id, host, mac));
            break;
        }
        found.ok_or_else(|| anyhow!("invalid join code or network full"))?
    };

    let rep = HelloReply {
        ok: true,
        reason: None,
        server_static: Some(b64(&state.server_key.public_bytes())),
    };
    let mut out = serde_json::to_vec(&rep)?;
    out.push(b'\n');
    sock.write_all(&out).await?;

    let salt = format!("ggl-ctrl-{}", hello.node_id);
    let keys = control_keys(&state.server_key, &agent_static, salt.as_bytes());
    let chan = Arc::new(tokio::sync::Mutex::new(CtrlChannel::server(keys)));

    let (wtx, mut wrx) = mpsc::channel::<Vec<u8>>(256);
    {
        let mut nets = state.nets.lock().await;
        let net = nets.get_mut(&net_id).unwrap();
        net.peers.insert(
            hello.node_id,
            Peer {
                name: hello.name.clone(),
                static_pub: agent_static,
                host,
                mac,
                endpoints: Vec::new(),
                tx: wtx.clone(),
            },
        );
    }
    state.analytics.joined_total.fetch_add(1, Ordering::Relaxed);

    let mut welcome = {
        let nets = state.nets.lock().await;
        let net = nets.get(&net_id).unwrap();
        SrvMsg::Welcome {
            net_id: net.id,
            net_name: net.name.clone(),
            subnet: gg_lan_core::format_cidr(net.subnet_base, 16),
            mode: net.mode.clone(),
            my_ip: ip_str(net.allocator.ip_of(host)),
            my_mac: mac_str(&mac),
            my_host: (host & 0xffff) as u16,
            relay_udp: String::new(),
            udp_token: 0,
            peers: ServerState::roster(net, Some(hello.node_id)).await,
        }
    };
    if let SrvMsg::Welcome {
        relay_udp,
        udp_token,
        ..
    } = &mut welcome
    {
        let up = state.udp_port.load(Ordering::Relaxed);
        *relay_udp = format!("{}:{}", peer_addr.ip(), up);
        let tok = rand::rngs::OsRng.next_u64();
        *udp_token = tok;
        state
            .udp_tokens
            .lock()
            .await
            .insert((net_id, hello.node_id), tok);
    }

    // writer task: encrypt + send everything queued on wtx, plus the welcome.
    {
        let (r, mut w) = sock.into_split();
        let first = { chan.lock().await.seal(&welcome) };
        w.write_all(&first).await?;
        let chan_w = Arc::clone(&chan);
        tokio::spawn(async move {
            while let Some(plain) = wrx.recv().await {
                let framed = {
                    let mut c = chan_w.lock().await;
                    // Re-frame from raw serialized SrvMsg JSON.
                    c.seal_bytes(&plain)
                };
                if w.write_all(&framed).await.is_err() {
                    break;
                }
            }
        });
        // reader continues on r below
        reader_loop(Arc::clone(&state), net_id, hello.clone(), r, chan, wtx).await?;
    }

    // cleanup
    {
        let mut nets = state.nets.lock().await;
        if let Some(net) = nets.get_mut(&net_id) {
            if let Some(p) = net.peers.remove(&hello.node_id) {
                net.allocator.release(p.host);
            }
        }
        state
            .udp_endpoints
            .lock()
            .await
            .remove(&(net_id, hello.node_id));
        state
            .udp_tokens
            .lock()
            .await
            .remove(&(net_id, hello.node_id));
    }
    eprintln!("[-] left '{}' node {}", hello.name, hello.node_id);
    broadcast_roster(&state, net_id).await;
    Ok(())
}

async fn reader_loop(
    state: Arc<ServerState>,
    net_id: NetId,
    hello: Hello,
    mut r: tokio::net::tcp::OwnedReadHalf,
    chan: Arc<tokio::sync::Mutex<CtrlChannel>>,
    wtx: mpsc::Sender<Vec<u8>>,
) -> Result<()> {
    let mut codec = CtrlCodec::new();
    let mut rbuf = vec![0u8; 8192];
    loop {
        let n = r.read(&mut rbuf).await?;
        if n == 0 {
            break;
        }
        let mut frames = Vec::new();
        codec.feed(&rbuf[..n], |f| frames.push(f));
        for f in frames {
            let msg: AgtMsg = {
                let mut c = chan.lock().await;
                match c.open(&f) {
                    Ok(m) => m,
                    Err(_) => return Err(anyhow!("ctrl frame auth failed")),
                }
            };
            match msg {
                AgtMsg::Ping => {
                    let pong = serde_json::to_vec(&SrvMsg::Pong).expect("json");
                    let _ = wtx.try_send(pong);
                }
                AgtMsg::EndpointsAnnounce { endpoints } => {
                    let eps: Vec<SocketAddr> =
                        endpoints.iter().filter_map(|e| e.parse().ok()).collect();
                    update_peer_endpoints(&state, net_id, hello.node_id, eps, false).await;
                }
                AgtMsg::Stats { .. } => {}
                AgtMsg::Leave => return Ok(()),
            }
        }
    }
    Ok(())
}
