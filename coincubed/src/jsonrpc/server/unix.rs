//! JSONRPC2 server
//!
//! This module implements the connections and streams handling logic for receiving
//! JSONRPC2 requests on a Unix Domain Socket.

use crate::{
    jsonrpc::{
        api,
        rpc::{Request, Response},
    },
    DaemonControl,
};

use std::{
    fs, io,
    os::unix::{fs::PermissionsExt, net},
    path,
    sync::{self, atomic},
    thread, time,
};

// Maximum number of concurrent RPC connections we may accept.
const MAX_CONNECTIONS: u32 = 16;
// One JSON-RPC message. Caps the doubling read buffer so a peer cannot force
// an unbounded allocation while holding a connection slot.
const MAX_RPC_MESSAGE: usize = 4 * 1024 * 1024;
// An idle local client must eventually release its handler slot. This bounds
// waiting for the next request byte, not execution of a wallet operation.
const CONNECTION_IDLE_TIMEOUT: time::Duration = time::Duration::from_secs(30);

// Read a command from the stream.
//
// In order to both treat commands separately (respond as soon as we read one), and support
// multiple commands in a single read or in multiple parts, we are given the context as writable
// arguments:
//   - `buf` is the buffer used to read from the socket. It will be extended as needed. It must be
//   initialized.
//   - `end`: The index of the end of the data read from the stream. Since `buf` needs to be
//   initialized with dummy values, it can be very different from `buf.len()`. Used to not check
//   for the separator character in the parts of the buffer with dummy values.
//   - `cursor`: The index at which we checked for the separator character (`\n`). Used to not
//   check twice for it on the same buffer chunk.
fn read_command(
    stream: &mut dyn io::Read,
    buf: &mut Vec<u8>,
    end: &mut usize,
    cursor: &mut usize,
) -> Result<Option<Request>, io::Error> {
    assert!(!buf.is_empty());

    loop {
        // First off, check if there are no existing commands in the buffer.
        let pos = buf[*cursor..*end].iter().position(|byt| byt == &b'\n');
        if let Some(pos) = pos {
            // TODO: don't return an io::Error here, instead try to parse a Request. Failing that,
            // try to parse a serde_json::Value. Then return accordingly a JSONRPC "malformed
            // request" or "invalid JSON" error.
            let req: Request = serde_json::from_slice(&buf[..*cursor + pos])?;
            *buf = buf[pos + 1..].to_vec(); // FIXME: can we avoid reallocating here?
            *cursor = 0;
            *end -= pos + 1;

            return Ok(Some(req));
        }

        // If nothing can be gathered from the buffer, continue reading.
        let new_read = stream.read(&mut buf[*end..])?;
        if new_read == 0 {
            return Ok(None);
        }

        // If we filled the buffer, increase its size and try again.
        *end += new_read;
        if *end > MAX_RPC_MESSAGE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "JSON-RPC message exceeds 4 MiB",
            ));
        }
        let buffer_filled = *end == buf.len();
        if buffer_filled {
            let next = buf.len().saturating_mul(2).min(MAX_RPC_MESSAGE);
            if next <= buf.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "JSON-RPC message exceeds 4 MiB",
                ));
            }
            buf.resize(next, 0);
            continue;
        }
    }
}

// Handle all messages from this connection.
fn connection_handler(
    mut control: DaemonControl,
    mut stream: net::UnixStream,
    shutdown: sync::Arc<atomic::AtomicBool>,
) -> Result<(), io::Error> {
    let mut buf = vec![0; 2048];
    let mut end = 0;
    let mut cursor = 0;

    while !shutdown.load(atomic::Ordering::Relaxed) {
        let req = match read_command(&mut stream, &mut buf, &mut end, &mut cursor) {
            Ok(Some(req)) => req,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                // Accepted streams are blocking; these errors mean the finite
                // idle timeout elapsed, not that a delayed request was polled.
                return Ok(());
            }
            Err(error) => return Err(error),
            Ok(None) => {
                // Connection closed.
                return Ok(());
            }
        };

        let req_id = req.id.clone();
        if &req.method == "stop" {
            shutdown.store(true, atomic::Ordering::Relaxed);
            log::info!("Stopping the coincube daemon.");
        }

        log::trace!("JSONRPC request method={} id={:?}", req.method, req.id);
        let response =
            api::handle_request(&mut control, req).unwrap_or_else(|e| Response::error(req_id, e));
        log::trace!("JSONRPC response written");
        if let Err(e) = serde_json::to_writer(&stream, &response) {
            log::error!("Error writing response: '{}'", e);
            return Ok(());
        }
    }

    Ok(())
}

