use std::sync::{Arc, Mutex};

use coincube_core::signer::HotSigner;
use coincube_ui::{component::text::*, widget::*};
use coincube_ui::{
    icon,
    theme::{self},
};
use iced::Alignment;
use iced::{widget::container, widget::Column, widget::Space, Length};

use crate::app::state::BackupWalletStep;
use crate::app::view::message::{BackupWalletMessage, Message};

pub fn active_settings_view<'a>(
    active_signer: Arc<Mutex<HotSigner>>,
    backup_step: &'a BackupWalletStep,
) -> Element<'a, Message> {
    match backup_step {
        BackupWalletStep::MainMenu { backed_up, mfa } => main_menu_view(*backed_up, *mfa),
        BackupWalletStep::Intro(checked) => backup_intro_view(*checked),
        BackupWalletStep::RecoveryPhrase => {
            recovery_phrase_view(active_signer.lock().expect("Mutex Lock Poisoned").words())
        }
        BackupWalletStep::Verification {
            word_2,
            word_5,
            word_9,
            error,
        } => verification_view(word_2, word_5, word_9, error.as_deref()),
        BackupWalletStep::Completed => completed_view(),
    }
}

fn main_menu_view(backed_up: bool, mfa: bool) -> Element<'static, Message> {
    let backup = settings_section(
        "Back up your wallet",
        "Protect your wallet by creating and safely storing a recovery phrase.",
        icon::lock_icon(),
        icon::arrow_right(),
        if !backed_up {
            CapsuleState::Danger
        } else {
            CapsuleState::Success
        },
        if !backed_up {
            icon::warning_icon()
        } else {
            icon::check_icon()
        },
        if !backed_up {
            "Not backed up"
        } else {
            "Completed"
        },
        Message::ActiveSettings(BackupWalletMessage::Start),
    );

    let mfa = settings_section(
        "Two-factor authentication method",
        "Manage your two-factor authentication settings to enhance account security.",
        icon::phone(),
        icon::arrow_right(),
        if !mfa {
            CapsuleState::Danger
        } else {
            CapsuleState::Success
        },
        if !mfa {
            icon::warning_icon()
        } else {
            icon::check_icon()
        },
        if !mfa { "Disabled" } else { "Completed" },
        Message::Settings(crate::app::view::SettingsMessage::GeneralSection),
    );

    Column::new()
        .spacing(20)
        .width(Length::Fill)
        .push(h3("Settings"))
        .push(Space::with_height(Length::Fixed(20.0)))
        .push(backup)
        .push(mfa)
        .into()
}

