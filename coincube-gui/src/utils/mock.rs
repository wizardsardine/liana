use crate::daemon::{client::Client, DaemonError};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
use std::fmt::Debug;
use std::sync::{
    mpsc::{channel, Receiver, Sender},
    Mutex,
};
use std::thread;

type TransportReceiver = Receiver<Result<Value, DaemonError>>;

#[derive(Debug)]
pub struct DaemonClient {
    transport: Mutex<(Sender<Value>, TransportReceiver)>,
}

impl Client for DaemonClient {
    type Error = DaemonError;
    fn request<S: Serialize + Debug, D: DeserializeOwned + Debug>(
        &self,
        method: &str,
        params: Option<S>,
    ) -> Result<D, Self::Error> {
        let req = json!({"method": method, "params": params});
        let connection = self.transport.lock().expect("Failed to unlock");
        connection
            .0
            .send(req)
            .expect("Mock client failed to send request");
        connection
            .1
            .recv()
            .expect("Mock client failed to receive response")
            .map(|value| serde_json::from_value(value).unwrap())
    }
}

pub struct Daemon {
    requests: Vec<(Option<Value>, Result<Value, DaemonError>)>,
    standing: Vec<(String, Value)>,
}

impl Daemon {
    pub fn new(requests: Vec<(Option<Value>, Result<Value, DaemonError>)>) -> Self {
        Self {
            requests,
            standing: Vec::new(),
        }
    }

    /// Answer `method` from a standing response matched by name, however many
    /// times it arrives — including zero — instead of consuming an entry from
    /// the ordered `requests` queue.
    ///
    /// The queue is strictly positional, so it can only describe calls whose
    /// order the code under test actually fixes. A call a panel kicks off in a
    /// detached task (`tokio::spawn`) is not one of those: it lands whenever
    /// the scheduler gets to it, which may be after the next ordered call or
    /// after the test has finished. Register those here so they can't shift the
    /// queue out from under the calls that *are* ordered.
    pub fn with_standing_response(mut self, method: &str, response: Value) -> Self {
        self.standing.push((method.to_string(), response));
        self
    }

    pub fn run(self) -> DaemonClient {
        let (client_sender, daemon_receiver) = channel::<Value>();
        let (daemon_sender, client_receiver) = channel();

        thread::spawn(move || {
            let mut requests = self.requests.into_iter();
            let standing = self.standing;
            while let Ok(msg) = daemon_receiver.recv() {
                let method = msg.get("method").and_then(Value::as_str);
                if let Some((_, response)) = standing
                    .iter()
                    .find(|(name, _)| Some(name.as_str()) == method)
                {
                    daemon_sender
                        .send(Ok(response.clone()))
                        .expect("Mock daemon failed to send standing response");
                    continue;
                }
                let request = requests
                    .next()
                    .expect("Mock Daemon must have all requests mocked in the right order");
                if let Some(body) = request.0 {
                    assert_eq!(body, msg);
                }
                daemon_sender
                    .send(request.1)
                    .expect("Mock daemon failed to send response")
            }
            // close the daemon -> client channel after
            // the client -> daemon channel is closed.
            // (client -> daemon channel is closed when DaemonClient is dropped)
            drop(daemon_sender);
            // Readable with `cargo test -- --nocapture`
            println!("The daemon has stopped!");
        });

        DaemonClient {
            transport: Mutex::new((client_sender, client_receiver)),
        }
    }
}
