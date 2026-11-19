//! Synthetic screenshots of the production Vault Settings presenters.
//! Set `R11A_CAPTURE_DIR` to emit PNGs and a JSONL manifest.

use super::*;
use crate::app::menu::SettingsOption;
use crate::app::state::settings::local_signing::{LocalSigningState, PairableKey, PairingFlow};
use crate::export::{ImportExportState, ImportExportType};
use crate::phone_signer::errors::PairingError;
use crate::phone_signer::mdns::DiscoveredPhone;
use crate::phone_signer::pairing::PairingOffer;
use crate::phone_signer::pairing_store::PairedPhone;
use coincube_core::descriptors::CoincubeDescriptor;
use coincube_core::miniscript::bitcoin::{
    bip32::{ChainCode, Xpub},
    secp256k1::PublicKey,
    NetworkKind,
};
use coincube_core::wallet_descriptor::ImportedWalletDescriptor;
use coincube_ui::locale::copy as localized_copy;
use iced::advanced::{
    layout,
    renderer::Headless,
    widget::{Id, Operation, Tree},
    Layout,
};
use iced::widget::qr_code;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::str::FromStr;
use std::sync::Arc;

// Public-only fixture material. No private key, seed, descriptor literal or
// xpub literal enters the capture source or the rendered pixels.
fn public_xpub(chain_byte: u8) -> Xpub {
    Xpub {
        network: NetworkKind::Test,
        depth: 3,
        parent_fingerprint: [1, 2, 3, 4].into(),
        child_number: coincube_core::miniscript::bitcoin::bip32::ChildNumber::Hardened { index: 0 },
        public_key: PublicKey::from_str(
            "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
        )
        .unwrap(),
        chain_code: ChainCode::from([chain_byte; 32]),
    }
}

fn wallet_descriptor(imported: bool) -> WalletDescriptor {
    let first = public_xpub(3);
    if imported {
        let descriptor = format!("wpkh([01020304/84'/1'/0']{first}/<0;1>/*)");
        WalletDescriptor::Imported(
            ImportedWalletDescriptor::parse(&descriptor, None, crate::chain::ChainId::Regtest)
                .unwrap(),
        )
    } else {
        let second = public_xpub(5);
        let third = public_xpub(7);
        let descriptor = format!(
            "wsh(andor(pk([01020304/48'/1'/0'/2']{first}/<0;1>/*),older(144),andor(pk([05060708/48'/1'/0'/2']{second}/<0;1>/*),older(52596),pk([090a0b0c/48'/1'/0'/2']{third}/<0;1>/*))))"
        );
        WalletDescriptor::Vault(CoincubeDescriptor::from_str(&descriptor).unwrap())
    }
}

fn synthetic_aliases() -> Vec<(Fingerprint, form::Value<String>)> {
    [
        (
            [1, 2, 3, 4],
            "Synthetic first recovery signer with a long display name",
        ),
        (
            [5, 6, 7, 8],
            "Synthetic second recovery signer with a long display name",
        ),
        (
            [9, 10, 11, 12],
            "Synthetic primary signer with a long display name",
        ),
    ]
    .iter()
    .map(|(bytes, name)| {
        (
            Fingerprint::from(*bytes),
            form::Value {
                value: (*name).to_owned(),
                valid: true,
                warning: None,
            },
        )
    })
    .collect()
}

#[derive(Default)]
struct Labels(Vec<(String, iced::Rectangle)>, Vec<iced::Rectangle>);

impl Operation for Labels {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn text(&mut self, _: Option<&Id>, bounds: iced::Rectangle, value: &str) {
        self.0.push((value.to_owned(), bounds));
    }

    fn scrollable(
        &mut self,
        _: Option<&Id>,
        bounds: iced::Rectangle,
        _: iced::Rectangle,
        _: iced::Vector,
        _: &mut dyn iced::advanced::widget::operation::Scrollable,
    ) {
        self.1.push(bounds);
    }
}

fn menu(section: SettingsOption) -> Menu {
    Menu::Vault(VaultSubMenu::Settings(Some(section)))
}

