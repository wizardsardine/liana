//! Ephemeral display evidence for Bitcoin recovery. No signing authority.
use super::{cache::Cache, wallet::Wallet};
use crate::{
    daemon::model::Coin,
    services::foreign_scan::known::{KnownOutput, KnownUnspent, FORK_HEIGHT},
};
use coincube_core::{
    chain::ChainId,
    miniscript::bitcoin::{self},
    wallet_descriptor::WalletDescriptor,
};
use std::{collections::HashSet, time::Instant};

#[derive(Debug, Clone)]
pub struct Notice {
    pub proof: KnownUnspent,
    pub generation: u64,
    pub descriptor: WalletDescriptor,
}

pub fn owned(cache: &Cache, descriptor: &WalletDescriptor) -> Vec<KnownOutput> {
    owned_outputs(cache.coins().iter(), cache.blockheight(), descriptor)
}

fn owned_outputs<'a>(
    coins: impl Iterator<Item = &'a Coin>,
    blockheight: i32,
    descriptor: &WalletDescriptor,
) -> Vec<KnownOutput> {
    coins
        .filter_map(|coin| {
            let height = coin.block_height?;
            if coin.spend_info.is_some()
                || coin.is_immature
                || height <= 0
                || height as u32 >= FORK_HEIGHT
                || height > blockheight
                || coin.derivation_index.is_hardened()
            {
                return None;
            }
            let branch = if coin.is_change {
                descriptor.change_descriptor()
            } else {
                descriptor.receive_descriptor()
            };
            let script = branch
                .script_pubkey(u32::from(coin.derivation_index))
                .ok()?;
            if script != coin.address.script_pubkey() {
                return None;
            }
            Some(KnownOutput {
                outpoint: coin.outpoint,
                output: bitcoin::TxOut {
                    value: coin.amount,
                    script_pubkey: script,
                },
                bitcoin_height: height as u32,
            })
        })
        .collect()
}

impl Notice {
    pub fn new(proof: KnownUnspent, generation: u64, wallet: &Wallet) -> Self {
        Self {
            proof,
            generation,
            descriptor: wallet.main_descriptor.clone(),
        }
    }
    pub fn visible(&self, cache: &Cache, now: Instant) -> bool {
        if cache.chain() != ChainId::Bitcoin
            || !cache.connect_authenticated
            || !cache.btcb2_server_enabled
            || !self.proof.is_current(self.generation, now)
        {
            return false;
        }
        let outputs = self.proof.outputs();
        if outputs.is_empty() {
            return false;
        }

        // The notice depends only on the coins in the proof. Avoid deriving
        // scripts for unrelated wallet coins each time the view is rendered.
        let proven_outpoints: HashSet<_> = outputs.iter().map(|output| output.outpoint).collect();
        let candidates = cache
            .coins()
            .iter()
            .filter(|coin| proven_outpoints.contains(&coin.outpoint));
        let current = owned_outputs(candidates, cache.blockheight(), &self.descriptor);
        outputs.iter().all(|output| current.contains(output))
    }
}

/// Copy for the Bitcoin Cube's recovery screens (`coincube-api#276` I8):
/// coins received before the fork also exist on Bitcoin Blake2b until a
/// BTCB2 Cube sweeps them. This is the only copy of the line; the owner kept
/// this shipped wording on 2026-09-30 (#556). Display is gated by
/// [`Notice::visible`], never by this constant.
pub const UNSWEPT_RECOVERY_NOTICE: &str =
    "Some of this Cube’s coins also exist on Bitcoin Blake2b until swept there.";

