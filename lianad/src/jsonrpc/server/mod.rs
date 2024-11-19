#[cfg(unix)]
mod unix;

use std::{
    io, path,
    sync::{atomic::AtomicBool, Arc},
};

use crate::DaemonControl;

#[cfg(unix)]
pub fn run(
    socket_path: &path::Path,
    daemon_control: DaemonControl,
    shutdown: Arc<AtomicBool>,
) -> Result<(), io::Error> {
    let listener = unix::rpcserver_setup(socket_path)?;
    log::info!("JSONRPC server started.");
    let res = unix::rpcserver_loop(listener, daemon_control, shutdown);
    log::info!("JSONRPC server stopped.");
    res
}

#[cfg(windows)]
pub fn run(
    _socket_path: &path::Path,
    _daemon_control: DaemonControl,
    _shutdown: Arc<AtomicBool>,
) -> Result<(), io::Error> {
    todo!("Implement a json rpc server over Named pipe");
}
