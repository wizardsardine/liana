use crate::state::{views::login::LoginState, Msg, State};
use iced::Length;
use liana_ui::{component::text, widget::*};

pub mod account_select;
pub mod code;
pub mod email;

pub fn login_view(state: &State) -> Element<'_, Msg> {
    match state.views.login.current {
        LoginState::AccountSelect => account_select::account_select_view(state),
        LoginState::EmailEntry => email::login_email_view(state),
        LoginState::CodeEntry => code::login_code_view(state),
        LoginState::Authenticated => {
            // Should not reach here
            Container::new(text::h2("Authenticated"))
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into()
        }
    }
}