fn phone() -> DiscoveredPhone {
    DiscoveredPhone {
        cert_fp8: "a1b2c3d4".into(),
        addr: "192.0.2.20:54502".parse().unwrap(),
        instance_name: "Synthetic Mobile".into(),
    }
}

fn pair_state(scenario: &str) -> LocalSigningState {
    let mut state = LocalSigningState::default();
    let vault_id = Fingerprint::from([1, 2, 3, 4]);
    state.wallet_fingerprint = Some(vault_id);
    state.keychain_keys_recorded = true;
    let synthetic_xpub = public_xpub(3).to_string();
    state.selected_key = Some(synthetic_xpub.clone());
    state.vault_keys.push(PairableKey {
        xpub: synthetic_xpub,
        descriptor_key: String::new(),
        fingerprint: Some(vault_id),
        name: Some("Synthetic Mobile key".into()),
        keychain: true,
    });
    state.flow = match scenario {
        "pair_picker" => PairingFlow::PhonePicker {
            discovered: vec![phone()],
        },
        "pair_wait" => PairingFlow::Waiting {
            phone: phone(),
            offer: PairingOffer {
                signer_xpub: String::new(),
                descriptor_sha256: String::new(),
                version: 2,
                cert_der_b64: String::new(),
                cert_fp: String::new(),
                service_name: String::new(),
                wallet_fingerprint: vault_id,
                expires_at_unix: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
                    + 90,
                psk_b64: String::new(),
            },
            // Synthetic QR exercises the rendered waiting state without a live
            // pairing offer or any wallet material.
            qr: Some(qr_code::Data::new("synthetic-pairing-offer-not-for-use").unwrap()),
        },
        "pair_error" => PairingFlow::Error(PairingError::OfferExpired),
        _ => PairingFlow::Idle,
    };
    if scenario == "pair_paired" {
        state.phones.phones.push(PairedPhone {
            signer_binding: None,
            cert_pin: [0x42; 32],
            name: "Synthetic phone".into(),
            paired_at_unix: 1_700_000_000,
            wallet_fingerprints: Vec::new(),
            vault_fingerprint: vault_id,
            transport_pubkey: Vec::new(),
            fallback_addr: None,
        });
    }
    state
}

fn node_panel<'a>(fixture: &'a Fixture, offline: bool) -> Element<'a, Message> {
    let status = node_backend_status(
        if offline {
            localized_copy("Backend unavailable")
        } else {
            localized_copy("Local node")
        },
        if offline {
            icon::network_icon()
        } else {
            icon::bitcoin_icon()
        },
        None,
        None,
        None,
        None,
        None,
        false,
        !offline,
        false,
        false,
        false,
        false,
        false,
        false,
        offline.then(|| "Local node status unavailable".to_owned()),
    )
    .map(|msg| Message::Settings(SettingsMessage::NodeSettings(msg)));
    let mut panels = vec![status];
    if !offline {
        panels.push(node_resources_section(
            &fixture.prune,
            &fixture.max_mempool,
            false,
        ));
        panels.push(
            rescan(
                &fixture.year,
                &fixture.month,
                &fixture.day,
                None,
                false,
                false,
                true,
                false,
                false,
                false,
                false,
            )
            .map(|msg| Message::Settings(SettingsMessage::BitcoindSettings(msg))),
        );
    }
    bitcoind_settings(&fixture.menu, &fixture.cache, panels)
}

struct Fixture {
    menu: Menu,
    cache: Cache,
    pairing: LocalSigningState,
    descriptor: WalletDescriptor,
    alias: form::Value<String>,
    keys_aliases: Vec<(Fingerprint, form::Value<String>)>,
    provider_keys: HashMap<Fingerprint, ProviderKey>,
    conflict: ImportExportType,
    prune: form::Value<String>,
    max_mempool: form::Value<String>,
    year: form::Value<String>,
    month: form::Value<String>,
    day: form::Value<String>,
}

fn value(value: &str) -> form::Value<String> {
    form::Value {
        value: value.to_owned(),
        valid: true,
        warning: None,
    }
}

