//! Refreshes App-owned persisted preferences and backup status after a save.
use super::*;

impl App {
    pub(super) fn handle_settings_saved(&mut self) -> Task<Message> {
        // Settings saved - reload unit preference and fiat_price from cube settings
        let network_dir = self
            .cache
            .datadir_path
            .network_directory(self.cube_settings.network);
        if let Ok(settings) = settings::Settings::from_file(&network_dir) {
            self.imported_kit_metadata
                .replace(&self.datadir, &self.cube_settings, &settings);
            if let Some(cube) = settings
                .cubes
                .iter()
                .find(|c| c.id == self.cube_settings.id)
            {
                self.refresh_persisted_cube_settings(cube);
            }
        }

        // Reload global settings into cache
        {
            use settings::global::GlobalSettings;
            let global_path = GlobalSettings::path(&self.cache.datadir_path);
            self.cache.show_direction_badges =
                GlobalSettings::load_show_direction_badges(&global_path);
        }

        // Forward to state panels so they can reload their internal state
        if let Some(panel) = self.panels.current_mut() {
            return Task::batch(vec![
                panel.update(self.daemon.clone(), &self.cache, Message::SettingsSaved),
                Task::done(Message::CacheUpdated),
            ]);
        }

        Task::done(Message::CacheUpdated)
    }

