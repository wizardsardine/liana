use crate::component::text::P1_SIZE;
use crate::widget::Text;
use iced::{alignment, Font, Length};

const BOOTSTRAP_ICONS: Font = Font::with_name("bootstrap-icons");
const COINCUBE_ICONS: Font = Font::with_name("coincube-icons");

fn bootstrap_icon<'a>(unicode: char) -> Text<'a> {
    Text::new(unicode)
        .font(BOOTSTRAP_ICONS)
        .width(Length::Fixed(20.0))
        .align_x(alignment::Horizontal::Center)
        .size(P1_SIZE)
}

pub fn cross_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F62A}')
}

pub fn arrow_down<'a>() -> Text<'a> {
    bootstrap_icon('\u{F128}')
}

pub fn arrow_back<'a>() -> Text<'a> {
    bootstrap_icon('\u{F12E}')
}

pub fn arrow_right<'a>() -> Text<'a> {
    bootstrap_icon('\u{F138}')
}

pub fn arrow_return_right<'a>() -> Text<'a> {
    bootstrap_icon('\u{F132}')
}

pub fn chevron_right<'a>() -> Text<'a> {
    bootstrap_icon('\u{F285}')
}

pub fn recovery_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F467}')
}

/// Bootstrap `diagram-2`: the Bitcoin path branching into Bitcoin and BTCB2.
pub fn claim_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F2EC}')
}

pub fn plug_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F4F6}')
}

pub fn reload_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F130}')
}

pub fn import_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F30A}')
}

pub fn wallet_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F615}')
}

pub fn card_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F2D9}')
}

pub fn bitcoin_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F635}')
}

pub fn dollar_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F636}')
}

pub fn globe_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F91B}')
}

pub fn block_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F1C8}')
}

pub fn dot_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F287}')
}

pub fn email_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F7BE}')
}

pub fn person_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F4DA}')
}

pub fn tooltip_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F431}')
}

pub fn plus_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F4FE}')
}

pub fn warning_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F33B}')
}

pub fn chip_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F2D6}')
}

pub fn trash_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F5DE}')
}

pub fn pencil_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F4CB}')
}

pub fn collapse_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F285}')
}

pub fn collapsed_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F282}')
}

pub fn down_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F279}')
}

pub fn up_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F27C}')
}

pub fn left_right_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F12B}')
}

pub fn up_down_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F127}')
}

pub fn network_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F40D}')
}

pub fn previous_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F284}')
}

pub fn check_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F633}')
}

pub fn check2_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F272}')
}

pub fn round_key_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F44E}')
}

pub fn backup_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F356}')
}

pub fn restore_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F358}')
}

pub fn wrench_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F621}')
}

pub fn link_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F470}')
}

pub fn paste_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F290}')
}

pub fn usb_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F6DC}')
}

pub fn usb_drive_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F6F2}')
}

pub fn hdd_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F412}')
}

pub fn enter_box_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F1BE}')
}

pub fn qr_code_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F6AE}')
}

pub fn arrow_repeat<'a>() -> Text<'a> {
    bootstrap_icon('\u{F130}')
}

pub fn send_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F603}')
}

pub fn receive_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F30A}')
}

pub fn home_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F3FC}')
}

pub fn settings_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F3E5}')
}

pub fn key_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F44F}')
}

pub fn history_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F479}')
}

pub fn escape_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F7EE}')
}

pub fn coins_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F585}')
}

/// Outline stacked `layers` glyph — the outline counterpart used for the Coins
/// sidebar item, since the filled [`coins_icon`] (`stack`) has no outline twin.
pub fn coins_outline_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F45B}')
}

pub fn clock_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F293}')
}

pub fn clipboard_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F290}')
}

pub fn square_check_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F26D}')
}

pub fn square_cross_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F629}')
}

pub fn building_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F87D}')
}

pub fn vault_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F53F}')
}

/// Spark's asterisk (`static/logos/spark_badge.svg`): the Spark wallet's mark.
pub fn spark_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{E000}').font(COINCUBE_ICONS)
}

pub fn lightning_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F46E}')
}