impl Cache {
    pub fn unswept_recovery_notice(&self) -> Option<&'static str> {
        self.unswept_notice
            .as_ref()
            .filter(|notice| notice.visible(self, Instant::now()))
            .map(|_| UNSWEPT_RECOVERY_NOTICE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::vault::test_support::unified::fixture;
    use bitcoin::{Address, Amount, Network};

    fn setup() -> (Cache, tokio::sync::watch::Sender<u64>) {
        let f = fixture();
        let script = f
            .descriptor
            .receive_descriptor()
            .derive(
                3.into(),
                &bitcoin::secp256k1::Secp256k1::verification_only(),
            )
            .script_pubkey();
        let mut cache = Cache {
            connect_authenticated: true,
            btcb2_server_enabled: true,
            ..Cache::default()
        };
        cache.daemon_cache.blockheight = 1_000_000;
        cache.daemon_cache.coins.push(Coin {
            amount: Amount::from_sat(50_000),
            outpoint: f.psbt.unsigned_tx.input[0].previous_output,
            address: Address::from_script(&script, Network::Bitcoin).unwrap(),
            block_height: Some(900_000),
            derivation_index: 3.into(),
            spend_info: None,
            is_immature: false,
            is_change: false,
            is_from_self: false,
        });
        let (tx, rx) = tokio::sync::watch::channel(7);
        let proof = KnownUnspent::for_notice_test(
            owned(&cache, &f.descriptor.clone().into()),
            7,
            rx,
            Instant::now(),
        );
        cache.unswept_notice = Some(Notice {
            proof,
            generation: 7,
            descriptor: f.descriptor.into(),
        });
        (cache, tx)
    }
    #[test]
    fn unswept_notice_withdraws_for_changed_owned_coins() {
        let (cache, _live) = setup();
        assert!(cache.unswept_recovery_notice().is_some());
        for change in [0, 1, 2, 3, 4, 5, 6] {
            let mut altered = cache.clone();
            match change {
                0 => altered.daemon_cache.coins.clear(),
                1 => altered.daemon_cache.coins[0].block_height = None,
                2 => altered.daemon_cache.coins[0].amount += Amount::from_sat(1),
                3 => altered.daemon_cache.coins[0].derivation_index = 4.into(),
                4 => altered.daemon_cache.coins[0].is_change = true,
                5 => altered.daemon_cache.blockheight = 899_999,
                _ => {
                    altered.daemon_cache.coins[0].spend_info =
                        Some(coincubed::commands::LCSpendInfo {
                            txid: altered.daemon_cache.coins[0].outpoint.txid,
                            height: None,
                        })
                }
            }
            assert!(
                altered.unswept_recovery_notice().is_none(),
                "mutation {}",
                change
            );
        }
    }
    #[test]
    fn unswept_notice_does_not_substitute_an_unproven_outpoint() {
        let (mut cache, _live) = setup();
        let mut unrelated = cache.daemon_cache.coins[0].clone();
        unrelated.outpoint.vout += 1;
        cache.daemon_cache.coins.push(unrelated);
        assert!(cache.unswept_recovery_notice().is_some());

        cache.daemon_cache.coins[0].block_height = Some(FORK_HEIGHT as i32);
        assert!(cache.unswept_recovery_notice().is_none());
    }

    #[test]
    fn unswept_notice_is_session_chain_feature_and_age_bound() {
        let (cache, live) = setup();
        let mut altered = cache.clone();
        altered.connect_authenticated = false;
        assert!(altered.unswept_recovery_notice().is_none());
        altered = cache.clone();
        altered.btcb2_server_enabled = false;
        assert!(altered.unswept_recovery_notice().is_none());
        for chain in [
            ChainId::BitcoinBlake2b,
            ChainId::Testnet4,
            ChainId::Signet,
            ChainId::Regtest,
        ] {
            altered = cache.clone();
            altered.fiat_chain = chain;
            assert!(altered.unswept_recovery_notice().is_none(), "{:?}", chain);
        }
        assert!(!cache
            .unswept_notice
            .as_ref()
            .unwrap()
            .visible(&cache, Instant::now() + std::time::Duration::from_secs(61)));
        live.send_replace(8);
        assert!(cache.unswept_recovery_notice().is_none());
    }
    #[tokio::test]
    async fn unswept_notice_app_ignores_old_callbacks_and_revokes_live_evidence() {
        use crate::app::{cache::AppGeneration, message::Message, shell_tests::bitcoin_app};
        let root = std::env::temp_dir().join(format!("coincube-notice-{}", uuid::Uuid::new_v4()));
        let (mut app, _) = bitcoin_app(&root);
        let generation = *app.panels.session_generation.borrow();
        app.unswept_in_flight = Some(generation);
        let _ = app.update(Message::UnsweptNotice {
            app: AppGeneration::next(),
            generation,
            result: Ok(None),
        });
        assert_eq!(app.unswept_in_flight, Some(generation));
        let _ = app.update(Message::UnsweptNotice {
            app: app.cache.app_generation,
            generation: generation + 1,
            result: Ok(None),
        });
        assert_eq!(app.unswept_in_flight, Some(generation));
        let _ = app.update(Message::UnsweptNotice {
            app: app.cache.app_generation,
            generation,
            result: Ok(None),
        });
        assert!(app.unswept_in_flight.is_none());
        let (cache, _live) = setup();
        app.cache.unswept_notice = cache.unswept_notice;
        app.unswept_in_flight = Some(generation);
        app.revoke_claim();
        assert!(app.cache.unswept_notice.is_none());
        assert!(app.unswept_in_flight.is_none());
        assert_ne!(*app.panels.session_generation.borrow(), generation);
        let _ = app.update(Message::UnsweptNotice {
            app: app.cache.app_generation,
            generation,
            result: Ok(None),
        });
        assert!(app.cache.unswept_notice.is_none());
        let _ = std::fs::remove_dir_all(root);
    }
    #[test]
    fn unswept_notice_account_and_provider_replacement_revokes() {
        use crate::app::{message::Message, shell_tests::bitcoin_app, view};
        let _guard = crate::app::session::test_guard();
        let root =
            std::env::temp_dir().join(format!("coincube-notice-session-{}", uuid::Uuid::new_v4()));
        for provider_change in [false, true] {
            let (mut app, _) = bitcoin_app(&root);
            app.panels.connect.account.user = Some(crate::services::coincube::User {
                id: 7,
                email: "fixture@example.invalid".into(),
                email_verified: Some(true),
            });
            app.panels.connect.account.step =
                crate::app::state::connect::account::ConnectFlowStep::Dashboard;
            app.panels.connect.account.client = crate::services::coincube::CoincubeClient::for_test(
                "https://first.example.invalid",
            );
            app.panels.connect.account.client.set_token("fixture-token");
            app.panels.connect.account.features = Some(
                serde_json::from_value(
                    serde_json::json!({"plans": [], "bitcoinBlake2bEnabled": true}),
                )
                .unwrap(),
            );
            app.unswept_session = app.claim_connect_session();
            assert!(app.unswept_session.is_some());
            let generation = *app.panels.session_generation.borrow();
            app.unswept_in_flight = Some(generation);
            drop(app.update(Message::View(view::Message::ConnectAccount(
                view::ConnectAccountMessage::PlanLoaded(None, 0),
            ))));
            assert!(app.cache.btcb2_server_enabled);
            assert_eq!(
                app.unswept_in_flight,
                Some(generation),
                "unchanged session must preserve request"
            );
            if provider_change {
                app.panels.connect.account.client =
                    crate::services::coincube::CoincubeClient::for_test(
                        "https://second.example.invalid",
                    );
                app.panels.connect.account.client.set_token("fixture-token");
            } else {
                app.panels.connect.account.user.as_mut().unwrap().id = 8;
            }
            drop(app.update(Message::View(view::Message::ConnectAccount(
                view::ConnectAccountMessage::PlanLoaded(None, 0),
            ))));
            assert!(app.unswept_in_flight.is_none());
            assert!(app.unswept_session.is_none());
            assert_ne!(*app.panels.session_generation.borrow(), generation);
        }
        let _ = std::fs::remove_dir_all(root);
    }
    /// A Bitcoin app with one owned pre-fork coin and a live Connect session,
    /// holding a request in flight at the current session generation.
    fn app_with_unswept_request(
        root: &std::path::Path,
    ) -> (crate::app::App, std::sync::Arc<Wallet>, u64) {
        use crate::app::shell_tests::bitcoin_app;
        let (mut app, wallet) = {
            let _guard = crate::app::session::test_guard();
            bitcoin_app(root)
        };
        let (cache, _live) = setup();
        app.cache.daemon_cache = cache.daemon_cache;
        let script = wallet
            .main_descriptor
            .receive_descriptor()
            .script_pubkey(3)
            .unwrap();
        app.cache.daemon_cache.coins[0].address =
            Address::from_script(&script, Network::Bitcoin).unwrap();
        app.cache.connect_authenticated = true;
        app.panels.connect.account.user = Some(crate::services::coincube::User {
            id: 7,
            email: "fixture@example.invalid".into(),
            email_verified: Some(true),
        });
        app.panels.connect.account.step =
            crate::app::state::connect::account::ConnectFlowStep::Dashboard;
        app.panels.connect.account.client =
            crate::services::coincube::CoincubeClient::for_test("https://notice.example.invalid");
        app.panels.connect.account.client.set_token("fixture-token");
        app.unswept_session = app.claim_connect_session();
        let generation = *app.panels.session_generation.borrow();
        app.unswept_in_flight = Some(generation);
        (app, wallet, generation)
    }
    fn proof_for(app: &crate::app::App, wallet: &Wallet, generation: u64) -> KnownUnspent {
        KnownUnspent::for_notice_test(
            owned(&app.cache, &wallet.main_descriptor),
            generation,
            app.panels.session_generation.subscribe(),
            Instant::now(),
        )
    }
    #[tokio::test]
    async fn unswept_notice_positive_callback_displays_until_local_spend() {
        use crate::app::message::Message;
        let root =
            std::env::temp_dir().join(format!("coincube-notice-positive-{}", uuid::Uuid::new_v4()));
        let (mut app, wallet, generation) = app_with_unswept_request(&root);
        let proof = proof_for(&app, &wallet, generation);
        assert!(app.cache.unswept_recovery_notice().is_none());
        drop(app.update(Message::UnsweptNotice {
            app: app.cache.app_generation,
            generation,
            result: Ok(Some(proof)),
        }));
        assert!(app.cache.unswept_recovery_notice().is_some());
        let coin = &mut app.cache.daemon_cache.coins[0];
        coin.spend_info = Some(coincubed::commands::LCSpendInfo {
            txid: coin.outpoint.txid,
            height: None,
        });
        assert!(app.cache.unswept_recovery_notice().is_none());
        let _ = std::fs::remove_dir_all(root);
    }
    /// Each sync tick refreshes the evidence. The visible notice must survive
    /// the refresh until its callback replaces it, or the line blinks off and
    /// on every few seconds; ineligibility and a negative answer still withdraw it.
    #[tokio::test]
    async fn unswept_notice_survives_sync_refresh_until_its_callback() {
        use crate::app::message::Message;
        let root =
            std::env::temp_dir().join(format!("coincube-notice-refresh-{}", uuid::Uuid::new_v4()));
        let (mut app, wallet, generation) = app_with_unswept_request(&root);
        let proof = proof_for(&app, &wallet, generation);
        drop(app.update(Message::UnsweptNotice {
            app: app.cache.app_generation,
            generation,
            result: Ok(Some(proof)),
        }));
        assert!(app.cache.unswept_recovery_notice().is_some());
        assert!(app.unswept_in_flight.is_none());

        drop(app.unswept_notice_task());
        assert_eq!(app.unswept_in_flight, Some(generation), "refresh started");
        assert!(
            app.cache.unswept_recovery_notice().is_some(),
            "starting a refresh must not blank the notice"
        );
        drop(app.unswept_notice_task());
        assert!(
            app.cache.unswept_recovery_notice().is_some(),
            "a refresh already in flight must not blank the notice"
        );

        drop(app.update(Message::UnsweptNotice {
            app: app.cache.app_generation,
            generation,
            result: Ok(None),
        }));
        assert!(app.cache.unswept_recovery_notice().is_none());

        let proof = proof_for(&app, &wallet, generation);
        app.unswept_in_flight = Some(generation);
        drop(app.update(Message::UnsweptNotice {
            app: app.cache.app_generation,
            generation,
            result: Ok(Some(proof)),
        }));
        assert!(app.cache.unswept_recovery_notice().is_some());
        app.cache.btcb2_server_enabled = false;
        drop(app.unswept_notice_task());
        assert!(
            app.cache.unswept_notice.is_none(),
            "ineligible refresh withdraws"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// #556 owner decision (2026-09-30): the shipped wording is the approved
    /// line. Pins the exact bytes (including the typographic apostrophe) and
    /// that both recovery surfaces render that line, and only while the
    /// evidence is visible. Driven through the real view functions and the
    /// widget tree's text operation, so a surface that drops the notice or
    /// renders a second copy of the string fails here.
    #[tokio::test]
    async fn unswept_notice_renders_the_pinned_line_on_both_recovery_surfaces() {
        use crate::app::{
            menu::{CubeSettingsOption, CubeSubMenu, Menu, VaultSubMenu},
            settings::{fiat::PriceSetting, unit::UnitSetting},
            state::settings::general::{BackupSeedState, SettingsSection},
            view::{settings::general::settings_content_section, vault::recovery::recovery},
        };
        use coincube_ui::widget::Element;
        use iced::advanced::{
            layout,
            renderer::Headless,
            widget::{Id, Operation, Tree},
            Layout,
        };

        assert_eq!(
            UNSWEPT_RECOVERY_NOTICE,
            "Some of this Cube\u{2019}s coins also exist on Bitcoin Blake2b until swept there."
        );

        #[derive(Default)]
        struct Labels(Vec<String>);
        impl Operation for Labels {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                operate(self);
            }
            fn text(&mut self, _: Option<&Id>, _: iced::Rectangle, text: &str) {
                self.0.push(text.to_owned());
            }
        }
        let renderer = <iced::Renderer as Headless>::new(
            iced::Font::DEFAULT,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        )
        .await
        .expect("software renderer available for view regression");
        let labels = |mut element: Element<'_, crate::app::view::Message>| {
            let mut tree = Tree::new(element.as_widget());
            let node = element.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(iced::Size::ZERO, iced::Size::new(1200.0, 1600.0)),
            );
            let mut labels = Labels::default();
            element
                .as_widget_mut()
                .operate(&mut tree, Layout::new(&node), &renderer, &mut labels);
            labels.0
        };

        let vault_menu = Menu::Vault(VaultSubMenu::Recovery);
        let settings_menu = Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::Recovery));
        let price = PriceSetting::default();
        let unit = UnitSetting::default();
        let backup = BackupSeedState::None;
        let pin = crate::pin_input::PinInput::default();
        let surfaces = |cache: &Cache| {
            [
                (
                    "vault recovery",
                    labels(recovery(&vault_menu, cache, Vec::new(), None, None)),
                ),
                (
                    "settings recovery",
                    labels(settings_content_section(
                        SettingsSection::Recovery,
                        &settings_menu,
                        cache,
                        &price,
                        &unit,
                        &[],
                        false,
                        false,
                        &backup,
                        &pin,
                        None,
                        None,
                        None,
                        None,
                        false,
                    )),
                ),
            ]
        };

        let (cache, _live) = setup();
        assert_eq!(
            cache.unswept_recovery_notice(),
            Some(UNSWEPT_RECOVERY_NOTICE)
        );
        for (surface, rendered) in surfaces(&cache) {
            assert_eq!(
                rendered
                    .iter()
                    .filter(|s| s.as_str() == UNSWEPT_RECOVERY_NOTICE)
                    .count(),
                1,
                "{}: {:?}",
                surface,
                rendered
            );
        }

        let mut hidden = cache.clone();
        hidden.connect_authenticated = false;
        assert_eq!(hidden.unswept_recovery_notice(), None);
        for (surface, rendered) in surfaces(&hidden) {
            assert!(
                !rendered
                    .iter()
                    .any(|s| s.contains("Bitcoin Blake2b until swept")),
                "{}: {:?}",
                surface,
                rendered
            );
        }
    }
}
