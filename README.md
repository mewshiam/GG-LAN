# GG LAN

**A self-hosted virtual LAN for your games — anywhere.**

GG LAN is an open-source, fully self-hosted mesh VPN designed for LAN gaming over
the internet: it puts you and your friends on the same virtual Ethernet network,
so LAN-mode games, file shares, and discovery tools just work. Think Radmin VPN /
Hamachi class of tool — but independent, modern, encrypted end-to-end, and
shipped as one small native client with zero telemetry.

```
   you ──┐                                    ┌── friend B
         ├── direct P2P (hole-punched UDP) ────┤
 friend A ──┘         └── or encrypted relay ──┴── friend C
                     (your own coordinator, one binary)
```

## Highlights

- 🎮 **True layer-2 virtual LAN** — ARP, broadcasts and Ethernet frames, so LAN
  game lobbies, auto-discovery and SMB behave like on a real switch
- 🕳️ **NAT traversal** — multi-wave, multi-port UDP hole punching with automatic
  encrypted relay fallback when direct connection is impossible
- 🔐 **Modern crypto, end-to-end** — X25519 ephemeral key exchange with
  PSK-authenticated join codes, ChaCha20-Poly1305 per-packet AEAD, HKDF-derived
  subkeys, 64-bit counters with a sliding replay window. The coordinator never
  holds data-plane keys — even relayed traffic is opaque to the host
- 🏠 **Self-hosted** — the coordinator is a single static binary you run
  anywhere (a VPS, or right inside the GUI with *Host a network*)
- ⚡ **Native client** — one small binary (eframe/egui, no Electron), Windows /
  Linux / macOS
- 🔀 **mesh or hub per network** — direct peer-to-peer with relay fallback, or
  force all traffic through the host (strict firewall / anti-cheat friendly)
- 📊 **Live observability** — per-peer RTT, path (direct/relay) badges, traffic
  charts, packet/byte counters, JSON status APIs

## Getting started

Download the latest archive for your OS from
[**Releases**](https://github.com/mewshiam/GG-LAN/releases), unpack it, and run
**`gg-lan`** (the GUI client). All binaries live side by side — the client
finds `gg-lan-agent` automatically.

> Releases are fully automated: pushing a new `version` in `Cargo.toml` to
> `main` (or pushing a `v*` tag) builds Linux, Windows and macOS archives with
> checksums and publishes the GitHub Release.

### Play with friends

1. **One person hosts** — in the GG LAN client click **Host a network**, give it
   a name, pick *Mesh* (or *Hub* to relay everything through you)
2. **Share** the shown address (`ip:port`) and the 12-character join code
3. **Everyone else** clicks **Join a network**, pastes the address and code
4. Launch your game, host a **LAN** game, and everyone sees it in the lobby

To run the coordinator on a VPS instead (recommended for bigger groups):

```bash
./gg-lan-server --ctrl 0.0.0.0:7100 --udp 0.0.0.0:7100 \
                --net-name "friday-night" --join-code supersecret123
# tell your friends: server <your-ip>:7100, code supersecret123
```

The join code doubles as the network password — only holders can join, and the
code is verified before any key material is exchanged.

### The client

| | |
|---|---|
| **Networks sidebar** | every network with a live status pill and a power switch |
| **Dashboard** | your virtual IP, peer count, live up/down charts, total traffic |
| **Peers** | name, IP, `direct`/`relay` path badge, RTT, per-peer traffic, one-click ping |
| **Host a network** | spins up a local coordinator and joins it automatically |
| **Event log** | agent/coordinator stderr for quick diagnosis |

Configuration lives in the platform config dir (`~/.config/gg-lan/` on Linux,
`%APPDATA%\gg-lan\` on Windows, `~/Library/Application Support/gg-lan/` on
macOS) — `gui.json` for the client, `identity.txt` for your node key.

## Security model

| Threat | Mitigation |
|---|---|
| Rogue peer | Join requires the PSK join code; keys are bound to the exchange |
| Passive capture | ChaCha20-Poly1305 AEAD on every data frame; per-pair ephemeral X25519 keys (forward secrecy) |
| Replay / tamper | AEAD tags + 64-bit counters + sliding replay window |
| Malicious coordinator | Data-plane keys live only on the peers; the relay forwards ciphertext it cannot read |
| Traffic analysis (partial) | Virtual MACs and 26.x.x.x addresses carry no host metadata by default |

Full details in [`docs/architecture.md`](docs/architecture.md).

## Building from source

```bash
cargo build --release --workspace
```

- Linux GUI builds need the usual `libxkbcommon-dev libwayland-dev` (plus X11
  headers on minimal systems)
- Run the test suite with `cargo test --workspace`
- End-to-end (server + 3 agents, mesh and hub): `cargo run --release -p gg-lan-e2e`

## Repository layout

```
crates/gg-lan-core    types, crypto, wire protocol, virtual NICs, IP allocator
crates/gg-lan-server  coordinator + relay (TCP control plane, UDP data plane)
crates/gg-lan-agent   peer daemon: punching, P2P AEAD, MAC learning, NIC bridge
crates/gg-lan-cli     operator CLI (status / peers / ping)
crates/gg-lan-gui     native desktop client (eframe/egui)
crates/gg-lan-e2e     end-to-end test harness
docs/architecture.md  protocol + architecture specification
.github/workflows/    CI (test + lint) and automated release pipeline
```

## Status & roadmap

**v0.1** — core protocol stable (8/8 unit tests, e2e mesh+hub green,
~180 Mbit/s loopback benchmark, 0% loss at 3000 frames), native desktop
client, automated multi-OS releases.

Next up:

- Windows/macOS `wintun`/`utun` virtual NICs — v1 uses the userspace frame bridge
  everywhere (apps connect over a local TCP frame socket); Linux TAP support is
  implemented in core (`tap` feature) and pending agent integration
- IPv6 virtual addressing
- Trusted-device pairing (server-side static-key pinning)
- In-app network chat

## FAQ

**Is this a VPN?** It's a *virtual LAN* — it connects machines into one
Ethernet segment for games and LAN tools, not a privacy VPN. Only LAN traffic
you route to the 26.x.x.x network touches it.

**Why 26.x addresses?** The 26.0.0.0/8 block is reserved for DoD use and
effectively never routed on the public internet — a common choice for virtual
LAN overlays.

**Can the host read my traffic?** In mesh mode, no — peer-to-peer frames are
end-to-end encrypted; the coordinator only ever sees ciphertext. In hub mode,
traffic *flows through* the host but is still peer-encrypted end to end; only
timing and size metadata leak.

## License

MIT — see [LICENSE](LICENSE).
