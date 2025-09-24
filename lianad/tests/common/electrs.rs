use electrsd::corepc_node::Node;
use electrsd::{downloaded_exe_path, ElectrsD};

pub fn start_electrs(bitcoind: &Node) -> anyhow::Result<ElectrsD> {
    let mut conf = electrsd::Conf::default();
    conf.view_stderr = true;
    let exe_path = match std::env::var("ELECTRS_PATH") {
        Ok(path) if !path.is_empty() => {
            println!("Using custom electrs binary");
            path
        }
        _ => {
            println!("Using downloaded electrs binary");
            downloaded_exe_path().ok_or_else(|| anyhow::anyhow!("electrs binary not available"))?
        }
    };
    println!("electrs binary: {}", exe_path);
    let electrs = ElectrsD::with_conf(exe_path, bitcoind, &conf)?;
    Ok(electrs)
}
