//! GG LAN peer agent: joins a network, punches through NATs (mesh mode) or
//! relays through the server (hub mode), and bridges a virtual NIC into an
//! encrypted L2 mesh.

use anyhow::{anyhow, Context, Result};
use gg_lan_core::arp::*;
use gg_lan_core::control::*;
use gg_lan_core::crypto::{
    b64, control_keys, unb64, AeadCipher, HandshakeInitiator, HandshakeResponder, Session,
    StaticKeypair, PUB_LEN,
};
use gg_lan_core::nic::{FrameTx, UserspaceNic, VirtualNic};
use gg_lan_core::wire::Packet;
use gg_lan_core::{parse_ip, parse_mac, Mac, NodeId};
use rand::RngCore;
use serde_json::json;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::{mpsc, oneshot, Mutex};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Debug, Clone, PartialEq)]
enum LinkPhase {
    Punching,
    Direct,
    Relay,
}

struct PeerLink {
    info: PeerInfo,
    phase: LinkPhase,
    token: u64,
    endpoints: Vec<SocketAddr>,
    direct: Option<(Arc<UdpSocket>, SocketAddr)>,
    session: Option<Session>,
    initiator: Option<HandshakeInitiator>,
    /// Encoded HELLO_ACK of the last completed inbound handshake (for ACK-loss retries).
    last_ack: Option<Vec<u8>>,
    queue: Vec<Vec<u8>>,
    bytes_out: u64,
    bytes_in: u64,
    pkts_out: u64,
    pkts_in: u64,
    rtt: Option<u64>,
    last_activity: Instant,
}

#[derive(Default)]
struct Counters {
    tx_bytes: AtomicU64,
    rx_bytes: AtomicU64,
    tx_pkts: AtomicU64,
    rx_pkts: AtomicU64,
}

struct Series {
    samples: StdMutex<Vec<(u64, u64)>>,
    last_rx: AtomicU64,
    last_tx: AtomicU64,
}

#[derive(Clone)]
struct NetCtx {
    id: u32,
    name: String,
    subnet: String,
    mode: NetMode,
    my_ip: u32,
    my_mac: Mac,
    #[allow(dead_code)]
    udp_token: u64,
}

pub struct Agent {
    node_id: NodeId,
    name: String,
    statics: StaticKeypair,
    peers: Mutex<HashMap<NodeId, PeerLink>>,
    macs: StdMutex<HashMap<Mac, NodeId>>,
    sockets: Vec<Arc<UdpSocket>>,
    server_ctrl: StdMutex<Option<SocketAddr>>,
    server_udp: StdMutex<Option<SocketAddr>>,
    #[allow(dead_code)]
    ctrl_tx: mpsc::Sender<Vec<u8>>,
    counters: Counters,
    series: Series,
    net: StdMutex<Option<NetCtx>>,
    nic: StdMutex<Option<Arc<dyn VirtualNic>>>,
    pending_pings: StdMutex<HashMap<(u16, u16), oneshot::Sender<u64>>>,
}

fn get_arg(k: &str, d: &str) -> String {
    let args: Vec<String> = std::env::args().collect();
    let mut it = args.iter().peekable();
    let mut v = d.to_string();
    while let Some(a) = it.next() {
        if a == k {
            if let Some(n) = it.peek() {
                v = (*n).clone();
            }
        }
    }
    v
}

#[tokio::main]
async fn main() -> Result<()> {
    let server = get_arg("--server", "");
    if server.is_empty() {
        eprintln!(
            "usage: gg-lan-agent --server host:port --code <join-code> --name <name> \
             [--frame-port 7900] [--status 127.0.0.1:7800] [--identity file]"
        );
        std::process::exit(2);
    }
    let code = get_arg("--code", "");
    let name = get_arg("--name", "peer");
    let frame_port: u16 = get_arg("--frame-port", "7900")
        .parse()
        .context("frame-port")?;
    let status_bind = get_arg("--status", "127.0.0.1:7800");
    let identity_file = get_arg("--identity", "");

    let (node_id, statics) = load_identity(&identity_file)?;

    let mut sockets = Vec::new();
    for _ in 0..5 {
        let s = UdpSocket::bind("0.0.0.0:0").await?;
        sockets.push(Arc::new(s));
    }

    let (ctrl_tx, ctrl_rx) = mpsc::channel::<Vec<u8>>(64);

    let agent = Arc::new(Agent {
        node_id,
        name: name.clone(),
        statics,
        peers: Mutex::new(HashMap::new()),
        macs: StdMutex::new(HashMap::new()),
        sockets,
        server_ctrl: StdMutex::new(server.parse().ok()),
        server_udp: StdMutex::new(None),
        ctrl_tx,
        counters: Counters::default(),
        series: Series {
            samples: StdMutex::new(Vec::new()),
            last_rx: AtomicU64::new(0),
            last_tx: AtomicU64::new(0),
        },
        net: StdMutex::new(None),
        nic: StdMutex::new(None),
        pending_pings: StdMutex::new(HashMap::new()),
    });

    eprintln!(
        "gg-lan-agent v{} node={} name='{}' → {}",
        env!("CARGO_PKG_VERSION"),
        node_id,
        name,
        server
    );

    // Userspace virtual NIC (frame bridge).
    {
        let a = Arc::clone(&agent);
        let rt = tokio::runtime::Handle::current();
        let handler: FrameTx = Arc::new(move |frame: &[u8]| {
            let a = Arc::clone(&a);
            let f = frame.to_vec();
            rt.spawn(async move {
                a.route_outbound(f).await;
            });
        });
        let nic = UserspaceNic::bind(frame_port, handler)?;
        eprintln!("virtual nic: {}", nic.describe());
        *agent.nic.lock().unwrap() = Some(nic);
    }

    // UDP socket receive loops (sockets[0] is the main/relay socket).
    for (ix, sock) in agent.sockets.iter().enumerate() {
        let a = Arc::clone(&agent);
        let sock = Arc::clone(sock);
        tokio::spawn(async move {
            a.udp_loop(sock, ix == 0).await;
        });
    }

    // Periodic series sampling.
    {
        let a = Arc::clone(&agent);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            loop {
                tick.tick().await;
                let rx = a.counters.rx_bytes.load(Ordering::Relaxed);
                let tx = a.counters.tx_bytes.load(Ordering::Relaxed);
                let prx = a.counters.rx_bytes.load(Ordering::Relaxed);
                let _ = prx;
                let mut s = a.series.samples.lock().unwrap();
                let (lrx, ltx) = (
                    a.series.last_rx.load(Ordering::Relaxed),
                    a.series.last_tx.load(Ordering::Relaxed),
                );
                s.push((rx.saturating_sub(lrx) * 8, tx.saturating_sub(ltx) * 8));
                if s.len() > 60 {
                    s.remove(0);
                }
                drop(s);
                a.series.last_rx.store(rx, Ordering::Relaxed);
                a.series.last_tx.store(tx, Ordering::Relaxed);
            }
        });
    }

    // Control-plane connection (reconnecting).
    {
        let a = Arc::clone(&agent);
        tokio::spawn(async move {
            a.control_loop(&server, &code, ctrl_rx).await;
        });
    }

    // Status HTTP API on a dedicated (non-runtime) thread so its handlers may
    // legally use Handle::block_on to peek at async state.
    {
        let rt = tokio::runtime::Handle::current();
        let a = Arc::clone(&agent);
        let bind = status_bind.clone();
        std::thread::Builder::new()
            .name("status-api".into())
            .spawn(move || {
                if let Err(e) = run_status_api(a, &bind, rt) {
                    eprintln!("status api: {e:#}");
                }
            })
            .expect("spawn status thread");
    }

    // Park the main task until Ctrl-C.
    tokio::signal::ctrl_c().await.ok();
    Ok(())
}

