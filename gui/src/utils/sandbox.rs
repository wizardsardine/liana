use std::sync::Arc;

use iced_native::command::Action;

use crate::{
    app::{cache::Cache, message::Message, state::State},
    daemon::Daemon,
};

pub struct Sandbox<S: State> {
    state: S,
}

impl<S: State + Send + 'static> Sandbox<S> {
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
        for action in cmd.actions() {
            if let Action::Future(f) = action {
                let msg = f.await;
                let _cmd = self.state.update(daemon.clone(), cache, msg);
            }
        }

        self
    }

    pub async fn load(mut self, daemon: Arc<dyn Daemon + Sync + Send>, cache: &Cache) -> Self {
        let cmd = self.state.load(daemon.clone());
        for action in cmd.actions() {
            if let Action::Future(f) = action {
                let msg = f.await;
                self = self.update(daemon.clone(), cache, msg).await;
            }
        }

        self
    }
}
