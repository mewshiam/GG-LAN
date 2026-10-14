//! IP allocation for virtual networks.

use crate::{parse_cidr, NodeId};
use std::collections::HashSet;

/// Allocates host addresses inside a /16 subnet on a first-come basis.
pub struct HostAllocator {
    pub base: u32,  // e.g. 0x1a070000 for 26.7.0.0
    pub prefix: u8, // 16
    used: HashSet<u32>,
}

impl HostAllocator {
    pub fn new(cidr: &str) -> Option<Self> {
        let (base, prefix) = parse_cidr(cidr)?;
        if prefix > 24 {
            return None;
        }
        Some(Self {
            base,
            prefix,
            used: HashSet::new(),
        })
    }

    /// Assign the next free host number (network-order host part) for a node.
    pub fn assign(&mut self, _node: NodeId) -> Option<u32> {
        for host in 1..=65534u32 {
            if !self.used.contains(&host) {
                self.used.insert(host);
                return Some(host);
            }
        }
        None
    }

    pub fn release(&mut self, host: u32) {
        self.used.remove(&host);
    }

    pub fn ip_of(&self, host: u32) -> u32 {
        let mask: u32 = if self.prefix == 0 {
            0
        } else {
            !0u32 << (32 - self.prefix)
        };
        (self.base & mask) | (host & !mask)
    }

    pub fn is_free(&self, host: u32) -> bool {
        !self.used.contains(&host)
    }
}

/// Deterministic "which subnet" picker for multiple networks on one server.
pub fn pick_subnet(existing: &[u32]) -> Option<u32> {
    // 26.1.0.0 .. 26.254.0.0
    for second in 1..=254u32 {
        let cand = 0x1a00_0000 | (second << 16);
        if !existing.contains(&cand) {
            return Some(cand);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assigns_unique_hosts() {
        let mut a = HostAllocator::new("26.7.0.0/16").unwrap();
        let h1 = a.assign(1).unwrap();
        let h2 = a.assign(2).unwrap();
        assert_ne!(h1, h2);
        assert_eq!(a.ip_of(h1), 0x1a07_0000 | h1);
    }

    #[test]
    fn picks_unused_subnet() {
        assert_eq!(pick_subnet(&[]), Some(0x1a01_0000));
        assert_eq!(pick_subnet(&[0x1a01_0000]), Some(0x1a02_0000));
    }
}
