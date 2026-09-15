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
    egress_from_listed(dest, if_addrs::get_if_addrs().ok())
}

pub fn egress_from_listed(
    dest: IpAddr,
    addrs: Option<Vec<if_addrs::Interface>>,
) -> Option<Ipv4Addr> {
    let IpAddr::V4(dest) = dest else {
        return None;
    };
    egress_ipv4_from(dest, &addrs?)
}

pub fn egress_ipv4_from(dest: Ipv4Addr, addrs: &[if_addrs::Interface]) -> Option<Ipv4Addr> {
    for iface in addrs {
        if iface.is_loopback() {
            continue;
        }
        let if_addrs::IfAddr::V4(ref v4) = iface.addr else {
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
    list_from(if_addrs::get_if_addrs().ok())
}

pub fn list_from(addrs: Option<Vec<if_addrs::Interface>>) -> Vec<InterfaceInfo> {
    let Some(addrs) = addrs else {
        return Vec::new();
    };
    let mut out = Vec::new();
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
        push_unique(
            &mut out,
            InterfaceInfo {
                name: iface.name,
                ipv4: ipv4.to_string(),
            },
        );
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn push_unique(out: &mut Vec<InterfaceInfo>, info: InterfaceInfo) {
    if out
        .iter()
        .any(|i| i.name == info.name && i.ipv4 == info.ipv4)
    {
        return;
    }
    out.push(info);
}

pub fn optional_bind(name: &str) -> Result<Option<InterfaceBind>, String> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(None);
    }
    Ok(Some(resolve_interface(name)?))
}

pub fn iface_list_error(e: impl std::fmt::Display) -> String {
    format!("list interfaces: {e}")
}

pub fn resolve_interface(name: &str) -> Result<InterfaceBind, String> {
    resolve_from_listed(name, if_addrs::get_if_addrs().map_err(iface_list_error))
}

pub fn resolve_from_listed(
    name: &str,
    addrs: Result<Vec<if_addrs::Interface>, String>,
) -> Result<InterfaceBind, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("interface name is empty".into());
    }
    let addrs = addrs?;
    let names = addrs
        .iter()
        .map(|i| i.name.clone())
        .collect::<std::collections::BTreeSet<_>>();
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
        Err(format!(
            "interface {name} not found (available: {})",
            names.into_iter().collect::<Vec<_>>().join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::ipv4_same_subnet;
    use super::{optional_bind, resolve_interface};
    use std::net::Ipv4Addr;

    #[test]
    fn same_subnet_matches_vlan() {
        let local = Ipv4Addr::new(10, 11, 1, 10);
        let mask = Ipv4Addr::new(255, 255, 255, 0);
        assert!(ipv4_same_subnet(local, mask, Ipv4Addr::new(10, 11, 1, 124)));
        assert!(!ipv4_same_subnet(local, mask, Ipv4Addr::new(10, 0, 0, 100)));
    }

    #[test]
    fn optional_bind_empty_is_none() {
        assert!(optional_bind("").unwrap().is_none());
        assert!(optional_bind("  ").unwrap().is_none());
    }

    #[test]
    fn resolve_interface_rejects_empty_name() {
        assert!(resolve_interface("").is_err());
    }

    #[test]
    fn resolve_loopback_has_no_usable_ipv4() {
        let _ = resolve_interface("lo0");
        let _ = resolve_interface("lo");
        assert!(resolve_interface("wledplay-no-such-nic").is_err());
    }

    #[test]
    fn list_and_egress_helpers() {
        assert!(super::list_from(None).is_empty());
        let listed = super::list_interfaces();
        assert!(
            listed.is_empty()
                || resolve_interface(&listed[0].name).map(|b| b.name) == Ok(listed[0].name.clone())
        );

        assert!(super::egress_ipv4_toward("::1".parse().unwrap()).is_none());
        let _ = super::egress_ipv4_toward("10.11.1.124".parse().unwrap());
        assert!(super::egress_from_listed("::1".parse().unwrap(), Some(vec![])).is_none());
        assert!(super::egress_from_listed("10.0.0.1".parse().unwrap(), None).is_none());
        let _ = super::egress_ipv4_from(Ipv4Addr::new(10, 0, 0, 1), &[]);
        assert!(super::iface_list_error("boom").contains("list interfaces"));
        assert!(
            super::resolve_from_listed("eth0", Err("list interfaces: boom".into()))
                .unwrap_err()
                .contains("list interfaces")
        );
        assert!(super::resolve_from_listed("", Ok(vec![]))
            .unwrap_err()
            .contains("empty"));
        let mask = Ipv4Addr::new(255, 255, 255, 0);
        let usable = vec![iface("eth0", v4(Ipv4Addr::new(10, 11, 1, 10), mask))];
        let bind = super::resolve_from_listed("eth0", Ok(usable)).unwrap();
        assert_eq!(bind.ipv4, Ipv4Addr::new(10, 11, 1, 10));
        let loopback = vec![iface("lo0", v4(Ipv4Addr::LOCALHOST, mask))];
        assert!(super::resolve_from_listed("lo0", Ok(loopback))
            .unwrap_err()
            .contains("no usable IPv4"));
        assert!(super::resolve_from_listed("eth9", Ok(vec![]))
            .unwrap_err()
            .contains("not found"));

        let mut out = vec![super::InterfaceInfo {
            name: "eth0".into(),
            ipv4: "10.0.0.1".into(),
        }];
        super::push_unique(
            &mut out,
            super::InterfaceInfo {
                name: "eth0".into(),
                ipv4: "10.0.0.1".into(),
            },
        );
        super::push_unique(
            &mut out,
            super::InterfaceInfo {
                name: "eth1".into(),
                ipv4: "10.0.0.2".into(),
            },
        );
        assert_eq!(out.len(), 2);

        assert!(resolve_interface("wledplay-no-such-nic").is_err());
    }

    fn iface(name: &str, addr: if_addrs::IfAddr) -> if_addrs::Interface {
        if_addrs::Interface {
            name: name.into(),
            addr,
            index: Some(1),
        }
    }

    fn v4(ip: Ipv4Addr, mask: Ipv4Addr) -> if_addrs::IfAddr {
        if_addrs::IfAddr::V4(if_addrs::Ifv4Addr {
            ip,
            netmask: mask,
            prefixlen: 24,
            broadcast: None,
        })
    }

    #[test]
    fn egress_and_list_from_synthetic_interfaces() {
        let mask = Ipv4Addr::new(255, 255, 255, 0);
        let addrs = vec![
            iface("lo0", v4(Ipv4Addr::LOCALHOST, mask)),
            iface(
                "en0",
                if_addrs::IfAddr::V6(if_addrs::Ifv6Addr {
                    ip: "2001:db8::1".parse().unwrap(),
                    netmask: std::net::Ipv6Addr::from(0),
                    prefixlen: 128,
                    broadcast: None,
                }),
            ),
            iface("en1", v4(Ipv4Addr::UNSPECIFIED, mask)),
            iface("en2", v4(Ipv4Addr::new(169, 254, 1, 1), mask)),
            iface("eth0", v4(Ipv4Addr::new(10, 11, 1, 10), mask)),
            iface("eth0", v4(Ipv4Addr::new(10, 11, 1, 11), mask)),
        ];
        let listed = super::list_from(Some(addrs.clone()));
        assert!(listed.iter().any(|i| i.name == "eth0"));
        assert!(!listed.iter().any(|i| i.name == "lo0"));

        assert_eq!(
            super::egress_ipv4_from(Ipv4Addr::new(10, 11, 1, 124), &addrs),
            Some(Ipv4Addr::new(10, 11, 1, 10))
        );
        assert!(super::egress_ipv4_from(Ipv4Addr::new(192, 168, 1, 1), &addrs).is_none());

        assert_eq!(
            super::resolve_from_listed("eth0", Ok(addrs.clone()))
                .unwrap()
                .name,
            "eth0"
        );
        assert!(super::resolve_from_listed("en0", Ok(addrs.clone()))
            .unwrap_err()
            .contains("no usable IPv4"));
        assert!(super::resolve_from_listed("eth9", Ok(addrs))
            .unwrap_err()
            .contains("not found"));
    }
}
