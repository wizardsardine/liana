//! AI access presentation. Authorization and pairing state stay in `local_mcp`;
//! every control here maps to one of its existing messages.
//!
//! The page follows the Settings artboard: a "This Cube" group holding the
//! Cube-wide switch and the Vault clients may use, the paired clients as ruled
//! rows with their pairing form beneath, then what each kind of access allows.
//! Read access, wallet preparation and unsigned transaction preparation are
//! separate pairings, so the form offers them as one explicit choice.

use super::{pairings, Message, Settings};
use crate::app::view;
use crate::app::view::settings::page;
#[cfg(feature = "mcp-transactions")]
use coincube_mcp::transaction::Prepared;
use coincube_ui::component::tenshu::{radio_list, segment, text_field, toggle};
use coincube_ui::font;
use coincube_ui::locale::copy as localized_copy;
use coincube_ui::localized_format;
use coincube_ui::theme::{self, tenshu::Tone};
use coincube_ui::widget::{action::Action, Element};
use iced::widget::{column, container, row, text, Column, Space};
use iced::{Background, Border, Length};
use std::sync::atomic::Ordering;

type Ui<'a> = Element<'a, view::Message>;

fn message(value: Message) -> view::Message {
    view::Message::LocalMcp(value)
}

fn muted<'a>(
    value: impl iced::widget::text::IntoFragment<'a>,
) -> iced::widget::Text<'a, theme::Theme> {
    text(value).size(13).style(theme::text::secondary)
}

fn heading<'a>(
    value: impl iced::widget::text::IntoFragment<'a>,
) -> iced::widget::Text<'a, theme::Theme> {
    text(value).size(16).font(font::SEMIBOLD)
}

/// A 14px semibold label over a part of the pairing form.
fn subheading<'a>(
    value: impl iced::widget::text::IntoFragment<'a>,
) -> iced::widget::Text<'a, theme::Theme> {
    text(value).size(14).font(font::SEMIBOLD)
}

fn card<'a>(content: impl Into<Ui<'a>>) -> Ui<'a> {
    container(content)
        .padding([16, 18])
        .width(Length::Fill)
        .style(|theme: &theme::Theme| iced::widget::container::Style {
            text_color: Some(theme.colors.shell.ink),
            background: Some(Background::Color(theme.colors.shell.panel)),
            border: Border {
                color: theme.colors.shell.line,
                width: 1.0,
                radius: 10.0.into(),
            },
            ..Default::default()
        })
        .into()
}

fn action<'a>(
    id: impl Into<iced::advanced::widget::Id>,
    label: &'static str,
    intent: Option<view::Message>,
    kind: theme::tenshu::button::ActionKind,
) -> Ui<'a> {
    use theme::tenshu::button::{action, ActionShape};
    Action::new(
        id,
        text(localized_copy(label)).size(13).font(font::MEDIUM),
        view::Message::FocusAction,
    )
    .padding([8, 12])
    .focus_radius(8.0)
    .style(move |theme, status| action(theme, status, kind, ActionShape::Compact))
    .on_press_maybe(intent)
    .into()
}

/// The exact chain of this Cube's pairing registry, when one is known.
fn cube_chain(settings: &Settings) -> Option<crate::chain::ChainId> {
    settings
        .pairing_location
        .as_ref()
        .and_then(pairings::Location::chain)
}

/// The five exact chains local MCP documents and supports; the same set as
/// `coincube_mcp::Chain` and `transactions::supported_chain`, kept here so the
/// page needs no optional feature.
pub(super) fn chain_supported(chain: crate::chain::ChainId) -> bool {
    matches!(
        chain,
        crate::chain::ChainId::Bitcoin
            | crate::chain::ChainId::Testnet4
            | crate::chain::ChainId::Regtest
            | crate::chain::ChainId::BitcoinBlake2b
            | crate::chain::ChainId::BitcoinBlake2bTestnet4
    )
}

/// The scope tag naming an exact supported chain. Bitcoin and Bitcoin Blake2b
/// share address and key encodings, so the tag is never inferred from either.
pub(super) fn chain_tag(chain: Option<crate::chain::ChainId>) -> &'static str {
    match chain {
        Some(crate::chain::ChainId::Bitcoin) => "BTC · MAINNET",
        Some(crate::chain::ChainId::Testnet4) => "BTC · TESTNET4",
        Some(crate::chain::ChainId::Regtest) => "REGTEST",
        Some(crate::chain::ChainId::BitcoinBlake2b) => "BTCB2 · MAINNET",
        Some(crate::chain::ChainId::BitcoinBlake2bTestnet4) => "BTCB2 · TESTNET4",
        _ => localized_copy("UNSUPPORTED"),
    }
}

pub(super) const ENABLE_ID: &str = "tenshu.ai-access.enable";
pub(super) const PAIR_OPEN_ID: &str = "tenshu.ai-access.open";
pub(super) const NAME_ID: &str = "tenshu.ai-access.name";
const WALLET_ID: &str = "tenshu.ai-access.wallet";
const READ_ID: &str = "tenshu.ai-access.access.read";
#[cfg(feature = "mcp-transactions")]
const TRANSACTIONS_ID: &str = "tenshu.ai-access.transactions";
const PAIR_ID: &str = "tenshu.ai-access.pair";
const CANCEL_ID: &str = "tenshu.ai-access.cancel";
pub(super) const FORM_ENABLE_ID: &str = "tenshu.ai-access.form.enable";
const PERMISSIONS: [(&str, &str, &str); 3] = [
    ("tenshu.ai-access.permission.balances", "Balances", ""),
    (
        "tenshu.ai-access.permission.history",
        "Confirmed history",
        "Direction and order only.",
    ),
    (
        "tenshu.ai-access.permission.policy",
        "Spending-policy summary",
        "",
    ),
];

const INTRO: &str =
    "Let AI apps on this computer use this Cube's Vault, with only the access you choose.";
#[cfg(feature = "mcp-proposals")]
const SETUP_INTRO: &str =
    "Let an AI app help plan a new Vault. You choose the keys and create it in COINCUBE Desktop.";
const ENABLE_ON: &str = "Clients work while this Cube is unlocked.";
const ENABLE_SUSPENDED: &str = "Clients are suspended after a storage error.";
const ENABLE_OFF: &str = "Clients are paused. Turning on grants no new access.";
const UNSUPPORTED: &str = "Needs a Vault run by COINCUBE Desktop on Bitcoin (mainnet, Testnet4 or Regtest) or Bitcoin Blake2b (mainnet or Testnet4). Spark, Liquid, imported wallets and external daemons aren't supported.";
const PAIR_HINT: &str = "Access lasts until you revoke it.";
pub(super) const CAUTION: &str =
    "Local isn't private. What a client reads can reach its AI provider.";