impl Fixture {
    fn new(scenario: &str, section: SettingsOption) -> Self {
        let descriptor = wallet_descriptor(scenario == "wallet_imported");
        let (sender, _receiver) = tokio::sync::mpsc::channel(1);
        let conflict = ImportExportType::ImportBackup {
            network_dir: crate::dir::NetworkDirectory::new(std::path::PathBuf::from(
                "/synthetic/unused",
            )),
            wallet: Arc::new(crate::app::wallet::Wallet::new(
                descriptor
                    .as_vault()
                    .cloned()
                    .unwrap_or_else(|| wallet_descriptor(false).as_vault().unwrap().clone()),
            )),
            overwrite_labels: Some(sender),
            overwrite_aliases: None,
        };
        Self {
            menu: menu(section),
            cache: Cache {
                cube_name: "Synthetic Cube".to_owned(),
                has_vault: scenario != "wallet_imported",
                has_imported_bitcoin: scenario == "wallet_imported",
                // As the App sets it: an imported wallet has no Keychain key.
                vault_lacks_keychain_key: scenario == "wallet_imported",
                ..Cache::default()
            },
            pairing: pair_state(scenario),
            descriptor,
            alias: value("Synthetic vault"),
            keys_aliases: synthetic_aliases(),
            provider_keys: HashMap::new(),
            conflict,
            prune: value("4096"),
            max_mempool: value("300"),
            year: value("2024"),
            month: value("7"),
            day: value("20"),
        }
    }

    fn render(&self, scenario: &str) -> Element<'_, Message> {
        match scenario {
            "data_conflict" => crate::app::view::vault::export::export_modal(
                &ImportExportState::Progress(50.0),
                None,
                "Import wallet",
                &self.conflict,
                None,
            ),
            "wallet_default" | "wallet_imported" => wallet_settings(
                &self.menu,
                &self.cache,
                &self.descriptor,
                Fingerprint::from([1, 2, 3, 4]),
                &self.alias,
                &self.keys_aliases,
                &self.provider_keys,
                false,
                false,
                None,
                false,
                // Advanced open, so the imported-wallet check sees its body.
                scenario == "wallet_imported",
            ),
            "node_local" => node_panel(self, false),
            "node_offline" => node_panel(self, true),
            "data_rows" => import_export(&self.menu, &self.cache, true),
            "pair_idle" | "pair_picker" | "pair_wait" | "pair_error" | "pair_paired" => {
                crate::app::view::settings::local_signing::section(
                    &self.menu,
                    &self.cache,
                    &self.pairing,
                )
            }
            _ => unreachable!(),
        }
    }
}

fn fixture_nav_context(cache: &Cache) -> crate::app::view::nav::NavContext<'_> {
    crate::app::view::nav::NavContext {
        has_vault: cache.has_vault,
        has_imported_bitcoin: cache.has_imported_bitcoin,
        has_p2p: cache.has_p2p,
        network: cache.chain(),
        p2p_test_coordinator: cache.p2p_test_coordinator,
        marketplace_flags: cache.marketplace_flags,
        btcb2_server_enabled: cache.btcb2_server_enabled,
        btcb2_already_claimed: cache.btcb2_already_claimed,
        cube_name: &cache.cube_name,
        spark_alias: cache.wallet_names.spark.as_deref(),
        vault_alias: cache.wallet_names.vault.as_deref(),
        lightning_address: cache.lightning_address.as_deref(),
        avatar: cache.avatar_handle.as_ref(),
        theme_mode: cache.theme_mode,
        connect_authenticated: cache.sidebar_connect_signed_in,
        connect_email: cache.connect_email.as_deref(),
        connect_stream_status: &cache.connect_stream_status,
        connect: Default::default(),
    }
}