/// Outline (non-filled) lightning bolt — the `lightning` twin of the filled
/// [`lightning_icon`]. Used in the sidebar nav so all rail icons read as outline.
pub fn lightning_outline_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F46F}')
}

pub fn droplet_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F30C}')
}

pub fn eye_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F33E}')
}

pub fn eye_outline_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F341}')
}

pub fn eye_slash_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F33F}')
}

pub fn cube_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F1C8}')
}

/// Outline (non-filled) cube — paired with [`cube_icon`] to distinguish a
/// Sovereign (local-only) Cube from a Connect-registered one in the tri-state
/// Cube indicator.
pub fn cube_outline_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F1C7}')
}

pub fn receipt_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F50F}')
}

pub fn arrow_down_up_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F127}')
}

pub fn arrow_left_right_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F12B}')
}

pub fn cash_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F247}')
}

pub fn lock_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F47B}')
}

pub fn file_earmark_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F373}')
}

pub fn cloud_arrow_down_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F295}')
}

pub fn check_circle_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F26B}')
}

pub fn phone_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F4E7}')
}

pub fn chain_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F470}')
}

pub fn invoice_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F86F}')
}

pub fn star_fill_icon() -> Text<'static> {
    bootstrap_icon('\u{F586}')
}

pub fn calendar_icon() -> Text<'static> {
    bootstrap_icon('\u{F1F4}')
}

pub fn chat_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F268}')
}

pub fn usd_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F636}')
}

pub fn pen_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F4CA}')
}

pub fn graph_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F3F2}')
}

pub fn connect_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F2EE}')
}

pub fn shop_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F638}')
}

pub fn sun_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F5A2}')
}

pub fn moon_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F497}')
}

pub fn shield_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F53F}')
}

pub fn shield_plus_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F53A}')
}

pub fn cloud_check_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F299}')
}

pub fn cloud_slash_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F2B8}')
}

pub fn grid_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F3FA}')
}

pub fn shuffle_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F544}')
}

pub fn search_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F52A}')
}

pub fn display_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F302}')
}

pub fn exclamation_circle_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F333}')
}

pub fn person_outline_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F4E1}')
}

pub fn people_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F4D0}')
}

pub fn laptop_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F456}')
}

pub fn shield_check_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F52F}')
}

pub fn bell_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F18A}')
}

pub fn credit_card_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F2DC}')
}

pub fn dash_circle_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F2E6}')
}

pub fn question_circle_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F505}')
}

pub fn x_circle_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F623}')
}

pub fn circle_half_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F288}')
}

pub fn code_slash_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F2C6}')
}

pub fn activity_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F66B}')
}

pub fn folder_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F3D7}')
}

pub fn copy_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F759}')
}

/// Share (`box-arrow-up`): an arrow leaving a tray.
pub fn share_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F1C6}')
}

pub fn list_ul_icon<'a>() -> Text<'a> {
    bootstrap_icon('\u{F478}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coincube_icons_is_loaded_under_the_name_spark_icon_asks_for() {
        // iced falls back to another font, without an error, when its font
        // database skips a font or no face has the family name a
        // `Font::with_name` asks for. Load the bytes into that database.
        use iced::advanced::graphics::text::cosmic_text::{self, fontdb};
        let bytes = crate::font::COINCUBE_ICONS_BYTES;
        assert!(crate::font::load()
            .iter()
            .any(|font| font.as_ref() == bytes));
        let mut db = fontdb::Database::new();
        db.load_font_data(bytes.to_vec());
        let face = db
            .query(&fontdb::Query {
                families: &[fontdb::Family::Name("coincube-icons")],
                ..fontdb::Query::default()
            })
            .and_then(|id| db.face(id))
            .expect("coincube-icons.ttf loads as the coincube-icons family");
        assert_eq!(COINCUBE_ICONS, Font::with_name("coincube-icons"));
        let font = cosmic_text::Font::new(&db, face.id, face.weight).expect("shapeable font");
        assert_ne!(
            font.as_swash().charmap().map('\u{E000}'),
            0,
            "{}",
            "U+E000 maps to the Spark glyph"
        );
    }
}
