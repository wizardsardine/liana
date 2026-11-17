//! Spanish guard: render screens in Spanish and fail when English reaches
//! them (`Inspection::assert_spanish`). Each screen added to Tenshu should get
//! a case here, or call `assert_spanish` from its own capture test.

use coincube_ui::locale::{render_in, Language};
use coincube_ui::theme::Theme;

use crate::app::cache::Cache;
use crate::app::menu::{CubeSubMenu, Menu, SparkSubMenu, VaultSubMenu};
use crate::app::state::preferences::BalancePrivacy;
use crate::app::view::nav::shell::{capture_tests, sidebar, ShellState};
use crate::app::view::nav::NavContext;
use crate::app::view::shared::landing::capture;

fn nav_context(cache: &Cache) -> NavContext<'_> {
    NavContext {
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

#[test]
fn side_panel_is_spanish_with_backup_card_and_connect_open() {
    let cache = capture_tests::cache(false, BalancePrivacy::Shown);
    let ctx = nav_context(&cache);
    let state = ShellState {
        backup_attention: true,
        connect_open: true,
        ..ShellState::default()
    };
    for menu in [
        Menu::Cube(CubeSubMenu::Overview),
        Menu::Vault(VaultSubMenu::Overview),
        Menu::Spark(SparkSubMenu::Overview),
    ] {
        let view = render_in(Language::Spanish, || sidebar(&menu, &ctx, state));
        capture::inspect_full_window("side-panel-es", view, (1440, 900), "dark", Theme::dark())
            .assert_spanish(&format!("side panel at {menu:?}"));
    }
}

#[test]
fn vault_connections_and_data_tabs_are_spanish() {
    use crate::app::view::vault::settings::{connections, import_export};
    let cache = capture_tests::cache(false, BalancePrivacy::Shown);
    let menu = Menu::Vault(VaultSubMenu::Settings(None));

    let view = render_in(Language::Spanish, || connections(&cache));
    capture::inspect(
        "vault-connections-es",
        view,
        (1440, 900),
        "dark",
        Theme::dark(),
    )
    .assert_spanish("Vault › Settings › Connections");

    let view = render_in(Language::Spanish, || import_export(&menu, &cache, true));
    capture::inspect("vault-data-es", view, (1440, 900), "dark", Theme::dark())
        .assert_spanish("Vault › Settings › Data & exports");
}
