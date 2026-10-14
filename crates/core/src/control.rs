//! Control-plane messages (agent⇄server, over TCP) and framing helpers.

use crate::crypto::{unb64, KEY_LEN};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

/// First plaintext line sent by the agent on connect.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub v: u32,
    pub node_id: u64,
    pub name: String,
    pub static_pub: String, // b64
    pub join_code: String,
}

/// First plaintext reply from the server.
#[derive(Debug, Serialize, Deserialize)]
pub struct HelloReply {
    pub ok: bool,
    pub reason: Option<String>,
    pub server_static: Option<String>, // b64
}

/// Peer entry as announced by the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerInfo {
    pub id: u64,
    pub name: String,
    pub mac: String, // "aa:bb:.."
    pub ip: String,  // dotted
    pub static_pub: String,
    /// Observed UDP endpoints ("ip:port") of this peer.
    pub endpoints: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NetMode {
    Mesh,
    Hub,
}

/// Server → agent messages (all inside the AEAD session after Hello).
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "t", content = "c")]
pub enum SrvMsg {
    Welcome {
        net_id: u32,
        net_name: String,
        subnet: String, // "26.7.0.0/16"
        mode: NetMode,
        my_ip: String,
        my_mac: String,
        my_host: u16, // host part
        relay_udp: String,
        udp_token: u64,
        peers: Vec<PeerInfo>,
    },
    PeersUpdate {
        peers: Vec<PeerInfo>,
        removed: Vec<u64>,
    },
    PunchOrder {
        peer: u64,
        token: u64,
        endpoints: Vec<String>,
    },
    Pong,
    Bye {
        reason: String,
    },
}

/// Agent → server messages.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "t", content = "c")]
pub enum AgtMsg {
    Ping,
    /// Fired whenever our UDP endpoints changed (rebind).
    EndpointsAnnounce {
        endpoints: Vec<String>,
    },
    Stats {
        direct_peers: u32,
        relay_peers: u32,
        rtt_ms: std::collections::HashMap<u64, u64>,
    },
    Leave,
}

/// Length-prefixed (u32 LE) AEAD frame codec for the control TCP stream.
pub struct CtrlCodec {
    buf: Vec<u8>,
}

/// Stateful AEAD session over the control TCP stream.
pub struct CtrlChannel {
    send: crate::crypto::AeadCipher,
    recv: crate::crypto::AeadCipher,
    send_ctr: u64,
    recv_seen: std::collections::HashSet<u64>,
}

impl CtrlChannel {
    /// Agent (initiator) side.
    pub fn agent(keys: (crate::crypto::AeadCipher, crate::crypto::AeadCipher)) -> Self {
        Self {
            send: keys.0,
            recv: keys.1,
            send_ctr: 1,
            recv_seen: Default::default(),
        }
    }

    /// Server (responder) side.
    pub fn server(keys: (crate::crypto::AeadCipher, crate::crypto::AeadCipher)) -> Self {
        Self {
            send: keys.1,
            recv: keys.0,
            send_ctr: 1,
            recv_seen: Default::default(),
        }
    }

    pub fn seal(&mut self, msg: &impl Serialize) -> Vec<u8> {
        let json = serde_json::to_vec(msg).expect("json");
        self.seal_bytes(&json)
    }

    /// Seal an already-serialized JSON message.
    pub fn seal_bytes(&mut self, json: &[u8]) -> Vec<u8> {
        let ct = self.send.seal(self.send_ctr, b"gg-lan-ctrl", json);
        let framed = frame_msg(&ct);
        self.send_ctr += 1;
        framed
    }

    pub fn open<T: serde::de::DeserializeOwned>(&mut self, frame: &[u8]) -> Result<T> {
        // Find the counter by tracking highest seen + 1: we accept frames in
        // order on a reliable stream, so counter == recv_seen.len()+1.
        let counter = self.recv_seen.len() as u64 + 1;
        let pt = self.recv.open(counter, b"gg-lan-ctrl", frame)?;
        self.recv_seen.insert(counter);
        Ok(serde_json::from_slice(&pt)?)
    }
}

impl CtrlCodec {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }
}

impl Default for CtrlCodec {
    fn default() -> Self {
        Self::new()
    }
}

impl CtrlCodec {
    /// Feed received bytes; yields complete frames.
    pub fn feed(&mut self, data: &[u8], mut on_frame: impl FnMut(Vec<u8>)) {
        self.buf.extend_from_slice(data);
        loop {
            if self.buf.len() < 4 {
                return;
            }
            let len = u32::from_le_bytes(self.buf[0..4].try_into().unwrap()) as usize;
            if len > 1 << 20 {
                // Corrupt stream: drop everything; the session is dead anyway.
                self.buf.clear();
                return;
            }
            if self.buf.len() < 4 + len {
                return;
            }
            let frame: Vec<u8> = self.buf[4..4 + len].to_vec();
            self.buf.drain(..4 + len);
            on_frame(frame);
        }
    }
}

pub fn frame_msg(msg: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + msg.len());
    out.extend_from_slice(&(msg.len() as u32).to_le_bytes());
    out.extend_from_slice(msg);
    out
}

/// Read one newline-terminated JSON line from a stream (tokio).
pub async fn read_json_line<R>(r: &mut R) -> Result<Vec<u8>>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    let mut line = Vec::with_capacity(512);
    let mut byte = [0u8; 1];
    loop {
        let n = r.read(&mut byte).await?;
        if n == 0 {
            if line.is_empty() {
                return Err(anyhow!("eof"));
            }
            break;
        }
        if byte[0] == b'\n' {
            break;
        }
        line.push(byte[0]);
        if line.len() > 1 << 16 {
            return Err(anyhow!("hello line too long"));
        }
    }
    Ok(line)
}

pub fn parse_static_pub(s: &str) -> Result<[u8; KEY_LEN]> {
    let v = unb64(s)?;
    v.try_into().map_err(|_| anyhow!("bad static pub length"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_codec_frames() {
        let mut c = CtrlCodec::new();
        let mut got = Vec::new();
        let a = frame_msg(b"hello");
        let b = frame_msg(b"world!!");
        let mut stream = a.clone();
        stream.extend_from_slice(&b);
        c.feed(&stream[..3], |f| got.push(f));
        assert!(got.is_empty());
        c.feed(&stream[3..], |f| got.push(f));
        assert_eq!(got, vec![b"hello".to_vec(), b"world!!".to_vec()]);
    }
}