/// The Cube-wide switch. Turning it on is allowed only where a client could
/// be paired; turning it off (pausing every client) is always allowed.
fn enable_row(settings: &Settings, available: bool, switchable: bool) -> Ui<'_> {
    let row = page::row(localized_copy("Allow AI clients"))
        .description(localized_copy(if !available {
            UNSUPPORTED
        } else if settings.pairing_poisoned {
            ENABLE_SUSPENDED
        } else if settings.enabled {
            ENABLE_ON
        } else {
            ENABLE_OFF
        }))
        .control(
            toggle(ENABLE_ID, settings.enabled, view::Message::FocusAction)
                .state_words(localized_copy("On"), localized_copy("Off"))
                .on_toggle_maybe(switchable.then_some(|on| message(Message::Enable(on)))),
        );
    if available {
        row.into()
    } else {
        row.accessory(page::status(
            page::Status::Warn,
            localized_copy("Unavailable for this Cube"),
        ))
        .top()
        .into()
    }
}

/// The only wallet a client can be paired with, with its exact chain.
fn vault_row(settings: &Settings) -> Option<Ui<'_>> {
    let chain = cube_chain(settings)?;
    Some(
        page::row(localized_copy("Vault"))
            .scope_tag(chain_tag(Some(chain)))
            .into(),
    )
}

pub(super) fn view_for_cube(settings: &Settings, supported: bool) -> Ui<'_> {
    // Only offer pairing for a Vault on one of the documented chains, so a
    // Vault is never tagged UNSUPPORTED beside an enabled Pair action.
    let available =
        cfg!(any(unix, windows)) && supported && cube_chain(settings).is_none_or(chain_supported);
    let switchable =
        cfg!(any(unix, windows)) && settings.switch_focusable(supported) || settings.enabled;
    let mut cube = vec![enable_row(settings, available, switchable)];
    if available {
        cube.extend(vault_row(settings));
    }
    let mut clients = Column::new().width(Length::Fill);
    clients = clients.push(page::rows(client_rows(settings, available, false)));
    if available && settings.pairing_open {
        clients = clients.push(
            container(pairing_card(settings))
                .padding(iced::Padding::default().top(16.0))
                .width(Length::Fill),
        );
    }
    page_frame(
        settings,
        INTRO,
        cube,
        clients.into(),
        kinds_of_access(false),
    )
}

/// The shared page rhythm: intro, any storage error, the Cube group, the
/// clients group and the explanation of access kinds.
fn page_frame<'a>(
    settings: &'a Settings,
    intro: &'static str,
    cube: Vec<Ui<'a>>,
    clients: Ui<'a>,
    kinds: Ui<'a>,
) -> Ui<'a> {
    let mut content = Column::new()
        .width(Length::Fill)
        .push(Space::new().height(Length::Fixed(page::GROUP_GAP)))
        .push(page::muted(localized_copy(intro)));
    // A storage failure outranks everything below it, so it leads the page.
    if let Some(error) = settings.pairing_error {
        content = content.push(page::note(
            Tone::Bad,
            column![
                page::status(page::Status::Bad, localized_copy("Pairing problem")),
                text(localized_copy(error)).size(13),
            ]
            .spacing(6),
            None,
        ));
    }
    content
        .push(page::group(
            localized_copy("This Cube"),
            None,
            page::rows(cube),
        ))
        .push(page::group(localized_copy("Paired clients"), None, clients))
        .push(page::group(localized_copy("Access"), None, kinds))
        .into()
}

/// Read access, transaction preparation and wallet preparation explained
/// side by side, then what no client ever receives and the caution.
fn kinds_of_access<'a>(setup: bool) -> Ui<'a> {
    let mut rows: Vec<Ui<'a>> = Vec::new();
    if setup {
        rows.push(
            page::row(localized_copy("Propose a wallet policy"))
                .description(localized_copy(
                    "3-of-5 with a recovery path you choose. No keys.",
                ))
                .into(),
        );
        rows.push(
            page::row(localized_copy("Prepare a wallet from public keys"))
                .description(localized_copy(
                    "You verify each key and create the Vault in COINCUBE Desktop.",
                ))
                .into(),
        );
    } else {
        rows.push(
            page::row(localized_copy("Read wallet information"))
                .description(localized_copy("Only the parts you allow."))
                .into(),
        );
        rows.push(
            page::row(localized_copy("Prepare unsigned transactions"))
                .description(localized_copy(
                    "Drafts payments. You review and sign in COINCUBE Desktop.",
                ))
                .into(),
        );
    }
    rows.push(
        page::row(localized_copy("Never shared"))
            .description(localized_copy(
                "Seeds, keys, PINs, passwords, passkeys, descriptors, labels.",
            ))
            .into(),
    );
    rows.push(
        page::row(localized_copy("Changing access"))
            .description(localized_copy(
                "Paired clients never gain new access. To change one, revoke and pair again.",
            ))
            .into(),
    );
    Column::new()
        .width(Length::Fill)
        .push(page::rows(rows))
        .push(page::note(Tone::Warn, muted(localized_copy(CAUTION)), None))
        .into()
}

/// What one paired client may do, in the words the pairing form uses.
fn scope_line(scope: &pairings::Scope) -> String {
    match scope {
        pairings::Scope::Read { capabilities, .. } => {
            let granted: Vec<&str> = capabilities
                .iter()
                .zip(PERMISSIONS)
                .filter(|(enabled, _)| **enabled)
                .map(|(_, (_, label, _))| localized_copy(label))
                .collect();
            format!("{} · {}", localized_copy("Read"), granted.join(", "))
        }
        pairings::Scope::TransactionPreparation { .. } => {
            localized_copy("Prepare unsigned transactions").to_owned()
        }
        pairings::Scope::Proposal => localized_copy("Propose a wallet policy").to_owned(),
        pairings::Scope::WalletCreation => {
            localized_copy("Prepare a wallet from public keys").to_owned()
        }
    }
}

/// A client's state, its glyph and the one-line reason. "Available" means
/// Tenshu will answer it; it never claims the app is connected.
fn client_status(
    settings: &Settings,
    available: bool,
) -> (page::Status, &'static str, &'static str) {
    if settings.pairing_poisoned {
        (page::Status::Bad, "Suspended", "Storage error")
    } else if !settings.enabled {
        (page::Status::Off, "Paused", "AI access is off")
    } else if available {
        (page::Status::Ok, "Available", "")
    } else {
        (page::Status::Off, "Paused", "Vault not loaded or changed")
    }
}