// FIXME: have a decent way to share the DaemonControl between connections. Maybe make it Clone?
/// The main event loop. Wait for connections, and treat requests sent through them.
pub fn rpcserver_loop(
    listener: net::UnixListener,
    socket_path: &path::Path,
    daemon_control: DaemonControl,
    shutdown: sync::Arc<atomic::AtomicBool>,
) -> Result<(), io::Error> {
    rpcserver_loop_with_timeout(
        listener,
        socket_path,
        daemon_control,
        shutdown,
        CONNECTION_IDLE_TIMEOUT,
        sync::Arc::new(atomic::AtomicU32::new(0)),
    )
}

/// Reject a peer whose uid is not the socket file's owner. Mode `0600` already
/// blocks other users on Linux; this catches a socket whose mode was widened.
fn ensure_peer_owns_socket(stream: &net::UnixStream, socket_path: &path::Path) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let owner = fs::metadata(socket_path)?.uid();
    let Some(peer) = peer_uid(stream)? else {
        return Ok(());
    };
    if peer != owner {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "JSON-RPC peer uid does not own the socket",
        ));
    }
    Ok(())
}

/// `None` on a Unix where peer credentials are not available. Callers then
/// rely on the socket mode alone.
fn peer_uid(stream: &net::UnixStream) -> io::Result<Option<u32>> {
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::io::AsRawFd;
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let rc = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut cred as *mut libc::ucred as *mut libc::c_void,
                &mut len,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Some(cred.uid))
    }
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd"
    ))]
    {
        use std::os::unix::io::AsRawFd;
        let mut uid: libc::uid_t = 0;
        let mut gid: libc::gid_t = 0;
        let rc = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Some(uid as u32))
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd"
    )))]
    {
        let _ = stream;
        Ok(None)
    }
}

fn prepare_connection(
    connection: &net::UnixStream,
    socket_path: &path::Path,
    idle_timeout: time::Duration,
) -> bool {
    // The listener is non-blocking so this loop can poll `shutdown`, and on
    // macOS `accept(2)` hands back a socket that inherits that flag (Linux's
    // `accept4` does not). Left inherited, the handler's first `read` returns
    // `WouldBlock` for any client whose bytes have not landed yet, which
    // `read_command` propagates and which closes the connection with the
    // request unread. Each connection is served by its own blocking thread;
    // only the accept loop needs to poll.
    // A client can disconnect between accept and socket configuration.
    // The socket belongs to that client, so its failure must not stop the
    // listener or keep later clients from reaching the daemon.
    if let Err(error) = connection.set_nonblocking(false) {
        log::warn!(
            "Discarding JSONRPC client: blocking setup failed: {}",
            error
        );
        return false;
    }
    if let Err(error) = connection.set_read_timeout(Some(idle_timeout)) {
        log::warn!(
            "Discarding JSONRPC client: read timeout setup failed: {}",
            error
        );
        return false;
    }
    if let Err(error) = ensure_peer_owns_socket(connection, socket_path) {
        log::warn!("Discarding JSONRPC client: {}", error);
        return false;
    }
    true
}

fn wait_for_capacity(
    connections_counter: &atomic::AtomicU32,
    shutdown: &atomic::AtomicBool,
) -> bool {
    while connections_counter.load(atomic::Ordering::Relaxed) >= MAX_CONNECTIONS {
        if shutdown.load(atomic::Ordering::Relaxed) {
            return false;
        }
        thread::sleep(time::Duration::from_millis(50));
    }
    if shutdown.load(atomic::Ordering::Relaxed) {
        return false;
    }
    true
}