fn source_sha() -> String {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("read source commit");
    assert!(output.status.success(), "read source commit");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn load_fonts() {
    for bytes in coincube_ui::font::load() {
        iced::advanced::graphics::text::font_system()
            .write()
            .unwrap()
            .load_font(bytes);
    }
}

fn assert_labels(scenario: &str, labels: &Labels, width: u32) {
    let expected = match scenario {
        "wallet_default" | "wallet_imported" => "Wallet & signers",
        "node_local" | "node_offline" => "Node",
        "data_rows" => "Protection material",
        "data_conflict" => "Import wallet",
        "pair_idle" | "pair_picker" | "pair_wait" | "pair_error" | "pair_paired" => "Pair a phone",
        _ => unreachable!(),
    };
    assert!(
        labels.0.iter().any(|(label, _)| label == expected),
        "{}: missing {}",
        scenario,
        expected
    );
    if scenario == "node_local" {
        for lower_control in ["Node resources", "Rescan from a date"] {
            assert!(
                labels.0.iter().any(|(label, _)| label == lower_control),
                "node capture did not mount {}",
                lower_control
            );
        }
    }
    if scenario == "wallet_imported" {
        assert!(
            !labels
                .0
                .iter()
                .any(|(label, _)| label == "Export encrypted…"),
            "imported wallet offered a Vault-only export"
        );
    }
    for (label, bounds) in &labels.0 {
        assert!(
            bounds.x >= -1.0 && bounds.x + bounds.width <= width as f32 + 1.0,
            "{}: {} exceeds {}px: {:?}",
            scenario,
            label,
            width,
            bounds
        );
        for forbidden in ["tpub", "xpub6", "wsh(", ".cookie", ".onion"] {
            assert!(
                !label.contains(forbidden),
                "{}: sensitive label matching {}",
                scenario,
                forbidden
            );
        }
    }
}

#[tokio::test]
async fn compact_mounted_wallet_policy_keeps_both_recovery_clauses_visible() {
    load_fonts();
    let fixture = Fixture::new("wallet_default", SettingsOption::Wallet);
    assert_eq!(
        fixture
            .descriptor
            .as_vault()
            .unwrap()
            .policy()
            .recovery_paths()
            .len(),
        2,
        "fixture must exercise two relative recovery paths"
    );
    for (width, height) in [(1024, 768), (1440, 900)] {
        for theme in [theme::Theme::dark(), theme::Theme::light()] {
            let mut renderer = <iced::Renderer as Headless>::new(
                coincube_ui::font::REGULAR,
                iced::Pixels(16.0),
                Some("tiny-skia"),
            )
            .await
            .expect("software renderer");
            let mut element = fixture.render("wallet_default");
            let mut tree = Tree::new(element.as_widget());
            let size = iced::Size::new(width as f32, height as f32);
            let node = element.as_widget_mut().layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(iced::Size::ZERO, size),
            );
            let mut labels = Labels::default();
            element
                .as_widget_mut()
                .operate(&mut tree, Layout::new(&node), &renderer, &mut labels);
            assert_mounted_wallet_policy(&labels, width);
            element.as_widget().draw(
                &tree,
                &mut renderer,
                &theme,
                &iced::advanced::renderer::Style {
                    text_color: theme.colors.text.primary,
                },
                Layout::new(&node),
                iced::mouse::Cursor::Unavailable,
                &iced::Rectangle::with_size(size),
            );
        }
    }
}

fn assert_mounted_wallet_policy(labels: &Labels, width: u32) {
    let heading = labels
        .0
        .iter()
        .find(|(text, _)| text == "Identity and policy")
        .expect("mounted policy heading")
        .1;
    // The shell's vertical scrollable is the actual content viewport.
    let viewport = labels
        .1
        .iter()
        .find(|bounds| {
            heading.x >= bounds.x
                && heading.x < bounds.x + bounds.width
                && heading.y >= bounds.y
                && heading.y < bounds.y + bounds.height
        })
        .expect("mounted settings content viewport");
    let start = labels
        .0
        .iter()
        .position(|(text, _)| text == "Identity and policy")
        .unwrap();
    let end = labels
        .0
        .iter()
        .position(|(text, _)| text == "Signers")
        .unwrap();
    let policy_labels = &labels.0[start..end];
    for required in ["Normal spending", "Recovery path #1", "Recovery path #2"] {
        assert!(
            policy_labels.iter().any(|(text, _)| text == required),
            "missing policy row: {}",
            required
        );
    }
    for signer in [
        "Synthetic first recovery signer with a long display name",
        "Synthetic second recovery signer with a long display name",
        "Synthetic primary signer with a long display name",
    ] {
        assert!(
            policy_labels.iter().any(|(text, _)| text.contains(signer)),
            "policy does not name signer: {}",
            signer
        );
    }
    assert_eq!(
        policy_labels
            .iter()
            .filter(|(text, _)| text.contains("without spending"))
            .count(),
        2,
        "both recovery clauses must mount"
    );
    for (text, bounds) in policy_labels {
        assert!(
            bounds.x >= viewport.x - 1.0
                && bounds.x + bounds.width <= viewport.x + viewport.width + 1.0,
            "policy fragment escapes mounted content viewport: {} {:?} {:?}",
            text,
            bounds,
            viewport
        );
        assert!(
            bounds.x >= -1.0 && bounds.x + bounds.width <= width as f32 + 1.0,
            "policy fragment exceeds {}px: {} {:?}",
            width,
            text,
            bounds
        );
    }
}

