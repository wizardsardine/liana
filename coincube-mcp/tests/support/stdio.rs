// Shared by companion integration tests and the desktop's native acceptance tests.
// Never compiled into a shipping executable.
use serde_json::Value;
use std::{
    ffi::OsStr,
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
// Native IPC has its own ten-second deadline. Leave time to report its error.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const FRAME_LIMIT: u64 = 65_536;
const UNAVAILABLE: &str =
    "Tenshu MCP unavailable. Check the desktop grant and private client configuration.";

#[derive(Default)]
struct Diagnostics {
    bytes: usize,
    unavailable: bool,
}

pub struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    diagnostics: Arc<Mutex<Diagnostics>>,
    stderr_task: tokio::task::JoinHandle<()>,
    next_id: u64,
}

impl Drop for Client {
    fn drop(&mut self) {
        self.stderr_task.abort();
        // Child::kill_on_drop also covers assertions in the acceptance caller.
    }
}

impl Client {
    pub async fn start(executable: &OsStr, environment: &[(String, String)]) -> Self {
        let mut child = Command::new(executable)
            .env_remove("COINCUBE_MCP_SOCKET")
            .env_remove("COINCUBE_MCP_PAIRING")
            .env_remove("COINCUBE_MCP_CREDENTIAL")
            .envs(environment.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn MCP test companion");
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut stderr = child.stderr.take().unwrap();
        let diagnostics = Arc::new(Mutex::new(Diagnostics::default()));
        let observed = diagnostics.clone();
        // Drain continuously, but retain only a byte count and a known fixed
        // diagnostic. Never print arbitrary stderr, credentials or tool data.
        let stderr_task = tokio::spawn(async move {
            let mut buffer = [0; 1024];
            let mut prefix = Vec::new();
            while let Ok(count) = stderr.read(&mut buffer).await {
                if count == 0 {
                    break;
                }
                prefix
                    .extend_from_slice(&buffer[..count.min(256usize.saturating_sub(prefix.len()))]);
                let mut state = observed.lock().unwrap();
                state.bytes = state.bytes.saturating_add(count);
                state.unavailable = prefix
                    .windows(UNAVAILABLE.len())
                    .any(|w| w == UNAVAILABLE.as_bytes());
            }
        });
        let mut client = Self {
            child,
            stdin,
            stdout,
            diagnostics,
            stderr_task,
            next_id: 1,
        };
        let initialized = client
            .exchange(
                "initialize",
                serde_json::json!({
                    "protocolVersion":"2025-11-25", "capabilities":{},
                    "clientInfo":{"name":"native-acceptance", "version":"1"}
                }),
                STARTUP_TIMEOUT,
            )
            .await;
        assert_eq!(initialized["result"]["serverInfo"]["name"], "coincube-mcp");
        client
            .write(
                b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
                "initialized",
                0,
            )
            .await;
        client
    }

    async fn write(&mut self, bytes: &[u8], phase: &str, id: u64) {
        assert!(
            bytes.len() <= 8192,
            "test request exceeds companion frame bound"
        );
        let outcome = tokio::time::timeout(
            REQUEST_TIMEOUT,
            self.stdin.as_mut().unwrap().write_all(bytes),
        )
        .await;
        if !matches!(outcome, Ok(Ok(()))) {
            self.fail(phase, id, "write failed or timed out").await;
        }
    }

    pub async fn exchange(
        &mut self,
        method: &'static str,
        params: Value,
        deadline: Duration,
    ) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let started = std::time::Instant::now();
        let mut bytes = serde_json::to_vec(
            &serde_json::json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}),
        )
        .unwrap();
        bytes.push(b'\n');
        self.write(&bytes, method, id).await;
        let result = read_response(&mut self.stdout, id, deadline).await;
        if std::env::var_os("COINCUBE_MCP_TEST_TRACE").is_some() {
            eprintln!(
                "MCP test phase={method} id={id} elapsed_ms={}",
                started.elapsed().as_millis()
            );
        }
        match result {
            Ok(value) => value,
            Err(reason) => self.fail(method, id, reason).await,
        }
    }

    pub async fn request(&mut self, method: &'static str, params: Value) -> Value {
        self.exchange(method, params, REQUEST_TIMEOUT).await
    }

    pub async fn call(&mut self, name: &str, arguments: Value) -> Value {
        let response = self
            .request(
                "tools/call",
                serde_json::json!({"name":name,"arguments":arguments}),
            )
            .await;
        assert!(
            response.get("error").is_none(),
            "unexpected MCP protocol error"
        );
        response["result"].clone()
    }

    async fn fail(&mut self, phase: &str, id: u64, reason: &str) -> ! {
        let status = self.child.try_wait().ok().flatten();
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await;
        let _ = tokio::time::timeout(Duration::from_secs(1), &mut self.stderr_task).await;
        let state = self.diagnostics.lock().unwrap();
        panic!(
            "MCP test phase={phase} id={id}: {reason}; child_status={status:?}; stderr_bytes={}; known_unavailable_diagnostic={}",
            state.bytes, state.unavailable
        );
    }

    pub async fn close(mut self) {
        drop(self.stdin.take());
        let status = match tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await {
            Ok(Ok(status)) => status,
            _ => {
                self.fail("shutdown", self.next_id, "child did not exit after EOF")
                    .await
            }
        };
        assert!(status.success(), "MCP companion failed on EOF");
        tokio::time::timeout(Duration::from_secs(1), &mut self.stderr_task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            self.diagnostics.lock().unwrap().bytes,
            0,
            "unexpected companion stderr (contents withheld)"
        );
        let mut trailing = [0; 1];
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), self.stdout.read(&mut trailing))
                .await
                .unwrap()
                .unwrap(),
            0,
            "unexpected trailing protocol output"
        );
    }
}

