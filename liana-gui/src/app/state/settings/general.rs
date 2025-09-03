use std::collections::HashMap;
use std::sync::Arc;

use iced::Task;
use liana::miniscript::bitcoin::Network;
use liana_ui::widget::Element;

use crate::app::cache::Cache;
use crate::app::error::Error;
use crate::app::message::{FiatMessage, Message};
use crate::app::settings::fiat::PriceSetting;
use crate::app::settings::update_settings_file;
use crate::app::state::State;
use crate::app::view;
use crate::app::wallet::Wallet;
use crate::daemon::Daemon;
use crate::dir::LianaDirectory;
use crate::services::fiat::api::PriceApi;
use crate::services::fiat::client::PriceClient;
use crate::services::fiat::currency::Currency;
use crate::services::fiat::source::PriceSource;
use crate::utils::now;

/// Time to live of the list of available currencies for a given `PriceSource`.
const CURRENCIES_LIST_TTL_SECS: u64 = 3_600; // 1 hour

async fn update_price_setting(
    data_dir: LianaDirectory,
    network: Network,
    wallet: Arc<Wallet>,
    new_price_setting: PriceSetting,
) -> Result<Arc<Wallet>, Error> {
    let mut wallet = wallet.as_ref().clone();
    wallet = wallet.with_fiat_price_setting(Some(new_price_setting.clone()));
    let network_dir = data_dir.network_directory(network);
    let wallet_id = wallet.id();
    update_settings_file(&network_dir, |mut settings| {
        if let Some(wallet_setting) = settings
            .wallets
            .iter_mut()
            .find(|w| w.wallet_id() == wallet_id)
        {
            wallet_setting.fiat_price = Some(new_price_setting);
        }
        settings
    })
    .await?;
    Ok(Arc::new(wallet))
}

// Returns the wallet's fiat `PriceSetting` or the default value if not set. We only
// expect it to be `None` if `Wallet::or_default_fiat_price_setting` left it so.
fn wallet_price_setting_or_default(wallet: &Wallet) -> PriceSetting {
    wallet
        .fiat_price_setting
        .as_ref()
        .cloned()
        .unwrap_or_default()
}

pub struct GeneralSettingsState {
    wallet: Arc<Wallet>,
    new_price_setting: PriceSetting,
    currencies_list: HashMap<PriceSource, (/* timestamp */ u64, Vec<Currency>)>,
    error: Option<Error>,
}

impl From<GeneralSettingsState> for Box<dyn State> {
    fn from(s: GeneralSettingsState) -> Box<dyn State> {
        Box::new(s)
    }
}

impl GeneralSettingsState {
    pub fn new(wallet: Arc<Wallet>) -> Self {
        let new_price_setting = wallet_price_setting_or_default(&wallet);
        Self {
            wallet,
            new_price_setting,
            currencies_list: HashMap::new(),
            error: None,
        }
    }
}

impl State for GeneralSettingsState {
    fn view<'a>(&'a self, cache: &'a Cache) -> Element<'a, view::Message> {
        view::settings::general::general_section(
            cache,
            &self.new_price_setting,
            self.currencies_list
                .get(&self.new_price_setting.source)
                .map(|(_, list)| &list[..])
                .unwrap_or(&[]),
            self.error.as_ref(),
        )
    }
    fn reload(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        wallet: Arc<Wallet>,
    ) -> iced::Task<Message> {
        self.new_price_setting = wallet_price_setting_or_default(&wallet);
        self.wallet = wallet;
        if self.new_price_setting.is_enabled {
            let source = self.new_price_setting.source;
            return Task::perform(async move { source }, |source| {
                FiatMessage::ListCurrencies(source).into()
            });
        } else if self.wallet.fiat_price_setting.is_none() {
            // If the wallet does not have a fiat price setting, save the default disabled setting
            // to indicate that the user has seen the setting option (and a notification is no longer required).
            tracing::info!(
                "Fiat price setting is missing for wallet '{}'. Saving default setting.",
                self.wallet.id()
            );
            return Task::perform(async move {}, |_| FiatMessage::SaveChanges.into());
        }
        Task::none()
    }

