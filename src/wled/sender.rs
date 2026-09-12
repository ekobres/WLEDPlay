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

        if let IpAddr::V4(addr) = multicast {
            socket.join_multicast_v4(&addr, &local)?;
            if !local.is_unspecified() {
                let _ = socket.set_multicast_if_v4(&local);
            }
        }

        Ok(Self {
            socket: socket.into(),
            multicast_addr: SocketAddr::new(multicast, port),
            unicast_destinations,
            packets_sent: 0,
            udp_errors: 0,
        })
    }

    pub fn send(&mut self, packet: &AudioSyncPacket) -> Result<(), SendError> {
        let bytes = packet.encode();
        match self.socket.send_to(&bytes, self.multicast_addr) {
            Ok(_) => self.packets_sent += 1,
            Err(e) => {
                self.udp_errors += 1;
                tracing::warn!("multicast send failed: {e}");
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
        if self.unicast_destinations.is_empty() {
            format!("{}", self.multicast_addr)
        } else {
            format!(
                "{} + {}",
                self.multicast_addr,
                self.unicast_destinations
                    .iter()
                    .map(|d| d.ip().to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            )
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

#[cfg(any(target_os = "linux", target_os = "android"))]
fn bind_udp_device(socket: &Socket, name: &str, local: Ipv4Addr) {
    if let Err(e) = socket.bind_device(Some(name.as_bytes())) {
        tracing::warn!("SO_BINDTODEVICE {name} failed ({e}); using bind address {local}");
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn bind_udp_device(_socket: &Socket, _name: &str, _local: Ipv4Addr) {}
