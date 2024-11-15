// This is based on https://github.com/iced-rs/iced/blob/master/examples/download_progress/src/download.rs
// with some modifications to store the downloaded bytes in `Progress::Finished` and `State::Downloading`
// and to keep track of any download errors.
use iced::subscription;

use std::hash::Hash;

// Just a little utility function
pub fn file<I: 'static + Hash + Copy + Send + Sync, T: ToString>(
    id: I,
    url: T,
) -> iced::Subscription<(I, Progress)> {
    subscription::unfold(id, State::Ready(url.to_string()), move |state| {
        download(id, state)
    })
}

#[derive(Debug, Hash, Clone)]
pub struct Download<I> {
    id: I,
    url: String,
}

/// Possible errors with download.
#[derive(PartialEq, Eq, Debug, Clone)]
pub enum DownloadError {
    UnknownContentLength,
    RequestError(String),
}

impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::UnknownContentLength => {
                write!(f, "Response has unknown content length.")
            }
            Self::RequestError(e) => {
                write!(f, "Request error: '{}'.", e)
            }
        }
    }
}

async fn download<I: Copy>(id: I, state: State) -> ((I, Progress), State) {
    match state {
        State::Ready(url) => {
            let response = reqwest::get(&url).await;

            match response {
                Ok(response) => {
                    if let Some(total) = response.content_length() {
                        (
                            (id, Progress::Started),
                            State::Downloading {
                                response,
                                total,
                                downloaded: 0,
                                bytes: Vec::new(),
                            },
                        )
                    } else {
                        (
                            (id, Progress::Errored(DownloadError::UnknownContentLength)),
                            State::Finished,
                        )
                    }
                }
                Err(e) => (
                    (
                        id,
                        Progress::Errored(DownloadError::RequestError(e.to_string())),
                    ),
                    State::Finished,
                ),
            }
        }
        State::Downloading {
            mut response,
            total,
            downloaded,
            mut bytes,
        } => match response.chunk().await {
            Ok(Some(chunk)) => {
                let downloaded = downloaded + chunk.len() as u64;

                let percentage = (downloaded as f32 / total as f32) * 100.0;

                bytes.append(&mut chunk.to_vec());

                (
                    (id, Progress::Advanced(percentage)),
                    State::Downloading {
                        response,
                        total,
                        downloaded,
                        bytes,
                    },
                )
            }
            Ok(None) => ((id, Progress::Finished(bytes)), State::Finished),
            Err(e) => (
                (
                    id,
                    Progress::Errored(DownloadError::RequestError(e.to_string())),
                ),
                State::Finished,
            ),
        },
        State::Finished => {
            // We do not let the stream die, as it would start a
            // new download repeatedly if the user is not careful
            // in case of errors.
            iced::futures::future::pending().await
        }
    }
}

#[derive(Debug, Clone)]
pub enum Progress {
    Started,
    Advanced(f32),
    Finished(Vec<u8>),
    Errored(DownloadError),
}

pub enum State {
    Ready(String),
    Downloading {
        response: reqwest::Response,
        total: u64,
        downloaded: u64,
        bytes: Vec<u8>,
    },
    Finished,
}
