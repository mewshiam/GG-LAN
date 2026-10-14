//! Data-plane wire format (UDP datagrams, all little-endian).
//!
//! ```text
//! PUNCH      (28B clear):  "GLP1" k=1 wave port_ix _ | net u32 | from u64 | token u64
//! HELLO      (~128B clear): "GLH1" k=2 _ _ _ | net u32 | from u64 | to u64 |
//!                           eph_pub[32] | proof (16+len)   — proof = AEAD over ctx
//! HELLO_ACK  (~128B clear): "GLH1" k=3 _ _ _ | net u32 | from u64 | to u64 |
//!                           eph_pub[32] | proof
//! DATA       (hdr+ct):      "GLD1" k=4 _ _ _ | net u32 | sender u64 | counter u64 |
//!                           ct = AEAD(ethernet frame) — tag = last 16B of ct
//! PING/PONG  (32B clear):   "MLK1" k=5/6 _ _ _ | net u32 | from u64 | echoed_ts u64
//! RELAY wrap (28B + inner): "MLR1" k=7 _ _ _ | net u32 | from u64 | to u64 | inner...
//! SERVER UDP:
//! MLPING/MLPONG (32B):      "MLS1" k=8/9 _ _ _ | net u32 | node u64 | token u64
//! ```

use crate::crypto::PUB_LEN;
use anyhow::{anyhow, Result};

pub const MAGIC_PUNCH: &[u8; 4] = b"GLP1";
pub const MAGIC_HELLO: &[u8; 4] = b"GLH1";
pub const MAGIC_DATA: &[u8; 4] = b"GLD1";
pub const MAGIC_KEEP: &[u8; 4] = b"MLK1";
pub const MAGIC_RELAY: &[u8; 4] = b"MLR1";
pub const MAGIC_SERVER: &[u8; 4] = b"MLS1";

/// Maximum Ethernet frame we transport (10 KiB like the observed class ceiling).
pub const MAX_FRAME: usize = 10_240;

#[derive(Debug, Clone, PartialEq)]
pub enum Packet {
    Punch {
        wave: u8,
        port_ix: u8,
        net: u32,
        from: u64,
        token: u64,
    },
    Hello {
        ack: bool,
        net: u32,
        from: u64,
        to: u64,
        eph_pub: [u8; PUB_LEN],
        proof: Vec<u8>,
    },
    Data {
        net: u32,
        sender: u64,
        counter: u64,
        ct: Vec<u8>, // ciphertext incl. trailing 16B tag
    },
    Ping {
        pong: bool,
        net: u32,
        from: u64,
        echoed_ts: u64,
    },
    /// Wrapper used when sending through the server relay.
    Relay {
        net: u32,
        from: u64,
        to: u64,
        inner: Vec<u8>,
    },
    /// Agent⇄server UDP registration ping.
    ServerPing {
        pong: bool,
        net: u32,
        node: u64,
        token: u64,
    },
}

pub const K_PUNCH: u8 = 1;
pub const K_HELLO: u8 = 2;
pub const K_HELLO_ACK: u8 = 3;
pub const K_DATA: u8 = 4;
pub const K_PING: u8 = 5;
pub const K_PONG: u8 = 6;
pub const K_RELAY: u8 = 7;
pub const K_SPING: u8 = 8;
pub const K_SPONG: u8 = 9;

fn put_u32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_le_bytes());
}

fn put_u64(b: &mut Vec<u8>, v: u64) {
    b.extend_from_slice(&v.to_le_bytes());
}

fn get_u32(b: &[u8], off: usize) -> Result<u32> {
    if off + 4 > b.len() {
        return Err(anyhow!("short u32"));
    }
    Ok(u32::from_le_bytes(b[off..off + 4].try_into().unwrap()))
}

fn get_u64(b: &[u8], off: usize) -> Result<u64> {
    if off + 8 > b.len() {
        return Err(anyhow!("short u64"));
    }
    Ok(u64::from_le_bytes(b[off..off + 8].try_into().unwrap()))
}

impl Packet {
    pub fn encode(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(
            64 + match self {
                Packet::Data { ct, .. } => ct.len(),
                Packet::Relay { inner, .. } => inner.len(),
                Packet::Hello { proof, .. } => proof.len(),
                _ => 0,
            },
        );
        match self {
            Packet::Punch {
                wave,
                port_ix,
                net,
                from,
                token,
            } => {
                b.extend_from_slice(MAGIC_PUNCH);
                b.push(K_PUNCH);
                b.push(*wave);
                b.push(*port_ix);
                b.push(0);
                put_u32(&mut b, *net);
                put_u64(&mut b, *from);
                put_u64(&mut b, *token);
            }
            Packet::Hello {
                ack,
                net,
                from,
                to,
                eph_pub,
                proof,
            } => {
                b.extend_from_slice(MAGIC_HELLO);
                b.push(if *ack { K_HELLO_ACK } else { K_HELLO });
                b.extend_from_slice(&[0, 0, 0]);
                put_u32(&mut b, *net);
                put_u64(&mut b, *from);
                put_u64(&mut b, *to);
                b.extend_from_slice(eph_pub);
                b.extend_from_slice(proof);
            }
            Packet::Data {
                net,
                sender,
                counter,
                ct,
            } => {
                b.extend_from_slice(MAGIC_DATA);
                b.push(K_DATA);
                b.extend_from_slice(&[0, 0, 0]);
                put_u32(&mut b, *net);
                put_u64(&mut b, *sender);
                put_u64(&mut b, *counter);
                b.extend_from_slice(ct);
            }
            Packet::Ping {
                pong,
                net,
                from,
                echoed_ts,
            } => {
                b.extend_from_slice(MAGIC_KEEP);
                b.push(if *pong { K_PONG } else { K_PING });
                b.extend_from_slice(&[0, 0, 0]);
                put_u32(&mut b, *net);
                put_u64(&mut b, *from);
                put_u64(&mut b, *echoed_ts);
            }
            Packet::Relay {
                net,
                from,
                to,
                inner,
            } => {
                b.extend_from_slice(MAGIC_RELAY);
                b.push(K_RELAY);
                b.extend_from_slice(&[0, 0, 0]);
                put_u32(&mut b, *net);
                put_u64(&mut b, *from);
                put_u64(&mut b, *to);
                b.extend_from_slice(inner);
            }
            Packet::ServerPing {
                pong,
                net,
                node,
                token,
            } => {
                b.extend_from_slice(MAGIC_SERVER);
                b.push(if *pong { K_SPONG } else { K_SPING });
                b.extend_from_slice(&[0, 0, 0]);
                put_u32(&mut b, *net);
                put_u64(&mut b, *node);
                put_u64(&mut b, *token);
            }
        }
        b
    }

