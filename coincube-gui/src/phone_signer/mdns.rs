//! mDNS browse + advertise for the LAN signer protocol.
//!
//! Service type: `_coincube-signer._tcp.local.`
//! TXT records on the phone's advertised service:
//!   - `v=1` (protocol version)
//!   - `fp=<8-hex>` (first 4 bytes of the phone's identity pubkey —
//!     the desktop filters its paired list against this)

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Mutex, OnceLock};

use mdns_sd::{Receiver, ServiceDaemon, ServiceEvent, ServiceInfo};

/// Service type announced and queried over mDNS.
pub const SERVICE_TYPE: &str = "_coincube-signer._tcp.local.";

/// Protocol version advertised in the TXT record (`v=…`).
pub const SERVICE_PROTOCOL_VERSION: u32 = 1;

/// One discovered phone, surfaced to the discovery loop.
#[derive(Debug, Clone)]
pub struct DiscoveredPhone {
    /// First 8 hex chars of the phone's cert fingerprint
    /// (`SHA-256(cert DER)`). Comes from the `fp=` TXT record. We
    /// compare this against `pin_hex8(&paired.cert_pin)` to
    /// match a discovery against a paired entry.
    pub cert_fp8: String,

    /// Address to dial. From the mDNS A/AAAA + SRV port.
    pub addr: SocketAddr,

    /// Service instance fullname. Surfaces in logs.
    pub instance_name: String,
}

/// Process-shared daemon. mdns-sd's `ServiceDaemon` is internally an
/// event loop; we want one per process so the browse keeps receiving
/// updates between ticks.
fn daemon() -> Result<&'static ServiceDaemon, mdns_sd::Error> {
    static DAEMON: OnceLock<ServiceDaemon> = OnceLock::new();
    if let Some(d) = DAEMON.get() {
        return Ok(d);
    }
    let d = ServiceDaemon::new()?;
    let _ = DAEMON.set(d);
    Ok(DAEMON.get().expect("just set"))
}

/// Most recently observed discovered phones, keyed by fullname.
/// Updated by [`browse`] as `ServiceEvent`s arrive; surfaced to
/// `hw.rs` as a flat snapshot.
fn cache() -> &'static Mutex<HashMap<String, DiscoveredPhone>> {
    static CACHE: OnceLock<Mutex<HashMap<String, DiscoveredPhone>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Cached browse channel — initialised on first call so the daemon
/// keeps the browse subscription open across discovery ticks.
fn browse_receiver() -> Result<&'static Receiver<ServiceEvent>, mdns_sd::Error> {
    static RECEIVER: OnceLock<Receiver<ServiceEvent>> = OnceLock::new();
    if let Some(r) = RECEIVER.get() {
        return Ok(r);
    }
    let d = daemon()?;
    let r = d.browse(SERVICE_TYPE)?;
    let _ = RECEIVER.set(r);
    Ok(RECEIVER.get().expect("just set"))
}

/// Browse the LAN for phones advertising the signer service. Returns
/// the current snapshot. Intended to be called every discovery tick
/// from `hw.rs::make_refresh_stream`.
pub fn browse() -> Vec<DiscoveredPhone> {
    if let Err(e) = drain_events() {
        tracing::debug!("local-signer mdns drain: {}", e);
    }
    let cache = cache().lock().expect("mdns cache poisoned");
    cache.values().cloned().collect()
}

fn drain_events() -> Result<(), mdns_sd::Error> {
    let rx = browse_receiver()?;
    // Non-blocking drain: process whatever's pending and bail on
    // either Empty (nothing more right now) or Disconnected.
    while let Ok(event) = rx.try_recv() {
        apply_event(event);
    }
    Ok(())
}

fn apply_event(event: ServiceEvent) {
    let mut cache = cache().lock().expect("mdns cache poisoned");
    match event {
        ServiceEvent::ServiceResolved(info) => {
            if let Some(d) = to_discovered(&info) {
                cache.insert(info.get_fullname().to_string(), d);
            }
        }
        ServiceEvent::ServiceRemoved(_, fullname) => {
            cache.remove(&fullname);
        }
        _ => {}
    }
}

fn to_discovered(info: &ServiceInfo) -> Option<DiscoveredPhone> {
    let v = info.get_property_val_str("v").unwrap_or("");
    if v.parse::<u32>().ok() != Some(SERVICE_PROTOCOL_VERSION) {
        return None;
    }
    let fp = info.get_property_val_str("fp")?.to_string();
    // Need at least one A/AAAA + port to dial.
    let port = info.get_port();
    let ip = pick_address(info.get_addresses().iter().copied())?;
    let addr = SocketAddr::new(ip, port);
    Some(DiscoveredPhone {
        cert_fp8: fp,
        addr,
        instance_name: info.get_fullname().to_string(),
    })
}