    fn update(
        &mut self,
        _daemon: Arc<dyn Daemon + Sync + Send>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::WalletUpdated(res) => {
                match res {
                    Ok(wallet) => {
                        self.error = None;
                        self.new_price_setting = wallet_price_setting_or_default(&wallet); // no change expected since wallet was updated with new price setting
                        self.wallet = wallet;
                    }
                    Err(e) => {
                        self.error = Some(e);
                    }
                }
                Task::none()
            }
            Message::Fiat(FiatMessage::SaveChanges) => {
                if self.error.is_none()
                    && Some(&self.new_price_setting) != self.wallet.fiat_price_setting.as_ref()
                {
                    tracing::info!(
                        "Saving fiat price setting for wallet '{}': {:?}",
                        self.wallet.id(),
                        self.new_price_setting
                    );
                    let wallet = self.wallet.clone();
                    let price_setting = self.new_price_setting.clone();
                    let network = cache.network;
                    let datadir_path = cache.datadir_path.clone();
                    return Task::perform(
                        async move {
                            update_price_setting(datadir_path, network, wallet, price_setting).await
                        },
                        Message::WalletUpdated,
                    );
                }
                Task::none()
            }
            Message::Fiat(FiatMessage::ValidateCurrencySetting) => {
                if let Some((_, list)) = self.currencies_list.get(&self.new_price_setting.source) {
                    self.error = None;
                    // If the currently selected currency is not in the list of available currencies,
                    // set it to the default currency if eligible or otherwise the first available currency.
                    if !list.contains(&self.new_price_setting.currency) {
                        if list.contains(&Currency::default()) {
                            self.new_price_setting.currency = Currency::default();
                        } else if let Some(curr) = list.first() {
                            self.new_price_setting.currency = *curr;
                        } else {
                            self.error = Some(Error::Unexpected(
                                "No available currencies in the list.".to_string(),
                            ));
                            return Task::none();
                        }
                    }
                    return Task::perform(async move {}, |_| FiatMessage::SaveChanges.into());
                }
                Task::none()
            }
            Message::Fiat(FiatMessage::ListCurrenciesResult(source, requested_at, res)) => {
                match res {
                    Ok(list) => {
                        self.error = None;
                        // Update the currencies list only if the requested_at is newer than the existing one.
                        if !self
                            .currencies_list
                            .get(&source)
                            .is_some_and(|(old, _)| *old > requested_at)
                        {
                            tracing::debug!(
                                "Updating currencies list for source '{}' as requested at {}.",
                                source,
                                requested_at,
                            );
                            self.currencies_list
                                .insert(source, (requested_at, list.currencies));
                        }
                        return Task::perform(async move {}, |_| {
                            FiatMessage::ValidateCurrencySetting.into()
                        });
                    }
                    Err(e) => {
                        self.error = Some(e.into());
                    }
                }
                Task::none()
            }
            Message::Fiat(FiatMessage::ListCurrencies(source)) => {
                if self.new_price_setting.is_enabled {
                    // Update the currencies list if the cached list is stale.
                    let now = now().as_secs();
                    match self.currencies_list.get(&source) {
                        Some((old, _)) if now.saturating_sub(*old) <= CURRENCIES_LIST_TTL_SECS => {
                            return Task::perform(async move {}, |_| {
                                FiatMessage::ValidateCurrencySetting.into()
                            });
                        }
                        _ => {
                            return Task::perform(
                                async move {
                                    let client = PriceClient::default_from_source(source);
                                    (source, now, client.list_currencies().await)
                                },
                                |(source, now, res)| {
                                    FiatMessage::ListCurrenciesResult(source, now, res).into()
                                },
                            );
                        }
                    }
                }
                Task::none()
            }
            Message::View(view::Message::Settings(view::SettingsMessage::Fiat(msg))) => {
                match msg {
                    view::FiatMessage::Enable(is_enabled) => {
                        self.new_price_setting.is_enabled = is_enabled;
                        if self.new_price_setting.is_enabled {
                            let source = self.new_price_setting.source;
                            return Task::perform(async move { source }, |source| {
                                FiatMessage::ListCurrencies(source).into()
                            });
                        } else {
                            return Task::perform(async move {}, |_| {
                                FiatMessage::SaveChanges.into()
                            });
                        }
                    }
                    view::FiatMessage::SourceEdited(source) => {
                        self.new_price_setting.source = source;
                        if self.new_price_setting.is_enabled {
                            let source = self.new_price_setting.source;
                            return Task::perform(async move { source }, |source| {
                                FiatMessage::ListCurrencies(source).into()
                            });
                        }
                    }
                    view::FiatMessage::CurrencyEdited(currency) => {
                        self.new_price_setting.currency = currency;
                        return Task::perform(async move {}, |_| {
                            FiatMessage::ValidateCurrencySetting.into()
                        });
                    }
                }
                Task::none()
            }
            _ => Task::none(),
        }
    }
}