    fn refresh_persisted_cube_settings(&mut self, cube: &settings::CubeSettings) {
        self.cache.bitcoin_unit = cube.unit_setting.display_unit;
        self.cube_settings.fiat_price = cube.fiat_price.clone();
        if self.cube_settings.network.is_blake2b()
            && self.cache.fiat_price.as_ref().is_some_and(|price| {
                !cube.fiat_price.as_ref().is_some_and(|setting| {
                    setting.is_enabled && setting.currency == price.currency()
                })
            })
        {
            self.cache.clear_btcb2_fiat();
            self.cache.btcb2_price_request = None;
        }
        // The backup attention row follows persisted state.
        self.cache.current_cube_backed_up = cube.backed_up;
        self.cube_settings.backed_up = cube.backed_up;
        // Mirror the drift fingerprint cache (W12). Refreshing
        // on every SettingsSaved keeps the Recovery-Kit card
        // in sync after a successful upload or remove.
        self.cache
            .recovery_kit_last_backed_up_descriptor_fingerprint = cube
            .recovery_kit_last_backed_up_descriptor_fingerprint
            .clone();
        self.cube_settings
            .recovery_kit_last_backed_up_descriptor_fingerprint = cube
            .recovery_kit_last_backed_up_descriptor_fingerprint
            .clone();
        // Same for the keychain (phone) drift slot (per-method
        // drift, PR 3).
        self.cache
            .recovery_kit_last_backed_up_keychain_descriptor_fingerprint = cube
            .recovery_kit_last_backed_up_keychain_descriptor_fingerprint
            .clone();
        self.cube_settings
            .recovery_kit_last_backed_up_keychain_descriptor_fingerprint = cube
            .recovery_kit_last_backed_up_keychain_descriptor_fingerprint
            .clone();
        // And the password kit's *presence* flag — the signal
        // `has_recovery_kit()` reads for that method, which a
        // Vault-less (seed-only) kit has no fingerprint to carry.
        self.cache.recovery_kit_password_backed_up = cube.recovery_kit_password_backed_up;
        self.cube_settings.recovery_kit_password_backed_up = cube.recovery_kit_password_backed_up;

        // Clear cached fiat display price if disabled.
        // Note: btc_usd_price is NOT cleared — it's needed for
        // USDt→sats conversion regardless of fiat display setting.
        if !cube.fiat_price.as_ref().is_some_and(|p| p.is_enabled) {
            self.cache.fiat_price = None;
            if self.cube_settings.network.is_blake2b() {
                self.cache.clear_btcb2_fiat();
                self.cache.btcb2_price_request = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain::ChainId;
    use crate::services::fiat::{api::GetPriceResult, Currency, PriceSource};
    use iced::futures::StreamExt;

    fn with_app(chain: ChainId, test: impl FnOnce(&mut App)) {
        let _guard = session::test_guard();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _entered = runtime.enter();
        let root = tempfile::tempdir().unwrap();
        let mut app = if chain.is_blake2b() {
            let (app, startup) = App::without_wallet(
                None,
                None,
                Config::new(false),
                CoincubeDirectory::new(root.path().to_path_buf()),
                chain.bitcoin_network(),
                settings::CubeSettings::new("Fixture".into(), chain),
                None,
            );
            drop(startup);
            app
        } else {
            shell_tests::bitcoin_app(root.path()).0
        };
        app.panels.current = Menu::Vault(menu::VaultSubMenu::Send);
        app.panels.create_spend = None;
        assert!(app.panels.current_mut().is_none());
        test(&mut app);
    }

    fn save(app: &App, chain: ChainId, cubes: Vec<settings::CubeSettings>) {
        let dir = app.cache.datadir_path.network_directory(chain);
        std::fs::create_dir_all(dir.path()).unwrap();
        let saved = settings::Settings {
            cubes,
            ..Default::default()
        };
        std::fs::write(
            dir.path().join(settings::SETTINGS_FILE_NAME),
            serde_json::to_vec(&saved).unwrap(),
        )
        .unwrap();
    }

    fn refresh(app: &mut App) {
        let outputs = iced::futures::executor::block_on(
            iced_runtime::task::into_stream(app.update(Message::SettingsSaved))
                .unwrap()
                .collect::<Vec<_>>(),
        );
        assert_eq!(outputs.len(), 1);
        assert!(matches!(
            outputs[0],
            iced_runtime::Action::Output(Message::CacheUpdated)
        ));
    }

    fn quote(app: &App, currency: Currency) -> cache::FiatPrice {
        cache::FiatPrice {
            res: Ok(GetPriceResult {
                value: 123.0,
                updated_at: None,
            }),
            request: if app.cube_settings.network.is_blake2b() {
                cache::FiatPriceRequest::for_btcb2(
                    currency,
                    cache::LookupOrigin {
                        app: app.cache.app_generation,
                        chain: app.cube_settings.network,
                    },
                )
            } else {
                cache::FiatPriceRequest::new(PriceSource::Coincube, currency)
            },
            btcb2_valid_until: None,
        }
    }

    #[test]
    fn selected_cube_refreshes_and_clears_all_backup_slots_without_using_sibling_or_other_chain() {
        with_app(ChainId::Bitcoin, |app| {
            let mut selected = app.cube_settings.clone();
            selected.unit_setting.display_unit = settings::unit::BitcoinDisplayUnit::BTC;
            selected.backed_up = true;
            selected.recovery_kit_password_backed_up = true;
            selected.recovery_kit_last_backed_up_descriptor_fingerprint = Some("vault".into());
            selected.recovery_kit_last_backed_up_keychain_descriptor_fingerprint =
                Some("phone".into());
            selected.fiat_price = Some(settings::fiat::PriceSetting {
                currency: Currency::EUR,
                ..Default::default()
            });
            let mut sibling = app.cube_settings.clone();
            sibling.id = "sibling".into();
            save(
                app,
                ChainId::Bitcoin,
                vec![sibling.clone(), selected.clone()],
            );
            let mut wrong_chain = sibling;
            wrong_chain.id = selected.id.clone();
            save(app, ChainId::Testnet4, vec![wrong_chain]);
            refresh(app);
            assert_eq!(
                app.cache.bitcoin_unit,
                settings::unit::BitcoinDisplayUnit::BTC
            );
            assert_eq!(app.cube_settings.fiat_price, selected.fiat_price);
            assert!(app.cache.current_cube_backed_up && app.cube_settings.backed_up);
            assert!(
                app.cache.recovery_kit_password_backed_up
                    && app.cube_settings.recovery_kit_password_backed_up
            );
            assert_eq!(
                app.cache
                    .recovery_kit_last_backed_up_descriptor_fingerprint
                    .as_deref(),
                Some("vault")
            );
            assert_eq!(
                app.cube_settings
                    .recovery_kit_last_backed_up_descriptor_fingerprint
                    .as_deref(),
                Some("vault")
            );
            assert_eq!(
                app.cache
                    .recovery_kit_last_backed_up_keychain_descriptor_fingerprint
                    .as_deref(),
                Some("phone")
            );
            assert_eq!(
                app.cube_settings
                    .recovery_kit_last_backed_up_keychain_descriptor_fingerprint
                    .as_deref(),
                Some("phone")
            );
            selected.backed_up = false;
            selected.recovery_kit_password_backed_up = false;
            selected.recovery_kit_last_backed_up_descriptor_fingerprint = None;
            selected.recovery_kit_last_backed_up_keychain_descriptor_fingerprint = None;
            selected.unit_setting.display_unit = settings::unit::BitcoinDisplayUnit::Sats;
            save(app, ChainId::Bitcoin, vec![selected]);
            refresh(app);
            assert!(!app.cache.current_cube_backed_up && !app.cube_settings.backed_up);
            assert!(
                !app.cache.recovery_kit_password_backed_up
                    && !app.cube_settings.recovery_kit_password_backed_up
            );
            assert!(app
                .cache
                .recovery_kit_last_backed_up_descriptor_fingerprint
                .is_none());
            assert!(app
                .cube_settings
                .recovery_kit_last_backed_up_descriptor_fingerprint
                .is_none());
            assert!(app
                .cache
                .recovery_kit_last_backed_up_keychain_descriptor_fingerprint
                .is_none());
            assert!(app
                .cube_settings
                .recovery_kit_last_backed_up_keychain_descriptor_fingerprint
                .is_none());
            assert_eq!(
                app.cache.bitcoin_unit,
                settings::unit::BitcoinDisplayUnit::Sats
            );
        });
    }

    #[test]
    fn failed_or_unselected_cube_load_still_refreshes_global_badges_and_completes_with_panel() {
        with_app(ChainId::Bitcoin, |app| {
            app.panels.current =
                Menu::Cube(menu::CubeSubMenu::Settings(menu::CubeSettingsOption::About));
            assert!(app.panels.current_mut().is_some());
            let path = settings::global::GlobalSettings::path(&app.cache.datadir_path);
            let original = app.cube_settings.clone();
            for (index, contents) in [None, Some("{broken"), Some("{\"cubes\":[]}")]
                .iter()
                .copied()
                .enumerate()
            {
                let dir = app.cache.datadir_path.network_directory(ChainId::Bitcoin);
                std::fs::create_dir_all(dir.path()).unwrap();
                if let Some(contents) = contents {
                    std::fs::write(dir.path().join(settings::SETTINGS_FILE_NAME), contents)
                        .unwrap();
                }
                let enabled = index % 2 == 0;
                settings::global::GlobalSettings::update_show_direction_badges(&path, enabled)
                    .unwrap();
                app.cache.show_direction_badges = !enabled;
                refresh(app);
                assert_eq!(app.cache.show_direction_badges, enabled);
                assert_eq!(app.cube_settings.fiat_price, original.fiat_price);
                assert_eq!(app.cube_settings.backed_up, original.backed_up);
            }
        });
    }

    #[test]
    fn disabled_bitcoin_display_keeps_usd_conversion_quote() {
        with_app(ChainId::Bitcoin, |app| {
            for setting in [
                None,
                Some(settings::fiat::PriceSetting {
                    is_enabled: false,
                    ..Default::default()
                }),
            ] {
                app.cache.fiat_price = Some(quote(app, Currency::USD));
                app.cache.btc_usd_price = Some(67000.0);
                let mut selected = app.cube_settings.clone();
                selected.fiat_price = setting;
                save(app, ChainId::Bitcoin, vec![selected]);
                refresh(app);
                assert!(app.cache.fiat_price.is_none());
                assert_eq!(app.cache.btc_usd_price, Some(67000.0));
            }
        });
    }

    #[test]
    fn fork_quote_and_pending_request_follow_enabled_currency() {
        with_app(ChainId::BitcoinBlake2b, |app| {
            for (enabled, currency, retained) in [
                (true, Currency::USD, true),
                (true, Currency::EUR, false),
                (false, Currency::USD, false),
            ] {
                let quote = quote(app, Currency::USD);
                app.cache.btcb2_price_request = Some(quote.request);
                app.cache.fiat_price = Some(quote);
                let mut selected = app.cube_settings.clone();
                selected.fiat_price = Some(settings::fiat::PriceSetting {
                    is_enabled: enabled,
                    currency,
                    ..Default::default()
                });
                save(app, ChainId::BitcoinBlake2b, vec![selected]);
                refresh(app);
                assert_eq!(app.cache.fiat_price.is_some(), retained);
                assert_eq!(app.cache.btcb2_price_request.is_some(), retained);
            }
        });
    }
}