/// One ruled row per paired client: name, status, scope and reason, with
/// Manage opening its controls beneath; then the row that pairs another.
fn client_rows(settings: &Settings, available: bool, setup: bool) -> Vec<Ui<'_>> {
    let mut rows = Vec::new();
    let clients = settings
        .pairing_store
        .as_ref()
        .map(pairings::Store::clients)
        .unwrap_or_default();
    for client in clients {
        let runtime = settings
            .paired
            .iter()
            .find(|runtime| runtime.id == client.id);
        let ready = !settings.pairing_poisoned
            && settings.enabled
            && runtime.is_some_and(|runtime| runtime.server.grant.active());
        let (status, word, reason) = client_status(settings, ready);
        let open = settings.managing.as_deref() == Some(client.id.as_str())
            || settings.revoke_confirmation.as_deref() == Some(client.id.as_str());
        rows.push(
            page::row(client.name.clone())
                .accessory(page::status(status, localized_copy(word)))
                .description(if reason.is_empty() {
                    scope_line(&client.scope)
                } else {
                    format!("{} · {}", scope_line(&client.scope), localized_copy(reason))
                })
                .control(page::button(
                    format!("tenshu.ai-access.client.{}.manage", client.id),
                    localized_copy(if open { "Close" } else { "Manage" }),
                    Some(message(Message::ManageClient(client.id.clone()))),
                ))
                .into(),
        );
        if open {
            rows.push(client_controls(settings, client, runtime, ready));
        }
    }
    let empty = clients.is_empty();
    // While the form is open its own Cancel closes it; the row keeps only
    // its state so there is one way out.
    let pair = (!settings.pairing_open).then(|| {
        page::primary(
            PAIR_OPEN_ID,
            localized_copy("Pair a client"),
            (available && !settings.pairing_poisoned).then(|| message(Message::TogglePairing)),
        )
    });
    let mut last = page::row(localized_copy(if empty {
        "No clients paired"
    } else {
        "Pair another client"
    }));
    if empty {
        last = last.description(localized_copy(if setup {
            "Access lasts until you revoke it. Drafts end when this Cube locks."
        } else {
            PAIR_HINT
        }));
    }
    rows.push(last.control_maybe(pair).into());
    rows
}

/// A paired client's controls, inset under its row: what its label and
/// count mean, copying its private configuration, revoking it, and any
/// draft it prepared for native review.
fn client_controls<'a>(
    settings: &'a Settings,
    client: &'a pairings::Client,
    runtime: Option<&'a super::PairedRuntime>,
    available: bool,
) -> Ui<'a> {
    let calls = runtime.map_or(0, |runtime| {
        runtime
            .server
            .grant
            .successful_calls
            .load(Ordering::Relaxed)
    });
    let copied = settings.copied.as_deref() == Some(client.id.as_str());
    #[allow(unused_mut)]
    let mut body = Column::new()
        .spacing(10)
        .width(Length::Fill)
        .push(muted(localized_format!(
            "Unverified label · {} requests this session (not a connection status)",
            "Etiqueta no verificada · {} solicitudes en esta sesión (no indica conexión)",
            calls
        )))
        .push(
            row![
                action(
                    format!("tenshu.ai-access.client.{}.copy", client.id),
                    "Copy private client configuration",
                    (!settings.pairing_poisoned && available)
                        .then(|| message(Message::CopyPair(client.id.clone()))),
                    theme::tenshu::button::ActionKind::Secondary
                ),
                action(
                    format!("tenshu.ai-access.client.{}.revoke", client.id),
                    "Revoke access",
                    Some(message(Message::RequestRevokePair(client.id.clone()))),
                    theme::tenshu::button::ActionKind::Destructive
                ),
            ]
            .spacing(8),
        )
        .push(if copied && available {
            column![
                page::status(page::Status::Ok, localized_copy("Copied")),
                muted(localized_copy(
                    "Paste into this app's MCP settings, then clear your clipboard."
                )),
            ]
            .spacing(4)
            .into()
        } else if settings.pairing_poisoned {
            Ui::from(muted(localized_copy(
                "Copy is unavailable after a storage error.",
            )))
        } else if available {
            Ui::from(muted(localized_copy(
                "It works like a password. Keep it out of chats.",
            )))
        } else {
            Ui::from(muted(localized_copy(
                "Copy works while the client is available.",
            )))
        });

    #[cfg(feature = "mcp-transactions")]
    if let Some(service) = runtime
        .filter(|_| available)
        .and_then(|runtime| runtime.transactions.as_ref())
    {
        for draft in service.summaries() {
            body = body.push(transaction_card(&client.id, draft));
        }
    }
    #[cfg(feature = "mcp-proposals")]
    if let Some(proposal) = runtime
        .filter(|_| available)
        .and_then(|runtime| runtime.proposals.as_ref())
        .and_then(|store| store.pending().ok())
        .flatten()
    {
        let keys_ready = runtime
            .and_then(|runtime| runtime.proposals.as_ref())
            .is_some_and(|store| {
                store
                    .creation_input(&proposal.proposal_id)
                    .ok()
                    .flatten()
                    .is_some()
            });
        body = body.push(proposal_card(
            iced::advanced::widget::Id::from(format!(
                "tenshu.ai-access.client.{}.proposal",
                client.id
            )),
            proposal.topology.recovery.quorum.threshold,
            proposal.topology.recovery.quorum.key_count,
            proposal.topology.recovery.relative_delay_blocks,
            keys_ready,
            message(Message::ReviewPairedProposal(client.id.clone())),
        ));
    }
    Column::new()
        .push(page::disclosure_body([container(body)
            .padding([12, 0])
            .width(Length::Fill)
            .into()]))
        .push(page::rule())
        .into()
}

/// A wallet proposal waiting for native review. Validation proves the
/// descriptor matches the plan, not who holds the keys or their backups.
#[cfg(feature = "mcp-proposals")]
fn proposal_card<'a>(
    id: iced::advanced::widget::Id,
    threshold: impl std::fmt::Display,
    key_count: impl std::fmt::Display,
    delay: impl std::fmt::Display,
    keys_ready: bool,
    review: view::Message,
) -> Ui<'a> {
    card(
        Column::new()
            .spacing(10)
            .width(Length::Fill)
            .push(heading(localized_copy("Wallet proposal")))
            .push(muted(localized_format!(
                "3-of-5 primary · recovery {}-of-{} after {} blocks",
                "Principal 3 de 5 · recuperación {} de {} después de {} bloques",
                threshold,
                key_count,
                delay
            )))
            .push(if keys_ready {
                page::status(page::Status::Ok, localized_copy("Descriptor validated"))
            } else {
                page::status(page::Status::Warn, localized_copy("Keys not selected yet"))
            })
            .push(muted(localized_copy(if keys_ready {
                "A valid descriptor doesn't prove separate owners or backups. Check each key on its device."
            } else {
                "Select real keys in COINCUBE Desktop."
            })))
            .push(action(
                id,
                if keys_ready {
                    "Review prepared wallet in COINCUBE Desktop"
                } else {
                    "Review proposal and select keys"
                },
                Some(review),
                theme::tenshu::button::ActionKind::Primary,
            )),
    )
}