/// An imported Vault's Wallet tab names its policy and the keys stored in
/// the Cube, offering Add recovery words only when none are stored.
#[tokio::test]
async fn imported_wallet_tab_shows_its_policy_and_stored_keys() {
    use crate::home::restore_frame::rendered_labels;
    for (stored, add, count) in [
        (Some(0), true, None),
        (Some(1), false, Some("1 from recovery words")),
        (None, false, None),
    ] {
        let mut fixture = Fixture::new("wallet_imported", SettingsOption::Wallet);
        fixture.cache.imported_stored_keys = stored;
        let shown = rendered_labels(fixture.render("wallet_imported")).await;
        assert!(
            shown.iter().any(|label| label == "Normal spending"),
            "{:?}",
            shown
        );
        assert_eq!(
            shown.iter().any(|label| label == "Add recovery words"),
            add,
            "{:?}: {:?}",
            stored,
            shown
        );
        if let Some(count) = count {
            assert!(
                shown.iter().any(|label| label.contains(count)),
                "{:?}",
                shown
            );
        }
        assert!(!shown.iter().any(|label| label.contains("no local keys")));
    }
}

#[tokio::test]
async fn mounted_settings_pair_route_accepts_vault_and_hides_it_for_imported_wallet() {
    // Settings routes are tabs on the pinned page head's rule (W5); the Cube
    // sidebar contains only the Settings entry. Exercise the mounted choice
    // in its actual location.
    load_fonts();
    let size = iced::Size::new(1440.0, 2400.0);
    let viewport = iced::Rectangle::with_size(size);
    for scenario in ["wallet_default", "wallet_imported"] {
        let fixture = Fixture::new(scenario, SettingsOption::Wallet);
        let renderer = <iced::Renderer as Headless>::new(
            coincube_ui::font::REGULAR,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        )
        .await
        .expect("software renderer");
        let mut element = fixture.render(scenario);
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(iced::Size::ZERO, size),
        );
        let mut labels = Labels::default();
        element
            .as_widget_mut()
            .operate(&mut tree, Layout::new(&node), &renderer, &mut labels);
        if scenario == "wallet_imported" {
            // No phone can pair for an imported wallet, as for a Vault
            // without a Keychain key: it has no Mobile tab
            // (`frame::tests::imported_bitcoin_has_the_vault_tabs_without_phone_pairing`).
            // "Mobile" here is the Wallet page's own Unavailable row.
            continue;
        }
        let pair_bounds = labels
            .0
            .iter()
            .find(|(label, _)| label == "Mobile")
            .expect("mounted Pair settings choice")
            .1;
        let pair_center = pair_bounds.center();
        // The content viewport is the wide scrollable in Pair's column that
        // starts nearest below it: the pinned head sits directly above it,
        // so the choice can't scroll away. Any other scrollable (a narrower
        // one, one in another column, or one above Pair) can't stand in.
        let pair_bottom = pair_bounds.y + pair_bounds.height;
        let content_scroll = labels
            .1
            .iter()
            .filter(|bounds| {
                bounds.width > 500.0
                    && pair_center.x >= bounds.x
                    && pair_center.x <= bounds.x + bounds.width
                    && bounds.y >= pair_bottom
            })
            .min_by(|a, b| a.y.total_cmp(&b.y))
            .expect("a settings content viewport below the pinned Pair choice");
        // Pair is in the head, not inside any scrollable: tabs moved back
        // into the body would sit in one.
        assert!(
            !labels.1.iter().any(|bounds| bounds.contains(pair_center)),
            "Pair settings choice is inside a scrollable: {:?} in {:?}",
            pair_bounds,
            labels.1
        );
        assert!(
            pair_bounds.x >= crate::app::view::nav::shell::FULL_WIDTH
                && pair_bounds.x >= content_scroll.x
                && pair_bounds.x + pair_bounds.width <= content_scroll.x + content_scroll.width
                && pair_bounds.y >= 0.0
                && pair_bounds.y + pair_bounds.height <= content_scroll.y,
            "Pair settings choice is not in the pinned head above the content viewport: {:?} over {:?}",
            pair_bounds,
            content_scroll
        );

        let mut messages = Vec::new();
        for event in [
            iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left),
            iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left),
        ] {
            element.as_widget_mut().update(
                &mut tree,
                &iced::Event::Mouse(event),
                Layout::new(&node),
                iced::mouse::Cursor::Available(pair_center),
                &renderer,
                &mut iced::advanced::clipboard::Null,
                &mut iced::advanced::Shell::new(&mut messages),
                &viewport,
            );
        }
        assert!(
            matches!(
                messages.as_slice(),
                [Message::FocusAction(id), Message::Menu(Menu::Vault(
                    VaultSubMenu::Settings(Some(SettingsOption::LocalSigning))
                ))] if id == &Id::new("tenshu.settings.route.vault.pair")
            ),
            "eligible Pair settings choice must focus and route once: {:?}",
            messages
        );
    }
}

