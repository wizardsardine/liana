//! mDNS browse + advertise for the LAN signer protocol.
//!
//! Service type: `_coincube-signer._tcp.local.`
//! TXT records on the phone's advertised service:
//!   - `v=1`               (protocol version)
//!   - `fp=<8-hex>`        (first 4 bytes of the phone's identity
//!                          pubkey — the desktop filters its paired
//!                          list against this)

use std::collections::HashMap;
use std::net::SocketAddr;
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
    /// compare this against `pin_hex8(&paired.identity_pubkey)` to
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
    let ip = info.get_addresses().iter().next()?;
    let addr = SocketAddr::new(*ip, port);
    Some(DiscoveredPhone {
        cert_fp8: fp,
        addr,
        instance_name: info.get_fullname().to_string(),
    })
}

// `advertise_pairing_target` + `AdvertiseHandle` were removed in
// the cross-repo interop fixes: the phone is the TLS server in all
// flows now, including pairing, so the desktop never advertises.
// See plans/PLAN-local-signer-lan-interop-fixes-desktop.md §1.1.
