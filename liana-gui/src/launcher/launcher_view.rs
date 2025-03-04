use super::launcher::*;

impl Launcher {
    pub fn view(&self) -> Element<Message> {
        let content = self.view_core().map(Message::View);
        self.with_modal(self.with_children(content))
    }

    fn view_core(&self) -> Element<ViewMessage> {
        scrollable(
            Column::new()
                .push(self.view_navigation_bar())
                .push(self.view_body().center_x(Length::Fill))
                .push(Space::with_height(Length::Fixed(100.0))),
        )
        .into()
    }

    fn view_body(&self) -> Container<ViewMessage> {
        Container::new(
            Column::new()
                .align_x(Alignment::Center)
                .spacing(30)
                .push(if matches!(self.state, WalletState::Wallet { .. }) {
                    text("Welcome back").size(50).bold()
                } else {
                    text("Welcome").size(50).bold()
                })
                .push_maybe(self.error.as_ref().map(|e| card::simple(text(e))))
                .push(match &self.state {
                    WalletState::Unchecked => Column::new(),
                    WalletState::Wallet {
                        email, checksum, ..
                    } => self.view_wallet(email, checksum),
                    WalletState::NoWallet => self.view_no_wallet(),
                })
                .max_width(500),
        )
    }

    fn view_wallet(
        &self,
        email: &Option<String>,
        checksum: &Option<String>,
    ) -> Column<ViewMessage> {
        Column::new().push(
            Row::new()
                .align_y(Alignment::Center)
                .spacing(20)
                .push(
                    Container::new(
                        Button::new(
                            Column::new()
                                .push(p1_bold(format!(
                                    "My Liana {} wallet",
                                    match self.network {
                                        Network::Bitcoin => "Bitcoin",
                                        Network::Signet => "Signet",
                                        Network::Testnet => "Testnet",
                                        Network::Regtest => "Regtest",
                                        _ => "",
                                    }
                                )))
                                .push_maybe(checksum.as_ref().map(|checksum| {
                                    p1_regular(format!("Liana-{}", checksum))
                                        .style(theme::text::secondary)
                                }))
                                .push_maybe(email.as_ref().map(|email| {
                                    Row::new()
                                        .push(Space::with_width(Length::Fill))
                                        .push(p1_regular(email).style(theme::text::secondary))
                                })),
                        )
                        .on_press(ViewMessage::Run)
                        .padding(15)
                        .style(theme::button::container_border)
                        .width(Length::Fill),
                    )
                    .style(theme::card::simple),
                )
                .push(
                    Button::new(icon::trash_icon())
                        .style(theme::button::secondary)
                        .padding(10)
                        .on_press(ViewMessage::DeleteWallet(DeleteWalletMessage::ShowModal)),
                ),
        )
    }

    fn view_no_wallet(&self) -> Column<ViewMessage> {
        Column::new()
            .push(
                Row::new()
                    .align_y(Alignment::End)
                    .spacing(20)
                    .push(
                        Container::new(
                            Column::new()
                                .spacing(20)
                                .align_x(Alignment::Center)
                                .push(image::create_new_wallet_icon().width(Length::Fixed(100.0)))
                                .push(
                                    p1_regular("Create a new Liana wallet")
                                        .style(theme::text::secondary),
                                )
                                .push(
                                    button::secondary(None, "Select")
                                        .width(Length::Fixed(200.0))
                                        .on_press(ViewMessage::CreateWallet),
                                )
                                .align_x(Alignment::Center),
                        )
                        .padding(20),
                    )
                    .push(
                        Container::new(
                            Column::new()
                                .spacing(20)
                                .align_x(Alignment::Center)
                                .push(image::restore_wallet_icon().width(Length::Fixed(100.0)))
                                .push(
                                    p1_regular("Add an existing Liana wallet")
                                        .style(theme::text::secondary),
                                )
                                .push(
                                    button::secondary(None, "Select")
                                        .width(Length::Fixed(200.0))
                                        .on_press(ViewMessage::ImportWallet),
                                )
                                .align_x(Alignment::Center),
                        )
                        .padding(20),
                    ),
            )
            .align_x(Alignment::Center)
    }

    fn view_navigation_bar(&self) -> Row<ViewMessage> {
        Row::new()
            .spacing(20)
            .push(
                Container::new(image::liana_brand_grey().width(Length::Fixed(200.0)))
                    .width(Length::Fill),
            )
            .push(button::secondary(None, "Share Xpubs").on_press(ViewMessage::ShareXpubs))
            .push(
                pick_list(
                    &NETWORKS[..],
                    Some(self.network),
                    ViewMessage::SelectNetwork,
                )
                .style(theme::pick_list::primary)
                .padding(10),
            )
            .align_y(Alignment::Center)
            .padding(100)
    }

    fn with_children<'a>(&'a self, content: Element<'a, Message>) -> Element<'a, Message> {
        if self.network != Network::Bitcoin {
            Column::with_children(vec![network_banner(self.network).into(), content]).into()
        } else {
            content
        }
    }

    fn with_modal<'a>(&'a self, content: Element<'a, Message>) -> Element<'a, Message> {
        if let Some(modal) = &self.delete_wallet_modal {
            Modal::new(Container::new(content).height(Length::Fill), modal.view())
                .on_blur(Some(Message::View(ViewMessage::DeleteWallet(
                    DeleteWalletMessage::CloseModal,
                ))))
                .into()
        } else {
            content
        }
    }
}
