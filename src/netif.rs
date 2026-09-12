//! Resolve a NIC name to the IPv4 we should bind on a multi-homed host.

use std::net::{IpAddr, Ipv4Addr};

#[derive(Debug, Clone)]
pub struct InterfaceBind {
    pub name: String,
    pub ipv4: Ipv4Addr,
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