fn backup_intro_view(checked: bool) -> Element<'static, Message> {
    use coincube_ui::color;
    use coincube_ui::theme;
    use coincube_ui::widget::{CheckBox, Column, Container, Row, Text};
    use iced::{widget::Space, Alignment, Length};
    let primary_color = color::ORANGE;
    Column::new()
        .spacing(20)
        .width(Length::Fill)
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Start)
                .push(
                    coincube_ui::component::button::secondary(Some(icon::chevron_left().size(24)), "PREVIOUS")
                        .on_press(Message::ActiveSettings(BackupWalletMessage::PreviousStep))
                        .style(theme::button::transparent)
                )
                .push(Space::with_width(Length::Fill))
        )
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::with_width(Length::Fill))
                .push(
                    Column::new()
                        .width(Length::Shrink)
                        .align_x(Alignment::Center)
                        .push(h3("Backup Phrase"))
                )
                .push(Space::with_width(Length::Fill))
        )
        .push(Space::with_height(Length::Fixed(16.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::with_width(Length::Fill))
                .push(
                    icon::file_earmark().size(140).color(primary_color)
                )
                .push(Space::with_width(Length::Fill))
        )
        .push(Space::with_height(Length::Fixed(16.0)))
        .push(
            Container::new(
                Column::new()
                    .align_x(Alignment::Center)
                    .push(
                        Text::new("You will be shown 12 words. Write them down and keep them in a safe place.")
                            .size(20)
                            .align_x(iced::alignment::Horizontal::Center)
                    )
                    .push(
                        Text::new("Do not share these words with anyone.")
                            .size(20).bold().color(primary_color)
                            .align_x(iced::alignment::Horizontal::Center)
                    )
                    .push(
                        Text::new("Without them, you will not be able to restore your wallet if you lose your phone.")
                            .size(20)
                            .align_x(iced::alignment::Horizontal::Center)
                    )
            )
            .padding(20)
            .width(Length::Fill)
            .align_x(iced::alignment::Horizontal::Center)
        )
        .push(Space::with_height(Length::Fixed(16.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::with_width(Length::Fill))
                .push(
                    CheckBox::new(
                        "I UNDERSTAND THAT IF I LOSE THESE WORDS, MY FUNDS CANNOT BE RECOVERED",
                        checked
                    )
                    .on_toggle(|_| Message::ActiveSettings(BackupWalletMessage::ToggleBackupIntroCheck))
                    .style(theme::checkbox::primary).size(20)
                )
                .push(Space::with_width(Length::Fill))
        )
        .push(Space::with_height(Length::Fixed(16.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::with_width(Length::Fill))
                .push({
                    let btn: Element<'static, Message> = if checked {
                        coincube_ui::component::button::primary(None, "Show My Recovery Phrase")
                            .on_press(Message::ActiveSettings(BackupWalletMessage::NextStep))
                            .padding([8, 16])
                            .width(Length::Fixed(300.0))
                            .into()
                    } else {
                        coincube_ui::component::button::primary(None, "Show My Recovery Phrase")
                            .padding([8, 16])
                            .width(Length::Fixed(300.0))
                            .into()
                    };
                    btn
                })
                .push(Space::with_width(Length::Fill))
        )
        .into()
}

fn recovery_phrase_view(mnemonic: [&'static str; 12]) -> Element<'static, Message> {
    use coincube_ui::widget::{Container, Row, Text};

    // Create the mnemonic grid (3 rows x 4 columns)
    let mut grid = Column::new().spacing(30).align_x(Alignment::Center);

    for row in 0..3 {
        let mut row_widget = Row::new().spacing(40).align_y(Alignment::Center);

        for col in 0..4 {
            let index = row * 4 + col;
            let word = mnemonic[index];

            let word_container = Container::new(
                Text::new(format!("{}. {}", index + 1, word))
                    .size(16)
                    .align_x(iced::alignment::Horizontal::Center),
            )
            .padding(12)
            .width(Length::Fixed(150.0))
            .align_x(iced::alignment::Horizontal::Center)
            .style(|_theme| container::Style {
                border: iced::Border {
                    color: iced::Color::from_rgb8(0x80, 0x80, 0x80),
                    width: 1.0,
                    radius: 10.0.into(),
                },
                ..Default::default()
            });

            row_widget = row_widget.push(word_container);
        }

        grid = grid.push(row_widget);
    }

    Column::new()
        .spacing(20)
        .width(Length::Fill)
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Start)
                .push(
                    coincube_ui::component::button::secondary(Some(icon::chevron_left().size(24)), "PREVIOUS")
                        .on_press(Message::ActiveSettings(BackupWalletMessage::PreviousStep))
                        .style(theme::button::transparent)
                )
                .push(Space::with_width(Length::Fill))
        )
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::with_width(Length::Fill))
                .push(
                    Column::new()
                        .width(Length::Shrink)
                        .align_x(Alignment::Center)
                        .push(h3("Your Recovery Phrase"))
                )
                .push(Space::with_width(Length::Fill))
        )
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::with_width(Length::Fill))
                .push(
                    Container::new(
                        Text::new("Write these words down in order and keep them offline. Anyone with these words can access your wallet.")
                            .size(20)
                            .align_x(iced::alignment::Horizontal::Center)
                    )
                    .width(Length::Fixed(700.0))
                    .align_x(iced::alignment::Horizontal::Center)
                )
                .push(Space::with_width(Length::Fill))
        )
        .push(Space::with_height(Length::Fixed(24.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::with_width(Length::Fill))
                .push(grid)
                .push(Space::with_width(Length::Fill))
        )
        .push(Space::with_height(Length::Fixed(24.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::with_width(Length::Fill))
                .push(
                    coincube_ui::component::button::primary(None, "I've Written It Down")
                        .on_press(Message::ActiveSettings(BackupWalletMessage::NextStep))
                        .padding([8, 16])
                        .width(Length::Fixed(300.0))
                )
                .push(Space::with_width(Length::Fill))
        )
        .into()
}

fn verification_view<'a>(
    word_2: &'a str,
    word_5: &'a str,
    word_9: &'a str,
    error: Option<&'a str>,
) -> Element<'a, Message> {
    use coincube_ui::widget::{Container, Row, Text, TextInput};

    let all_filled = !word_2.is_empty() && !word_5.is_empty() && !word_9.is_empty();

    let mut content = Column::new().spacing(20).width(Length::Fill);

    // Previous button
    content = content.push(
        Row::new()
            .width(Length::Fill)
            .align_y(Alignment::Start)
            .push(
                coincube_ui::component::button::secondary(
                    Some(icon::chevron_left().size(24)),
                    "PREVIOUS",
                )
                .on_press(Message::ActiveSettings(BackupWalletMessage::PreviousStep))
                .style(theme::button::transparent),
            )
            .push(Space::with_width(Length::Fill)),
    );

    // Heading
    content = content.push(
        Row::new()
            .width(Length::Fill)
            .align_y(Alignment::Center)
            .push(Space::with_width(Length::Fill))
            .push(
                Column::new()
                    .width(Length::Shrink)
                    .align_x(Alignment::Center)
                    .push(h3("Verify Your Recovery Phrase")),
            )
            .push(Space::with_width(Length::Fill)),
    );

    // Subheading
    content = content.push(
        Row::new()
            .width(Length::Fill)
            .align_y(Alignment::Center)
            .push(Space::with_width(Length::Fill))
            .push(
                Text::new("To make sure you saved your recovery phrase correctly, please enter words 2, 5, and 9.")
                    .size(20)
                    .align_x(iced::alignment::Horizontal::Center)
                    .width(Length::Fixed(700.0))
            )
            .push(Space::with_width(Length::Fill))
    );

    content = content.push(Space::with_height(Length::Fixed(24.0)));

    // Error message if verification failed
    if let Some(err) = error {
        content =
            content.push(
                Row::new()
                    .width(Length::Fill)
                    .align_y(Alignment::Center)
                    .push(Space::with_width(Length::Fill))
                    .push(
                        Container::new(Text::new(err).size(16).style(|_| {
                            iced::widget::text::Style {
                                color: Some(iced::Color::from_rgb8(0xDD, 0x02, 0x02)),
                            }
                        }))
                        .padding(12)
                        .style(|_theme| container::Style {
                            background: Some(iced::Background::Color(iced::Color::from_rgb8(
                                0x4c, 0x01, 0x01,
                            ))),
                            border: iced::Border {
                                radius: 8.0.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }),
                    )
                    .push(Space::with_width(Length::Fill)),
            );
        content = content.push(Space::with_height(Length::Fixed(16.0)));
    }

    // Custom text input style with no border
    let no_border_style = |theme: &coincube_ui::theme::Theme,
                           status: iced::widget::text_input::Status| {
        let default_style = theme::text_input::primary(theme, status);
        iced::widget::text_input::Style {
            border: iced::Border {
                width: 0.0,
                ..default_style.border
            },
            ..default_style
        }
    };

    // Input fields with bottom border
    let input_fields = Column::new()
        .spacing(40)
        .align_x(Alignment::Center)
        .push(
            Column::new()
                .push(
                    Row::new()
                        .spacing(12)
                        .align_y(Alignment::Center)
                        .push(Text::new("2.").size(18))
                        .push(
                            TextInput::new("", word_2)
                                .on_input(|input| {
                                    Message::ActiveSettings(BackupWalletMessage::Word2Input(input))
                                })
                                .padding(8)
                                .width(Length::Fixed(300.0))
                                .style(no_border_style),
                        ),
                )
                .push(
                    Container::new(Space::with_height(Length::Fixed(0.0)))
                        .width(Length::Fixed(340.0))
                        .height(Length::Fixed(1.0))
                        .style(|_theme| container::Style {
                            background: Some(iced::Background::Color(iced::Color::from_rgb8(
                                0x80, 0x80, 0x80,
                            ))),
                            ..Default::default()
                        }),
                ),
        )
        .push(
            Column::new()
                .push(
                    Row::new()
                        .spacing(12)
                        .align_y(Alignment::Center)
                        .push(Text::new("5.").size(18))
                        .push(
                            TextInput::new("", word_5)
                                .on_input(|input| {
                                    Message::ActiveSettings(BackupWalletMessage::Word5Input(input))
                                })
                                .padding(8)
                                .width(Length::Fixed(300.0))
                                .style(no_border_style),
                        ),
                )
                .push(
                    Container::new(Space::with_height(Length::Fixed(0.0)))
                        .width(Length::Fixed(340.0))
                        .height(Length::Fixed(1.0))
                        .style(|_theme| container::Style {
                            background: Some(iced::Background::Color(iced::Color::from_rgb8(
                                0x80, 0x80, 0x80,
                            ))),
                            ..Default::default()
                        }),
                ),
        )
        .push(
            Column::new()
                .push(
                    Row::new()
                        .spacing(12)
                        .align_y(Alignment::Center)
                        .push(Text::new("9.").size(18))
                        .push(
                            TextInput::new("", word_9)
                                .on_input(|input| {
                                    Message::ActiveSettings(BackupWalletMessage::Word9Input(input))
                                })
                                .padding(8)
                                .width(Length::Fixed(300.0))
                                .style(no_border_style),
                        ),
                )
                .push(
                    Container::new(Space::with_height(Length::Fixed(0.0)))
                        .width(Length::Fixed(340.0))
                        .height(Length::Fixed(1.0))
                        .style(|_theme| container::Style {
                            background: Some(iced::Background::Color(iced::Color::from_rgb8(
                                0x80, 0x80, 0x80,
                            ))),
                            ..Default::default()
                        }),
                ),
        );

    content = content.push(
        Row::new()
            .width(Length::Fill)
            .align_y(Alignment::Center)
            .push(Space::with_width(Length::Fill))
            .push(input_fields)
            .push(Space::with_width(Length::Fill)),
    );

    content = content.push(Space::with_height(Length::Fixed(24.0)));

    // Verify button
    content = content.push(
        Row::new()
            .width(Length::Fill)
            .align_y(Alignment::Center)
            .push(Space::with_width(Length::Fill))
            .push(if all_filled {
                coincube_ui::component::button::primary(None, "Verify")
                    .on_press(Message::ActiveSettings(BackupWalletMessage::VerifyPhrase))
                    .padding([8, 16])
                    .width(Length::Fixed(300.0))
            } else {
                coincube_ui::component::button::primary(None, "Verify")
                    .padding([8, 16])
                    .width(Length::Fixed(300.0))
            })
            .push(Space::with_width(Length::Fill)),
    );

    content.into()
}

