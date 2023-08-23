pub mod cache;
pub mod config;
pub mod menu;
pub mod message;
pub mod settings;
pub mod state;
pub mod view;
pub mod wallet;

mod error;

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use iced::{clipboard, time, Command, Subscription};
use tracing::{info, warn};

pub use liana::{config::Config as DaemonConfig, miniscript::bitcoin};
use liana_ui::widget::Element;

pub use config::Config;
pub use message::Message;

use state::{
    CoinsPanel, CreateSpendPanel, Home, PsbtsPanel, ReceivePanel, RecoveryPanel, State,
    TransactionsPanel,
};

use crate::{
    app::{cache::Cache, error::Error, menu::Menu, wallet::Wallet},
    bitcoind::stop_internal_bitcoind,
    daemon::{embedded::EmbeddedDaemon, Daemon},
};

pub struct App {
    data_dir: PathBuf,
    state: Box<dyn State>,
    cache: Cache,
    config: Config,
    wallet: Arc<Wallet>,
    daemon: Arc<dyn Daemon + Sync + Send>,
}

impl App {
    pub fn new(
        cache: Cache,
        wallet: Arc<Wallet>,
        config: Config,
        daemon: Arc<dyn Daemon + Sync + Send>,
        data_dir: PathBuf,
    ) -> (App, Command<Message>) {
        let state: Box<dyn State> = Home::new(wallet.clone(), &cache.coins).into();
        let cmd = state.load(daemon.clone());
        (
            Self {
                data_dir,
                state,
                cache,
                config,
                daemon,
                wallet,
            },
            cmd,
        )
    }

    fn load_state(&mut self, menu: &Menu) -> Command<Message> {
        self.state = match menu {
            menu::Menu::Settings => {
                state::SettingsState::new(self.data_dir.clone(), self.wallet.clone()).into()
            }
            menu::Menu::Home => Home::new(self.wallet.clone(), &self.cache.coins).into(),
            menu::Menu::Coins => CoinsPanel::new(
                &self.cache.coins,
                self.wallet.main_descriptor.first_timelock_value(),
            )
            .into(),
            menu::Menu::Recovery => RecoveryPanel::new(
                self.wallet.clone(),
                &self.cache.coins,
                self.cache.blockheight,
            )
            .into(),
            menu::Menu::Receive => ReceivePanel::default().into(),
            menu::Menu::Transactions => TransactionsPanel::new().into(),
            menu::Menu::PSBTs => PsbtsPanel::new(self.wallet.clone(), &self.cache.spend_txs).into(),
            menu::Menu::CreateSpendTx => CreateSpendPanel::new(
                self.wallet.clone(),
                &self.cache.coins,
                self.cache.blockheight as u32,
            )
            .into(),
            menu::Menu::RefreshCoins(preselected) => CreateSpendPanel::new_self_send(
                self.wallet.clone(),
                &self.cache.coins,
                self.cache.blockheight as u32,
                preselected,
            )
            .into(),
        };
        self.state.load(self.daemon.clone())
    }

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch(vec![
            time::every(Duration::from_secs(5)).map(|_| Message::Tick),
            self.state.subscription(),
        ])
    }

    pub fn stop(&mut self) {
        info!("Close requested");
        if !self.daemon.is_external() {
            self.daemon.stop();
            info!("Internal daemon stopped");
            if self.config.internal_bitcoind_exe_config.is_some() {
                if let Some(daemon_config) = self.daemon.config() {
                    if let Some(bitcoind_config) = &daemon_config.bitcoind_config {
                        stop_internal_bitcoind(bitcoind_config);
                    }
                }
            }
        }
    }

    pub fn update(&mut self, message: Message) -> Command<Message> {
        // Update cache when values are passing by.
        // State will handle the error case.
        match &message {
            Message::Coins(Ok(coins)) => {
                self.cache.coins = coins.clone();
            }
            Message::SpendTxs(Ok(txs)) => {
                self.cache.spend_txs = txs.clone();
            }
            Message::Info(Ok(info)) => {
                self.cache.blockheight = info.block_height;
                self.cache.rescan_progress = info.rescan_progress;
            }
            Message::StartRescan(Ok(())) => {
                self.cache.rescan_progress = Some(0.0);
            }
            _ => {}
        };

        match message {
            Message::Tick => {
                let daemon = self.daemon.clone();
                Command::perform(
                    async move { daemon.get_info().map_err(|e| e.into()) },
                    Message::Info,
                )
            }
            Message::LoadDaemonConfig(cfg) => {
                let path = self.config.daemon_config_path.clone().expect(
                    "Application config must have a daemon configuration file path at this point.",
                );
                let res = self.load_daemon_config(&path, *cfg);
                self.update(Message::DaemonConfigLoaded(res))
            }
            Message::LoadWallet => {
                let res = self.load_wallet();
                self.update(Message::WalletLoaded(res))
            }
            Message::View(view::Message::Menu(menu)) => self.load_state(&menu),
            Message::View(view::Message::Clipboard(text)) => clipboard::write(text),
            _ => self.state.update(self.daemon.clone(), &self.cache, message),
        }
    }

    pub fn load_daemon_config(
        &mut self,
        daemon_config_path: &PathBuf,
        cfg: DaemonConfig,
    ) -> Result<(), Error> {
        self.daemon.stop();
        let daemon = EmbeddedDaemon::start(cfg)?;
        self.daemon = Arc::new(daemon);

        let mut daemon_config_file = OpenOptions::new()
            .write(true)
            .open(daemon_config_path)
            .map_err(|e| Error::Config(e.to_string()))?;

        let content =
            toml::to_string(&self.daemon.config()).map_err(|e| Error::Config(e.to_string()))?;

        daemon_config_file
            .write_all(content.as_bytes())
            .map_err(|e| {
                warn!("failed to write to file: {:?}", e);
                Error::Config(e.to_string())
            })?;

        Ok(())
    }

    pub fn load_wallet(&mut self) -> Result<Arc<Wallet>, Error> {
        let wallet = Wallet::new(self.wallet.main_descriptor.clone()).load_settings(
            &self.config,
            &self.data_dir,
            self.cache.network,
        )?;

        self.wallet = Arc::new(wallet);

        Ok(self.wallet.clone())
    }

    pub fn view(&self) -> Element<Message> {
        self.state.view(&self.cache).map(Message::View)
    }
}