/// The only live transaction state is an unsigned chain-bound construction. Keep
/// the artboard's review rhythm while deriving every value from the MCP result.
#[cfg(feature = "mcp-transactions")]
fn transaction_card(client_id: &str, draft: Prepared) -> Ui<'static> {
    let chain = draft
        .network
        .parse::<crate::chain::ChainId>()
        .ok()
        .filter(|chain| super::transactions::supported_chain(*chain));
    let ready = chain.is_some()
        && draft.status == "unsigned_ready_for_native_review"
        && draft.signing_required;
    let total = draft.amount_sats.checked_add(draft.fee_sats);
    let mut details: Column<'static, view::Message, theme::Theme> = Column::new()
        .spacing(0)
        .width(Length::Fill)
        .push(transaction_field(
            localized_copy("From"),
            chain
                .map(crate::chain::canonical_network_name)
                .unwrap_or_else(|| localized_copy("Unsupported network")),
            false,
        ));
    for (index, recipient) in draft.recipients.iter().enumerate() {
        let label = if draft.recipients.len() == 1 {
            localized_copy("Recipient").to_owned()
        } else {
            localized_format!("Recipient {}", "Destinatario {}", index + 1)
        };
        let amount_label = if draft.recipients.len() == 1 {
            localized_copy("Amount").to_owned()
        } else {
            localized_format!("Amount {}", "Importe {}", index + 1)
        };
        details = details
            .push(transaction_field(label, recipient.address.clone(), true))
            .push(transaction_field(
                amount_label,
                transaction_sats(recipient.amount_sats),
                true,
            ));
    }
    details = details
        .push(transaction_field(
            localized_copy("Payment total"),
            localized_format!(
                "{} · exact amount requested",
                "{} · importe exacto solicitado",
                transaction_sats(draft.amount_sats)
            ),
            true,
        ))
        .push(transaction_field(
            localized_copy("Miner fee"),
            localized_format!(
                "{} · estimated {} sat/vB · added on top",
                "{} · estimada en {} sat/vB · se suma al importe",
                transaction_sats(draft.fee_sats),
                draft.fee_rate_sat_vb
            ),
            true,
        ))
        .push(transaction_field(
            localized_copy("Total leaving Vault"),
            total.map_or_else(
                || localized_copy("Unavailable").to_owned(),
                transaction_sats,
            ),
            true,
        ))
        .push(transaction_field(
            localized_copy("Change"),
            if draft.change_sats == 0 {
                localized_copy("No change output").to_owned()
            } else {
                localized_format!(
                    "{} returned to this Vault",
                    "{} vuelven a este Vault",
                    transaction_sats(draft.change_sats)
                )
            },
            true,
        ))
        .push(transaction_field(
            localized_copy("Inputs"),
            localized_format!(
                "{} selected by COINCUBE Desktop",
                "{} seleccionadas por COINCUBE Desktop",
                draft.input_count
            ),
            false,
        ))
        .push(transaction_field(
            localized_copy("Signatures"),
            localized_copy("None yet · signing happens in COINCUBE Desktop's review"),
            false,
        ));

    let review = ready && total.is_some();
    let stale = chain.is_some() && draft.status == "stale_requires_new_request";
    let action_id = iced::advanced::widget::Id::from(format!(
        "tenshu.ai-access.client.{}.transaction.{}",
        client_id, draft.draft_id
    ));
    let review_message = review.then(|| {
        message(Message::ReviewTransaction {
            client_id: client_id.to_owned(),
            draft_id: draft.draft_id,
        })
    });
    let (status, status_text) = if review {
        (page::Status::Ok, "Unsigned · ready for your review")
    } else if stale {
        (
            page::Status::Warn,
            "Out of date · ask the client to prepare it again",
        )
    } else {
        (page::Status::Warn, "Draft unavailable for native review")
    };
    let content = Column::new()
        .spacing(12)
        .width(Length::Fill)
        .push(
            row![
                heading(localized_copy("Prepared transaction")),
                page::scope_tag(chain_tag(chain))
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        )
        .push(muted(localized_copy(
            "Drafted by this client, which can't sign or send. You review and sign in COINCUBE Desktop.",
        )))
        .push(page::status(status, localized_copy(status_text)))
        .push(details)
        .push(action(
            action_id,
            "Review prepared transaction",
            review_message,
            theme::tenshu::button::ActionKind::Primary,
        ))
        .push(muted(localized_copy(
            "Check every address and amount in the review.",
        )));
    card(content)
}

#[cfg(feature = "mcp-transactions")]
fn transaction_sats(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    format!("{grouped} SATS")
}

#[cfg(feature = "mcp-transactions")]
fn transaction_field(
    label: impl Into<String>,
    value: impl Into<String>,
    mono: bool,
) -> Ui<'static> {
    let value = text(value.into())
        .size(13)
        .width(Length::Fill)
        .wrapping(iced::widget::text::Wrapping::WordOrGlyph);
    let value = if mono {
        value.font(font::MONO_REGULAR)
    } else {
        value
    };
    column![
        page::rule(),
        container(
            row![muted(label.into()).width(Length::Fixed(142.0)), value,]
                .spacing(12)
                .width(Length::Fill),
        )
        .padding([9, 0]),
    ]
    .width(Length::Fill)
    .into()
}

#[cfg(all(test, feature = "mcp-transactions"))]
mod mcp_transaction_presentation_tests {
    use super::*;
    use crate::app::view::shared::landing::capture;
    use coincube_core::miniscript::bitcoin::{
        hashes::Hash, Address, Network, ScriptBuf, WScriptHash,
    };
    use coincube_mcp::transaction::Recipient;
    use coincube_ui::widget::Renderer;
    use iced::advanced::{
        layout,
        renderer::Headless,
        widget::{operation, operation::focusable, operation::Scrollable, Operation, Tree},
        Layout,
    };
    use iced::{Rectangle, Size, Vector};

    const SCROLL_ID: &str = "r25.transaction.qa-scroll";
    const REVIEW_ID: &str = "tenshu.ai-access.client.synthetic-client.transaction.synthetic-draft";

    #[derive(Default)]
    struct Probe {
        scroll: Option<(Rectangle, Rectangle, Vector)>,
        focus: Vec<(iced::advanced::widget::Id, Rectangle, bool)>,
    }