fn completed_view() -> Element<'static, Message> {
    use coincube_ui::widget::{Column, Row, Text};

    let primary_color = coincube_ui::color::ORANGE;

    Column::new()
        .spacing(20)
        .width(Length::Fill)
        .push(Space::with_height(Length::Fixed(20.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::with_width(Length::Fill))
                .push(
                    icon::check_circle().size(140).color(primary_color)
                )
                .push(Space::with_width(Length::Fill))
        )
        .push(Space::with_height(Length::Fixed(16.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::with_width(Length::Fill))
                .push(
                    Column::new()
                        .width(Length::Shrink)
                        .align_x(Alignment::Center)
                        .push(h3("Backup Complete"))
                )
                .push(Space::with_width(Length::Fill))
        )
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::with_width(Length::Fill))
                .push(
                    Text::new("Your recovery phrase has been securely backed up. Keep it safe. It's the only way to restore your wallet.")
                        .size(20)
                        .align_x(iced::alignment::Horizontal::Center)
                        .width(Length::Fixed(700.0))
                )
                .push(Space::with_width(Length::Fill))
        )
        .push(Space::with_height(Length::Fixed(24.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::with_width(Length::Fill))
                .push(
                    coincube_ui::component::button::primary(None, "Back to Settings")
                        .on_press(Message::ActiveSettings(BackupWalletMessage::Complete))
                        .padding([8, 16])
                        .width(Length::Fixed(300.0))
                )
                .push(Space::with_width(Length::Fill))
        )
        .into()
}

