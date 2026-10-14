# GG LAN — Clean-Room Architecture Specification

**Product:** GG LAN — self-hosted virtual-LAN/VPN (independent, interoperability-inspired alternative in the class of Radmin VPN / Hamachi / ZeroTier).
**Origin:** 100% original implementation. Design informed by the public architecture *class* of mesh VPNs; no proprietary code, protocol constants, or assets from any product are reused. All cryptography follows open RFCs: X25519 (RFC 7748), ChaCha20-Poly1305 (RFC 8439), HKDF (RFC 5869).

## 1. Goals

| Goal | Approach |
|---|---|
| LAN gaming / virtual LAN | Layer-2 virtual Ethernet (TAP on Linux; userspace frame-bridge elsewhere) |
| Works over any NAT | UDP hole-punching with multi-wave, multi-port probes; automatic encrypted relay fallback |
| Self-hosted & independent | Single static `gg-lan-server` binary; no external cloud dependencies |
| Modern security | X25519 ephemeral ECDH + ChaCha20-Poly1305 AEAD per packet, HKDF-derived keys, 64-bit counters, replay window, PSK-authenticated join codes |
| Cross-platform | Pure-Rust core (no C deps); Linux TAP + userspace NIC; Windows/macOS via userspace NIC v1 |
| Switchable topology | Per-network `mesh` (P2P direct + relay fallback) or `hub` (all through server) |
| Observability | Per-peer counters, RTT matrix, throughput history exposed as JSON; native desktop client |
| LAN-tool friendliness | ARP synthesis, broadcast + unknown-unicast flooding, virtual MACs |

## 2. Component map

```
gg-lan (Rust workspace)
├── crates/gg-lan-core   # protocol, crypto, frames, NIC trait, IP allocator
├── crates/gg-lan-server # coordinator + relay (one binary)
├── crates/gg-lan-agent   # peer daemon (TAP or userspace NIC)
├── crates/gg-lan-cli     # operator CLI
├── crates/gg-lan-gui     # native desktop client (eframe/egui)
└── crates/gg-lan-e2e     # end-to-end test harness
```

### 2.1 gg-lan-core
- **crypto**: `Keypair` (x25519-dalek); `Session` = AEAD channel with send/recv subkeys from HKDF(sha256, ecdh_secret, ctx = network_id ‖ role ‖ node ids); nonce = 96-bit: 32-bit zero ‖ 64-bit monotonic counter; sliding replay window (64).
- **frames (data plane, UDP)** — binary, little-endian:
  - `PUNCH {magic "GLP1", net_id u32, from u64, token u64, wave u8, port_ix u8}` — 24 B probe
  - `HELLO {magic "GLH1", ephemeral pub [u8;32], sender node id, sender static pub, nonce, tag}` — starts X25519
  - `DATA {channel_id u32, counter u64, tag [u8;16], ciphertext ≤ 10240}` — AEAD(ethernet frame) (10 KiB cap mirrors observed MTU-safe ceilings)
  - `KEEPALIVE {counter}` — RTT probe every 1 s; 3 misses → path dead → switch/relay
- **control messages** (server⇄agent): JSON-in-AEAD frames over TCP: `Hello{node_id, static_pub, join_code, want_net, capabilities}`, `Welcome{net_id, subnet, my_mac, my_ip, peers[], relay}`; roster updates `Peers{added[], removed[]}`; `PunchOrder{peer, token, endpoints[]}`; `RelayBind{relay_token}`.
- **VirtualNic trait**: `send_frame(&[u8])`, `set_recv_handler`, plus impls:
  - `TapNic` — Linux `/dev/net/tun` IFF_TAP (real deployment)
  - `UserspaceNic` — TCP frame-bridge on localhost port for sandbox/tests/laptop use
