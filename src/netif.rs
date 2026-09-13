//! Resolve a NIC name to the IPv4 we should bind on a multi-homed host.

use serde::Serialize;
use std::net::{IpAddr, Ipv4Addr};

pub fn ipv4_same_subnet(local: Ipv4Addr, mask: Ipv4Addr, dest: Ipv4Addr) -> bool {
    let local = u32::from(local);
    let mask = u32::from(mask);
    let dest = u32::from(dest);
    local & mask == dest & mask
}

/// Local IPv4 that should send toward `dest` (same subnet). Used for multicast IF.
pub fn egress_ipv4_toward(dest: IpAddr) -> Option<Ipv4Addr> {
    let IpAddr::V4(dest) = dest else {
        return None;
    };
    let Ok(addrs) = if_addrs::get_if_addrs() else {
        return None;
    };
    for iface in addrs {
        if iface.is_loopback() {
            continue;
        }
        let if_addrs::IfAddr::V4(v4) = iface.addr else {
            continue;
        };
        if v4.ip.is_unspecified() || v4.ip.is_link_local() {
            continue;
        }
        if ipv4_same_subnet(v4.ip, v4.netmask, dest) {
            return Some(v4.ip);
        }
    }
    None
}

#[derive(Debug, Clone)]
pub struct InterfaceBind {
    pub name: String,
    pub ipv4: Ipv4Addr,
}

#[derive(Debug, Clone, Serialize)]
pub struct InterfaceInfo {
    pub name: String,
    pub ipv4: String,
}

pub fn list_interfaces() -> Vec<InterfaceInfo> {
    let mut out = Vec::new();
    let Ok(addrs) = if_addrs::get_if_addrs() else {
        return out;
    };
    for iface in addrs {
        if iface.is_loopback() {
            continue;
        }
        let IpAddr::V4(ipv4) = iface.ip() else {
            continue;
        };
        if ipv4.is_unspecified() || ipv4.is_link_local() {
            continue;
        }
        if out.iter().any(|i: &InterfaceInfo| i.name == iface.name && i.ipv4 == ipv4.to_string()) {
            continue;
        }
        out.push(InterfaceInfo {
            name: iface.name,
            ipv4: ipv4.to_string(),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub fn optional_bind(name: &str) -> Result<Option<InterfaceBind>, String> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(None);
    }
    Ok(Some(resolve_interface(name)?))
}

pub fn resolve_interface(name: &str) -> Result<InterfaceBind, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("interface name is empty".into());
    }
    let addrs = if_addrs::get_if_addrs().map_err(|e| format!("list interfaces: {e}"))?;
    let mut found = false;
    for iface in addrs {
        if iface.name != name {
            continue;
        }
        found = true;
        if let IpAddr::V4(ipv4) = iface.ip() {
            if !ipv4.is_loopback() && !ipv4.is_unspecified() && !ipv4.is_link_local() {
                return Ok(InterfaceBind {
                    name: name.to_string(),
                    ipv4,
                });
            }
        }
    }
    if found {
        Err(format!(
            "interface {name} has no usable IPv4 address (need a non-loopback, non-link-local address)"
        ))
    } else {
        let names = if_addrs::get_if_addrs()
            .unwrap_or_default()
            .into_iter()
            .map(|i| i.name)
            .collect::<std::collections::BTreeSet<_>>();
        Err(format!(
            "interface {name} not found (available: {})",
            names.into_iter().collect::<Vec<_>>().join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::ipv4_same_subnet;
    use std::net::Ipv4Addr;

    #[test]
    fn same_subnet_matches_vlan() {
        let local = Ipv4Addr::new(10, 11, 1, 10);
        let mask = Ipv4Addr::new(255, 255, 255, 0);
        assert!(ipv4_same_subnet(local, mask, Ipv4Addr::new(10, 11, 1, 124)));
        assert!(!ipv4_same_subnet(local, mask, Ipv4Addr::new(10, 0, 0, 100)));
    }
}