#[derive(Clone, Copy)]
pub enum CapsuleState {
    Danger,
    Success,
}

fn settings_section(
    title: &str,
    subtitle: &str,
    icon: coincube_ui::widget::Text<'static>,
    right_icon: coincube_ui::widget::Text<'static>,
    capsule_state: CapsuleState,
    capsule_icon: coincube_ui::widget::Text<'static>,
    capsule_text: &str,
    msg: Message,
) -> Container<'static, Message> {
    Container::new(
        Button::new(
            Row::new()
                .push(icon)
                .push(
                    Column::new()
                        .push(
                            Row::new()
                                .push(text(title).bold())
                                .push({
                                    let (bg, fg) = match capsule_state {
                                        CapsuleState::Danger => (
                                            iced::Color::from_rgb8(0x4c, 0x01, 0x01),
                                            iced::Color::from_rgb8(0xDD, 0x02, 0x02),
                                        ),
                                        CapsuleState::Success => (
                                            iced::Color::from_rgb8(0x01, 0x4c, 0x14),
                                            iced::Color::from_rgb8(0x00, 0xC3, 0x32),
                                        ),
                                    };
                                    Container::new(
                                        Row::new()
                                            .push(capsule_icon.size(14).style(move |_| {
                                                iced::widget::text::Style { color: Some(fg) }
                                            }))
                                            .push(text(capsule_text).bold().size(14).style(
                                                move |_| iced::widget::text::Style {
                                                    color: Some(fg),
                                                },
                                            ))
                                            .spacing(4),
                                    )
                                    .padding([2, 8])
                                    .style(move |_| {
                                        iced::widget::container::Style {
                                            background: Some(iced::Background::Color(bg)),
                                            border: iced::Border {
                                                radius: 12.0.into(),
                                                ..Default::default()
                                            },
                                            ..Default::default()
                                        }
                                    })
                                })
                                .spacing(8),
                        )
                        .push(text(subtitle).small())
                        .spacing(2)
                        .align_x(Alignment::Start),
                )
                .push(Space::with_width(Length::Fill))
                .push(right_icon)
                .padding(18)
                .spacing(20)
                .align_y(Alignment::Center)
                .width(Length::Fill),
        )
        .width(Length::Fill)
        .style(theme::button::transparent_border)
        .on_press(msg),
    )
    .width(Length::Fill)
    .style(theme::card::simple)
}
