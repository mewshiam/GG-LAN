//! Crypto: X25519 keypairs, HKDF key derivation, ChaCha20-Poly1305 AEAD sessions.
//!
//! Design (clean-room, RFC-based):
//! - Each node owns a long-term X25519 static keypair, authorized by the coordinator roster.
//! - Peer channels run an authenticated ephemeral handshake:
//!   1. K_auth = HKDF(DH(static_initiator, static_responder), salt = net||ids)
//!   2. HELLO carries ephemeral_i pubkey + AEAD_{K_auth}(context) — proves sender identity
//!   3. HELLO_ACK carries ephemeral_r pubkey + AEAD_{K_auth}(context)
//!   4. Session keys = HKDF(DH(eph_i, eph_r), salt = net||ids) → send/recv split keys
//! - Every DATA frame is AEAD-encrypted with a 4+8-byte zero‖counter nonce and an
//!   AAD binding of the full plaintext header; a 64-entry sliding window defeats replay.

use anyhow::{anyhow, Result};
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use rand::rngs::OsRng;
use sha2::Sha256;
use x25519_dalek::{EphemeralSecret, PublicKey, StaticSecret};

pub const KEY_LEN: usize = 32;
pub const TAG_LEN: usize = 16;
pub const PUB_LEN: usize = 32;
pub const NONCE_LEN: usize = 12;

pub fn b64(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

pub fn unb64(s: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .map_err(|e| anyhow!("base64: {e}"))
}

/// Long-term node identity.
#[derive(Clone)]
pub struct StaticKeypair {
    secret: StaticSecret,
    public: PublicKey,
}

impl StaticKeypair {
    pub fn generate() -> Self {
        let secret = StaticSecret::random_from_rng(OsRng);
        let public = PublicKey::from(&secret);
        Self { secret, public }
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        let secret = StaticSecret::from(bytes);
        let public = PublicKey::from(&secret);
        Self { secret, public }
    }

    pub fn public_bytes(&self) -> [u8; PUB_LEN] {
        *self.public.as_bytes()
    }

    pub fn secret_bytes(&self) -> [u8; 32] {
        self.secret.to_bytes()
    }

    /// DH with a remote static public key.
    pub fn dh(&self, remote_pub: &[u8; PUB_LEN]) -> [u8; 32] {
        *self
            .secret
            .diffie_hellman(&PublicKey::from(*remote_pub))
            .as_bytes()
    }
}

/// HKDF-SHA256 helper.
pub fn derive_keys(ikm: &[u8], salt: &[u8], info: &[u8], out: &mut [u8]) {
    let hk = Hkdf::<Sha256>::new(Some(salt), ikm);
    hk.expand(info, out).expect("hkdf expand fits");
}

/// One AEAD cipher over one key.
pub struct AeadCipher {
    cipher: ChaCha20Poly1305,
}

impl AeadCipher {
    pub fn new(key: &[u8; KEY_LEN]) -> Self {
        Self {
            cipher: ChaCha20Poly1305::new(Key::from_slice(key)),
        }
    }

    pub fn seal(&self, counter: u64, aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
        let nonce = counter_nonce(counter);
        self.cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .expect("chacha20poly1305 encrypt cannot fail")
    }

    pub fn open(&self, counter: u64, aad: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>> {
        let nonce = counter_nonce(counter);
        self.cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: ciphertext,
                    aad,
                },
            )
            .map_err(|_| anyhow!("aead: auth failure (tamper/replay/wrong key)"))
    }
}

/// 96-bit nonce: 4 zero bytes || 64-bit LE counter.
pub fn counter_nonce(counter: u64) -> [u8; NONCE_LEN] {
    let mut n = [0u8; NONCE_LEN];
    n[4..12].copy_from_slice(&counter.to_le_bytes());
    n
}

/// An authenticated ephemeral handshake initiator state.
pub struct HandshakeInitiator {
    pub eph: EphemeralSecret,
    pub eph_pub: [u8; PUB_LEN],
    auth_key: [u8; KEY_LEN],
}

pub struct HandshakeResponder {
    pub eph: EphemeralSecret,
    pub eph_pub: [u8; PUB_LEN],
    auth_key: [u8; KEY_LEN],
}

fn auth_key_from_static_dh(
    my_static: &StaticKeypair,
    peer_static_pub: &[u8; PUB_LEN],
    salt: &[u8],
) -> [u8; KEY_LEN] {
    let dh = my_static.dh(peer_static_pub);
    let mut k = [0u8; KEY_LEN];
    derive_keys(&dh, salt, b"gglan-hs-auth-v1", &mut k);
    k
}

