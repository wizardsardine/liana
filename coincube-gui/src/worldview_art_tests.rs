use coincube_ui::component::{
    amount::BitcoinDisplayUnit,
    payment_result::{payment_result, Outcome},
    quote_display,
};
use coincube_ui::illustration::Slot;
use coincube_ui::image;
use coincube_ui::theme::{ArtworkStyle, Theme, WorldviewMode};

use crate::app::view::settings::page::illustrated_flow;
use crate::app::view::shared::landing::capture;
use crate::app::view::spark::{SparkTransactionsStatus, SparkTransactionsView};
use crate::chain::ChainId;
use crate::home::launcher_presenter::{opening_body, project_opening};
use crate::home::restore_frame::done_art;

#[test]
fn worldview_art_fits_existing_views() {
    check_worldview_art(ArtworkStyle::Existing, "existing");
}

#[test]
fn worldview_art_fits_full_scene_views() {
    check_worldview_art(ArtworkStyle::Scene, "scene");
}

fn check_worldview_art(artwork: ArtworkStyle, art_label: &str) {
    let loading = quote_display::image_handle_for_context("loading");
    let empty = quote_display::image_handle_for_context("empty-wallet");
    let loading_quote = quote_display::random_quote("loading");
    let empty_quote = quote_display::random_quote("empty-wallet");
    let startup_error = crate::loader::Step::Error(Box::new(crate::loader::Error::Unexpected(
        "Fixture startup error".into(),
    )));
    let error_quote = quote_display::random_quote("error");
    let error_image = quote_display::image_handle_for_context("error");
    let sent_quote = quote_display::random_quote("bitcoin-send");
    let sent_image = quote_display::image_handle_for_context("bitcoin-send");
    let received_quote = quote_display::random_quote("spark-receive");
    let received_image = quote_display::image_handle_for_context("spark-receive");
    for (mode, name) in [
        (WorldviewMode::Samurai, "samurai"),
        (WorldviewMode::Suitcoiner, "suitcoiner"),
        (WorldviewMode::Luke, "luke"),
    ] {
        for (appearance, theme) in [("dark", Theme::dark()), ("light", Theme::light())] {
            let theme = theme.with_worldview(mode).with_artwork(artwork);
            let opened = capture::inspect_filled(
                &format!("worldview-{name}-{art_label}-opening"),
                opening_body(
                    project_opening("Everyday", ChainId::Bitcoin),
                    &crate::loader::Step::Connecting,
                    &loading_quote,
                    &loading,
                    false,
                ),
                (1024, 768),
                appearance,
                theme,
                (1024, 768),
            );
            assert!(opened.contains("Connecting to daemon"));
            let failed = capture::inspect_filled(
                &format!("worldview-{name}-{art_label}-startup-error"),
                crate::loader::view(&startup_error, &error_quote, &error_image),
                (1024, 768),
                appearance,
                theme,
                (1024, 768),
            );
            assert!(failed.contains("Tenshu failed to start"));
            let history = capture::inspect(
                &format!("worldview-{name}-{art_label}-empty-history"),
                SparkTransactionsView {
                    status: SparkTransactionsStatus::Loaded(vec![]),
                    recent_transactions: &[],
                    fiat_converter: None,
                    bitcoin_unit: BitcoinDisplayUnit::Sats,
                    show_direction_badges: false,
                    empty_state_quote: &empty_quote,
                    empty_state_image_handle: &empty,
                    current_page: 0,
                    is_last_page: true,
                    processing: false,
                    mask_amounts: true,
                }
                .render(),
                (1024, 768),
                appearance,
                theme,
            );
            assert!(history.contains("Your Spark wallet is ready"));

            use coincube_ui::worldview_copy::{self, VoiceLine};
            let sent_line = VoiceLine::PaymentSent.for_mode(mode);
            let received_line = VoiceLine::PaymentReceived.for_mode(mode);
            for (screen, view, heading) in [
                (
                    "sent",
                    payment_result(
                        Outcome::Sent,
                        "50,000 sats",
                        "bitcoin-send",
                        &sent_quote,
                        &sent_image,
                        (),
                    )
                    .voice(sent_line)
                    .into(),
                    "Payment sent",
                ),
                (
                    "received",
                    worldview_copy::render_in(mode, || {
                        payment_result(
                            Outcome::Received,
                            "50,000 sats",
                            "spark-receive",
                            &received_quote,
                            &received_image,
                            (),
                        )
                        .voice(received_line)
                        .into()
                    }),
                    "Payment received",
                ),
                ("restored", done_art(appearance == "light"), ""),
                (
                    "protected",
                    illustrated_flow(
                        "Protect your Cube",
                        "Set up a Recovery Kit.",
                        image::recovery_kit_protected(),
                        Slot::KitProtection,
                    )
                    .into(),
                    "Protect your Cube",
                ),
                (
                    "withdrawn",
                    illustrated_flow(
                        "Remove Recovery Kit",
                        "Confirm removal.",
                        image::recovery_kit_withdrawn(),
                        Slot::KitRemoval,
                    )
                    .into(),
                    "Remove Recovery Kit",
                ),
                (
                    "pin",
                    illustrated_flow(
                        "Unlock your seed phrase",
                        "Enter your Cube PIN.",
                        image::pin_unlock_lock(),
                        Slot::Pin,
                    )
                    .into(),
                    "Unlock your seed phrase",
                ),
            ] {
                let rendered = capture::inspect(
                    &format!("worldview-{name}-{art_label}-{screen}"),
                    view,
                    (1024, 768),
                    appearance,
                    theme,
                );
                if !heading.is_empty() {
                    assert!(rendered.contains(heading));
                }
                if let Some(line) = match screen {
                    "sent" => sent_line,
                    "received" => received_line,
                    _ => None,
                } {
                    assert!(rendered.contains(line));
                }
            }
        }
    }
}

#[test]
fn spanish_samurai_wallet_actions_and_errors_render_together() {
    use coincube_ui::component::notification;
    use coincube_ui::locale::{render_in, Language};

    let quote = quote_display::random_quote("empty-wallet");
    let image = quote_display::image_handle_for_context("empty-wallet");
    let error = crate::user_error::UserError::new(
        "That fee rate isn't valid",
        "Enter a whole number of sats per vByte greater than zero.",
        "CC-XFER-INPUT",
        false,
    );
    let view = render_in(Language::Spanish, || {
        SparkTransactionsView {
            status: SparkTransactionsStatus::Loaded(vec![]),
            recent_transactions: &[],
            fiat_converter: None,
            bitcoin_unit: BitcoinDisplayUnit::Sats,
            show_direction_badges: false,
            empty_state_quote: &quote,
            empty_state_image_handle: &image,
            current_page: 0,
            is_last_page: true,
            processing: false,
            mask_amounts: true,
        }
        .render()
    });
    let activity = capture::inspect("spark-history-es", view, (1024, 768), "dark", Theme::dark());
    assert!(activity.contains("Tu cartera Spark está lista"));
    assert!(activity.contains("Enviar"));
    assert!(activity.contains("Recibir sats"));

    let error_card: coincube_ui::widget::Element<'_, ()> = render_in(Language::Spanish, || {
        notification::error_card(error.title, error.guidance, error.reference, None).into()
    });
    let rendered = capture::inspect(
        "transfer-error-es",
        error_card,
        (1024, 768),
        "dark",
        Theme::dark(),
    );
    assert!(rendered.contains("La tasa de comisión no es válida"));
    assert!(rendered.contains("Ingresa un número entero de sats"));
    assert!(rendered.contains("CC-XFER-INPUT"));
}