- **arp**: build/parse ARP packets; agent answers ARP requests for any peer IP with that peer's virtual MAC (proxy-ARP), replies for its own IP.
- **ip-alloc**: deterministic allocator; default network subnet `26.<n>.0.0/16` (n per server allocation, 1–254), MACs `02:GL:net:hi:lo` locally-administered.

### 2.2 gg-lan-server (coordinator + relay)
- tokio TCP listener (control, default port 7100) + UDP listener (relay + endpoint discovery, same port number as data plane 7101… actually one UDP socket suffices).
- Duties: verify join code (PSK), assign virtual MAC/IP, maintain roster, observe agents' reflexive UDP endpoints (implicit STUN), issue punch tokens, run **relay mode** (forward AEAD frames between peers that can't go direct), aggregate analytics, announce mode (mesh/hub) per network.
- Persistence: optional JSON state file; ephemeral networks by default.
- Exposes `/status` JSON (networks, peers, rates) for dashboards.

### 2.3 gg-lan-agent (peer daemon)
- Control client (reconnect w/ backoff), UDP data socket, per-peer `PeerLink` state machine: `Punching → Direct(RTT-probed) | Relayed`, or `Hub` when network mode says so.
- Punch: on `PunchOrder`, both sides emit 4 waves × 8 ports (progressive delays 0/50/150/400 ms; ports = own socket + ephemeral candidates via rebinding) carrying tokens; first valid `HELLO` from the peer wins; on 3 failed waves → relay.
- Frame pipeline: NIC → (broadcast? fan-out : route via dest-MAC peer table) → AEAD → UDP; inbound reverse, then inject into NIC. Unknown unicast → flood (learning-switch table fills from source MACs).
- Local status API (HTTP JSON :7800): peers, RTT, throughput series, path (direct/relay), mode.
- Frame bridge port configurable via `--frame-port` (userspace NIC; the optional Linux `tap` feature provides a `TapNic` impl in core for future kernel integration).

### 2.4 gg-lan-cli
Operator companion for a running agent: `gg-lan-cli status [--agent 127.0.0.1:7800]`, `peers` (compact table), `ping <ip>` (agent-side synthetic ARP+ICMP probe).

### 2.5 gg-lan-gui (native desktop client)
Single-binary native app (eframe/egui, no Electron). Radmin VPN-style UI: networks sidebar with per-network power switches, peer table (path/RTT badges, per-peer ping), live traffic charts, stat cards, event log. Manages one `gg-lan-agent` child process per joined network and can host a local `gg-lan-server` coordinator ("Host a network"). Talks to agents over their localhost `/status` HTTP API; persists `gui.json` under the platform config dir.

## 3. Security model

| Threat | Mitigation |
|---|---|
| Rogue peer | Join requires PSK code (server-issued); static key pinned at join; server rejects unknown static keys |
| Passive capture | ChaCha20-Poly1305 on every data frame; keys from ephemeral X25519 per peer-pair (PFS) |
| Replay/tamper | AEAD tags + 64-bit counters + 64-entry sliding window |
| Malicious coordinator | Coordinator never holds data-plane keys (E2E between peers; relay sees only ciphertext); hub mode documented as trusted-server mode |
| Punch-token abuse | Tokens single-purpose, 30 s TTL, bound to net_id+peer pair |

## 4. Test matrix

1. Unit: crypto roundtrip + tamper/replay rejection; frame codec; ARP builder; allocator uniqueness.
2. Integration (`mesh-mode`): server + 3 agents on localhost (distinct UDP ports), userspace NICs: ARP resolution + ICMP echo across peers via direct links; analytics counters advance.
3. Integration (`hub-mode`): block direct (force punch failure) → relay path carries same traffic.
4. Bench: throughput smoke (≥ 30 MB/s loopback target on sandbox), loss counters under induced drop.

## 5. Out of scope (v1)
Windows wintun driver, kernel fast-path, IPv6 virtual addressing, chat/voice, UPnP-aided punching, NAT-PMP. (Roadmapped in README.)