fn load_identity(path: &str) -> Result<(NodeId, StaticKeypair)> {
    if !path.is_empty() {
        if let Ok(s) = std::fs::read_to_string(path) {
            let v: serde_json::Value = serde_json::from_str(&s)?;
            let id = v["node_id"].as_u64().context("node_id")?;
            let sec: [u8; 32] = unb64(v["secret"].as_str().context("secret")?)?
                .try_into()
                .map_err(|_| anyhow!("bad secret"))?;
            return Ok((id, StaticKeypair::from_bytes(sec)));
        }
        let id = rand::rngs::OsRng.next_u64();
        let kp = StaticKeypair::generate();
        std::fs::write(
            path,
            json!({"node_id": id, "secret": b64(&kp.secret_bytes())}).to_string(),
        )?;
        return Ok((id, kp));
    }
    Ok((rand::rngs::OsRng.next_u64(), StaticKeypair::generate()))
}

// ---------------------------------------------------------------- control

impl Agent {
    async fn control_loop(
        self: Arc<Self>,
        server: &str,
        code: &str,
        mut rx: mpsc::Receiver<Vec<u8>>,
    ) {
        loop {
            match Arc::clone(&self)
                .control_session(server, code, &mut rx)
                .await
            {
                Ok(()) => eprintln!("[ctrl] session ended"),
                Err(e) => eprintln!("[ctrl] error: {e:#}"),
            }
            self.peers.lock().await.clear();
            *self.net.lock().unwrap() = None;
            eprintln!("[ctrl] reconnecting in 3s…");
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    }

    async fn control_session(
        self: Arc<Self>,
        server: &str,
        code: &str,
        rx: &mut mpsc::Receiver<Vec<u8>>,
    ) -> Result<()> {
        let mut sock = TcpStream::connect(server).await?;
        sock.set_nodelay(true).ok();
        let (mut r, mut w) = sock.split();

        let hello = Hello {
            v: 1,
            node_id: self.node_id,
            name: self.name.clone(),
            static_pub: b64(&self.statics.public_bytes()),
            join_code: code.to_string(),
        };
        let mut line = serde_json::to_vec(&hello)?;
        line.push(b'\n');
        w.write_all(&line).await?;

        let reply: HelloReply = serde_json::from_slice(&read_json_line(&mut r).await?)?;
        if !reply.ok {
            return Err(anyhow!("server rejected: {:?}", reply.reason));
        }
        let server_static = parse_static_pub(&reply.server_static.unwrap_or_default())?;
        let salt = format!("ggl-ctrl-{}", self.node_id);
        let keys = control_keys(&self.statics, &server_static, salt.as_bytes());
        let mut chan = CtrlChannel::agent(keys);

        let mut codec = CtrlCodec::new();
        let mut buf = vec![0u8; 8192];
        let mut ping = tokio::time::interval(Duration::from_secs(5));
        ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                n = r.read(&mut buf) => {
                    let n = n?;
                    if n == 0 { return Ok(()); }
                    let mut frames = Vec::new();
                    codec.feed(&buf[..n], |f| frames.push(f));
                    for f in frames {
                        let msg: SrvMsg = chan.open(&f)?;
                        self.clone().handle_srv(msg).await?;
                    }
                }
                Some(plain) = rx.recv() => {
                    let framed = chan.seal_bytes(&plain);
                    w.write_all(&framed).await?;
                }
                _ = ping.tick() => {
                    let p = serde_json::to_vec(&AgtMsg::Ping)?;
                    let framed = chan.seal_bytes(&p);
                    w.write_all(&framed).await?;
                }
            }
        }
    }

    async fn handle_srv(self: Arc<Self>, msg: SrvMsg) -> Result<()> {
        match msg {
            SrvMsg::Welcome {
                net_id,
                net_name,
                subnet,
                mode,
                my_ip,
                my_mac,
                relay_udp,
                udp_token,
                peers,
                my_host: _,
            } => {
                if let Ok(ra) = relay_udp.parse::<SocketAddr>() {
                    let ip = if ra.ip().is_unspecified() {
                        self.server_ctrl
                            .lock()
                            .unwrap()
                            .map(|c| c.ip())
                            .unwrap_or(ra.ip())
                    } else {
                        ra.ip()
                    };
                    *self.server_udp.lock().unwrap() = Some(SocketAddr::new(ip, ra.port()));
                }
                *self.net.lock().unwrap() = Some(NetCtx {
                    id: net_id,
                    name: net_name.clone(),
                    subnet,
                    mode: mode.clone(),
                    my_ip: parse_ip(&my_ip).context("my ip")?,
                    my_mac: parse_mac(&my_mac).context("my mac")?,
                    udp_token,
                });
                eprintln!("[net] joined '{net_name}' — I am {my_ip} ({my_mac}), mode {mode:?}");
                Arc::clone(&self).sync_peers(peers).await?;

                // register UDP endpoints with the server
                self.announce_udp().await;
                let a = Arc::clone(&self);
                tokio::spawn(async move {
                    let mut t = tokio::time::interval(Duration::from_secs(2));
                    loop {
                        t.tick().await;
                        a.announce_udp().await;
                    }
                });
            }
            SrvMsg::PeersUpdate { peers, .. } => {
                Arc::clone(&self).sync_peers(peers).await?;
            }
            SrvMsg::PunchOrder {
                peer,
                token,
                endpoints,
            } => {
                let mut ps = self.peers.lock().await;
                if let Some(link) = ps.get_mut(&peer) {
                    link.token = token;
                    let eps: Vec<SocketAddr> =
                        endpoints.iter().filter_map(|e| e.parse().ok()).collect();
                    if !eps.is_empty() {
                        link.endpoints = eps;
                    }
                }
                drop(ps);
                let mesh = self.is_mesh();
                if mesh {
                    let a = Arc::clone(&self);
                    tokio::spawn(async move {
                        a.start_punch(peer).await;
                    });
                }
            }
            SrvMsg::Pong => {}
            SrvMsg::Bye { reason } => return Err(anyhow!("bye: {reason}")),
        }
        Ok(())
    }

    fn is_mesh(&self) -> bool {
        matches!(
            self.net.lock().unwrap().as_ref().map(|n| &n.mode),
            Some(NetMode::Mesh)
        )
    }

    async fn announce_udp(&self) {
        let (net_id, token, srv) = {
            let n = self.net.lock().unwrap();
            match n.as_ref() {
                Some(c) => (c.id, c.udp_token, *self.server_udp.lock().unwrap()),
                None => return,
            }
        };
        let Some(srv) = srv else { return };
        // Only the main socket registers with the relay so the server's source
        // check stays stable; aux sockets exist purely for punch fan-out.
        let p = Packet::ServerPing {
            pong: false,
            net: net_id,
            node: self.node_id,
            token,
        };
        let _ = self.sockets[0].send_to(&p.encode(), srv).await;
    }

    async fn sync_peers(self: Arc<Self>, peers: Vec<PeerInfo>) -> Result<()> {
        let mut to_punch: Vec<NodeId> = Vec::new();
        {
            let mut ps = self.peers.lock().await;
            let ids: Vec<NodeId> = peers.iter().map(|p| p.id).collect();
            ps.retain(|id, _| ids.contains(id));

            for info in peers {
                if info.id == self.node_id {
                    continue; // never mesh with ourselves
                }
                let Some(mac) = parse_mac(&info.mac) else {
                    continue;
                };
                self.macs.lock().unwrap().insert(mac, info.id);
                let fresh_eps: Vec<SocketAddr> = info
                    .endpoints
                    .iter()
                    .filter_map(|s| s.parse().ok())
                    .collect();
                if let Some(link) = ps.get_mut(&info.id) {
                    link.info = info.clone();
                    if link.endpoints.is_empty() && !fresh_eps.is_empty() {
                        link.endpoints = fresh_eps;
                        if self.is_mesh() {
                            to_punch.push(info.id);
                        }
                    }
                } else {
                    let phase = if self.is_mesh() {
                        LinkPhase::Punching
                    } else {
                        LinkPhase::Relay
                    };
                    if matches!(phase, LinkPhase::Punching) {
                        to_punch.push(info.id);
                    } else if self.node_id < info.id {
                        // hub mode: the lower node id drives the relayed handshake
                        let a = Arc::clone(&self);
                        let pid = info.id;
                        tokio::spawn(async move {
                            a.relay_handshake_retry(pid).await;
                        });
                    }
                    ps.insert(
                        info.id,
                        PeerLink {
                            info: info.clone(),
                            phase,
                            token: 0,
                            endpoints: fresh_eps,
                            direct: None,
                            session: None,
                            initiator: None,
                            last_ack: None,
                            queue: Vec::new(),
                            bytes_out: 0,
                            bytes_in: 0,
                            pkts_out: 0,
                            pkts_in: 0,
                            rtt: None,
                            last_activity: Instant::now(),
                        },
                    );
                }
            }
            let names: Vec<String> = ps
                .values()
                .map(|p| format!("{}({})", p.info.name, p.info.ip))
                .collect();
            if !names.is_empty() {
                eprintln!("[peers] {} known: {}", ps.len(), names.join(", "));
            }
        }
        for id in to_punch {
            let a = Arc::clone(&self);
            tokio::spawn(async move {
                a.start_punch(id).await;
            });
        }
        Ok(())
    }

    // ---------------------------------------------------------------- punch

    async fn start_punch(self: Arc<Self>, peer: NodeId) {
        if peer == self.node_id {
            return;
        }
        let initiator_side = self.node_id < peer;
        let (endpoints, token, spub, already) = {
            let mut ps = self.peers.lock().await;
            let Some(link) = ps.get_mut(&peer) else {
                return;
            };
            let already = matches!(link.phase, LinkPhase::Direct);
            if !already && initiator_side && link.initiator.is_none() {
                if let Ok(sp) = parse_static_pub(&link.info.static_pub) {
                    if let Some(ctx) = self.net.lock().unwrap().clone() {
                        link.initiator = Some(HandshakeInitiator::new(
                            &self.statics,
                            &sp,
                            ctx.id,
                            self.node_id,
                            peer,
                        ));
                    }
                }
            }
            (
                link.endpoints.clone(),
                link.token,
                parse_static_pub(&link.info.static_pub).ok(),
                already,
            )
        };
        if already || spub.is_none() {
            return;
        }
        let net_id = self.net.lock().unwrap().as_ref().map(|n| n.id).unwrap_or(0);
        let a = Arc::clone(&self);
        a.punch_loop(peer, net_id, endpoints, token, initiator_side)
            .await;
    }

    async fn punch_loop(
        self: Arc<Self>,
        peer: NodeId,
        net_id: u32,
        endpoints: Vec<SocketAddr>,
        token: u64,
        initiator: bool,
    ) {
        const WAVE_DELAYS_MS: [u64; 4] = [0, 60, 180, 450];
        eprintln!(
            "[punch] → peer {peer} ({} endpoints, initiator={initiator})",
            endpoints.len()
        );
        for (wave, delay) in WAVE_DELAYS_MS.iter().enumerate() {
            if *delay > 0 {
                tokio::time::sleep(Duration::from_millis(*delay)).await;
            }
            match self.peers.lock().await.get(&peer) {
                Some(l) if matches!(l.phase, LinkPhase::Direct) => return,
                None => return,
                _ => {}
            }
            if self.net.lock().unwrap().as_ref().is_none() {
                return; // disconnected
            }
            for sock in self.sockets.iter() {
                for ep in endpoints.iter() {
                    let p = Packet::Punch {
                        wave: wave as u8,
                        port_ix: 0,
                        net: net_id,
                        from: self.node_id,
                        token,
                    };
                    let _ = sock.send_to(&p.encode(), ep).await;
                }
            }
            if initiator && wave >= 1 {
                if let Some(ep) = endpoints.first().cloned() {
                    let sock = Arc::clone(&self.sockets[0]);
                    self.send_hello(peer, Some((sock, ep))).await;
                }
            }
        }
        if initiator {
            tokio::time::sleep(Duration::from_millis(300)).await;
            if let Some(l) = self.peers.lock().await.get(&peer) {
                if matches!(l.phase, LinkPhase::Direct) {
                    return;
                }
            }
            if let Some(ep) = endpoints.first().cloned() {
                let sock = Arc::clone(&self.sockets[0]);
                self.send_hello(peer, Some((sock, ep))).await;
            }
        }
        // Give the handshake some time, then fall back to relay.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let mut ps = self.peers.lock().await;
        if let Some(l) = ps.get_mut(&peer) {
            if !matches!(l.phase, LinkPhase::Direct) {
                eprintln!("[punch] peer {peer}: falling back to relay");
                l.phase = LinkPhase::Relay;
                let q = std::mem::take(&mut l.queue);
                drop(ps);
                self.flush_queue(peer, q).await;
                // try to establish the session through the relay
                if initiator {
                    let a = Arc::clone(&self);
                    tokio::spawn(async move {
                        a.relay_handshake_retry(peer).await;
                    });
                }
            }
        }
    }

    async fn send_hello(&self, peer: NodeId, direct: Option<(Arc<UdpSocket>, SocketAddr)>) {
        let Some(ctx) = self.net.lock().unwrap().clone() else {
            return;
        };
        let (eph_pub, proof) = {
            let mut ps = self.peers.lock().await;
            let Some(l) = ps.get_mut(&peer) else { return };
            if l.initiator.is_none() {
                let Ok(sp) = parse_static_pub(&l.info.static_pub) else {
                    return;
                };
                l.initiator = Some(HandshakeInitiator::new(
                    &self.statics,
                    &sp,
                    ctx.id,
                    self.node_id,
                    peer,
                ));
            }
            match l.initiator.as_ref() {
                Some(i) => (i.eph_pub, i.proof(ctx.id, self.node_id, peer)),
                None => return,
            }
        };
        let pkt = Packet::Hello {
            ack: false,
            net: ctx.id,
            from: self.node_id,
            to: peer,
            eph_pub,
            proof,
        };
        if let Some((sock, ep)) = direct {
            let _ = sock.send_to(&pkt.encode(), ep).await;
        } else {
            self.send_packet_bytes(peer, pkt.encode()).await;
        }
    }

    /// Choose the current transport for a raw packet to a peer: direct when a
    /// direct path exists, otherwise wrapped through the server relay.
    async fn send_packet_bytes(&self, peer: NodeId, bytes: Vec<u8>) {
        let path = {
            let ps = self.peers.lock().await;
            let Some(l) = ps.get(&peer) else { return };
            match (&l.phase, &l.direct) {
                (LinkPhase::Direct, Some((s, ep))) => Some((Arc::clone(s), *ep, false)),
                _ => {
                    let srv = *self.server_udp.lock().unwrap();
                    srv.map(|s| (Arc::clone(&self.sockets[0]), s, true))
                }
            }
        };
        let Some((sock, ep, wrap)) = path else { return };
        let out = if wrap {
            let net = self.net.lock().unwrap().as_ref().map(|c| c.id).unwrap_or(0);
            Packet::Relay {
                net,
                from: self.node_id,
                to: peer,
                inner: bytes,
            }
            .encode()
        } else {
            bytes
        };
        let _ = sock.send_to(&out, ep).await;
    }

    /// Hub-mode (or unpunchable-mesh) handshake: retry HELLO through the relay
    /// until the session comes up.
    async fn relay_handshake_retry(self: Arc<Self>, peer: NodeId) {
        for attempt in 0..10 {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            let up = {
                let ps = self.peers.lock().await;
                match ps.get(&peer) {
                    Some(l) => l.session.is_some(),
                    None => return,
                }
            };
            if up {
                return;
            }
            if attempt == 0 {
                eprintln!("[relay] initiating handshake via relay → peer {peer}");
            }
            self.send_hello(peer, None).await;
        }
    }

    async fn flush_queue(&self, peer: NodeId, q: Vec<Vec<u8>>) {
        for f in q {
            self.send_to_peer(peer, &f).await;
        }
    }

    // ---------------------------------------------------------------- udp

    async fn udp_loop(self: Arc<Self>, sock: Arc<UdpSocket>, is_main: bool) {
        let mut buf = vec![0u8; 70_000];
        loop {
            let Ok((n, src)) = sock.recv_from(&mut buf).await else {
                return;
            };
            if n == 0 {
                continue;
            }
            self.clone()
                .handle_udp(&sock, is_main, &buf[..n], src)
                .await;
        }
    }

    async fn handle_udp(
        self: Arc<Self>,
        sock: &Arc<UdpSocket>,
        _is_main: bool,
        data: &[u8],
        src: SocketAddr,
    ) {
        let srv = *self.server_udp.lock().unwrap();
        let from_server = srv == Some(src);
        let pkt = match Packet::decode(data) {
            Ok(p) => p,
            Err(_) => return,
        };
        let net_ok = |net: u32| self.net.lock().unwrap().as_ref().map(|c| c.id) == Some(net);
        match pkt {
            Packet::ServerPing { .. } => {}
            Packet::Punch {
                net, from, token, ..
            } => {
                if !net_ok(net) {
                    return;
                }
                let reply_needed = {
                    let ps = self.peers.lock().await;
                    match ps.get(&from) {
                        Some(l) => l.token == token,
                        None => false,
                    }
                };
                if reply_needed {
                    let p = Packet::Punch {
                        wave: 9,
                        port_ix: 0,
                        net,
                        from: self.node_id,
                        token,
                    };
                    let _ = sock.send_to(&p.encode(), src).await;
                }
            }
            Packet::Hello {
                ack: false,
                net,
                from,
                to,
                eph_pub,
                proof,
            } => {
                if to != self.node_id || !net_ok(net) {
                    return;
                }
                let via_relay = from_server;
                self.clone()
                    .handle_hello_init(sock, src, via_relay, net, from, eph_pub, proof)
                    .await;
            }
            Packet::Hello {
                ack: true,
                net,
                from,
                to,
                eph_pub,
                proof,
            } => {
                if to != self.node_id || !net_ok(net) {
                    return;
                }
                let via_relay = from_server;
                Arc::clone(&self)
                    .handle_hello_ack(sock, src, via_relay, net, from, eph_pub, proof)
                    .await;
            }
            Packet::Data {
                net,
                sender,
                counter,
                ct,
            } => {
                if !net_ok(net) {
                    return;
                }
                self.handle_data(sender, counter, ct, sock, src).await;
            }
            Packet::Ping {
                pong: false,
                net,
                from,
                echoed_ts,
            } => {
                if !net_ok(net) {
                    return;
                }
                let p = Packet::Ping {
                    pong: true,
                    net,
                    from: self.node_id,
                    echoed_ts,
                };
                self.send_packet_bytes(from, p.encode()).await;
            }
            Packet::Ping {
                pong: true,
                from,
                echoed_ts,
                ..
            } => {
                let rtt = now_ms().saturating_sub(echoed_ts);
                let mut ps = self.peers.lock().await;
                if let Some(l) = ps.get_mut(&from) {
                    l.rtt = Some(rtt);
                    l.last_activity = Instant::now();
                }
            }
            Packet::Relay {
                net,
                from,
                to,
                inner,
            } => {
                if to != self.node_id || !net_ok(net) || !from_server {
                    return;
                }
                if std::env::var("MLDBG").is_ok() {
                    eprintln!("[dbg] relay unwrap from={from} innerlen={}", inner.len());
                }
                if let Ok(inner_pkt) = Packet::decode(&inner) {
                    match inner_pkt {
                        Packet::Data {
                            net: n2,
                            sender,
                            counter,
                            ct,
                        } if n2 == net => {
                            self.handle_data(sender, counter, ct, sock, src).await;
                        }
                        Packet::Hello {
                            ack: false,
                            net: n2,
                            from: f,
                            to: t,
                            eph_pub,
                            proof,
                        } if n2 == net && t == self.node_id => {
                            self.clone()
                                .handle_hello_init(sock, src, true, n2, f, eph_pub, proof)
                                .await;
                        }
                        Packet::Hello {
                            ack: true,
                            net: n2,
                            from: f,
                            to: t,
                            eph_pub,
                            proof,
                        } if n2 == net && t == self.node_id => {
                            Arc::clone(&self)
                                .handle_hello_ack(sock, src, true, n2, f, eph_pub, proof)
                                .await;
                        }
                        Packet::Ping {
                            pong: false,
                            net: n2,
                            from: f,
                            echoed_ts,
                        } if n2 == net => {
                            let p = Packet::Ping {
                                pong: true,
                                net: n2,
                                from: self.node_id,
                                echoed_ts,
                            };
                            self.send_packet_bytes(f, p.encode()).await;
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// Build the expected handshake auth key for pair (from=initiator, me=responder).
    fn hs_auth_key_responder(&self, peer_static: &[u8; PUB_LEN], net: u32, from: u64) -> [u8; 32] {
        let dh = self.statics.dh(peer_static);
        let mut salt = [0u8; 20];
        salt[0..4].copy_from_slice(&net.to_le_bytes());
        salt[4..12].copy_from_slice(&from.to_le_bytes());
        salt[12..20].copy_from_slice(&self.node_id.to_le_bytes());
        let mut k = [0u8; 32];
        gg_lan_core::crypto::derive_keys(&dh, &salt, b"gglan-hs-auth-v1", &mut k);
        k
    }

    #[allow(clippy::too_many_arguments)]
    async fn handle_hello_init(
        self: Arc<Self>,
        sock: &Arc<UdpSocket>,
        src: SocketAddr,
        via_relay: bool,
        net: u32,
        from: NodeId,
        eph_pub: [u8; PUB_LEN],
        proof: Vec<u8>,
    ) {
        let mut ps = self.peers.lock().await;
        let Some(l) = ps.get_mut(&from) else { return };
        if l.session.is_some() {
            // Already established: this is a retry of a handshake we answered
            // (its ACK may have been lost). Resend the cached ACK idempotently.
            if let Some(ack) = l.last_ack.clone() {
                drop(ps);
                self.send_packet_bytes(from, ack).await;
            }
            return;
        }
        let peer_static = match parse_static_pub(&l.info.static_pub) {
            Ok(s) => s,
            Err(_) => return,
        };
        // Verify the initiator's proof (bound to static DH + ids + eph key).
        let key = self.hs_auth_key_responder(&peer_static, net, from);
        let expect_ctx = |label: &[u8]| {
            let mut ctx = Vec::with_capacity(52);
            ctx.extend_from_slice(label);
            ctx.extend_from_slice(&net.to_le_bytes());
            ctx.extend_from_slice(&from.to_le_bytes());
            ctx.extend_from_slice(&self.node_id.to_le_bytes());
            ctx.extend_from_slice(&eph_pub);
            ctx
        };
        let c = AeadCipher::new(&key);
        let Ok(pt) = c.open(1, b"gglan-hs", &proof) else {
            return;
        };
        if pt != expect_ctx(b"gglan-hello-i") {
            return;
        }

        let resp = HandshakeResponder::new(&self.statics, &peer_static, net, self.node_id, from);
        let resp_eph = resp.eph_pub;
        let resp_proof = resp.proof(net, self.node_id, from);
        let session = resp.finish(&eph_pub, net, self.node_id, from);
        l.session = Some(session);
        if !via_relay {
            l.phase = LinkPhase::Direct;
            l.direct = Some((Arc::clone(sock), src));
        } else {
            if !matches!(l.phase, LinkPhase::Relay) {
                l.phase = LinkPhase::Relay;
            }
        }
        l.initiator = None;
        let q = std::mem::take(&mut l.queue);
        let peer_ip = l.info.ip.clone();
        let peer_name = l.info.name.clone();
        drop(ps);

        let pkt = Packet::Hello {
            ack: true,
            net,
            from: self.node_id,
            to: from,
            eph_pub: resp_eph,
            proof: resp_proof,
        };
        {
            let mut ps = self.peers.lock().await;
            if let Some(l) = ps.get_mut(&from) {
                l.last_ack = Some(pkt.encode());
            }
        }
        if via_relay {
            self.send_packet_bytes(from, pkt.encode()).await;
            eprintln!("[link] RELAY session established ← {peer_name} ({peer_ip})");
        } else {
            let _ = sock.send_to(&pkt.encode(), src).await;
            eprintln!("[link] DIRECT established ← {peer_name} ({peer_ip}) via {src}");
        }
        self.flush_queue(from, q).await;
        self.spawn_keepalive(from).await;
    }

    #[allow(clippy::too_many_arguments)]
    async fn handle_hello_ack(
        self: Arc<Self>,
        sock: &Arc<UdpSocket>,
        src: SocketAddr,
        via_relay: bool,
        net: u32,
        from: NodeId,
        eph_pub: [u8; PUB_LEN],
        proof: Vec<u8>,
    ) {
        let mut ps = self.peers.lock().await;
        let Some(l) = ps.get_mut(&from) else { return };
        if l.session.is_some() {
            return; // already converged; late/duplicate ACK
        }
        let Some(init) = l.initiator.take() else {
            return;
        };
        if parse_static_pub(&l.info.static_pub).is_err() {
            return;
        }
        let key = init.auth_key();
        let mut ctx = Vec::with_capacity(52);
        ctx.extend_from_slice(b"gglan-hello-r");
        ctx.extend_from_slice(&net.to_le_bytes());
        ctx.extend_from_slice(&from.to_le_bytes());
        ctx.extend_from_slice(&self.node_id.to_le_bytes());
        ctx.extend_from_slice(&eph_pub);
        let c = AeadCipher::new(&key);
        let Ok(pt) = c.open(2, b"gglan-hs", &proof) else {
            return;
        };
        if pt != ctx {
            return;
        }
        let session = init.finish(&eph_pub, net, self.node_id, from);
        l.session = Some(session);
        if !via_relay {
            l.phase = LinkPhase::Direct;
            l.direct = Some((Arc::clone(sock), src));
        }
        let q = std::mem::take(&mut l.queue);
        let peer_ip = l.info.ip.clone();
        let peer_name = l.info.name.clone();
        drop(ps);
        if via_relay {
            eprintln!("[link] RELAY session established → {peer_name} ({peer_ip})");
        } else {
            eprintln!("[link] DIRECT established → {peer_name} ({peer_ip}) via {src}");
        }
        self.flush_queue(from, q).await;
        self.spawn_keepalive(from).await;
    }

    /// 1 Hz keepalive/RTT probe on an established link (direct or relay).
    async fn spawn_keepalive(self: Arc<Self>, peer: NodeId) {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let net = self.net.lock().unwrap().as_ref().map(|c| c.id).unwrap_or(0);
                if net == 0 {
                    continue;
                }
                let p = Packet::Ping {
                    pong: false,
                    net,
                    from: self.node_id,
                    echoed_ts: now_ms(),
                };
                self.send_packet_bytes(peer, p.encode()).await;
            }
        });
    }

    async fn handle_data(
        self: Arc<Self>,
        sender: NodeId,
        counter: u64,
        ct: Vec<u8>,
        _sock: &Arc<UdpSocket>,
        _src: SocketAddr,
    ) {
        if std::env::var("MLDBG").is_ok() {
            eprintln!(
                "[dbg] handle_data from {sender} counter={counter} ctlen={}",
                ct.len()
            );
        }
        self.counters
            .rx_bytes
            .fetch_add(ct.len() as u64, Ordering::Relaxed);
        self.counters.rx_pkts.fetch_add(1, Ordering::Relaxed);
        let net_id = self.net.lock().unwrap().as_ref().map(|c| c.id).unwrap_or(0);
        let frame = {
            let mut ps = self.peers.lock().await;
            let Some(l) = ps.get_mut(&sender) else {
                if std::env::var("MLDBG").is_ok() {
                    eprintln!("[dbg] handle_data: no peer {sender}");
                }
                return;
            };
            l.pkts_in += 1;
            l.bytes_in += ct.len() as u64;
            l.last_activity = Instant::now();
            let Some(s) = l.session.as_mut() else { return };
            let aad = Packet::data_aad(net_id, sender, counter);
            match s.open_frame(counter, &aad, &ct) {
                Ok(f) => f,
                Err(e) => {
                    if std::env::var("MLDBG").is_ok() {
                        eprintln!("[dbg] open failed: {e}");
                    }
                    return;
                }
            }
        };
        self.clone().frame_from_network(sender, &frame).await;
    }

    // ---------------------------------------------------------------- L2

    /// Encrypt and transmit a frame to one peer (direct or relayed).
    async fn send_to_peer(&self, peer: NodeId, frame: &[u8]) {
        let (path, pkt_bytes) = {
            let mut ps = self.peers.lock().await;
            let Some(l) = ps.get_mut(&peer) else { return };
            let net = self.net.lock().unwrap().as_ref().map(|c| c.id).unwrap_or(0);
            let Some(s) = l.session.as_mut() else {
                if l.queue.len() < 64 {
                    l.queue.push(frame.to_vec());
                }
                return;
            };
            let counter = s.next_send_counter();
            let aad = Packet::data_aad(net, self.node_id, counter);
            let ct = s.seal(counter, &aad, frame);
            if std::env::var("MLDBG").is_ok() {
                eprintln!(
                    "[dbg] send_to_peer {peer} counter={counter} ctlen={} phase={:?}",
                    ct.len(),
                    l.phase
                );
            }
            let data = Packet::Data {
                net,
                sender: self.node_id,
                counter,
                ct,
            };
            let bytes = data.encode();
            let (path, out_bytes) = match (&l.phase, &l.direct) {
                (LinkPhase::Direct, Some((sock, ep))) => (Some((Arc::clone(sock), *ep)), bytes),
                _ => {
                    let srv = *self.server_udp.lock().unwrap();
                    match srv {
                        Some(srv) => {
                            let wrapped = Packet::Relay {
                                net,
                                from: self.node_id,
                                to: peer,
                                inner: bytes,
                            }
                            .encode();
                            (Some((Arc::clone(&self.sockets[0]), srv)), wrapped)
                        }
                        None => (None, bytes),
                    }
                }
            };
            l.pkts_out += 1;
            l.bytes_out += out_bytes.len() as u64;
            (path, out_bytes)
        };
        self.counters
            .tx_bytes
            .fetch_add(pkt_bytes.len() as u64, Ordering::Relaxed);
        self.counters.tx_pkts.fetch_add(1, Ordering::Relaxed);
        if let Some((sock, ep)) = path {
            let _ = sock.send_to(&pkt_bytes, ep).await;
        }
    }

    /// Broadcast to every peer.
    async fn send_broadcast(&self, frame: &[u8]) {
        let targets: Vec<NodeId> = {
            let ps = self.peers.lock().await;
            ps.keys().copied().collect()
        };
        for t in targets {
            self.send_to_peer(t, frame).await;
        }
    }

    /// Frame originated at our local host (NIC): route it through the mesh.
    pub async fn route_outbound(self: Arc<Self>, frame: Vec<u8>) {
        let Ok(eth) = EthFrame::parse(&frame) else {
            return;
        };
        if eth.src == [0; 6] || frame.len() > gg_lan_core::wire::MAX_FRAME {
            return;
        }
        if eth.ethertype == ETHERTYPE_ARP {
            if let Ok(arp) = Arp::parse(eth.payload) {
                // Answer locally if someone ARPs for our own IP.
                if arp.oper == ARP_OPER_REQUEST && arp.tpa == self.my_ip() {
                    let reply = Arp {
                        oper: ARP_OPER_REPLY,
                        sha: self.my_mac(),
                        spa: self.my_ip(),
                        tha: arp.sha,
                        tpa: arp.spa,
                    };
                    self.deliver_local(&reply.into_eth());
                    return;
                }
            }
        }
        if eth.is_broadcast() || eth.is_multicast() {
            self.send_broadcast(&frame).await;
            return;
        }
        // Known destination MAC?
        let dest = self.macs.lock().unwrap().get(&eth.dst).copied();
        match dest {
            Some(peer) => self.send_to_peer(peer, &frame).await,
            None => self.send_broadcast(&frame).await, // unknown unicast → flood
        }
    }

    fn my_ip(&self) -> u32 {
        self.net
            .lock()
            .unwrap()
            .as_ref()
            .map(|c| c.my_ip)
            .unwrap_or(0)
    }

    fn my_mac(&self) -> Mac {
        self.net
            .lock()
            .unwrap()
            .as_ref()
            .map(|c| c.my_mac)
            .unwrap_or([0; 6])
    }

    fn deliver_local(&self, frame: &[u8]) {
        if let Some(nic) = self.nic.lock().unwrap().as_ref() {
            nic.deliver(frame);
        }
    }

    /// Frame received from a mesh peer: learn MACs, feed the local stack, deliver.
    async fn frame_from_network(self: Arc<Self>, peer: NodeId, frame: &[u8]) {
        let Ok(eth) = EthFrame::parse(frame) else {
            return;
        };
        // Learn source MAC → peer.
        {
            let ps = self.peers.lock().await;
            if let Some(l) = ps.get(&peer) {
                if let Some(m) = parse_mac(&l.info.mac) {
                    drop(ps);
                    self.macs.lock().unwrap().insert(m, peer);
                }
            }
        }
        let mine = eth.dst == self.my_mac();
        if mine || eth.is_broadcast() || eth.is_multicast() {
            self.local_stack(frame, peer).await;
        }
        // Frames for third parties are not forwarded (endpoint-only v1).
    }

    /// The built-in L2/L3 mini-stack for userspace mode (no OS behind the NIC):
    /// answers ARP and ICMP echo, delivers everything else to the NIC.
    async fn local_stack(self: Arc<Self>, frame: &[u8], from_peer: NodeId) {
        let Ok(eth) = EthFrame::parse(frame) else {
            return;
        };
        if eth.ethertype == ETHERTYPE_ARP {
            if let Ok(arp) = Arp::parse(eth.payload) {
                if arp.oper == ARP_OPER_REQUEST && arp.tpa == self.my_ip() {
                    let reply = Arp {
                        oper: ARP_OPER_REPLY,
                        sha: self.my_mac(),
                        spa: self.my_ip(),
                        tha: arp.sha,
                        tpa: arp.spa,
                    };
                    self.send_to_peer(from_peer, &reply.into_eth()).await;
                }
            }
            self.deliver_local(frame);
            return;
        }
        if eth.ethertype == ETHERTYPE_IPV4 {
            if let Ok(ip) = Ipv4::parse(eth.payload) {
                if ip.dst == self.my_ip() && ip.protocol == 1 {
                    if let Some(reply) = icmp_echo_reply_eth(frame, self.my_mac(), self.my_ip()) {
                        self.send_to_peer(from_peer, &reply).await;
                    }
                    // Echo replies to our synthetic pings also land here.
                    if let Ok(icmp) = Icmp::parse(&ip.payload) {
                        if icmp.kind == ICMP_ECHO_REPLY {
                            let key = (icmp.ident, icmp.seq);
                            let tx = self.pending_pings.lock().unwrap().remove(&key);
                            if let Some(tx) = tx {
                                let _ = tx.send(now_ms());
                            }
                        }
                    }
                    return;
                }
            }
        }
        self.deliver_local(frame);
    }

    // ---------------------------------------------------------------- ping

    /// Synthetic ICMP ping through the whole pipeline. Returns RTT ms.
    pub async fn synthetic_ping(&self, target_ip: u32) -> Result<u64> {
        let (peer, mac) = {
            let ps = self.peers.lock().await;
            let mut found = None;
            for (id, l) in ps.iter() {
                if parse_ip(&l.info.ip) == Some(target_ip) {
                    found = Some((*id, parse_mac(&l.info.mac)));
                    break;
                }
            }
            found.ok_or_else(|| anyhow!("no peer with that IP"))?
        };
        let mac = mac.ok_or_else(|| anyhow!("bad peer mac"))?;
        let t0 = now_ms();
        let ident = (t0 & 0xffff) as u16;
        let seq = 1u16;
        let (tx, rx) = oneshot::channel();
        self.pending_pings.lock().unwrap().insert((ident, seq), tx);
        let frame = icmp_echo_request_eth(
            mac,
            self.my_mac(),
            self.my_ip(),
            target_ip,
            ident,
            seq,
            b"gg-lan",
        );
        self.send_to_peer(peer, &frame).await;
        match tokio::time::timeout(Duration::from_millis(2500), rx).await {
            Ok(Ok(t1)) => Ok(t1.saturating_sub(t0)),
            _ => Err(anyhow!("ping timeout")),
        }
    }

    // ---------------------------------------------------------------- status

    pub fn status_json(&self, rt: &tokio::runtime::Handle) -> serde_json::Value {
        let net = self.net.lock().unwrap().clone();
        let peers_json: Vec<serde_json::Value> = rt.block_on(async {
            let ps = self.peers.lock().await;
            ps.iter()
                .map(|(id, l)| {
                    json!({
                        "id": id,
                        "name": l.info.name,
                        "ip": l.info.ip,
                        "mac": l.info.mac,
                        "path": match l.phase {
                            LinkPhase::Punching => "punching",
                            LinkPhase::Direct => "direct",
                            LinkPhase::Relay => "relay",
                        },
                        "rtt_ms": l.rtt,
                        "bytes_in": l.bytes_in,
                        "bytes_out": l.bytes_out,
                        "pkts_in": l.pkts_in,
                        "pkts_out": l.pkts_out,
                    })
                })
                .collect()
        });
        let series: Vec<(u64, u64)> = self.series.samples.lock().unwrap().clone();
        let (rx, tx): (Vec<u64>, Vec<u64>) = series.iter().cloned().unzip();
        json!({
            "product": "gg-lan-agent",
            "version": env!("CARGO_PKG_VERSION"),
            "node_id": self.node_id,
            "name": self.name,
            "net": net.as_ref().map(|c| json!({
                "id": c.id, "name": c.name, "subnet": c.subnet,
                "mode": format!("{:?}", c.mode),
            })),
            "my": net.as_ref().map(|c| json!({
                "ip": gg_lan_core::ip_str(c.my_ip), "mac": gg_lan_core::mac_str(&c.my_mac),
            })),
            "peers": peers_json,
            "counters": {
                "tx_bytes": self.counters.tx_bytes.load(Ordering::Relaxed),
                "rx_bytes": self.counters.rx_bytes.load(Ordering::Relaxed),
                "tx_pkts": self.counters.tx_pkts.load(Ordering::Relaxed),
                "rx_pkts": self.counters.rx_pkts.load(Ordering::Relaxed),
            },
            "series": {"rx_bps": rx, "tx_bps": tx},
            "nic": self.nic.lock().unwrap().as_ref().map(|n| n.describe()),
        })
    }
}

// ---------------------------------------------------------------- status api

fn run_status_api(agent: Arc<Agent>, bind: &str, rt: tokio::runtime::Handle) -> Result<()> {
    let addr: SocketAddr = bind.parse().context("status bind")?;
    let srv = tiny_http::Server::http(addr).map_err(|e| anyhow!("status listen: {e}"))?;
    eprintln!("status api http://{addr}/status");
    for req in srv.incoming_requests() {
        let url = req.url().to_string();
        if url.starts_with("/status") || url == "/" {
            let body = agent.status_json(&rt).to_string();
            let resp = tiny_http::Response::from_string(body).with_header(
                tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                    .unwrap(),
            );
            let _ = req.respond(resp);
        } else if url.starts_with("/ping") {
            // /ping?ip=26.7.0.3
            let ip_str = url
                .split("ip=")
                .nth(1)
                .map(|s| s.split('&').next().unwrap_or("").to_string())
                .unwrap_or_default();
            let result = {
                let a = Arc::clone(&agent);
                let rt2 = rt.clone();
                std::thread::spawn(move || {
                    rt2.block_on(async move {
                        match parse_ip(&ip_str) {
                            Some(ip) => match a.synthetic_ping(ip).await {
                                Ok(ms) => json!({"ok": true, "ip": ip_str, "rtt_ms": ms}),
                                Err(e) => {
                                    json!({"ok": false, "ip": ip_str, "error": e.to_string()})
                                }
                            },
                            None => json!({"ok": false, "error": "bad ip"}),
                        }
                    })
                })
                .join()
                .unwrap_or_else(|_| json!({"ok": false, "error": "internal"}))
            };
            let resp = tiny_http::Response::from_string(result.to_string()).with_header(
                tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                    .unwrap(),
            );
            let _ = req.respond(resp);
        } else {
            let _ =
                req.respond(tiny_http::Response::from_string("not found").with_status_code(404));
        }
    }
    Ok(())
}
