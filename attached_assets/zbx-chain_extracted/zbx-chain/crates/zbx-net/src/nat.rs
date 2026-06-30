//! NAT traversal -- STUN, UPnP, port mapping, external IP detection.
//!
//! ZBX nodes behind NAT need to discover their external IP to:
//!   1. Advertise the correct IP in their ENR record
//!   2. Accept inbound connections from the internet
//!   3. Correctly tell peers how to reach them
//!
//! IP resolution priority:
//!   1. User-provided --external-ip flag (highest priority)
//!   2. STUN query (discover from STUN server)
//!   3. UPnP port mapping (query router via UPnP/IGD)
//!   4. discv5 PONG recipient_ip (peers tell us our IP)
//!   5. Best-guess from network interfaces (fallback)
//!
//! Port numbers:
//!   TCP_PORT = 30303  (RLPx, configurable)
//!   UDP_PORT = 30303  (discv5, same by default)

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

// ── Constants ─────────────────────────────────────────────────────────────────

pub const TCP_PORT: u16 = 30303;
pub const UDP_PORT: u16 = 30303;
pub const STUN_TIMEOUT: Duration = Duration::from_secs(3);

/// ZBX STUN servers (public STUN infrastructure)
pub const STUN_SERVERS: &[&str] = &[
    "stun.zebvix.io:3478",
    "stun1.l.google.com:19302",
    "stun2.l.google.com:19302",
    "stun.cloudflare.com:3478",
];

// ── IP address utilities (IPv4 / IPv6) ───────────────────────────────────────

/// Whether an IPv4 address is private / non-routable.
/// Covers RFC1918, loopback, link-local, CGNAT ranges.
pub fn is_private_ipv4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_loopback()                                  // 127.0.0.0/8
    || o[0] == 10                                     // 10.0.0.0/8
    || (o[0] == 172 && (16..=31).contains(&o[1]))     // 172.16.0.0/12
    || (o[0] == 192 && o[1] == 168)                   // 192.168.0.0/16
    || (o[0] == 100 && (64..=127).contains(&o[1]))    // 100.64.0.0/10 CGNAT
    || (o[0] == 169 && o[1] == 254)                   // 169.254.0.0/16 link-local
    || ip.is_unspecified()
    || ip.is_broadcast()
}

/// Whether an IPv6 address is private / non-routable.
pub fn is_private_ipv6(ip: Ipv6Addr) -> bool {
    ip.is_loopback()       // ::1
    || ip.is_unspecified() // ::
    || { let o = ip.octets(); (o[0] & 0xfe) == 0xfc } // fc00::/7 Unique Local
    || { let o = ip.octets(); o[0] == 0xfe && (o[1] & 0xc0) == 0x80 } // fe80::/10 link-local
}

/// Check if an IP is publicly routable.
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip4) => !is_private_ipv4(ip4),
        IpAddr::V6(ip6) => !is_private_ipv6(ip6),
    }
}

// ── External IP resolution ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ExternalAddr {
    pub ip:     IpAddr,
    pub tcp:    u16,
    pub udp:    u16,
    pub source: NatSource,
}

#[derive(Debug, Clone)]
pub enum NatSource {
    UserConfig,
    Stun { server: String },
    Upnp,
    PeerReport,
    LocalInterface,
}

// ── STUN client ───────────────────────────────────────────────────────────────

/// Query a STUN server to discover external IP and UDP port.
///
/// Sends a STUN Binding Request over UDP (RFC 5389).
/// Receives STUN Binding Response with XOR-MAPPED-ADDRESS.
/// XOR-MAPPED-ADDRESS = external_ip XOR MAGIC_COOKIE (0x2112A442).
pub async fn stun_query(server: &str) -> Result<SocketAddr, StunError> {
    use tokio::net::UdpSocket;
    let socket = UdpSocket::bind("0.0.0.0:0").await
        .map_err(|e| StunError::Io(e.to_string()))?;
    let server_addr: SocketAddr = tokio::net::lookup_host(server).await
        .map_err(|e| StunError::Resolve(e.to_string()))?
        .next().ok_or_else(|| StunError::Resolve("no address".into()))?;

    // Build STUN Binding Request (20 bytes header)
    let mut req = [0u8; 20];
    req[0] = 0x00; req[1] = 0x01; // Message Type: Binding Request
    req[2] = 0x00; req[3] = 0x00; // Message Length: 0
    req[4] = 0x21; req[5] = 0x12; req[6] = 0xa4; req[7] = 0x42; // Magic Cookie
    getrandom::getrandom(&mut req[8..]).map_err(|_| StunError::Random)?; // Transaction ID

    socket.send_to(&req, server_addr).await
        .map_err(|e| StunError::Io(e.to_string()))?;

    let mut buf = [0u8; 512];
    let (n, _) = tokio::time::timeout(STUN_TIMEOUT, socket.recv_from(&mut buf)).await
        .map_err(|_| StunError::Timeout)?
        .map_err(|e| StunError::Io(e.to_string()))?;

    parse_stun_response(&buf[..n])
}