/// Salt binding the handshake to (network, initiator, responder).
fn hs_salt(net_id: u32, a: u64, b: u64) -> [u8; 20] {
    let mut s = [0u8; 20];
    s[0..4].copy_from_slice(&net_id.to_le_bytes());
    s[4..12].copy_from_slice(&a.to_le_bytes());
    s[12..20].copy_from_slice(&b.to_le_bytes());
    s
}

impl HandshakeInitiator {
    pub fn new(
        my_static: &StaticKeypair,
        peer_static_pub: &[u8; PUB_LEN],
        net_id: u32,
        my_id: u64,
        peer_id: u64,
    ) -> Self {
        let eph = EphemeralSecret::random_from_rng(OsRng);
        let eph_pub = *PublicKey::from(&eph).as_bytes();
        let auth_key =
            auth_key_from_static_dh(my_static, peer_static_pub, &hs_salt(net_id, my_id, peer_id));
        Self {
            eph,
            eph_pub,
            auth_key,
        }
    }

    /// AEAD proof that binds the ephemeral key to the authenticated context.
    pub fn proof(&self, net_id: u32, my_id: u64, peer_id: u64) -> Vec<u8> {
        let cipher = AeadCipher::new(&self.auth_key);
        let mut ctx = Vec::with_capacity(52);
        ctx.extend_from_slice(b"gglan-hello-i");
        ctx.extend_from_slice(&net_id.to_le_bytes());
        ctx.extend_from_slice(&my_id.to_le_bytes());
        ctx.extend_from_slice(&peer_id.to_le_bytes());
        ctx.extend_from_slice(&self.eph_pub);
        cipher.seal(1, b"gglan-hs", &ctx)
    }

    /// Auth key so the caller can verify the responder's proof.
    pub fn auth_key(&self) -> [u8; KEY_LEN] {
        self.auth_key
    }

    /// Consume the responder's ACK, producing the data session.
    pub fn finish(
        self,
        peer_eph_pub: &[u8; PUB_LEN],
        net_id: u32,
        my_id: u64,
        peer_id: u64,
    ) -> Session {
        let shared = *self
            .eph
            .diffie_hellman(&PublicKey::from(*peer_eph_pub))
            .as_bytes();
        Session::from_shared(&shared, &hs_salt(net_id, my_id, peer_id), true)
    }
}

impl HandshakeResponder {
    pub fn new(
        my_static: &StaticKeypair,
        peer_static_pub: &[u8; PUB_LEN],
        net_id: u32,
        my_id: u64,
        peer_id: u64,
    ) -> Self {
        let eph = EphemeralSecret::random_from_rng(OsRng);
        let eph_pub = *PublicKey::from(&eph).as_bytes();
        let auth_key =
            auth_key_from_static_dh(my_static, peer_static_pub, &hs_salt(net_id, peer_id, my_id));
        Self {
            eph,
            eph_pub,
            auth_key,
        }
    }

    pub fn auth_key(&self) -> [u8; KEY_LEN] {
        self.auth_key
    }

    /// AEAD proof that binds the responder's ephemeral key to the context.
    pub fn proof(&self, net_id: u32, my_id: u64, peer_id: u64) -> Vec<u8> {
        let cipher = AeadCipher::new(&self.auth_key);
        let mut ctx = Vec::with_capacity(52);
        ctx.extend_from_slice(b"gglan-hello-r");
        ctx.extend_from_slice(&net_id.to_le_bytes());
        ctx.extend_from_slice(&my_id.to_le_bytes());
        ctx.extend_from_slice(&peer_id.to_le_bytes());
        ctx.extend_from_slice(&self.eph_pub);
        cipher.seal(2, b"gglan-hs", &ctx)
    }

    pub fn finish(
        self,
        peer_eph_pub: &[u8; PUB_LEN],
        net_id: u32,
        my_id: u64,
        peer_id: u64,
    ) -> Session {
        let shared = *self
            .eph
            .diffie_hellman(&PublicKey::from(*peer_eph_pub))
            .as_bytes();
        let salt = hs_salt(net_id, peer_id, my_id);
        Session::from_shared(&shared, &salt, false)
    }
}

/// A bidirectional data-plane session with independent send/receive keys and
/// replay protection on the receive side.
pub struct Session {
    send: AeadCipher,
    recv: AeadCipher,
    send_counter: u64,
    /// Sliding replay window (bit i = counter window_start+i received).
    recv_window: std::collections::BTreeSet<u64>,
    window_base: u64,
}

impl Session {
    pub fn from_shared(shared: &[u8; 32], salt: &[u8], initiator: bool) -> Self {
        let mut okm = [0u8; 64];
        derive_keys(shared, salt, b"gglan-data-v1", &mut okm);
        let (k1, k2) = okm.split_at(32);
        let (send_key, recv_key) = if initiator { (k1, k2) } else { (k2, k1) };
        Self {
            send: AeadCipher::new(send_key.try_into().unwrap()),
            recv: AeadCipher::new(recv_key.try_into().unwrap()),
            send_counter: 1,
            recv_window: std::collections::BTreeSet::new(),
            window_base: 1,
        }
    }

