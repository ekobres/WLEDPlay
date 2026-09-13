use crate::wled::protocol::AudioSyncPacket;
use socket2::{Domain, Protocol, Socket, Type};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SendError {
    #[error("socket error: {0}")]
    Io(#[from] std::io::Error),
}

pub struct WledSender {
    socket: UdpSocket,
    multicast_addr: SocketAddr,
    unicast_destinations: Vec<SocketAddr>,
    multicast_enabled: bool,
    packets_sent: u64,
    udp_errors: u64,
}

impl WledSender {
    pub fn new(
        multicast: IpAddr,
        port: u16,
        unicast_destinations: Vec<SocketAddr>,
        bind_ip: Option<Ipv4Addr>,
        bind_device: Option<&str>,
    ) -> Result<Self, SendError> {
        let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
        socket.set_reuse_address(true)?;
        #[cfg(unix)]
        socket.set_reuse_port(true)?;

        let local = bind_ip.unwrap_or(Ipv4Addr::UNSPECIFIED);
        socket.bind(&SocketAddr::from((local, 0)).into())?;

        if let Some(name) = bind_device.filter(|n| !n.is_empty()) {
            bind_udp_device(&socket, name, local);
        }

        let mcast_if = bind_ip
            .filter(|ip| !ip.is_unspecified())
            .or_else(|| {
                unicast_destinations
                    .iter()
                    .find_map(|d| crate::netif::egress_ipv4_toward(d.ip()))
            });
        // Kernel default is TTL 1 and no multicast IF: on a multi-homed host the
        // packets follow the default route NIC, not the WLED VLAN.
        if let Err(e) = socket.set_multicast_ttl_v4(20) {
            tracing::warn!("IP_MULTICAST_TTL: {e}");
        }
        if let Some(ip) = mcast_if {
            if let Err(e) = socket.set_multicast_if_v4(&ip) {
                tracing::warn!("IP_MULTICAST_IF {ip}: {e}");
            } else {
                tracing::info!("WLED multicast egress {ip} ttl=20");
            }
        } else {
            tracing::warn!(
                "WLED multicast has no egress NIC (set WLEDPLAY_INTERFACE); kernel default route will be used"
            );
        }

        if let IpAddr::V4(addr) = multicast {
            let join_if = mcast_if.unwrap_or(local);
            if let Err(e) = socket.join_multicast_v4(&addr, &join_if) {
                tracing::warn!("join {addr} on {join_if}: {e}");
            }
        }

        Ok(Self {
            socket: socket.into(),
            multicast_addr: SocketAddr::new(multicast, port),
            unicast_destinations,
            multicast_enabled: true,
            packets_sent: 0,
            udp_errors: 0,
        })
    }

    pub fn set_unicast(&mut self, destinations: Vec<SocketAddr>) {
        self.unicast_destinations = destinations;
    }

    pub fn set_multicast_enabled(&mut self, enabled: bool) {
        self.multicast_enabled = enabled;
    }

    pub fn send(&mut self, packet: &AudioSyncPacket) -> Result<(), SendError> {
        let bytes = packet.encode();
        if self.multicast_enabled {
            match self.socket.send_to(&bytes, self.multicast_addr) {
                Ok(_) => self.packets_sent += 1,
                Err(e) => {
                    self.udp_errors += 1;
                    tracing::warn!("multicast send failed: {e}");
                }
            }
        }
        for dest in &self.unicast_destinations.clone() {
            if let Err(e) = self.socket.send_to(&bytes, dest) {
                self.udp_errors += 1;
                tracing::warn!("unicast send to {dest} failed: {e}");
            } else {
                self.packets_sent += 1;
            }
        }
        Ok(())
    }

    pub fn packets_sent(&self) -> u64 {
        self.packets_sent
    }

    pub fn udp_errors(&self) -> u64 {
        self.udp_errors
    }

    pub fn destination(&self) -> String {
        let mut parts = Vec::new();
        if self.multicast_enabled {
            parts.push(self.multicast_addr.to_string());
        }
        for d in &self.unicast_destinations {
            parts.push(d.ip().to_string());
        }
        if parts.is_empty() {
            "none".into()
        } else {
            parts.join(" + ")
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn bind_udp_device(socket: &Socket, name: &str, local: Ipv4Addr) {
    if let Err(e) = socket.bind_device(Some(name.as_bytes())) {
        tracing::warn!("SO_BINDTODEVICE {name} failed ({e}); using bind address {local}");
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn bind_udp_device(_socket: &Socket, _name: &str, _local: Ipv4Addr) {}
