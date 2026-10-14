//! Ethernet / ARP / IPv4 / ICMP construction & parsing for the L2 engine.

use anyhow::{anyhow, Result};

pub const ETHERTYPE_IPV4: u16 = 0x0800;
pub const ETHERTYPE_ARP: u16 = 0x0806;

/// A parsed Ethernet-II frame header.
#[derive(Debug, Clone)]
pub struct EthFrame<'a> {
    pub dst: [u8; 6],
    pub src: [u8; 6],
    pub ethertype: u16,
    pub payload: &'a [u8],
}

pub fn build_eth(dst: [u8; 6], src: [u8; 6], ethertype: u16, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(14 + payload.len());
    f.extend_from_slice(&dst);
    f.extend_from_slice(&src);
    f.extend_from_slice(&ethertype.to_be_bytes());
    f.extend_from_slice(payload);
    f
}

impl<'a> EthFrame<'a> {
    pub fn parse(buf: &'a [u8]) -> Result<EthFrame<'a>> {
        if buf.len() < 14 {
            return Err(anyhow!("short eth frame"));
        }
        Ok(EthFrame {
            dst: buf[0..6].try_into().unwrap(),
            src: buf[6..12].try_into().unwrap(),
            ethertype: u16::from_be_bytes(buf[12..14].try_into().unwrap()),
            payload: &buf[14..],
        })
    }

    pub fn is_broadcast(&self) -> bool {
        self.dst == [0xff; 6]
    }

    pub fn is_multicast(&self) -> bool {
        self.dst[0] & 0x01 == 0x01
    }
}

// ---------------- ARP ----------------

pub const ARP_OPER_REQUEST: u16 = 1;
pub const ARP_OPER_REPLY: u16 = 2;

#[derive(Debug, Clone)]
pub struct Arp {
    pub oper: u16,
    pub sha: [u8; 6],
    pub spa: u32,
    pub tha: [u8; 6],
    pub tpa: u32,
}

impl Arp {
    pub fn encode(&self) -> Vec<u8> {
        let mut p = Vec::with_capacity(28);
        p.extend_from_slice(&1u16.to_be_bytes()); // htype ethernet
        p.extend_from_slice(&ETHERTYPE_IPV4.to_be_bytes());
        p.push(6);
        p.push(4);
        p.extend_from_slice(&self.oper.to_be_bytes());
        p.extend_from_slice(&self.sha);
        p.extend_from_slice(&self.spa.to_be_bytes());
        p.extend_from_slice(&self.tha);
        p.extend_from_slice(&self.tpa.to_be_bytes());
        p
    }

    pub fn parse(b: &[u8]) -> Result<Arp> {
        if b.len() < 28 {
            return Err(anyhow!("short arp"));
        }
        Ok(Arp {
            oper: u16::from_be_bytes(b[6..8].try_into().unwrap()),
            sha: b[8..14].try_into().unwrap(),
            spa: u32::from_be_bytes(b[14..18].try_into().unwrap()),
            tha: b[18..24].try_into().unwrap(),
            tpa: u32::from_be_bytes(b[24..28].try_into().unwrap()),
        })
    }