    /// Reserve the next send counter (used to build matching AAD).
    pub fn next_send_counter(&mut self) -> u64 {
        let c = self.send_counter;
        self.send_counter += 1;
        c
    }

    /// Seal with an explicit counter (must have been reserved via
    /// next_send_counter so AAD can embed it).
    pub fn seal(&self, counter: u64, aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
        self.send.seal(counter, aad, plaintext)
    }

    pub fn seal_frame(&mut self, aad: &[u8], plaintext: &[u8]) -> (u64, Vec<u8>) {
        let c = self.next_send_counter();
        (c, self.send.seal(c, aad, plaintext))
    }

    /// Open a frame, rejecting replays and stale far-past counters.
    pub fn open_frame(&mut self, counter: u64, aad: &[u8], ct: &[u8]) -> Result<Vec<u8>> {
        if counter == 0 {
            return Err(anyhow!("counter 0 forbidden"));
        }
        // Update window base to track the highest counter seen.
        if counter > self.window_base + 64 {
            self.window_base = counter - 64;
            self.recv_window.retain(|&c| c >= self.window_base);
        }
        if counter < self.window_base {
            return Err(anyhow!("replay: below window"));
        }
        if self.recv_window.contains(&counter) {
            return Err(anyhow!("replay: duplicate counter"));
        }
        let pt = self.recv.open(counter, aad, ct)?;
        self.recv_window.insert(counter);
        if self.recv_window.len() > 256 {
            if let Some(&min) = self.recv_window.iter().next() {
                self.recv_window.remove(&min);
            }
        }
        Ok(pt)
    }
}

/// Control-plane channel (agent⇄server): keys from static-static DH.
/// Returns (k_initiator, k_responder): the initiator (agent) sends with
/// k_initiator and receives with k_responder; the server mirrors that.
pub fn control_keys(
    my_static: &StaticKeypair,
    peer_static_pub: &[u8; PUB_LEN],
    salt: &[u8],
) -> (AeadCipher, AeadCipher) {
    let dh = my_static.dh(peer_static_pub);
    let mut okm = [0u8; 64];
    derive_keys(&dh, salt, b"gglan-ctrl-v1", &mut okm);
    let (k1, k2) = okm.split_at(32);
    (
        AeadCipher::new(k1.try_into().unwrap()),
        AeadCipher::new(k2.try_into().unwrap()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_handshake() -> (Session, Session) {
        let a = StaticKeypair::generate();
        let b = StaticKeypair::generate();
        let (net, ia, ib) = (7u32, 11u64, 22u64);
        let i = HandshakeInitiator::new(&a, &b.public_bytes(), net, ia, ib);
        let proof_i = i.proof(net, ia, ib);
        let i_eph = i.eph_pub;

        // Responder verifies initiator proof
        let auth_salt = hs_salt(net, ia, ib);
        let mut expect = Vec::new();
        expect.extend_from_slice(b"gglan-hello-i");
        expect.extend_from_slice(&net.to_le_bytes());
        expect.extend_from_slice(&ia.to_le_bytes());
        expect.extend_from_slice(&ib.to_le_bytes());
        expect.extend_from_slice(&i_eph);
        let ver = AeadCipher::new(&auth_key_from_static_dh(&b, &a.public_bytes(), &auth_salt));
        assert!(ver.open(1, b"gglan-hs", &proof_i).is_ok());

        let r = HandshakeResponder::new(&b, &a.public_bytes(), net, ib, ia);
        let _ = r.proof(net, ib, ia); // responder also emits an authenticated proof
        let r_eph = r.eph_pub;
        let si = i.finish(&r_eph, net, ia, ib);
        let sr = r.finish(&i_eph, net, ib, ia);
        (si, sr)
    }

    #[test]
    fn session_roundtrip() {
        let (mut a, mut b) = full_handshake();
        let (c, ct) = a.seal_frame(b"aad", b"hello world");
        let pt = b.open_frame(c, b"aad", &ct).unwrap();
        assert_eq!(pt, b"hello world");
    }

    #[test]
    fn session_rejects_tamper_and_replay() {
        let (mut a, mut b) = full_handshake();
        let (c, mut ct) = a.seal_frame(b"aad", b"data");
        ct[0] ^= 0x01;
        assert!(b.open_frame(c, b"aad", &ct).is_err());
        let (c2, ct2) = a.seal_frame(b"aad", b"data2");
        assert!(b.open_frame(c2, b"aad", &ct2).is_ok());
        assert!(b.open_frame(c2, b"aad", &ct2).is_err()); // replay
    }
}