fn rpcserver_loop_with_timeout(
    listener: net::UnixListener,
    socket_path: &path::Path,
    daemon_control: DaemonControl,
    shutdown: sync::Arc<atomic::AtomicBool>,
    idle_timeout: time::Duration,
    connections_counter: sync::Arc<atomic::AtomicU32>,
) -> Result<(), io::Error> {
    // Each connection has a blocking handler, bounded in count and idle time.

    listener.set_nonblocking(true)?;
    while !shutdown.load(atomic::Ordering::Relaxed) {
        let (connection, _) = match listener.accept() {
            Ok(c) => c,
            Err(_) => {
                thread::sleep(time::Duration::from_millis(100));
                continue;
            }
        };
        if !prepare_connection(&connection, socket_path, idle_timeout) {
            continue;
        }
        log::trace!("New JSONRPC connection");

        if !wait_for_capacity(&connections_counter, &shutdown) {
            return Ok(());
        }
        connections_counter.fetch_add(1, atomic::Ordering::Relaxed);

        let handler_id = connections_counter.load(atomic::Ordering::Relaxed);
        thread::Builder::new()
            .name(format!("coincube-jsonrpc-{}", handler_id))
            .spawn({
                let control = daemon_control.clone();
                let counter = connections_counter.clone();
                let shutdown = shutdown.clone();

                move || {
                    if let Err(e) = connection_handler(control, connection, shutdown) {
                        log::error!("Error while handling connection {}: '{}'", handler_id, e);
                    } else {
                        log::trace!("Connection {} terminated without error.", handler_id);
                    }
                    counter.fetch_sub(1, atomic::Ordering::Relaxed);
                }
            })?;
    }

    Ok(())
}

// Tries to bind to the socket, if we are told it's already in use try to connect
// to check there is actually someone listening and it's not a leftover from a
// crash.
fn bind(socket_path: &path::Path) -> Result<net::UnixListener, io::Error> {
    match net::UnixListener::bind(socket_path) {
        Ok(l) => Ok(l),
        Err(e) => {
            if e.kind() == io::ErrorKind::AddrInUse {
                return match net::UnixStream::connect(socket_path) {
                    Ok(_) => Err(e),
                    Err(_) => {
                        // Ok, no one's here. Just delete the socket and bind.
                        log::debug!("Removing leftover rpc socket.");
                        std::fs::remove_file(socket_path)?;
                        net::UnixListener::bind(socket_path)
                    }
                };
            }

            Err(e)
        }
    }
}

