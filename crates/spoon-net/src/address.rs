//! Address classification for egress policy.
//!
//! The rule is allowlist-shaped rather than blocklist-shaped: an address is
//! dialable only when it is clearly public unicast. A class this module does
//! not recognize is refused, so an address range standardized after this code
//! was written is denied by default instead of quietly reachable.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use serde::{Deserialize, Serialize};

/// Which address classes the host permits this adapter to dial.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AddressPolicy {
    /// Public unicast only. Loopback, private, link-local, unique-local,
    /// carrier-grade NAT, unspecified, multicast, broadcast, and reserved
    /// addresses are all refused.
    #[default]
    PublicOnly,
    /// Public unicast plus loopback. Intended for host-local services and
    /// tests; every other non-public class stays refused.
    LoopbackPermitted,
}

impl AddressPolicy {
    pub fn permits(self, address: &IpAddr) -> bool {
        match self {
            Self::PublicOnly => is_public_unicast(address),
            Self::LoopbackPermitted => is_public_unicast(address) || is_loopback(address),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::PublicOnly => "public-only",
            Self::LoopbackPermitted => "loopback-permitted",
        }
    }
}

fn is_loopback(address: &IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => address.is_loopback(),
        IpAddr::V6(address) => {
            address.is_loopback()
                || address
                    .to_ipv4_mapped()
                    .is_some_and(|address| address.is_loopback())
        }
    }
}

fn is_public_unicast(address: &IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_public_v4(address),
        IpAddr::V6(address) => is_public_v6(address),
    }
}

fn is_public_v4(address: &Ipv4Addr) -> bool {
    let [first, second, third, _] = address.octets();
    let reserved = address.is_unspecified()
        || address.is_loopback()
        || address.is_private()
        || address.is_link_local()
        || address.is_multicast()
        || address.is_broadcast()
        || address.is_documentation()
        // "This network" 0.0.0.0/8.
        || first == 0
        // Carrier-grade NAT 100.64.0.0/10, routable to a provider's interior.
        || (first == 100 && (64..128).contains(&second))
        // IETF protocol assignments 192.0.0.0/24, including NAT64 discovery.
        || (first == 192 && second == 0 && third == 0)
        // 6to4 relay anycast 192.88.99.0/24.
        || (first == 192 && second == 88 && third == 99)
        // Benchmarking 198.18.0.0/15.
        || (first == 198 && (18..20).contains(&second))
        // Reserved and multicast space 224.0.0.0/3, which subsumes broadcast.
        || first >= 224;
    !reserved
}

fn is_public_v6(address: &Ipv6Addr) -> bool {
    // An IPv4-mapped address dials the embedded IPv4 destination, so
    // `::ffff:127.0.0.1` has to be judged as loopback rather than as a
    // v6 address the v6 rules happen not to name.
    if let Some(mapped) = address.to_ipv4_mapped() {
        return is_public_v4(&mapped);
    }
    // Any other address inside the all-zero prefix is either unspecified,
    // loopback, or the deprecated IPv4-compatible form. None are dialable.
    if address.to_ipv4().is_some() {
        return false;
    }
    let segments = address.segments();
    let reserved = address.is_multicast()
        // Unique local fc00::/7.
        || segments[0] & 0xfe00 == 0xfc00
        // Link-local unicast fe80::/10.
        || segments[0] & 0xffc0 == 0xfe80
        // Deprecated site-local fec0::/10.
        || segments[0] & 0xffc0 == 0xfec0
        // Discard-only 100::/64.
        || (segments[0] == 0x0100 && segments[1] == 0 && segments[2] == 0 && segments[3] == 0)
        // Documentation 2001:db8::/32.
        || (segments[0] == 0x2001 && segments[1] == 0x0db8);
    !reserved
}
