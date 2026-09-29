// This is based on https://github.com/iced-rs/iced/blob/master/examples/download_progress/src/download.rs
// with some modifications to store the downloaded bytes in `Progress::Finished` and `State::Downloading`
// and to keep track of any download errors.
use iced::futures::{SinkExt, Stream, StreamExt};
use iced::stream::try_channel;
use tokio::time::{error::Elapsed, timeout};

use std::{hash::Hash, sync::Arc, time::Duration};

use crate::t;

/// A dropped connection may never be closed, so a download fails once no data arrives for this
/// long.
const STALL_TIMEOUT: Duration = Duration::from_secs(10);

// Just a little utility function
pub fn file<I: 'static + Hash + Copy + Send + Sync, T: ToString>(
    id: I,
    url: T,
) -> iced::Subscription<(I, Result<Progress, DownloadError>)> {
    crate::utils::subscription::run_with_id(
        id,
        download(url.to_string()).map(move |progress| (id, progress)),
    )
}

fn download(url: String) -> impl Stream<Item = Result<Progress, DownloadError>> {
    try_channel(
        100,
        move |mut output: iced::futures::channel::mpsc::Sender<Progress>| async move {
            let response = timeout(STALL_TIMEOUT, reqwest::get(&url)).await??;
            let total = response.content_length();

            let _ = output.send(Progress::Downloading(0.0)).await;

            let mut byte_stream = response.bytes_stream();
            let mut downloaded = 0;
            let mut bytes = Vec::new();

            while let Some(next_bytes) = timeout(STALL_TIMEOUT, byte_stream.next()).await? {
                let chunk = next_bytes?;
                downloaded += chunk.len();
                bytes.append(&mut chunk.to_vec());

                if let Some(total) = total {
                    let _ = output
                        .send(Progress::Downloading(
                            100.0 * downloaded as f32 / total as f32,
                        ))
                        .await;
                }
            }

            let _ = output.send(Progress::Finished(bytes)).await;

            Ok(())
        },
    )
}

#[derive(Debug, Clone)]
pub enum Progress {
    Downloading(f32),
    Finished(Vec<u8>),
}

#[derive(Debug, Clone)]
pub enum DownloadError {
    RequestFailed(Arc<reqwest::Error>),
    NoContentLength,
    Stalled,
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::NoContentLength => {
                write!(f, "Response has unknown content length.")
            }
            Self::RequestFailed(e) => {
                write!(f, "Request error: '{e}'.")
            }
            Self::Stalled => {
                write!(f, "{}", t!("download-stalled"))
            }
        }
    }
}

impl From<reqwest::Error> for DownloadError {
    fn from(error: reqwest::Error) -> Self {
        DownloadError::RequestFailed(Arc::new(error))
    }
}

impl From<Elapsed> for DownloadError {
    fn from(_: Elapsed) -> Self {
        DownloadError::Stalled
    }
}