    impl Operation for Probe {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }

        fn scrollable(
            &mut self,
            id: Option<&iced::advanced::widget::Id>,
            bounds: Rectangle,
            content: Rectangle,
            translation: Vector,
            _state: &mut dyn Scrollable,
        ) {
            if id == Some(&iced::advanced::widget::Id::from(SCROLL_ID)) {
                self.scroll = Some((bounds, content, translation));
            }
        }

        fn focusable(
            &mut self,
            id: Option<&iced::advanced::widget::Id>,
            bounds: Rectangle,
            state: &mut dyn operation::Focusable,
        ) {
            if let Some(id) = id {
                self.focus.push((id.clone(), bounds, state.is_focused()));
            }
        }
    }

    fn probe(
        element: &mut Ui<'_>,
        tree: &mut Tree,
        node: &layout::Node,
        renderer: &Renderer,
    ) -> Probe {
        let mut probe = Probe::default();
        element
            .as_widget_mut()
            .operate(tree, Layout::new(node), renderer, &mut probe);
        probe
    }

    fn operate(
        element: &mut Ui<'_>,
        tree: &mut Tree,
        node: &layout::Node,
        renderer: &Renderer,
        operation: impl Operation<()> + 'static,
    ) {
        let mut operation: Box<dyn Operation<()>> = Box::new(operation);
        loop {
            element.as_widget_mut().operate(
                tree,
                Layout::new(node),
                renderer,
                &mut operation::black_box(operation.as_mut()),
            );
            match operation.finish() {
                operation::Outcome::Chain(next) => operation = next,
                _ => break,
            }
        }
    }

    // Address-valid display fixture. Fee, rate and input count are independent
    // presentation values, not an output of the native construction engine.
    fn address_valid_display_draft() -> Prepared {
        Prepared {
            wallet_id: "synthetic-wallet".into(),
            draft_id: "synthetic-draft".into(),
            network: "regtest".into(),
            status: "unsigned_ready_for_native_review".into(),
            recipients: (1..=8)
                .map(|index| {
                    let script = ScriptBuf::new_p2wsh(&WScriptHash::from_byte_array([index; 32]));
                    Recipient {
                        address: Address::from_script(&script, Network::Regtest)
                            .unwrap()
                            .to_string(),
                        amount_sats: 10_000,
                    }
                })
                .collect(),
            amount_sats: 80_000,
            fee_sats: 500,
            fee_rate_sat_vb: 42,
            change_sats: 50_000,
            input_count: 12,
            signing_required: true,
        }
    }

    fn schema_boundary_draft() -> Prepared {
        // PrepareInput accepts this 90-character ASCII boundary, but native
        // address validation rejects it. This fixture stresses layout only.
        let address = format!("bcrt1{}", "q".repeat(85));
        assert_eq!(address.len(), 90);
        let mut draft = address_valid_display_draft();
        draft.recipients[0].address = address;
        draft.recipients[0].amount_sats = 123_456_789;
        draft.amount_sats = 123_526_789;
        draft.fee_sats = 12_345;
        draft.change_sats = 2_100_000_000_000;
        draft
    }

    #[test]
    fn transaction_card_boundary_capture_and_keyboard() {
        let draft = address_valid_display_draft();
        let review_id = iced::advanced::widget::Id::from(REVIEW_ID);
        for (appearance, palette) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            let inspection = capture::inspect(
                "r25-eight-address-valid-display-only",
                transaction_card("synthetic-client", draft.clone()),
                (1024, 768),
                appearance,
                palette,
            );
            assert!(inspection.contains("Recipient 8"));
            assert!(inspection.contains("Amount 8"));
            assert!(inspection.contains("80,000 SATS"));
            assert!(inspection.contains("80,500 SATS"));
            assert!(inspection.contains("50,000 SATS"));
            assert!(inspection.has_focus(review_id.clone()));
            let schema = capture::inspect(
                "r25-90-char-schema-layout-only",
                transaction_card("synthetic-client", schema_boundary_draft()),
                (1024, 768),
                appearance,
                if appearance == "dark" {
                    theme::Theme::dark()
                } else {
                    theme::Theme::light()
                },
            );
            assert!(schema.contains("123,526,789 SATS"));
            assert!(schema.contains("123,539,134 SATS"));
            assert!(schema.contains("2,100,000,000,000 SATS"));
        }
        for key in [
            iced::keyboard::key::Named::Enter,
            iced::keyboard::key::Named::Space,
        ] {
            let emitted = capture::activate_with_key(
                transaction_card("synthetic-client", draft.clone()),
                review_id.clone(),
                (1024, 768),
                false,
                key,
            );
            assert!(emitted.iter().any(|intent| matches!(intent,
                view::Message::LocalMcp(Message::ReviewTransaction { client_id, draft_id })
                    if client_id == "synthetic-client" && draft_id == "synthetic-draft"
            )));
        }
        let mut unavailable = draft;
        unavailable.change_sats = 0;
        let no_change = capture::inspect(
            "r25-zero-change",
            transaction_card("synthetic-client", unavailable.clone()),
            (1024, 768),
            "light",
            theme::Theme::light(),
        );
        assert!(no_change.contains("No change output"));
        unavailable.network = "mainnet".into();
        let inspection = capture::inspect(
            "r25-mainnet-gated",
            transaction_card("synthetic-client", unavailable.clone()),
            (1024, 768),
            "dark",
            theme::Theme::dark(),
        );
        assert!(inspection.contains("Draft unavailable for native review"));
        assert!(!inspection.has_focus(review_id));
    }

    #[test]
    fn transaction_card_tab_order_and_scroll_viewport() {
        for bytes in font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(bytes);
        }
        let mut renderer = iced::futures::executor::block_on(<Renderer as Headless>::new(
            font::REGULAR,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        // The 1024px shell leaves a 652px content column and a 668px viewport.
        let size = Size::new(652.0, 668.0);
        let content = iced::widget::column![
            action(
                "r25.transaction.previous",
                "Previous control",
                Some(message(Message::TogglePairing)),
                theme::tenshu::button::ActionKind::Secondary,
            ),
            transaction_card("synthetic-client", address_valid_display_draft()),
        ];
        let mut element: Ui<'_> = iced::widget::scrollable(content)
            .id(SCROLL_ID)
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, size),
        );
        let initial = probe(&mut element, &mut tree, &node, &renderer);
        let (viewport, content, _) = initial.scroll.expect("mounted scrollable");
        assert!(
            content.height > viewport.height,
            "eight recipients need vertical scroll"
        );
        let review = iced::advanced::widget::Id::from(REVIEW_ID);
        let review_bounds = initial
            .focus
            .iter()
            .find(|(id, _, _)| id == &review)
            .expect("native review CTA in Tab order")
            .1;
        assert!(review_bounds.y > viewport.y + viewport.height);

        operate(
            &mut element,
            &mut tree,
            &node,
            &renderer,
            focusable::focus_next::<()>(),
        );
        assert!(probe(&mut element, &mut tree, &node, &renderer)
            .focus
            .iter()
            .any(|(id, _, focused)| id
                == &iced::advanced::widget::Id::from("r25.transaction.previous")
                && *focused));
        operate(
            &mut element,
            &mut tree,
            &node,
            &renderer,
            focusable::focus_next::<()>(),
        );
        assert!(probe(&mut element, &mut tree, &node, &renderer)
            .focus
            .iter()
            .any(|(id, _, focused)| id == &review && *focused));
        operate(
            &mut element,
            &mut tree,
            &node,
            &renderer,
            focusable::focus_previous::<()>(),
        );
        assert!(probe(&mut element, &mut tree, &node, &renderer)
            .focus
            .iter()
            .any(|(id, _, focused)| id
                == &iced::advanced::widget::Id::from("r25.transaction.previous")
                && *focused));

        let offset = review_bounds.y - viewport.y - viewport.height + review_bounds.height + 12.0;
        operate(
            &mut element,
            &mut tree,
            &node,
            &renderer,
            operation::scrollable::scroll_to::<()>(
                iced::advanced::widget::Id::from(SCROLL_ID),
                operation::scrollable::AbsoluteOffset {
                    x: None,
                    y: Some(offset),
                },
            ),
        );
        let scrolled = probe(&mut element, &mut tree, &node, &renderer);
        let (viewport, _, translation) = scrolled.scroll.expect("scroll still mounted");
        assert!(translation.y > 0.0, "scroll offset must move");
        let cta = scrolled
            .focus
            .iter()
            .find(|(id, _, _)| id == &review)
            .expect("review CTA after scroll")
            .1;
        let visible_y = cta.y - translation.y;
        assert!(
            visible_y >= viewport.y && visible_y + cta.height <= viewport.y + viewport.height,
            "CTA {:?}, viewport {:?}, translation {:?}",
            cta,
            viewport,
            translation
        );
        if let Ok(directory) = std::env::var("R06A_CAPTURE_DIR") {
            std::fs::create_dir_all(&directory).unwrap();
            // Iced Button computes Active/Disabled on RedrawRequested. Drawing
            // the initial tree directly would falsely show every Action faded.
            let mut messages = Vec::new();
            element.as_widget_mut().update(
                &mut tree,
                &iced::Event::Window(iced::window::Event::RedrawRequested(
                    std::time::Instant::now(),
                )),
                Layout::new(&node),
                iced::mouse::Cursor::Unavailable,
                &renderer,
                &mut iced::advanced::clipboard::Null,
                &mut iced::advanced::Shell::new(&mut messages),
                &Rectangle::with_size(size),
            );
            for (appearance, palette) in [
                ("dark", theme::Theme::dark()),
                ("light", theme::Theme::light()),
            ] {
                element.as_widget().draw(
                    &tree,
                    &mut renderer,
                    &palette,
                    &iced::advanced::renderer::Style {
                        text_color: palette.colors.shell.ink,
                    },
                    Layout::new(&node),
                    iced::mouse::Cursor::Unavailable,
                    &Rectangle::with_size(size),
                );
                let pixels = renderer.screenshot(
                    Size::new(size.width as u32, size.height as u32),
                    1.0,
                    palette.colors.general.background,
                );
                image::save_buffer(
                    std::path::Path::new(&directory).join(format!(
                        "r25-eight-address-valid-display-only-scrolled-{appearance}-1024x768-content-only-synthetic.png"
                    )),
                    &pixels,
                    size.width as u32,
                    size.height as u32,
                    image::ColorType::Rgba8,
                )
                .unwrap();
            }
        }
    }
}

