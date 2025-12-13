// This is based on https://github.com/iced-rs/iced/blob/master/examples/download_progress/src/download.rs
// with some modifications to return the downloaded bytes upon completion
use iced::futures::StreamExt;
use iced::task::{sipper, Straw};

use std::sync::Arc;

/// Downloads a file from the given URL, reporting progress.
/// Returns a Straw that yields Progress updates and completes with the downloaded bytes.
pub fn download(url: impl AsRef<str>) -> impl Straw<Vec<u8>, Progress, DownloadError> {
    let url = url.as_ref().to_string();
    sipper(async move |mut progress| {
        let response = reqwest::get(&url).await?;
        let total = response.content_length();

        progress.send(Progress::downloading(0.0)).await;

        let mut byte_stream = response.bytes_stream();
        let mut downloaded = 0;
        let mut bytes = Vec::new();

        while let Some(next_bytes) = byte_stream.next().await {
            let chunk = next_bytes?;
            downloaded += chunk.len();
            bytes.append(&mut chunk.to_vec());

            if let Some(total) = total {
                progress
                    .send(Progress::downloading(
                        100.0 * downloaded as f32 / total as f32,
                    ))
                    .await;
            }
        }

        Ok(bytes)
    })
}

#[derive(Debug, Clone)]
pub struct Progress {
    pub percent: f32,
}

impl Progress {
    pub fn downloading(percent: f32) -> Self {
        Self { percent }
    }
}

#[derive(Debug, Clone)]
pub enum DownloadError {
    RequestFailed(Arc<reqwest::Error>),
    NoContentLength,
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::NoContentLength => {
                write!(f, "Response has unknown content length.")
            }
            Self::RequestFailed(e) => {
                write!(f, "Request error: '{}'.", e)
            }
        }
    }
}

impl From<reqwest::Error> for DownloadError {
    fn from(error: reqwest::Error) -> Self {
        DownloadError::RequestFailed(Arc::new(error))
    }
}
