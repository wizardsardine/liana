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
    bitcoind::Bitcoind,
    daemon::{embedded::EmbeddedDaemon, Daemon},
};

use self::state::SettingsState;

struct Panels {
    current: Menu,
    home: Home,
    coins: CoinsPanel,
    transactions: TransactionsPanel,
    psbts: PsbtsPanel,
    recovery: RecoveryPanel,
    receive: ReceivePanel,
    create_spend: CreateSpendPanel,
    settings: SettingsState,
}

impl Panels {
    fn new(
        cache: &Cache,
        wallet: Arc<Wallet>,
        data_dir: PathBuf,
        internal_bitcoind: Option<&Bitcoind>,
    ) -> Panels {
        Self {
            current: Menu::Home,
            home: Home::new(wallet.clone(), &cache.coins),
            coins: CoinsPanel::new(&cache.coins, wallet.main_descriptor.first_timelock_value()),
            transactions: TransactionsPanel::new(),
            psbts: PsbtsPanel::new(wallet.clone(), &cache.spend_txs),
            recovery: RecoveryPanel::new(wallet.clone(), &cache.coins, cache.blockheight),
            receive: ReceivePanel::new(data_dir.clone(), wallet.clone()),
            create_spend: CreateSpendPanel::new(
                wallet.clone(),
                &cache.coins,
                cache.blockheight as u32,
                cache.network,
            ),
            settings: state::SettingsState::new(
                data_dir.clone(),
                wallet.clone(),
                internal_bitcoind.is_some(),
            ),
        }
    }

    fn current(&self) -> &dyn State {
        match self.current {
            Menu::Home => &self.home,
            Menu::Receive => &self.receive,
            Menu::PSBTs => &self.psbts,
            Menu::Transactions => &self.transactions,
            Menu::Settings => &self.settings,
            Menu::Coins => &self.coins,
            Menu::CreateSpendTx => &self.create_spend,
            Menu::Recovery => &self.recovery,
            Menu::RefreshCoins(_) => &self.create_spend,
            Menu::PsbtPreSelected(_) => &self.psbts,
        }
    }

    fn current_mut(&mut self) -> &mut dyn State {
        match self.current {
            Menu::Home => &mut self.home,
            Menu::Receive => &mut self.receive,
            Menu::PSBTs => &mut self.psbts,
            Menu::Transactions => &mut self.transactions,
            Menu::Settings => &mut self.settings,
            Menu::Coins => &mut self.coins,
            Menu::CreateSpendTx => &mut self.create_spend,
            Menu::Recovery => &mut self.recovery,
            Menu::RefreshCoins(_) => &mut self.create_spend,
            Menu::PsbtPreSelected(_) => &mut self.psbts,
        }
    }
}

pub struct App {
    data_dir: PathBuf,
    cache: Cache,
    config: Config,
    wallet: Arc<Wallet>,
    daemon: Arc<dyn Daemon + Sync + Send>,
    internal_bitcoind: Option<Bitcoind>,

    panels: Panels,
}

impl App {
    pub fn new(
        cache: Cache,
        wallet: Arc<Wallet>,
        config: Config,
        daemon: Arc<dyn Daemon + Sync + Send>,
        data_dir: PathBuf,
        internal_bitcoind: Option<Bitcoind>,
    ) -> (App, Command<Message>) {
        let panels = Panels::new(
            &cache,
            wallet.clone(),
            data_dir.clone(),
            internal_bitcoind.as_ref(),
        );
        let cmd = panels.home.load(daemon.clone());
        (
            Self {
                panels,
                data_dir,
                cache,
                config,
                daemon,
                wallet,
                internal_bitcoind,
            },
            cmd,
        )
    }

    fn set_current_panel(&mut self, menu: Menu) -> Command<Message> {
        match &menu {
            menu::Menu::PsbtPreSelected(txid) => {
                // Get preselected spend from DB in case it's not yet in the cache.
                // We only need this single spend as we will go straight to its view and not show the PSBTs list.
                // In case of any error loading the spend or if it doesn't exist, fall back to using the cache
                // and load PSBTs list in usual way.
                self.panels.psbts = match self
                    .daemon
                    .list_spend_transactions(Some(&[*txid]))
                    .map(|txs| txs.first().cloned())
                {
                    Ok(Some(spend_tx)) => {
                        PsbtsPanel::new_preselected(self.wallet.clone(), spend_tx).into()
                    }
                    _ => PsbtsPanel::new(self.wallet.clone(), &self.cache.spend_txs).into(),
                };
            }
            menu::Menu::CreateSpendTx => {
                self.panels.create_spend = CreateSpendPanel::new(
                    self.wallet.clone(),
                    &self.cache.coins,
                    self.cache.blockheight as u32,
                    self.cache.network,
                )
                .into();
            }
            menu::Menu::RefreshCoins(preselected) => {
                self.panels.create_spend = CreateSpendPanel::new_self_send(
                    self.wallet.clone(),
                    &self.cache.coins,
                    self.cache.blockheight as u32,
                    preselected,
                    self.cache.network,
                )
                .into();
            }
            _ => {}
        };
        self.panels.current = menu;
        self.panels.current().load(self.daemon.clone())
    }

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch(vec![
            time::every(Duration::from_secs(5)).map(|_| Message::Tick),
            self.panels.current().subscription(),
        ])
    }

    pub fn stop(&mut self) {
        info!("Close requested");
        if !self.daemon.is_external() {
            self.daemon.stop();
            info!("Internal daemon stopped");
            if let Some(bitcoind) = &self.internal_bitcoind {
                bitcoind.stop();
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
            Message::View(view::Message::Menu(menu)) => self.set_current_panel(menu),
            Message::View(view::Message::Clipboard(text)) => clipboard::write(text),
            _ => self
                .panels
                .current_mut()
                .update(self.daemon.clone(), &self.cache, message),
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

        let content =
            toml::to_string(&self.daemon.config()).map_err(|e| Error::Config(e.to_string()))?;

        OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(daemon_config_path)
            .map_err(|e| Error::Config(e.to_string()))?
            .write_all(content.as_bytes())
            .map_err(|e| {
                warn!("failed to write to file: {:?}", e);
                Error::Config(e.to_string())
            })
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
        self.panels.current().view(&self.cache).map(Message::View)
    }
}
