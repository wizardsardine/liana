use iced::futures::StreamExt;
use std::sync::Arc;

use iced_runtime::{task::into_stream, Action};

use crate::{
    app::{cache::Cache, message::Message, state::State, wallet::Wallet},
    daemon::Daemon,
};

pub struct Sandbox<S: State> {
    state: S,
}

impl<S: State + 'static> Sandbox<S> {
    pub fn new(state: S) -> Self {
        Self { state }
    }

    pub fn state(&self) -> &S {
        &self.state
    }

    pub async fn update(
        mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Self {
        let cmd = self.state.update(daemon.clone(), cache, message);
        if let Some(mut stream) = into_stream(cmd) {
            while let Some(action) = stream.next().await {
                if let Action::Output(msg) = action {
                    let _cmd = self.state.update(daemon.clone(), cache, msg);
                }
            }
        }
        self
    }

    pub async fn load(
        mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        wallet: Arc<Wallet>,
    ) -> Self {
        let cmd = self.state.reload(daemon.clone(), wallet);
        if let Some(mut stream) = into_stream(cmd) {
            while let Some(action) = stream.next().await {
                if let Action::Output(msg) = action {
                    let _cmd = self.state.update(daemon.clone(), cache, msg);
                }
            }
        }

        self
    }
}
