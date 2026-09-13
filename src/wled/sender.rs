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
    multicast_socket: UdpSocket,
    unicast_socket: UdpSocket,
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
        unicast: Option<(&str, Ipv4Addr)>,
        multicast_bind: Option<(&str, Ipv4Addr)>,
    ) -> Result<Self, SendError> {
        let unicast_socket = bind_udp(unicast)?;
        let multicast_socket = bind_udp(multicast_bind)?;

        if let Err(e) = multicast_socket.set_multicast_ttl_v4(20) {
            tracing::warn!("IP_MULTICAST_TTL: {e}");
        }
        if let Some((_, ip)) = multicast_bind {
            if let Err(e) = multicast_socket.set_multicast_if_v4(&ip) {
                tracing::warn!("IP_MULTICAST_IF {ip}: {e}");
            } else {
                tracing::info!("WLED multicast egress {ip} ttl=20");
            }
        } else {
            tracing::warn!(
                "WLED multicast has no egress NIC (set network.multicast); kernel default route will be used"
            );
        }
        if let IpAddr::V4(addr) = multicast {
            let join_if = multicast_bind
                .map(|(_, ip)| ip)
                .unwrap_or(Ipv4Addr::UNSPECIFIED);
            if let Err(e) = multicast_socket.join_multicast_v4(&addr, &join_if) {
                tracing::warn!("join {addr} on {join_if}: {e}");
            }
        }
        if let Some((name, ip)) = unicast {
            tracing::info!("WLED unicast egress {name} ({ip})");
        }

        Ok(Self {
            multicast_socket: multicast_socket.into(),
            unicast_socket: unicast_socket.into(),
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
            match self.multicast_socket.send_to(&bytes, self.multicast_addr) {
                Ok(_) => self.packets_sent += 1,
                Err(e) => {
                    self.udp_errors += 1;
                    tracing::warn!("multicast send failed: {e}");
                }
            }
        }
        for dest in &self.unicast_destinations.clone() {
            if let Err(e) = self.unicast_socket.send_to(&bytes, dest) {
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

fn bind_udp(bind: Option<(&str, Ipv4Addr)>) -> Result<Socket, SendError> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    let local = bind.map(|(_, ip)| ip).unwrap_or(Ipv4Addr::UNSPECIFIED);
    socket.bind(&SocketAddr::from((local, 0)).into())?;
    if let Some((name, ip)) = bind.filter(|(n, _)| !n.is_empty()) {
        bind_udp_device(&socket, name, ip);
    }
    Ok(socket)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn bind_udp_device(socket: &Socket, name: &str, local: Ipv4Addr) {
    if let Err(e) = socket.bind_device(Some(name.as_bytes())) {
        tracing::warn!("SO_BINDTODEVICE {name} failed ({e}); using bind address {local}");
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn bind_udp_device(_socket: &Socket, _name: &str, _local: Ipv4Addr) {}
