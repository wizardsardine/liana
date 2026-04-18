use coincube_ui::{
    component::{button, form, text::*},
    widget::*,
};
use iced::Length;

/// Priority tier for the mempool-driven feerate presets. Mirrors the enum
/// already declared in `view/message.rs` but we re-declare here to avoid
/// cross-module dependency churn for a tiny enum.
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
pub fn feerate_input<'a, M, F>(feerate: &'a form::Value<String>, on_change: F) -> Element<'a, M>
where
    M: 'a + Clone,
    F: 'static + Fn(String) -> M,
{
    form::Form::new_trimmed("42 (in sats/vbyte)", feerate, on_change)
        .warning("Feerate must be an integer less than or equal to 1000 sats/vbyte")
        .size(P1_SIZE)
        .padding(10)
        .into()
}

/// Three mempool-driven feerate presets (Fast / Normal / Slow). The button
/// corresponding to `loading` is rendered non-pressable to indicate an
/// estimate is in flight; the on_select message fires for the others.
pub fn feerate_presets_row<'a, M>(
    loading: Option<FeeratePreset>,
    on_select: impl Fn(FeeratePreset) -> M + 'static,
) -> Element<'a, M>
where
    M: 'a + Clone,
{
    let make_button =
        |label: &'static str, preset: FeeratePreset, on_select: M| -> Element<'a, M> {
            let is_loading = loading == Some(preset);
            button::secondary(None, if is_loading { "…" } else { label })
                .width(Length::Fixed(110.0))
                .on_press_maybe((!is_loading).then_some(on_select))
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