pub(super) fn revoke_confirmation<'a>(client_name: &str, id: &str) -> Ui<'a> {
    container(column![
        heading(localized_copy("Revoke this client's access?")),
        muted(localized_format!(
            "{client_name} will lose access immediately. This cannot be undone; pairing again creates a new private configuration.",
            "{client_name} perderá el acceso de inmediato. Esto no se puede deshacer; si lo vinculas de nuevo, se creará una nueva configuración privada."
        )),
        muted(localized_copy("Information already read may remain with the AI provider. Revocation cannot erase data it has retained.")),
        row![
            action(
                "tenshu.ai-access.revoke.keep",
                "Keep paired",
                Some(message(Message::CancelRevokePair)),
                theme::tenshu::button::ActionKind::Secondary,
            ),
            action(
                "tenshu.ai-access.revoke.confirm",
                "Revoke access",
                Some(message(Message::RevokePair(id.to_owned()))),
                theme::tenshu::button::ActionKind::Destructive,
            ),
        ].spacing(8),
    ].spacing(14))
    .width(Length::Fixed(460.0))
    .padding(22)
    .style(|theme: &theme::Theme| iced::widget::container::Style {
        text_color: Some(theme.colors.shell.ink),
        background: Some(Background::Color(theme.colors.shell.panel)),
        border: Border {
            color: theme.colors.shell.line,
            width: 1.0,
            radius: 12.0.into(),
        },
        ..Default::default()
    })
    .into()
}

/// Which kind of access the pairing form is set to grant. Each is its own
/// pairing; nothing is chosen until the user picks one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Access {
    Read,
    #[cfg(feature = "mcp-transactions")]
    Transactions,
    #[cfg(feature = "mcp-proposals")]
    Proposal,
    #[cfg(feature = "mcp-proposals")]
    Creation,
}

fn chosen_access(settings: &Settings) -> Option<Access> {
    #[cfg(feature = "mcp-transactions")]
    if settings.transactions_allowed {
        return Some(Access::Transactions);
    }
    #[cfg(feature = "mcp-proposals")]
    {
        if settings.creation_allowed {
            return Some(Access::Creation);
        }
        if settings.proposals_allowed {
            return Some(Access::Proposal);
        }
    }
    (settings.read_chosen || settings.capabilities.iter().any(|on| *on)).then_some(Access::Read)
}

fn choose(access: Access) -> view::Message {
    message(match access {
        Access::Read => Message::ChooseRead,
        #[cfg(feature = "mcp-transactions")]
        Access::Transactions => Message::Transactions(true),
        #[cfg(feature = "mcp-proposals")]
        Access::Proposal => Message::Proposals(true),
        #[cfg(feature = "mcp-proposals")]
        Access::Creation => Message::Creation(true),
    })
}

