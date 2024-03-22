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
use tracing::{error, info, warn};

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
            transactions: TransactionsPanel::new(wallet.clone()),
            psbts: PsbtsPanel::new(wallet.clone()),
            recovery: RecoveryPanel::new(wallet.clone(), &cache.coins, cache.blockheight),
            receive: ReceivePanel::new(data_dir.clone(), wallet.clone()),
            create_spend: CreateSpendPanel::new(
                wallet.clone(),
                &cache.coins,
                cache.blockheight as u32,
                cache.network,
            ),
            settings: state::SettingsState::new(
                data_dir,
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
            Menu::TransactionPreSelected(_) => &self.transactions,
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
            Menu::TransactionPreSelected(_) => &mut self.transactions,
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
        let mut panels = Panels::new(
            &cache,
            wallet.clone(),
            data_dir.clone(),
            internal_bitcoind.as_ref(),
        );
        let cmd = panels.home.reload(daemon.clone(), wallet.clone());
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
            menu::Menu::TransactionPreSelected(txid) => {
                if let Ok(Some(tx)) = self
                    .daemon
                    .get_history_txs(&[*txid])
                    .map(|txs| txs.first().cloned())
                {
                    self.panels.transactions.preselect(tx);
                    self.panels.current = menu;
                    return Command::none();
                };
            }
            menu::Menu::PsbtPreSelected(txid) => {
                // Get preselected spend from DB in case it's not yet in the cache.
                // We only need this single spend as we will go straight to its view and not show the PSBTs list.
                // In case of any error loading the spend or if it doesn't exist, load PSBTs list in usual way.
                if let Ok(Some(spend_tx)) = self
                    .daemon
                    .list_spend_transactions(Some(&[*txid]))
                    .map(|txs| txs.first().cloned())
                {
                    self.panels.psbts.preselect(spend_tx);
                    self.panels.current = menu;
                    return Command::none();
                };
            }
            menu::Menu::RefreshCoins(preselected) => {
                self.panels.create_spend = CreateSpendPanel::new_self_send(
                    self.wallet.clone(),
                    &self.cache.coins,
                    self.cache.blockheight as u32,
                    preselected,
                    self.cache.network,
                );
            }
            menu::Menu::CreateSpendTx => {
                // redo the process of spending only if user want to start a new one.
                if !self.panels.create_spend.is_first_step() {
                    self.panels.create_spend = CreateSpendPanel::new(
                        self.wallet.clone(),
                        &self.cache.coins,
                        self.cache.blockheight as u32,
                        self.cache.network,
                    );
                }
            }
            _ => {}
        };
        self.panels.current = menu;
        self.panels
            .current_mut()
            .reload(self.daemon.clone(), self.wallet.clone())
    }

    pub fn subscription(&self) -> Subscription<Message> {
        Subscription::batch(vec![
            time::every(Duration::from_secs(10)).map(|_| Message::Tick),
            self.panels.current().subscription(),
        ])
    }

    pub fn stop(&mut self) {
        info!("Close requested");
        if !self.daemon.is_external() {
            if let Err(e) = self.daemon.stop() {
                error!("{}", e);
            } else {
                info!("Internal daemon stopped");
            }
            if let Some(bitcoind) = &self.internal_bitcoind {
                bitcoind.stop();
            }
        }
    }

    pub fn update(&mut self, message: Message) -> Command<Message> {
        match message {
            Message::Tick => {
                let daemon = self.daemon.clone();
                let datadir_path = self.cache.datadir_path.clone();
                Command::perform(
                    async move {
                        // we check every 10 second if the daemon poller is alive
                        daemon.is_alive()?;

                        let info = daemon.get_info()?;
                        // todo: filter coins to only have current coins.
                        let coins = daemon.list_coins()?;
                        Ok(Cache {
                            datadir_path,
                            coins: coins.coins,
                            network: info.network,
                            blockheight: info.block_height,
                            rescan_progress: info.rescan_progress,
                        })
                    },
                    Message::UpdateCache,
                )
            }
            Message::UpdateCache(res) => {
                match res {
                    Ok(cache) => self.cache = cache,
                    Err(e) => tracing::error!("Failed to update cache: {}", e),
                }
                Command::none()
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
        self.daemon.stop()?;
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
