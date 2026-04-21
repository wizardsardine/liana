use coincube_ui::{
    component::{button, form, text::*},
    widget::*,
};
use iced::Length;

/// Priority tier for the mempool-driven feerate presets. Canonical definition
/// — `view/message.rs` imports it for the `FetchTransferFeeratePreset` and
/// `TransferFeerateEstimated` message variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeeratePreset {
    /// ~4h confirmation target.
    Slow,
    /// ~1h confirmation target.
    Normal,
    /// ~10m confirmation target.
    Fast,
}

/// A simple feerate input (sats/vbyte). The warning string and placeholder
/// match the regular Vault spend flow (`view/vault/spend/mod.rs:367-372`) so
/// the two feerate controls feel identical.
///
/// `disabled = true` renders the input read-only — used on the Vault transfer
/// confirm screen after the PSBT is signed so the user can't edit the rate
/// out from under the signature.
pub fn feerate_input<'a, M, F>(
    feerate: &'a form::Value<String>,
    on_change: F,
    disabled: bool,
) -> Element<'a, M>
where
    M: 'a + Clone,
    F: 'static + Fn(String) -> M,
{
    let form = if disabled {
        form::Form::new_disabled("42 (in sats/vbyte)", feerate)
    } else {
        form::Form::new_trimmed("42 (in sats/vbyte)", feerate, on_change)
            .warning("Feerate must be an integer between 1 and 1000 sats/vbyte")
    };
    form.size(P1_SIZE).padding(10).into()
}

/// Three mempool-driven feerate presets, rendered left-to-right as
/// Slow / Normal / Fast (ascending priority). The button
/// corresponding to `loading` is rendered non-pressable to indicate an
/// estimate is in flight; the on_select message fires for the others. When
/// `disabled`, every button is non-pressable — see `feerate_input` for the
/// reason this matters on the Vault transfer confirm screen.
pub fn feerate_presets_row<'a, M>(
    loading: Option<FeeratePreset>,
    on_select: impl Fn(FeeratePreset) -> M + 'static,
    disabled: bool,
) -> Element<'a, M>
where
    M: 'a + Clone,
{
    let make_button =
        |label: &'static str, preset: FeeratePreset, on_select: M| -> Element<'a, M> {
            let is_loading = loading == Some(preset);
            button::secondary(None, if is_loading { "…" } else { label })
                .width(Length::Fixed(110.0))
                .on_press_maybe((!is_loading && !disabled).then_some(on_select))
                .into()
        };

    Row::new()
        .spacing(10)
        .push(make_button(
            "Slow (~4h)",
            FeeratePreset::Slow,
            on_select(FeeratePreset::Slow),
        ))
        .push(make_button(
            "Normal (~1h)",
            FeeratePreset::Normal,
            on_select(FeeratePreset::Normal),
        ))
        .push(make_button(
            "Fast (~10m)",
            FeeratePreset::Fast,
            on_select(FeeratePreset::Fast),
        ))
        .into()
}
