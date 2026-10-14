//! GG LAN core: shared types, crypto, wire protocol, virtual NICs.

pub mod alloc;
pub mod arp;
pub mod control;
pub mod crypto;
pub mod nic;
pub mod wire;

pub type NodeId = u64;
pub type NetId = u32;

/// 48-bit MAC address.
pub type Mac = [u8; 6];

pub const ETH_BROADCAST: Mac = [0xff; 6];

/// Build a locally-administered virtual MAC for a node inside a network.
pub fn virtual_mac(net_id: NetId, node_id: NodeId) -> Mac {
    [
        0x02,
        b'G',
        b'L',
        (net_id & 0xff) as u8,
        (node_id >> 8) as u8,
        (node_id & 0xff) as u8,
    ]
}

/// Parse "26.7.0.0/16" into (base u32, prefix u8). Returns None when malformed.
pub fn parse_cidr(s: &str) -> Option<(u32, u8)> {
    use std::str::FromStr;
    let (ip, len) = s.split_once('/')?;
    let ip: u32 = std::net::Ipv4Addr::from_str(ip.trim()).ok()?.into();
    let len: u8 = len.trim().parse().ok()?;
    if len > 32 {
        return None;
    }
    Some((ip, len))
}

/// Format (base u32, prefix) back into CIDR text.
pub fn format_cidr(base: u32, prefix: u8) -> String {
    let ip = std::net::Ipv4Addr::from(base);
    format!("{}/{}", ip, prefix)
}

/// Pretty print a MAC.
pub fn mac_str(m: &Mac) -> String {
    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        m[0], m[1], m[2], m[3], m[4], m[5]
    )
}

/// Parse a MAC from "aa:bb:cc:dd:ee:ff".
pub fn parse_mac(s: &str) -> Option<Mac> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 6 {
        return None;
    }
    let mut m = [0u8; 6];
    for i in 0..6 {
        m[i] = u8::from_str_radix(parts[i], 16).ok()?;
    }
    Some(m)
}

/// Convert an IPv4 u32 to dotted string.
pub fn ip_str(ip: u32) -> String {
    std::net::Ipv4Addr::from(ip).to_string()
}

/// Convert dotted string to IPv4 u32.
pub fn parse_ip(s: &str) -> Option<u32> {
    use std::str::FromStr;
    Some(std::net::Ipv4Addr::from_str(s.trim()).ok()?.into())
}

/// An IPv4 address inside a subnet with host part (1..=254 usual).
pub fn subnet_host(base: u32, host: u32) -> u32 {
    (base & 0xffff_0000) | (host & 0xffff)
}