/// While AI access is off the form says so and offers to turn it on, rather
/// than showing fields that could not pair.
fn access_off_note<'a>() -> Ui<'a> {
    page::note(
        Tone::Warn,
        muted(localized_copy(
            "AI access is off. Turning it on resumes existing clients and grants no new access.",
        )),
        Some(
            row![
                page::button(
                    FORM_ENABLE_ID,
                    localized_copy("Turn on"),
                    Some(message(Message::TurnOnFromForm)),
                ),
                page::button(
                    CANCEL_ID,
                    localized_copy("Cancel"),
                    Some(message(Message::TogglePairing)),
                ),
            ]
            .spacing(8)
            .into(),
        ),
    )
}

fn name_field(settings: &Settings) -> Ui<'_> {
    container(
        text_field(
            NAME_ID,
            localized_copy("Client name"),
            &settings.client_name,
            view::Message::FocusAction,
        )
        .placeholder("Claude Desktop")
        .hint(localized_copy(
            "A label only; COINCUBE Desktop can't verify the app.",
        ))
        .on_input(|value| message(Message::ClientName(value))),
    )
    .max_width(480)
    .into()
}

/// The form's closing part: the caution, the pair action and why it waits.
fn pair_actions<'a>(label: &'static str, can_pair: bool) -> Ui<'a> {
    let mut actions = Column::new()
        .width(Length::Fill)
        .spacing(10)
        .push(page::note(
            Tone::Warn,
            muted(localized_copy(
                "What this client receives can reach its AI provider.",
            )),
            None,
        ))
        .push(
            row![
                action(
                    PAIR_ID,
                    label,
                    can_pair.then(|| message(Message::Pair)),
                    theme::tenshu::button::ActionKind::Primary,
                ),
                action(
                    CANCEL_ID,
                    "Cancel",
                    Some(message(Message::TogglePairing)),
                    theme::tenshu::button::ActionKind::Secondary,
                ),
            ]
            .spacing(8),
        );
    if !can_pair {
        actions = actions.push(muted(localized_copy(
            "Name the client and choose its access.",
        )));
    }
    actions.into()
}

fn pairing_card(settings: &Settings) -> Ui<'_> {
    let mut body = Column::new()
        .spacing(16)
        .width(Length::Fill)
        .push(heading(localized_copy("Pair a client")));
    if !settings.enabled {
        return card(body.push(access_off_note()));
    }
    let chain = cube_chain(settings);
    body = body.push(name_field(settings)).push(
        Column::new()
            .width(Length::Fill)
            .push(subheading(localized_copy("Wallet")))
            .push(page::rows([page::row(localized_copy("This Cube's Vault"))
                .scope_tag(chain_tag(chain))
                .control(
                    toggle(
                        WALLET_ID,
                        settings.wallet_allowed,
                        view::Message::FocusAction,
                    )
                    .state_words(localized_copy("Selected"), localized_copy("Not selected"))
                    .on_toggle(|on| message(Message::Wallet(on))),
                )
                .into()])),
    );

    let chosen = chosen_access(settings);
    let read_choice = radio_list(
        [segment(
            READ_ID,
            Access::Read,
            localized_copy("Read wallet information"),
        )],
        chosen,
        view::Message::FocusAction,
    )
    .on_select(choose);
    let read = chosen == Some(Access::Read) || chosen.is_none();
    let permissions = PERMISSIONS
        .iter()
        .enumerate()
        .map(|(index, (id, label, detail))| {
            let row = page::row(localized_copy(label));
            let row = if detail.is_empty() {
                row
            } else {
                row.description(localized_copy(detail))
            };
            row.control(
                toggle(
                    *id,
                    settings.capabilities[index],
                    view::Message::FocusAction,
                )
                .state_words(localized_copy("Allowed"), localized_copy("Not allowed"))
                .on_toggle_maybe(read.then_some(move |on| message(Message::Capability(index, on)))),
            )
            .into()
        })
        .collect::<Vec<Ui<'_>>>();
    #[allow(unused_mut)]
    let mut access = Column::new()
        .width(Length::Fill)
        .push(subheading(localized_copy("Access")))
        .push(Space::new().height(Length::Fixed(4.0)))
        .push(muted(localized_copy("One per client.")))
        .push(Space::new().height(Length::Fixed(8.0)))
        .push(read_choice)
        // The read parts sit under their own choice, inset like a disclosure.
        .push(page::disclosure_body(permissions));
    #[cfg(feature = "mcp-transactions")]
    if settings
        .pairing_location
        .as_ref()
        .is_some_and(pairings::Location::transaction_supported)
    {
        access = access.push(
            radio_list(
                [segment(
                    TRANSACTIONS_ID,
                    Access::Transactions,
                    localized_copy("Prepare unsigned transactions"),
                )
                .description(localized_copy(
                    "No balances or history. You sign in COINCUBE Desktop.",
                ))],
                chosen,
                view::Message::FocusAction,
            )
            .on_select(choose),
        );
    }
    #[cfg(feature = "mcp-transactions")]
    if chosen == Some(Access::Transactions) {
        access = access
            .push(Space::new().height(Length::Fixed(8.0)))
            .push(muted(localized_copy(
                "Up to eight drafts per unlock, in exact sats.",
            )));
    }
    body = body.push(access);

    // Pairing is a no-op after a storage failure, so the action says so.
    let can_pair = !settings.pairing_poisoned
        && settings.wallet_allowed
        && (settings.capabilities.iter().any(|allowed| *allowed)
            || settings.transaction_selection())
        && !settings.client_name.trim().is_empty();
    body = body.push(pair_actions("Pair client until revoked", can_pair));
    body = legacy_and_failure(settings, body);
    card(body)
}