async fn read_response<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    id: u64,
    deadline: Duration,
) -> Result<Value, &'static str> {
    let mut bytes = Vec::new();
    let count = tokio::time::timeout(
        deadline,
        reader.take(FRAME_LIMIT + 1).read_until(b'\n', &mut bytes),
    )
    .await
    .map_err(|_| "response timeout")?
    .map_err(|_| "response read failed")?;
    if count == 0 {
        return Err("unexpected EOF");
    }
    if count as u64 > FRAME_LIMIT || !bytes.ends_with(b"\n") {
        return Err("oversized or incomplete frame");
    }
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| "invalid JSON frame (contents withheld)")?;
    if value["id"] != id || value["jsonrpc"] != "2.0" {
        return Err("unexpected response identity");
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn response_deadline_and_eof_remain_bounded() {
        let (reader, writer) = tokio::io::duplex(32);
        let mut reader = BufReader::new(reader);
        assert_eq!(
            read_response(&mut reader, 1, Duration::from_millis(20))
                .await
                .unwrap_err(),
            "response timeout"
        );
        drop(writer);
        assert_eq!(
            read_response(&mut reader, 1, Duration::from_secs(1))
                .await
                .unwrap_err(),
            "unexpected EOF"
        );
    }

    #[tokio::test]
    async fn response_frames_are_bounded_and_diagnostics_do_not_echo_data() {
        for (bytes, reason) in [
            (
                b"private-data\n".to_vec(),
                "invalid JSON frame (contents withheld)",
            ),
            (
                vec![b'x'; FRAME_LIMIT as usize + 1],
                "oversized or incomplete frame",
            ),
            (
                b"{\"jsonrpc\":\"2.0\",\"id\":2}\n".to_vec(),
                "unexpected response identity",
            ),
        ] {
            assert_eq!(
                read_response(&mut bytes.as_slice(), 1, Duration::from_secs(1))
                    .await
                    .unwrap_err(),
                reason
            );
        }
    }
}