/// Pick the most LAN-reachable address from the set the phone
/// advertised. mdns-sd surfaces every A / AAAA record the phone
/// publishes, in HashSet order, so picking `.iter().next()` would
/// non-deterministically grab the phone's global IPv6 — which is
/// routinely unreachable from a desktop on the same Wi-Fi when the
/// local network doesn't bridge IPv6 transit. RFC1918 IPv4 / ULA
/// IPv6 first, link-local next, public IPv4 after that, and only
/// fall back to global IPv6 when nothing else is on offer.
fn pick_address<I: IntoIterator<Item = IpAddr>>(addrs: I) -> Option<IpAddr> {
    addrs.into_iter().min_by_key(address_priority)
}

/// Lower is better. See [`pick_address`].
fn address_priority(ip: &IpAddr) -> u8 {
    match ip {
        IpAddr::V4(v4) => {
            if v4.is_private() {
                0
            } else if v4.is_link_local() {
                2
            } else if v4.is_loopback() {
                3
            } else {
                4
            }
        }
        IpAddr::V6(v6) => {
            let seg0 = v6.segments()[0];
            // ULA fc00::/7 — IPv6 equivalent of RFC1918.
            if (seg0 & 0xfe00) == 0xfc00 {
                1
            // Link-local fe80::/10.
            } else if (seg0 & 0xffc0) == 0xfe80 {
                2
            } else if v6.is_loopback() {
                3
            } else {
                // Global IPv6 — the bug case. Deprioritised because
                // home routers commonly hand out a global v6 prefix
                // on Wi-Fi but don't actually route v6 packets, so
                // the desktop's SYN goes nowhere even though both
                // devices are on the same SSID.
                5
            }
        }
    }
}

// `advertise_pairing_target` + `AdvertiseHandle` were removed in
// the cross-repo interop fixes: the phone is the TLS server in all
// flows now, including pairing, so the desktop never advertises.
// See plans/PLAN-local-signer-lan-interop-fixes-desktop.md §1.1.

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn v4(s: &str) -> IpAddr {
        s.parse::<Ipv4Addr>().expect("parse v4").into()
    }
    fn v6(s: &str) -> IpAddr {
        s.parse::<Ipv6Addr>().expect("parse v6").into()
    }

    /// The regression that motivated `pick_address`: the phone
    /// advertises a LAN IPv4 *and* a global IPv6, and the previous
    /// `.iter().next()` picker would grab whichever the HashSet
    /// happened to surface first. Now the LAN v4 wins.
    #[test]
    fn pick_address_prefers_rfc1918_v4_over_global_v6() {
        let lan = v4("192.168.1.50");
        let pub_v6 = v6("2806:103e:1b:4d12:38:949d:6e83:e89e");
        // Both orderings must give the LAN address — HashSet order
        // is non-deterministic in production, so the picker can't
        // rely on input order.
        assert_eq!(pick_address([lan, pub_v6]).unwrap(), lan);
        assert_eq!(pick_address([pub_v6, lan]).unwrap(), lan);
    }

    #[test]
    fn pick_address_prefers_ula_v6_over_global_v6() {
        // fd00::/8 (subset of fc00::/7 ULA) beats a global v6.
        let ula = v6("fd12:3456:789a::1");
        let pub_v6 = v6("2606:4700::1");
        assert_eq!(pick_address([pub_v6, ula]).unwrap(), ula);
    }

    #[test]
    fn pick_address_prefers_link_local_v6_over_global_v6() {
        let link_local = v6("fe80::1");
        let pub_v6 = v6("2606:4700::1");
        assert_eq!(pick_address([pub_v6, link_local]).unwrap(), link_local);
    }

    #[test]
    fn pick_address_falls_back_to_global_v6_when_nothing_better() {
        let pub_v6 = v6("2606:4700::1");
        assert_eq!(pick_address([pub_v6]).unwrap(), pub_v6);
    }

    #[test]
    fn pick_address_returns_none_for_empty_set() {
        let empty: [IpAddr; 0] = [];
        assert!(pick_address(empty).is_none());
    }

    #[test]
    fn pick_address_prefers_rfc1918_over_public_v4() {
        let lan = v4("10.0.0.5");
        let public = v4("8.8.8.8");
        assert_eq!(pick_address([public, lan]).unwrap(), lan);
    }
}