    pub fn decode(b: &[u8]) -> Result<Packet> {
        if b.len() < 28 {
            return Err(anyhow!("runt datagram"));
        }
        let kind = match &b[0..4] {
            m if m == MAGIC_PUNCH.as_slice() => K_PUNCH,
            m if m == MAGIC_HELLO.as_slice() => b[4],
            m if m == MAGIC_DATA.as_slice() => K_DATA,
            m if m == MAGIC_KEEP.as_slice() => b[4],
            m if m == MAGIC_RELAY.as_slice() => K_RELAY,
            m if m == MAGIC_SERVER.as_slice() => b[4],
            _ => return Err(anyhow!("unknown magic")),
        };
        Ok(match kind {
            K_PUNCH => Packet::Punch {
                wave: b[5],
                port_ix: b[6],
                net: get_u32(b, 8)?,
                from: get_u64(b, 12)?,
                token: get_u64(b, 20)?,
            },
            K_HELLO | K_HELLO_ACK => {
                if b.len() < 12 + 8 + 8 + PUB_LEN + 16 {
                    return Err(anyhow!("short hello"));
                }
                Packet::Hello {
                    ack: kind == K_HELLO_ACK,
                    net: get_u32(b, 8)?,
                    from: get_u64(b, 12)?,
                    to: get_u64(b, 20)?,
                    eph_pub: b[28..28 + PUB_LEN].try_into().unwrap(),
                    proof: b[28 + PUB_LEN..].to_vec(),
                }
            }
            K_DATA => Packet::Data {
                net: get_u32(b, 8)?,
                sender: get_u64(b, 12)?,
                counter: get_u64(b, 20)?,
                ct: b[28..].to_vec(),
            },
            K_PING | K_PONG => Packet::Ping {
                pong: kind == K_PONG,
                net: get_u32(b, 8)?,
                from: get_u64(b, 12)?,
                echoed_ts: get_u64(b, 20)?,
            },
            K_RELAY => Packet::Relay {
                net: get_u32(b, 8)?,
                from: get_u64(b, 12)?,
                to: get_u64(b, 20)?,
                inner: b[28..].to_vec(),
            },
            K_SPING | K_SPONG => Packet::ServerPing {
                pong: kind == K_SPONG,
                net: get_u32(b, 8)?,
                node: get_u64(b, 12)?,
                token: get_u64(b, 20)?,
            },
            _ => return Err(anyhow!("bad kind")),
        })
    }

    /// The AAD for DATA AEAD: the full header preceding the ciphertext.
    pub fn data_aad(net: u32, sender: u64, counter: u64) -> Vec<u8> {
        let mut b = Vec::with_capacity(28);
        b.extend_from_slice(MAGIC_DATA);
        b.push(K_DATA);
        b.extend_from_slice(&[0, 0, 0]);
        put_u32(&mut b, net);
        put_u64(&mut b, sender);
        put_u64(&mut b, counter);
        b
    }

    /// For DATA packets received via relay: AAD uses the *original* sender id,
    /// so the relay wrapper re-encodes the same header bytes. We expose the
    /// header slice of a decoded inner packet for that.
    pub fn is_data(b: &[u8]) -> bool {
        b.len() >= 28 && &b[0..4] == MAGIC_DATA.as_slice() && b[4] == K_DATA
    }
}

/// Relay wrapper helpers.
pub fn wrap_relay(net: u32, from: u64, to: u64, inner: &[u8]) -> Vec<u8> {
    Packet::Relay {
        net,
        from,
        to,
        inner: inner.to_vec(),
    }
    .encode()
}

/// Compute RTT from a PONG packet (ms).
pub fn rtt_ms(echoed_ts: u64, now_ms: u64) -> u64 {
    now_ms.saturating_sub(echoed_ts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codec_roundtrip() {
        let pkts = vec![
            Packet::Punch {
                wave: 3,
                port_ix: 5,
                net: 7,
                from: 42,
                token: 99,
            },
            Packet::Hello {
                ack: false,
                net: 7,
                from: 42,
                to: 43,
                eph_pub: [7u8; PUB_LEN],
                proof: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17],
            },
            Packet::Data {
                net: 7,
                sender: 42,
                counter: 9,
                ct: vec![9u8; 64 + crate::crypto::TAG_LEN],
            },
            Packet::Ping {
                pong: true,
                net: 7,
                from: 43,
                echoed_ts: 123456,
            },
            Packet::Relay {
                net: 7,
                from: 42,
                to: 43,
                inner: b"inner".to_vec(),
            },
            Packet::ServerPing {
                pong: false,
                net: 7,
                node: 42,
                token: 5,
            },
        ];
        for p in pkts {
            assert_eq!(Packet::decode(&p.encode()).unwrap(), p);
        }
    }
}
