use crate::wled::protocol::AudioSyncPacket;
use socket2::{Domain, Protocol, Socket, Type};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SendError {
    #[error("socket error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketTarget {
    /// Every current multicast and unicast destination.
    Fanout,
    Multicast,
    Unicast(SocketAddr),
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

        log_socket_result(
            multicast_socket.set_multicast_ttl_v4(20),
            "IP_MULTICAST_TTL",
        );
        if let Some((_, ip)) = multicast_bind {
            log_multicast_if(multicast_socket.set_multicast_if_v4(&ip), ip);
        } else {
            warn_no_multicast_nic();
        }
        if let IpAddr::V4(addr) = multicast {
            let join_if = multicast_bind
                .map(|(_, ip)| ip)
                .unwrap_or(Ipv4Addr::UNSPECIFIED);
            log_socket_result(
                multicast_socket.join_multicast_v4(&addr, &join_if),
                &format!("join {addr} on {join_if}"),
            );
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
        self.send_target(packet, PacketTarget::Fanout)
    }

    pub fn send_target(
        &mut self,
        packet: &AudioSyncPacket,
        target: PacketTarget,
    ) -> Result<(), SendError> {
        let bytes = packet.encode();
        match target {
            PacketTarget::Fanout => {
                if self.multicast_enabled {
                    self.send_multicast(&bytes);
                }
                for i in 0..self.unicast_destinations.len() {
                    let dest = self.unicast_destinations[i];
                    self.send_unicast(&bytes, dest);
                }
            }
            PacketTarget::Multicast => {
                if self.multicast_enabled {
                    self.send_multicast(&bytes);
                }
            }
            PacketTarget::Unicast(dest) => self.send_unicast(&bytes, dest),
        }
        Ok(())
    }

    fn send_multicast(&mut self, bytes: &[u8]) {
        record_send(
            &mut self.packets_sent,
            &mut self.udp_errors,
            self.multicast_socket.send_to(bytes, self.multicast_addr),
            "multicast send failed",
        );
    }

    fn send_unicast(&mut self, bytes: &[u8], dest: SocketAddr) {
        record_send(
            &mut self.packets_sent,
            &mut self.udp_errors,
            self.unicast_socket.send_to(bytes, dest),
            &format!("unicast send to {dest} failed"),
        );
    }

    pub fn packets_sent(&self) -> u64 {
        self.packets_sent
    }

    pub fn udp_errors(&self) -> u64 {
        self.udp_errors
    }

    pub fn multicast_target(&self) -> SocketAddr {
        self.multicast_addr
    }

    pub fn unicast_targets(&self) -> &[SocketAddr] {
        &self.unicast_destinations
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

fn log_multicast_if(result: std::io::Result<()>, ip: Ipv4Addr) {
    match result {
        Ok(()) => tracing::info!("WLED multicast egress {ip} ttl=20"),
        Err(e) => tracing::warn!("IP_MULTICAST_IF {ip}: {e}"),
    }
}

fn warn_no_multicast_nic() {
    tracing::warn!("WLED multicast has no egress NIC (set network.multicast); kernel default route will be used");
}

fn log_socket_result(result: std::io::Result<()>, label: &str) {
    if let Err(e) = result {
        tracing::warn!("{label}: {e}");
    }
}

fn record_send(
    packets_sent: &mut u64,
    udp_errors: &mut u64,
    result: std::io::Result<usize>,
    warn: &str,
) {
    match result {
        Ok(_) => *packets_sent += 1,
        Err(e) => {
            *udp_errors += 1;
            tracing::warn!("{warn}: {e}");
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wled::protocol::AudioSyncPacket;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn destination_string_reflects_multicast_and_unicast() {
        let mut sender = WledSender::new(
            IpAddr::V4(Ipv4Addr::new(239, 0, 0, 1)),
            11988,
            vec!["10.11.1.124:11988".parse().unwrap()],
            None,
            None,
        )
        .unwrap();
        let dest = sender.destination();
        assert!(dest.contains("239.0.0.1"));
        assert!(dest.contains("10.11.1.124"));

        sender.set_multicast_enabled(false);
        assert_eq!(sender.destination(), "10.11.1.124");

        sender.set_unicast(vec![]);
        assert_eq!(sender.destination(), "none");
    }

    #[test]
    fn send_to_unicast_increments_counter() {
        let mut sender = WledSender::new(
            IpAddr::V4(Ipv4Addr::new(239, 0, 0, 1)),
            11988,
            vec!["127.0.0.1:9".parse().unwrap()],
            None,
            None,
        )
        .unwrap();
        sender.set_multicast_enabled(false);
        sender.send(&AudioSyncPacket::silence()).unwrap();
        assert_eq!(sender.packets_sent(), 1);
        assert_eq!(sender.udp_errors(), 0);

        let mut two = WledSender::new(
            IpAddr::V4(Ipv4Addr::new(239, 0, 0, 1)),
            11988,
            vec![
                "127.0.0.1:9".parse().unwrap(),
                "127.0.0.1:19".parse().unwrap(),
            ],
            None,
            None,
        )
        .unwrap();
        two.set_multicast_enabled(false);
        two.send(&AudioSyncPacket::silence()).unwrap();
        assert_eq!(two.packets_sent(), 2);
    }

    #[test]
    fn multicast_and_bound_sockets() {
        let mut sender = WledSender::new(
            IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
            11988,
            vec![],
            Some(("lo0", Ipv4Addr::LOCALHOST)),
            Some(("lo0", Ipv4Addr::LOCALHOST)),
        )
        .unwrap();
        sender.set_multicast_enabled(true);
        sender.send(&AudioSyncPacket::silence()).unwrap();
        assert!(sender.packets_sent() + sender.udp_errors() >= 1);

        let mut v4 = WledSender::new(
            IpAddr::V4(Ipv4Addr::new(239, 0, 0, 1)),
            11988,
            vec!["127.0.0.1:9".parse().unwrap()],
            None,
            Some(("lo0", Ipv4Addr::LOCALHOST)),
        )
        .unwrap();
        v4.send(&AudioSyncPacket::silence()).unwrap();
        assert!(v4.packets_sent() + v4.udp_errors() >= 1);

        let mut sent = 0;
        let mut errors = 0;
        record_send(&mut sent, &mut errors, Ok(4), "ok");
        record_send(
            &mut sent,
            &mut errors,
            Err(std::io::Error::other("fail")),
            "failed",
        );
        assert_eq!(sent, 1);
        assert_eq!(errors, 1);

        log_socket_result(Ok(()), "ok");
        log_socket_result(Err(std::io::Error::other("ttl")), "IP_MULTICAST_TTL");
        log_socket_result(
            Err(std::io::Error::other("join")),
            "join 239.0.0.1 on 0.0.0.0",
        );
        log_multicast_if(Ok(()), Ipv4Addr::LOCALHOST);
        log_multicast_if(Err(std::io::Error::other("if")), Ipv4Addr::LOCALHOST);
        warn_no_multicast_nic();
    }

    #[test]
    fn send_target_reaches_one_destination() {
        let mut sender = WledSender::new(
            IpAddr::V4(Ipv4Addr::new(239, 0, 0, 1)),
            11988,
            vec!["127.0.0.1:9".parse().unwrap()],
            None,
            None,
        )
        .unwrap();
        sender
            .send_target(
                &AudioSyncPacket::silence(),
                PacketTarget::Unicast("127.0.0.1:9".parse().unwrap()),
            )
            .unwrap();
        sender
            .send_target(&AudioSyncPacket::silence(), PacketTarget::Multicast)
            .unwrap();
        sender.set_multicast_enabled(false);
        sender
            .send_target(&AudioSyncPacket::silence(), PacketTarget::Multicast)
            .unwrap();
        assert!(sender.packets_sent() + sender.udp_errors() >= 1);
    }
}