/// Bind to the UDS at `socket_path`
pub fn rpcserver_setup(socket_path: &path::Path) -> Result<net::UnixListener, io::Error> {
    // info, not debug: the control-socket path (hashed into the temp dir since
    // it must fit sun_path) is how clients — including the functional test
    // harness — discover where to connect. Keep it at the same level as the
    // "JSONRPC server started." line below so it's present whenever the server
    // is reachable, not only under debug logging.
    log::info!("Binding socket at {}", socket_path.display());
    // Create the socket with RW permissions only for the user
    let listener = bind(socket_path)?;

    // Set the permissions to RW for the user only
    let permissions = fs::Permissions::from_mode(0o600);
    fs::set_permissions(socket_path, permissions)?;

    Ok(listener)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jsonrpc::rpc::{Params, ReqId};

    use std::{env, fs, process};

    #[cfg(not(windows))]
    use std::io::{Read, Write};

    fn with_rpc_fixture(
        idle_timeout: time::Duration,
        saturate: bool,
        test: impl FnOnce(
            &path::Path,
            &sync::Arc<atomic::AtomicBool>,
            &thread::JoinHandle<Result<(), io::Error>>,
        ),
    ) {
        let daemon = crate::testutils::DummyCoincube::new(
            crate::testutils::DummyBitcoind::new(),
            crate::testutils::DummyDatabase::new(),
        );
        let socket_path = env::temp_dir().join(format!(
            "rpc-idle-{}-{:?}",
            process::id(),
            thread::current().id()
        ));
        let listener = rpcserver_setup(&socket_path).unwrap();
        let shutdown = sync::Arc::new(atomic::AtomicBool::new(false));
        let counter = sync::Arc::new(atomic::AtomicU32::new(0));
        let control = daemon.control().clone();
        let flag = shutdown.clone();
        let active = counter.clone();
        let server_path = socket_path.clone();
        let server = thread::spawn(move || {
            rpcserver_loop_with_timeout(listener, &server_path, control, flag, idle_timeout, active)
        });
        let idle: Vec<_> = (0..if saturate { MAX_CONNECTIONS } else { 0 })
            .map(|_| net::UnixStream::connect(&socket_path).unwrap())
            .collect();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if saturate {
                let deadline = time::Instant::now() + time::Duration::from_secs(5);
                while counter.load(atomic::Ordering::Relaxed) != MAX_CONNECTIONS {
                    assert!(
                        time::Instant::now() < deadline,
                        "server never reached capacity"
                    );
                    thread::sleep(time::Duration::from_millis(5));
                }
            }
            test(&socket_path, &shutdown, &server);
        }));
        // Always release clients before joining, including if an assertion fails.
        shutdown.store(true, atomic::Ordering::Relaxed);
        drop(idle);
        server.join().unwrap().unwrap();
        daemon.shutdown();
        fs::remove_file(socket_path).unwrap();
        if let Err(panic) = outcome {
            std::panic::resume_unwind(panic);
        }
    }

    fn request_from_late_client(socket_path: &path::Path, fragmented: bool) {
        let mut client = net::UnixStream::connect(socket_path).unwrap();
        client
            .set_read_timeout(Some(time::Duration::from_secs(5)))
            .unwrap();
        if fragmented {
            thread::sleep(time::Duration::from_millis(50));
        }
        let request = b"{\"jsonrpc\":\"2.0\",\"id\":17,\"method\":\"unknown_test_method\"}\n";
        client.write_all(&request[..20]).unwrap();
        if fragmented {
            thread::sleep(time::Duration::from_millis(50));
        }
        client.write_all(&request[20..]).unwrap();
        let response = serde_json::Deserializer::from_reader(&mut client)
            .into_iter::<serde_json::Value>()
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(response["id"], 17);
        assert!(
            response.get("error").is_some(),
            "unknown method was handled"
        );
    }

    #[test]
    fn idle_connections_release_capacity_for_later_requests() {
        with_rpc_fixture(time::Duration::from_secs(1), true, |path, _, _| {
            request_from_late_client(path, false);
        });
    }

    #[test]
    fn saturated_capacity_wait_observes_shutdown_before_idle_timeout() {
        with_rpc_fixture(
            time::Duration::from_secs(30),
            true,
            |path, shutdown, server| {
                // Force accept() into the capacity wait with a seventeenth client.
                let _pending = net::UnixStream::connect(path).unwrap();
                thread::sleep(time::Duration::from_millis(150));
                shutdown.store(true, atomic::Ordering::Relaxed);
                let deadline = time::Instant::now() + time::Duration::from_secs(3);
                while !server.is_finished() && time::Instant::now() < deadline {
                    thread::sleep(time::Duration::from_millis(10));
                }
                assert!(server.is_finished(), "shutdown waited for idle handlers");
            },
        );
    }

    #[test]
    fn finite_idle_timeout_accepts_delayed_and_fragmented_requests() {
        with_rpc_fixture(time::Duration::from_secs(1), false, |path, _, _| {
            request_from_late_client(path, true);
        });
    }

    /// A peer can close after connecting but before the accept loop configures
    /// its socket. That abandoned connection must not stop later RPC clients.
    #[test]
    fn closed_peer_before_accept_does_not_stop_rpc_server() {
        for closed_peer_first in [false, true] {
            let daemon = crate::testutils::DummyCoincube::new(
                crate::testutils::DummyBitcoind::new(),
                crate::testutils::DummyDatabase::new(),
            );
            let socket_path = env::temp_dir().join(format!(
                "rpc-closed-peer-{}-{:?}-{closed_peer_first}",
                process::id(),
                thread::current().id()
            ));
            let listener = rpcserver_setup(&socket_path).unwrap();
            if closed_peer_first {
                // Queue a real, already closed socket before starting accept().
                drop(net::UnixStream::connect(&socket_path).unwrap());
            }
            let shutdown = sync::Arc::new(atomic::AtomicBool::new(false));
            let server = thread::spawn({
                let control = daemon.control().clone();
                let shutdown = shutdown.clone();
                let server_path = socket_path.clone();
                move || {
                    rpcserver_loop_with_timeout(
                        listener,
                        &server_path,
                        control,
                        shutdown,
                        time::Duration::from_secs(30),
                        sync::Arc::new(atomic::AtomicU32::new(0)),
                    )
                }
            });

            let response = std::panic::catch_unwind(|| {
                request_from_late_client(&socket_path, false);
            });
            shutdown.store(true, atomic::Ordering::Relaxed);
            let server_result = server.join().unwrap();
            daemon.shutdown();
            fs::remove_file(&socket_path).unwrap();
            assert!(
                server_result.is_ok(),
                "RPC server exited after closed_peer_first={}: {:?}",
                closed_peer_first,
                server_result
            );
            if let Err(panic) = response {
                std::panic::resume_unwind(panic);
            }
        }
    }

    fn read_one_command(socket_path: &path::Path) -> thread::JoinHandle<Option<Request>> {
        let listener = rpcserver_setup(socket_path).unwrap();
        thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut buf = vec![0; 32];
            let mut end = 0;
            let mut cursor = 0;
            read_command(&mut conn, &mut buf, &mut end, &mut cursor).unwrap()
        })
    }

    fn read_all_commands(socket_path: &path::Path) -> thread::JoinHandle<Vec<Request>> {
        let listener = rpcserver_setup(socket_path).unwrap();
        thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut buf = vec![0; 32];
            let mut end = 0;
            let mut cursor = 0;
            let mut reqs = Vec::new();

            loop {
                match read_command(&mut conn, &mut buf, &mut end, &mut cursor).unwrap() {
                    Some(req) => {
                        reqs.push(req);
                    }
                    None => return reqs,
                }
            }
        })
    }

    /// A client that connects and then stays silent past the accept loop's
    /// 100 ms tick before sending its request — losing the race the real
    /// clients normally win. The server must still read the request when it
    /// eventually arrives.
    ///
    /// This is the deterministic form of the macOS defect in #478: `accept(2)`
    /// there hands back a socket that inherits the listener's `O_NONBLOCK`
    /// (Linux's `accept4` does not), so the handler's first `read` returns
    /// `WouldBlock`, `read_command`'s `?` propagates it out of
    /// `connection_handler`, and the connection is closed with the request
    /// unread. A client that writes immediately usually wins; one that pauses
    /// never does.
    #[test]
    fn a_slow_client_request_is_read_not_dropped() {
        let ms = crate::testutils::DummyCoincube::new(
            crate::testutils::DummyBitcoind::new(),
            crate::testutils::DummyDatabase::new(),
        );
        let socket_path = env::temp_dir().join(format!("cc-slow-client-{}.sock", process::id()));
        let _ = fs::remove_file(&socket_path);
        let listener = rpcserver_setup(&socket_path).unwrap();
        let shutdown = sync::Arc::new(atomic::AtomicBool::new(false));
        let server = thread::spawn({
            let control = ms.control().clone();
            let shutdown = shutdown.clone();
            let server_path = socket_path.clone();
            move || rpcserver_loop(listener, &server_path, control, shutdown)
        });

        let mut client = net::UnixStream::connect(&socket_path).unwrap();
        // Longer than the accept loop's 100 ms sleep, so the handler thread is
        // already blocked on its first read before a single byte is written.
        thread::sleep(time::Duration::from_millis(300));
        let req = Request {
            jsonrpc: "2.0".to_string(),
            method: "getinfo".to_string(),
            params: None,
            id: ReqId::Num(1),
        };
        client
            .write_all(&serde_json::to_vec(&req).unwrap())
            .unwrap();
        client.write_all(b"\n").unwrap();
        client.flush().unwrap();

        client
            .set_read_timeout(Some(time::Duration::from_secs(10)))
            .unwrap();
        let mut response = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let read = client
                .read(&mut chunk)
                .expect("the daemon closed the connection with the request unread (#478)");
            assert!(read > 0, "connection closed before a response (#478)");
            response.extend_from_slice(&chunk[..read]);
            if serde_json::from_slice::<serde_json::Value>(&response).is_ok() {
                break;
            }
        }
        let decoded: serde_json::Value = serde_json::from_slice(&response).unwrap();
        assert_eq!(decoded["id"], 1, "response: {}", decoded);
        assert!(
            decoded.get("result").is_some(),
            "expected a getinfo result, got: {}",
            decoded
        );

        shutdown.store(true, atomic::Ordering::Relaxed);
        drop(client);
        // Unblock the accept loop so the thread observes the shutdown flag.
        let _ = net::UnixStream::connect(&socket_path);
        let _ = server.join();
        let _ = fs::remove_file(&socket_path);
        ms.shutdown();
    }

    fn write_messages(socket_path: &path::Path, messages: &[&[u8]]) {
        let mut client = net::UnixStream::connect(socket_path).unwrap();
        for mess in messages {
            client.write_all(mess).unwrap();
            // Simulate throttling, this mimics real conditions and actually triggered a crash.
            thread::sleep(time::Duration::from_millis(50));
        }
    }

    #[test]
    fn command_read_single() {
        let socket_path = env::temp_dir().join(format!(
            "coincubed-jsonrpc-socket-{}-{:?}",
            process::id(),
            thread::current().id()
        ));

        // A simple dummy request
        let t = read_all_commands(&socket_path);
        let req = br#"{"jsonrpc": "2.0", "id": 0, "method": "test", "params": {"a": "b"}}"#;
        let parsed_req: Request = serde_json::from_slice(req).unwrap();
        write_messages(&socket_path, &[req, b"\n"]);
        let read_req = t.join().unwrap();
        assert_eq!(parsed_req, read_req[0]);

        // Same, but with params as a list and a string id
        let t = read_one_command(&socket_path);
        let req = br#"{"jsonrpc": "2.0", "id": "987-abc", "method": "test", "params": ["a", 10]}"#;
        let parsed_req: Request = serde_json::from_slice(req).unwrap();
        write_messages(&socket_path, &[req, b"\n"]);
        let read_req = t.join().unwrap().unwrap();
        assert_eq!(parsed_req, read_req);

        fs::remove_file(&socket_path).unwrap();
    }

    #[test]
    fn command_read_parts() {
        let socket_path = env::temp_dir().join(format!(
            "coincubed-jsonrpc-socket-{}-{:?}",
            process::id(),
            thread::current().id()
        ));

        // A single request written in two parts
        let t = read_one_command(&socket_path);
        let req = br#"{"jsonrpc": "2.0", "id": 0, "method": "test", "params": ["a", 10]}"#;
        let parsed_req: Request = serde_json::from_slice(req).unwrap();
        write_messages(
            &socket_path,
            &[&req[..req.len() / 2], &req[req.len() / 2..], b"\n"],
        );
        let read_req = t.join().unwrap().unwrap();
        assert_eq!(parsed_req, read_req);

        // A single request written in many parts
        let t = read_one_command(&socket_path);
        let req = br#"{"jsonrpc": "2.0", "id": 0, "method": "test", "params": ["a", 10]}"#;
        let parsed_req: Request = serde_json::from_slice(req).unwrap();
        let tmp: Vec<Vec<u8>> = req.iter().map(|c| vec![*c]).collect();
        let mut to_send: Vec<&[u8]> = tmp.iter().map(|v| v.as_slice()).collect();
        to_send.push(b"\n");
        write_messages(&socket_path, &to_send);
        let read_req = t.join().unwrap().unwrap();
        assert_eq!(parsed_req, read_req);

        fs::remove_file(&socket_path).unwrap();
    }

    #[test]
    fn command_read_multiple() {
        let socket_path = env::temp_dir().join(format!(
            "coincubed-jsonrpc-socket-{}-{:?}",
            process::id(),
            thread::current().id()
        ));

        // Multiple requests, in parts
        let t = read_all_commands(&socket_path);
        let reqs = [
            &br#"{"jsonrpc": "2.0", "id": 20478, "me"#[..],
            br#"thod": "test", "params": ["a", 10]}"#,
            b"\n",
            br#"{"jsonrpc": "2.0", "id": 20479, "method": "testADZ", "params": {}}"#,
            b"\n",
            br#"{"jsonrpc": "2.0", "id": 20499, "method": "t"#,
            br#"e_edzA", "params": {"ttt": 980}}"#,
            b"\n",
        ];
        let parsed_reqs: Vec<Request> = vec![
            serde_json::from_slice(&[reqs[0], reqs[1]].concat()).unwrap(),
            serde_json::from_slice(reqs[3]).unwrap(),
            serde_json::from_slice(&[reqs[5], reqs[6]].concat()).unwrap(),
        ];
        write_messages(&socket_path, &reqs);
        let read_reqs = t.join().unwrap();
        assert_eq!(parsed_reqs, read_reqs);

        // The same requests, sent at once.
        let t = read_all_commands(&socket_path);
        let req_parts = [
            &br#"{"jsonrpc": "2.0", "id": 20478, "method": "test", "params": ["a", 10]}"#[..],
            b"\n",
            br#"{"jsonrpc": "2.0", "id": 20479, "method": "testADZ", "params": {}}"#,
            b"\n",
            br#"{"jsonrpc": "2.0", "id": 20499, "method": "te_edzA", "params": {"ttt": 980}}"#,
            b"\n",
        ]
        .concat();
        write_messages(&socket_path, &[req_parts.as_slice()]);
        let read_reqs = t.join().unwrap();
        assert_eq!(parsed_reqs, read_reqs);

        fs::remove_file(&socket_path).unwrap();
    }

    #[test]
    fn command_read_linebreak() {
        let socket_path = env::temp_dir().join(format!(
            "coincubed-jsonrpc-socket-{}-{:?}",
            process::id(),
            thread::current().id()
        ));

        // Multiple requests, in parts
        let t = read_one_command(&socket_path);
        let mut params = serde_json::map::Map::new();
        params.insert(
            "dummy param".to_string(),
            "dummy value
        with line
        breaks"
                .to_string()
                .into(),
        );
        let req = Request {
            jsonrpc: "2.0".to_string(),
            method: "dummy".to_string(),
            params: Some(Params::Map(params)),
            id: ReqId::Num(0),
        };
        write_messages(&socket_path, &[&serde_json::to_vec(&req).unwrap(), b"\n"]);
        let read_req = t.join().unwrap().unwrap();
        assert_eq!(req, read_req);

        fs::remove_file(&socket_path).unwrap();
    }

    // Accepted sockets are blocking on every Unix platform (#490), and the
    // hashed socket path plus deadline below prevent the historical wait hang.
    #[test]
    fn server_sanity_check() {
        let ms = crate::testutils::DummyCoincube::new_server(
            crate::testutils::DummyBitcoind::new(),
            crate::testutils::DummyDatabase::new(),
        );
        // Derive the socket path exactly as the daemon does. The server binds a
        // random `$TMPDIR/cc-<hex>.sock` and publishes it in `rpc.socket`.
        // `coincubed_rpc_socket_path` returns that path only while something is
        // listening; before the bind it falls back to the legacy hash, which
        // is not created, so this loop keeps waiting. Hard-coding
        // `<datadir>/coincubed_rpc` meant waiting on a file that is never
        // created — and an unbounded loop then hung CI until the 6-hour job
        // timeout.
        let data_directory: path::PathBuf = [
            ms.tmp_dir.as_path(),
            path::Path::new("d"),
            path::Path::new("bitcoin"),
        ]
        .iter()
        .collect();
        // The daemon publishes a random socket name under the datadir. Re-read
        // it until the socket exists; the legacy hash is only a fallback when
        // that record has not been written yet.
        let data_dir = crate::datadir::DataDirectory::new(data_directory);
        let deadline = time::Instant::now() + time::Duration::from_secs(30);
        let socket_path = loop {
            let candidate = data_dir.coincubed_rpc_socket_path();
            if candidate.exists()
                && data_dir
                    .path()
                    .join(crate::datadir::RPC_SOCKET_RECORD)
                    .is_file()
            {
                break candidate;
            }
            assert!(time::Instant::now() < deadline, "RPC socket never appeared");
            thread::sleep(time::Duration::from_millis(100));
        };

        let stop_req = Request {
            jsonrpc: "2.0".to_string(),
            method: "stop".to_string(),
            params: None,
            id: ReqId::Num(0),
        };
        write_messages(
            &socket_path,
            &[&serde_json::to_vec(&stop_req).unwrap(), b"\n"],
        );

        ms.shutdown();
        // The socket lives in the system temp dir, not under `tmp_dir`, so
        // `shutdown()`'s `remove_dir_all` does not reach it.
        let _ = fs::remove_file(&socket_path);
    }
}