/// The legacy one-hour session grant, if one is running, and a failed start.
fn legacy_and_failure<'a>(
    settings: &'a Settings,
    #[allow(unused_mut)] mut body: Column<'a, view::Message, theme::Theme>,
) -> Column<'a, view::Message, theme::Theme> {
    #[cfg(any(unix, windows))]
    if let Some(server) = &settings.server {
        let active = server.grant.active();
        let calls = server.grant.successful_calls.load(Ordering::Relaxed);
        #[allow(unused_mut)]
        let mut legacy = Column::new()
            .spacing(8)
            .push(subheading(localized_copy("Temporary session grant")))
            .push(page::status(
                if active {
                    page::Status::Ok
                } else {
                    page::Status::Off
                },
                localized_copy(if active {
                    "Authorized"
                } else {
                    "Inactive — approve again"
                }),
            ))
            .push(muted(localized_format!(
                "{} · requests answered this session: {}",
                "{} · solicitudes respondidas en esta sesión: {}",
                settings.approved_name,
                calls
            )))
            .push(
                row![
                    action(
                        "tenshu.ai-access.temporary.copy",
                        "Copy private client configuration",
                        active.then(|| message(Message::CopyConfig)),
                        theme::tenshu::button::ActionKind::Secondary
                    ),
                    action(
                        "tenshu.ai-access.temporary.revoke",
                        "Revoke access now",
                        Some(message(Message::Revoke)),
                        theme::tenshu::button::ActionKind::Destructive
                    ),
                ]
                .spacing(8),
            );
        #[cfg(feature = "mcp-proposals")]
        if active {
            legacy = match settings
                .proposals
                .as_ref()
                .and_then(|store| store.pending().ok())
                .flatten()
            {
                Some(proposal) => legacy.push(proposal_card(
                    iced::advanced::widget::Id::from("tenshu.ai-access.temporary.proposal"),
                    proposal.topology.recovery.quorum.threshold,
                    proposal.topology.recovery.quorum.key_count,
                    proposal.topology.recovery.relative_delay_blocks,
                    false,
                    message(Message::ReviewProposal),
                )),
                None => legacy.push(muted(localized_copy(
                    "Waiting for a proposal from your client.",
                ))),
            };
        }
        body = body.push(page::note(Tone::Muted, legacy, None));
    }
    if settings.failed {
        body = body.push(page::note(
            Tone::Bad,
            text(localized_copy(
                "Could not start local MCP. No access was granted.",
            ))
            .size(13),
            None,
        ));
    }
    body
}

/// The empty-Cube page: the same rhythm as AI access, offering the two
/// wallet-preparation pairings instead of wallet reads.
#[cfg(feature = "mcp-proposals")]
pub(super) fn setup_view(settings: &Settings) -> Ui<'_> {
    let mut clients = Column::new()
        .width(Length::Fill)
        .push(page::rows(client_rows(settings, true, true)));
    if settings.pairing_open {
        clients = clients.push(
            container(setup_card(settings))
                .padding(iced::Padding::default().top(16.0))
                .width(Length::Fill),
        );
    }
    page_frame(
        settings,
        SETUP_INTRO,
        vec![enable_row(settings, true, true)],
        clients.into(),
        kinds_of_access(true),
    )
}

#[cfg(feature = "mcp-proposals")]
fn setup_card(settings: &Settings) -> Ui<'_> {
    let mut body = Column::new()
        .spacing(16)
        .width(Length::Fill)
        .push(heading(localized_copy("Pair a setup client")));
    if !settings.enabled {
        return card(body.push(access_off_note()));
    }
    let choices = [
        segment(
            "tenshu.ai-access.access.proposal",
            Access::Proposal,
            localized_copy("Propose a wallet policy"),
        )
        .description(localized_copy(
            "3-of-5 with a recovery path you choose. No keys.",
        )),
        segment(
            "tenshu.ai-access.access.creation",
            Access::Creation,
            localized_copy("Prepare a wallet from public keys"),
        )
        .description(localized_copy(
            "Its AI provider may see the public keys. You verify each in COINCUBE Desktop.",
        )),
    ];
    body = body.push(name_field(settings)).push(
        Column::new()
            .width(Length::Fill)
            .push(subheading(localized_copy("Access")))
            .push(Space::new().height(Length::Fixed(8.0)))
            .push(
                radio_list(choices, chosen_access(settings), view::Message::FocusAction)
                    .on_select(choose),
            ),
    );
    let can_pair = !settings.pairing_poisoned
        && (settings.proposals_allowed || settings.creation_allowed)
        && !settings.client_name.trim().is_empty();
    body = body.push(pair_actions("Pair setup client until revoked", can_pair));
    body = legacy_and_failure(settings, body);
    card(body)
}

#[cfg(all(
    test,
    any(unix, windows),
    feature = "mcp-proposals",
    feature = "mcp-transactions"
))]
mod design_gallery;

#[cfg(test)]
mod ai_access_tests {
    use super::*;
    use crate::app::view::shared::landing::capture;

    /// State first: an empty Cube says "No clients paired" beside "Pair a
    /// client", then what a client can and can't read, then one caution.
    #[test]
    fn an_empty_cube_leads_with_its_state_and_lists_what_is_shared() {
        let settings = Settings::default();
        let probe = capture::inspect(
            "ai-access-empty",
            view_for_cube(&settings, true),
            (1440, 900),
            "dark",
            theme::Theme::dark(),
        );
        for label in [
            "Paired clients",
            "No clients paired",
            "Pair a client",
            "Access lasts until you revoke it.",
            "Allow AI clients",
            "Turning on grants no new access.",
            "Read wallet information",
            "Prepare unsigned transactions",
            "Never shared",
            "Seeds, keys, PINs, passwords, passkeys",
            "Paired clients never gain new access.",
            "Local isn't private.",
        ] {
            assert!(probe.contains(label), "{}", label);
        }
        assert!(!probe.contains("Your AI provider may receive financial information"));
        assert!(!probe.contains("Open AI access"));
        let opened = capture::activate(
            view_for_cube(&settings, true),
            iced::advanced::widget::Id::new(PAIR_OPEN_ID),
            (1440, 900),
            true,
        );
        assert!(opened
            .iter()
            .any(|m| matches!(m, view::Message::LocalMcp(Message::TogglePairing))));
        // The Cube-wide switch is on the page itself, not inside the form.
        let enabled = capture::activate(
            view_for_cube(&settings, true),
            iced::advanced::widget::Id::new(ENABLE_ID),
            (1440, 900),
            true,
        );
        assert!(enabled
            .iter()
            .any(|m| matches!(m, view::Message::LocalMcp(Message::Enable(true)))));
    }

    /// A Cube without a supported Vault says which wallets and chains AI
    /// access supports, offers nothing to turn on and nothing to pair.
    #[test]
    fn unsupported_cube_names_the_documented_scope_and_stays_inert() {
        let settings = Settings::default();
        let probe = capture::inspect(
            "ai-access-unsupported",
            view_for_cube(&settings, false),
            (1024, 768),
            "dark",
            theme::Theme::dark(),
        );
        for label in [
            "Unavailable for this Cube",
            "Bitcoin (mainnet, Testnet4 or Regtest)",
            "Bitcoin Blake2b (mainnet or Testnet4)",
            "Spark, Liquid",
        ] {
            assert!(probe.contains(label), "{}", label);
        }
        for id in [ENABLE_ID, PAIR_OPEN_ID] {
            assert!(
                !probe.has_focus(iced::advanced::widget::Id::new(id)),
                "{} is inert",
                id
            );
        }
    }
}