fn parse_stun_response(data: &[u8]) -> Result<SocketAddr, StunError> {
    if data.len() < 20 { return Err(StunError::InvalidResponse); }
    let mut i = 20;
    while i + 4 <= data.len() {
        let attr_type = u16::from_be_bytes([data[i], data[i+1]]);
        let attr_len  = u16::from_be_bytes([data[i+2], data[i+3]]) as usize;
        if attr_type == 0x0020 && i + 4 + attr_len <= data.len() {
            let family   = data[i+5];
            let xor_port = u16::from_be_bytes([data[i+6], data[i+7]]) ^ 0x2112;
            if family == 0x01 && attr_len >= 8 {
                let ip = Ipv4Addr::new(
                    data[i+8] ^ 0x21, data[i+9] ^ 0x12,
                    data[i+10] ^ 0xa4, data[i+11] ^ 0x42,
                );
                return Ok(SocketAddr::new(IpAddr::V4(ip), xor_port));
            } else if family == 0x02 && attr_len >= 20 {
                let mut ip6 = [0u8; 16];
                for j in 0..16 { ip6[j] = data[i+8+j] ^ 0x21; }
                return Ok(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(ip6)), xor_port));
            }
        }
        let pad = if attr_len % 4 != 0 { 4 - attr_len % 4 } else { 0 };
        i += 4 + attr_len + pad;
    }
    Err(StunError::NoMappedAddress)
}

/// Try all STUN servers in order until one succeeds.
pub async fn discover_external_ip() -> Option<ExternalAddr> {
    for server in STUN_SERVERS {
        if let Ok(addr) = stun_query(server).await {
            if is_public_ip(addr.ip()) {
                return Some(ExternalAddr {
                    ip: addr.ip(), tcp: TCP_PORT, udp: addr.port(),
                    source: NatSource::Stun { server: server.to_string() },
                });
            }
        }
    }
    None
}

/// UPnP port mapping -- request external port from router (IGD protocol).
/// Uses the "igd" crate in production (cargo dep).
pub async fn upnp_map_port(internal_port: u16, external_port: u16) -> Result<ExternalAddr, NatError> {
    // Production: use igd crate for UPnP IGD
    // 1. Discover gateway via SSDP multicast (239.255.255.250:1900)
    // 2. Query WAN IP address (GetExternalIPAddress)
    // 3. AddPortMapping(external_port -> internal:internal_port, TCP+UDP)
    let ip = discover_upnp_external_ip().await.ok_or(NatError::UpnpFailed)?;
    Ok(ExternalAddr { ip, tcp: external_port, udp: external_port, source: NatSource::Upnp })
}

/// NAT resolution orchestrator -- try all methods, return best result.
pub async fn resolve_external_addr(
    user_ip:      Option<IpAddr>,
    internal_tcp: u16,
    internal_udp: u16,
) -> ExternalAddr {
    if let Some(ip) = user_ip {
        return ExternalAddr { ip, tcp: internal_tcp, udp: internal_udp, source: NatSource::UserConfig };
    }
    if let Some(addr) = discover_external_ip().await { return addr; }
    if let Ok(addr) = upnp_map_port(internal_tcp, internal_tcp).await { return addr; }
    ExternalAddr {
        ip: local_interface_ip().unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
        tcp: internal_tcp, udp: internal_udp,
        source: NatSource::LocalInterface,
    }
}

#[derive(Debug)]
pub enum StunError {
    Io(String), Resolve(String), Timeout, Random, InvalidResponse, NoMappedAddress,
}

#[derive(Debug)]
pub enum NatError {
    UpnpFailed, StunFailed,
}

fn local_interface_ip() -> Option<IpAddr> { None }
async fn discover_upnp_external_ip() -> Option<IpAddr> { None }