    /// Build a full Ethernet frame carrying this ARP.
    pub fn into_eth(self) -> Vec<u8> {
        let (dst, ethertype) = if self.oper == ARP_OPER_REQUEST {
            ([0xff; 6], ETHERTYPE_ARP)
        } else {
            (self.tha, ETHERTYPE_ARP)
        };
        build_eth(dst, self.sha, ethertype, &self.encode())
    }
}

// ---------------- IPv4 ----------------

#[derive(Debug, Clone)]
pub struct Ipv4 {
    pub src: u32,
    pub dst: u32,
    pub protocol: u8,
    pub payload: Vec<u8>,
}

fn ipv4_checksum(hdr: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    for ch in hdr.chunks(2) {
        let w = if ch.len() == 2 {
            u16::from_be_bytes([ch[0], ch[1]])
        } else {
            (ch[0] as u16) << 8
        };
        sum = sum.wrapping_add(w as u32);
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

impl Ipv4 {
    pub fn encode(&self) -> Vec<u8> {
        let total_len = 20 + self.payload.len();
        let mut hdr = vec![0u8; 20];
        hdr[0] = 0x45; // v4, IHL 5
        hdr[1] = 0x00; // DSCP
        hdr[2..4].copy_from_slice(&(total_len as u16).to_be_bytes());
        hdr[4..6].copy_from_slice(&0x1234u16.to_be_bytes()); // id
        hdr[6..8].copy_from_slice(&0x4000u16.to_be_bytes()); // DF
        hdr[8] = 64; // ttl
        hdr[9] = self.protocol;
        hdr[12..16].copy_from_slice(&self.src.to_be_bytes());
        hdr[16..20].copy_from_slice(&self.dst.to_be_bytes());
        let ck = ipv4_checksum(&hdr);
        hdr[10..12].copy_from_slice(&ck.to_be_bytes());
        let mut p = hdr;
        p.extend_from_slice(&self.payload);
        p
    }

    pub fn parse(b: &[u8]) -> Result<Ipv4> {
        if b.len() < 20 {
            return Err(anyhow!("short ip"));
        }
        let ihl = (b[0] & 0x0f) as usize * 4;
        if b.len() < ihl {
            return Err(anyhow!("bad ihl"));
        }
        Ok(Ipv4 {
            src: u32::from_be_bytes(b[12..16].try_into().unwrap()),
            dst: u32::from_be_bytes(b[16..20].try_into().unwrap()),
            protocol: b[9],
            payload: b[ihl..].to_vec(),
        })
    }
}

// ---------------- ICMP ----------------

pub const ICMP_ECHO_REQUEST: u8 = 8;
pub const ICMP_ECHO_REPLY: u8 = 0;

#[derive(Debug, Clone)]
pub struct Icmp {
    pub kind: u8,
    pub code: u8,
    pub ident: u16,
    pub seq: u16,
    pub payload: Vec<u8>,
}

fn icmp_checksum(pkt: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    for ch in pkt.chunks(2) {
        let w = if ch.len() == 2 {
            u16::from_be_bytes([ch[0], ch[1]])
        } else {
            (ch[0] as u16) << 8
        };
        sum = sum.wrapping_add(w as u32);
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

impl Icmp {
    pub fn encode(&self) -> Vec<u8> {
        let mut p = vec![0u8; 8];
        p[0] = self.kind;
        p[1] = self.code;
        p[4..6].copy_from_slice(&self.ident.to_be_bytes());
        p[6..8].copy_from_slice(&self.seq.to_be_bytes());
        p.extend_from_slice(&self.payload);
        let ck = icmp_checksum(&p);
        p[2..4].copy_from_slice(&ck.to_be_bytes());
        p
    }

    pub fn parse(b: &[u8]) -> Result<Icmp> {
        if b.len() < 8 {
            return Err(anyhow!("short icmp"));
        }
        Ok(Icmp {
            kind: b[0],
            code: b[1],
            ident: u16::from_be_bytes(b[4..6].try_into().unwrap()),
            seq: u16::from_be_bytes(b[6..8].try_into().unwrap()),
            payload: b[8..].to_vec(),
        })
    }
}

/// Convenience: full Ethernet frame with an ICMP echo request.
pub fn icmp_echo_request_eth(
    dst_mac: [u8; 6],
    src_mac: [u8; 6],
    src_ip: u32,
    dst_ip: u32,
    ident: u16,
    seq: u16,
    payload: &[u8],
) -> Vec<u8> {
    let icmp = Icmp {
        kind: ICMP_ECHO_REQUEST,
        code: 0,
        ident,
        seq,
        payload: payload.to_vec(),
    };
    let ip = Ipv4 {
        src: src_ip,
        dst: dst_ip,
        protocol: 1,
        payload: icmp.encode(),
    };
    build_eth(dst_mac, src_mac, ETHERTYPE_IPV4, &ip.encode())
}

/// Convenience: reply to an echo-request Ethernet frame (returns None if not a request).
pub fn icmp_echo_reply_eth(req_frame: &[u8], my_mac: [u8; 6], my_ip: u32) -> Option<Vec<u8>> {
    let eth = EthFrame::parse(req_frame).ok()?;
    if eth.ethertype != ETHERTYPE_IPV4 {
        return None;
    }
    let ip = Ipv4::parse(eth.payload).ok()?;
    if ip.protocol != 1 || ip.dst != my_ip {
        return None;
    }
    let icmp = Icmp::parse(&ip.payload).ok()?;
    if icmp.kind != ICMP_ECHO_REQUEST {
        return None;
    }
    let reply = Icmp {
        kind: ICMP_ECHO_REPLY,
        code: 0,
        ident: icmp.ident,
        seq: icmp.seq,
        payload: icmp.payload,
    };
    let ipr = Ipv4 {
        src: my_ip,
        dst: ip.src,
        protocol: 1,
        payload: reply.encode(),
    };
    Some(build_eth(eth.src, my_mac, ETHERTYPE_IPV4, &ipr.encode()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arp_roundtrip() {
        let a = Arp {
            oper: ARP_OPER_REQUEST,
            sha: [1, 2, 3, 4, 5, 6],
            spa: 0x1a07_0001,
            tha: [0; 6],
            tpa: 0x1a07_0002,
        };
        let p = a.encode();
        assert_eq!(Arp::parse(&p).unwrap().tpa, 0x1a07_0002);
    }

    #[test]
    fn icmp_reply_roundtrip() {
        let req = icmp_echo_request_eth(
            [2, 3, 3, 3, 3, 3],
            [1, 1, 1, 1, 1, 1],
            0x1a07_0001,
            0x1a07_0002,
            0x42,
            7,
            b"gglan-ping",
        );
        let rep = icmp_echo_reply_eth(&req, [2, 3, 3, 3, 3, 3], 0x1a07_0002).unwrap();
        let eth = EthFrame::parse(&rep).unwrap();
        assert_eq!(eth.dst, [1, 1, 1, 1, 1, 1]);
        let ip = Ipv4::parse(eth.payload).unwrap();
        let icmp = Icmp::parse(&ip.payload).unwrap();
        assert_eq!(icmp.kind, ICMP_ECHO_REPLY);
        assert_eq!(icmp.seq, 7);
    }
}