#[tokio::test]
async fn actual_vault_settings_capture_matrix() {
    load_fonts();
    let directory = std::env::var("R11A_CAPTURE_DIR").ok();
    if let Some(path) = &directory {
        fs::create_dir_all(path).unwrap();
        fs::File::create(Path::new(path).join("manifest.jsonl")).unwrap();
    }
    let sha = directory.as_ref().map(|_| source_sha());
    for scenario in [
        "wallet_default",
        "wallet_imported",
        "node_local",
        "node_offline",
        "pair_idle",
        "pair_picker",
        "pair_wait",
        "pair_error",
        "pair_paired",
        "data_rows",
        "data_conflict",
    ] {
        let section = if scenario.starts_with("wallet") {
            SettingsOption::Wallet
        } else if scenario.starts_with("node") {
            SettingsOption::Node
        } else if scenario.starts_with("pair") {
            SettingsOption::LocalSigning
        } else {
            SettingsOption::ImportExport
        };
        for (theme_name, theme) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            let sizes = if scenario == "node_local" || scenario == "data_rows" {
                vec![(1440, 900), (1024, 768), (1440, 2400)]
            } else {
                vec![(1440, 900), (1024, 768)]
            };
            for (width, height) in sizes {
                let mut renderer = <iced::Renderer as Headless>::new(
                    coincube_ui::font::REGULAR,
                    iced::Pixels(16.0),
                    Some("tiny-skia"),
                )
                .await
                .expect("software renderer");
                let fixture = Fixture::new(scenario, section);
                let mut element = fixture.render(scenario);
                let mut tree = Tree::new(element.as_widget());
                let size = iced::Size::new(width as f32, height as f32);
                let node = element.as_widget_mut().layout(
                    &mut tree,
                    &renderer,
                    &layout::Limits::new(iced::Size::ZERO, size),
                );
                let mut labels = Labels::default();
                element.as_widget_mut().operate(
                    &mut tree,
                    Layout::new(&node),
                    &renderer,
                    &mut labels,
                );
                assert_labels(scenario, &labels, width);
                if scenario == "wallet_default" {
                    assert_mounted_wallet_policy(&labels, width);
                }
                if let Some(directory) = &directory {
                    let viewport = iced::Rectangle::with_size(size);
                    let mut redraw_messages = Vec::new();
                    element.as_widget_mut().update(
                        &mut tree,
                        &iced::Event::Window(iced::window::Event::RedrawRequested(
                            std::time::Instant::now(),
                        )),
                        Layout::new(&node),
                        iced::mouse::Cursor::Unavailable,
                        &renderer,
                        &mut iced::advanced::clipboard::Null,
                        &mut iced::advanced::Shell::new(&mut redraw_messages),
                        &viewport,
                    );
                    assert!(
                        redraw_messages
                            .iter()
                            .all(|message| matches!(message, Message::Scroll(_))),
                        "{}: redraw emitted an action intent",
                        scenario
                    );
                    element.as_widget().draw(
                        &tree,
                        &mut renderer,
                        &theme,
                        &iced::advanced::renderer::Style {
                            text_color: theme.colors.text.primary,
                        },
                        Layout::new(&node),
                        iced::mouse::Cursor::Unavailable,
                        &viewport,
                    );
                    let pixels = renderer.screenshot(
                        iced::Size::new(width, height),
                        1.0,
                        theme.colors.general.background,
                    );
                    let name = format!("r11a-{scenario}-{theme_name}-{width}x{height}.png");
                    image::save_buffer(
                        Path::new(directory).join(&name),
                        &pixels,
                        width,
                        height,
                        image::ColorType::Rgba8,
                    )
                    .unwrap();
                    let hash = hex::encode(Sha256::digest(&pixels));
                    let image_hash = hex::encode(Sha256::digest(
                        fs::read(Path::new(directory).join(&name)).unwrap(),
                    ));
                    let record = serde_json::json!({
                        "fixture": "r11a-actual-iced",
                        "source_sha": sha.as_deref().expect("capture source SHA"),
                        "scenario": scenario,
                        "theme": theme_name,
                        "width": width,
                        "height": height,
                        "file": name,
                        "pixel_sha256": hash,
                        "image_sha256": image_hash,
                    });
                    let mut manifest = OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(Path::new(directory).join("manifest.jsonl"))
                        .unwrap();
                    writeln!(manifest, "{record}").unwrap();
                }
                if scenario == "wallet_imported" {
                    assert!(fixture.cache.has_imported_bitcoin);
                    assert!(!fixture.cache.has_vault);
                    let context = fixture_nav_context(&fixture.cache);
                    let items = crate::app::view::nav::tertiary::items_for(&fixture.menu, &context)
                        .expect("imported wallet settings rail");
                    let pair = items
                        .iter()
                        .find(|item| item.label == "Mobile")
                        .expect("imported Pair nav item");
                    let reason = pair
                        .disabled_reason
                        .as_deref()
                        .expect("imported Pair must explain its denial");
                    assert!(
                        reason.contains("unavailable for imported wallets")
                            && reason.contains("no Mobile-eligible key"),
                        "imported Pair denial reason changed: {}",
                        reason
                    );
                }
                if scenario == "pair_idle" || scenario == "data_rows" {
                    let target = if scenario == "pair_idle" {
                        "Pair phone"
                    } else {
                        "Open Backup & protection"
                    };
                    let bounds = labels
                        .0
                        .iter()
                        .find(|(label, _)| label == target)
                        .unwrap_or_else(|| panic!("{}: missing {}", scenario, target))
                        .1;
                    let mut messages = Vec::new();
                    let viewport = iced::Rectangle::with_size(size);
                    for event in [
                        iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left),
                        iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left),
                    ] {
                        element.as_widget_mut().update(
                            &mut tree,
                            &iced::Event::Mouse(event),
                            Layout::new(&node),
                            iced::mouse::Cursor::Available(bounds.center()),
                            &renderer,
                            &mut iced::advanced::clipboard::Null,
                            &mut iced::advanced::Shell::new(&mut messages),
                            &viewport,
                        );
                    }
                    if scenario == "pair_idle" {
                        // A Tenshu action may focus itself before its intent.
                        assert!(matches!(
                            messages.last(),
                            Some(Message::Settings(SettingsMessage::LocalSigning(
                                LocalSigningMessage::StartPairing
                            )))
                        ));
                    } else {
                        assert!(matches!(
                            messages.last(),
                            Some(Message::Menu(Menu::Cube(CubeSubMenu::Settings(
                                CubeSettingsOption::Recovery
                            ))))
                        ));
                    }
                }
            }
        }
    }
}
