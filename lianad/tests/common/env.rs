use crate::common::node::NodeKind;

/// Returns the node to use for tests.
pub fn node_kind() -> NodeKind {
    // we use same env var as from Python tests
    match std::env::var("BITCOIN_BACKEND_TYPE") {
        Ok(v) if v.eq_ignore_ascii_case("electrs") => NodeKind::Electrs,
        _ => NodeKind::Bitcoind,
    }
}

/// Whether test runs should use Taproot descriptors.
pub fn use_taproot() -> bool {
    matches!(std::env::var("USE_TAPROOT"), Ok(v) if v == "1" || v.eq_ignore_ascii_case("true"))
}
