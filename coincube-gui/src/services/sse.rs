use iced::futures::{SinkExt, Stream, StreamExt};
use iced::stream::channel;

/// A raw SSE event with event type and data
#[derive(Debug, Clone)]
pub struct SseEvent {
    pub event_type: Option<String>,
    pub data: Option<String>,
}

/// Events emitted by the SSE stream
#[derive(Debug, Clone)]
pub enum SseStreamEvent {
    Connected,
    Event(SseEvent),
    Error(String),
    Disconnected,
}

/// Configuration for connecting to an SSE endpoint
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SseConfig {
    pub url: String,
    pub headers: Vec<(String, String)>,
}

impl SseConfig {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            headers: Vec::new(),
        }
    }

    pub fn with_bearer_token(mut self, token: impl Into<String>) -> Self {
        self.headers
            .push(("Authorization".into(), format!("Bearer {}", token.into())));
        self
    }

    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((key.into(), value.into()));
        self
    }
}

/// Creates a generic SSE stream that connects to the specified endpoint.
/// Returns raw SSE events for the caller to parse.
pub fn sse_stream(config: SseConfig) -> impl Stream<Item = SseStreamEvent> {
    channel(10, async move |mut output| {
        let client = reqwest::Client::new();
        let mut request = client
            .get(&config.url)
            .header("Accept", "text/event-stream");

        for (key, value) in config.headers {
            request = request.header(key, value);
        }

        let response = match request.send().await {
            Ok(resp) => resp,
            Err(e) => {
                let _ = output.send(SseStreamEvent::Error(e.to_string())).await;
                return;
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            let _ = output
                .send(SseStreamEvent::Error(format!("{}: {}", status, text)))
                .await;
            return;
        }

        let _ = output.send(SseStreamEvent::Connected).await;

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    let text = match std::str::from_utf8(&bytes) {
                        Ok(t) => t,
                        Err(_) => continue,
                    };

                    buffer.push_str(text);

                    // Process complete SSE events (separated by double newlines)
                    while let Some(pos) = buffer.find("\n\n") {
                        let event_text = buffer[..pos].to_string();
                        buffer = buffer[pos + 2..].to_string();

                        let event = parse_sse_event(&event_text);
                        let _ = output.send(SseStreamEvent::Event(event)).await;
                    }
                }
                Err(e) => {
                    let _ = output.send(SseStreamEvent::Error(e.to_string())).await;
                    let _ = output.send(SseStreamEvent::Disconnected).await;
                    return;
                }
            }
        }

        let _ = output.send(SseStreamEvent::Disconnected).await;
    })
}

fn parse_sse_event(event_text: &str) -> SseEvent {
    let mut event_type = None;
    let mut data = None;

    for line in event_text.lines() {
        if let Some(value) = line.strip_prefix("event:") {
            event_type = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("data:") {
            data = Some(value.trim().to_string());
        }
    }

    SseEvent { event_type, data }
}
