//! Presentation language for desktop text, independent of worldview and wallet data.

use std::{borrow::Cow, cell::Cell, collections::HashMap, sync::LazyLock};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(from = "serde_json::Value")]
pub enum Language {
    #[default]
    #[serde(rename = "en")]
    English,
    #[serde(rename = "es")]
    Spanish,
}

impl From<serde_json::Value> for Language {
    fn from(value: serde_json::Value) -> Self {
        match value.as_str() {
            Some("es") => Self::Spanish,
            _ => Self::English,
        }
    }
}

thread_local! {
    static LANGUAGE: Cell<Language> = const { Cell::new(Language::English) };
}

/// Scope a synchronous view build to this window's accepted language.
pub fn render_in<T>(language: Language, render: impl FnOnce() -> T) -> T {
    struct Restore(Language);
    impl Drop for Restore {
        fn drop(&mut self) {
            LANGUAGE.set(self.0);
        }
    }

    let _restore = Restore(LANGUAGE.replace(language));
    render()
}

pub fn language() -> Language {
    LANGUAGE.get()
}

mod messages;
pub use messages::Text;

static SPANISH_DESKTOP: LazyLock<HashMap<String, String>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../static/desktop.es.json"))
        .expect("Spanish desktop copy must be valid")
});

/// Localize an existing English UI literal. Call this at presentation sites,
/// never on a wallet name, identifier, or any data used to choose an action.
pub fn copy(english: &str) -> &str {
    match language() {
        Language::English => english,
        Language::Spanish => SPANISH_DESKTOP.get(english).map_or(english, String::as_str),
    }
}

/// Localize an app-authored notice at display time. Unknown diagnostics and
/// support references are copied exactly.
pub fn notice(english: String) -> String {
    if language() == Language::English {
        return english;
    }
    if let Some((message, reference)) = english.rsplit_once(" (Ref: ") {
        if let Some((title, guidance)) = message.split_once(" — ") {
            if reference.strip_suffix(')').is_some_and(|code| {
                code.starts_with("CC-")
                    && code.bytes().all(|byte| {
                        byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-'
                    })
            }) {
                return format!(
                    "{} — {} (Ref.: {reference}",
                    notice_field(title),
                    notice_field(guidance)
                );
            }
        }
    }
    match notice_field(&english) {
        Cow::Borrowed(value) if value == english => english,
        value => value.into_owned(),
    }
}

fn notice_field(english: &str) -> Cow<'_, str> {
    // Transfer errors prepend a guarantee about the actual failure point.
    for prefix in [
        "Nothing was sent and your balances are unchanged. ",
        "Your funds were not moved. ",
    ] {
        if let Some(guidance) = english.strip_prefix(prefix) {
            return Cow::Owned(format!(
                "{} {}",
                copy(prefix.trim_end()),
                notice_value(guidance)
            ));
        }
    }
    notice_value(english)
}

fn notice_value(english: &str) -> Cow<'_, str> {
    if let Some(text) = SPANISH_DESKTOP.get(english) {
        return Cow::Borrowed(text);
    }
    // Keep these source templates aligned with the typed errors in user_error.
    // Interpolate only the original value. A diagnostic is never translated
    // by pattern unless the surrounding app-authored template matches.
    const TEMPLATES: &[(&str, &str, &str)] = &[
        ("Too many incorrect PINs. Try again in ", "s.", "Demasiados intentos con un PIN incorrecto. Inténtalo de nuevo en {value} s."),
        ("Too many incorrect PINs. Try again in ", " minute(s).", "Demasiados intentos con un PIN incorrecto. Inténtalo de nuevo en {value} min."),
        ("Password must be at least ", " characters.", "La contraseña debe tener al menos {value} caracteres."),
        ("The generated seed phrase failed its own checksum (", "). Nothing was written to disk — please start again.", "No se pudo verificar la frase de recuperación generada ({value}). No se guardó nada en el disco. Empieza de nuevo."),
        ("Failed to prepare payment: ", "", "No se pudo preparar el pago: {value}"),
        ("Couldn't complete the passkey check, so this Cube stayed locked. The Cube itself is fine — try again, or restore it from your Recovery Kit. (", ")", "No se pudo verificar la llave de acceso y el Cube sigue bloqueado. Tu Cube está intacto. Inténtalo de nuevo o restáuralo con tu kit de recuperación. ({value})"),
        ("PIN check failed to run: ", "", "No se pudo comprobar el PIN: {value}"),
        ("The payment was rejected, so nothing was sent: ", "", "El pago fue rechazado y no se envió nada: {value}"),
        ("Couldn't confirm what happened to this payment (", "). Refresh and check Transactions before sending again.", "No se pudo confirmar qué pasó con este pago ({value}). Actualiza y revisa Transacciones antes de volver a enviar."),
        ("Your ", " wallet isn't available", "Tu cartera {value} no está disponible"),
        ("Open the ", " wallet from the dashboard, then start the transfer again.", "Abre la cartera {value} desde el resumen y vuelve a iniciar la transferencia."),
        ("Couldn't get a deposit address from your ", " wallet", "No se pudo obtener una dirección de depósito de tu cartera {value}"),
        ("Check your internet connection and your ", " wallet, then try again.", "Revisa tu conexión a internet y tu cartera {value}. Después, inténtalo de nuevo."),
        ("", " Check the descriptor you entered and try again.", "Revisa el descriptor que ingresaste e inténtalo de nuevo. Detalle: {value}"),
        ("", " Check the amount and fee rate, then try again.", "Revisa el importe y la tasa de comisión e inténtalo de nuevo. Detalle: {value}"),
        ("", ". Check that the backup file is complete and matches this wallet.", "Comprueba que el archivo de copia esté completo y corresponda a esta cartera. Detalle: {value}"),
    ];
    for (before, after, template) in TEMPLATES {
        if let Some(value) = english
            .strip_prefix(before)
            .and_then(|s| s.strip_suffix(after))
        {
            return Cow::Owned(template.replacen("{value}", value, 1));
        }
    }
    Cow::Borrowed(english)
}

/// The chrono pattern for showing `date` in the accepted language. English
/// keeps `english` as written. Spanish puts the day first and writes month
/// and weekday names out as literals, e.g. "%b %d, %Y" → "5 nov 2026".
/// Use it for display only; exports, file names and protocol headers keep
/// their fixed English or ISO patterns.
pub fn date_pattern(english: &str, date: &impl chrono::Datelike) -> String {
    if language() == Language::English {
        return english.to_owned();
    }
    const MONTHS: [&str; 12] = [
        "enero",
        "febrero",
        "marzo",
        "abril",
        "mayo",
        "junio",
        "julio",
        "agosto",
        "septiembre",
        "octubre",
        "noviembre",
        "diciembre",
    ];
    const MONTHS_SHORT: [&str; 12] = [
        "ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sept", "oct", "nov", "dic",
    ];
    const DAYS: [&str; 7] = [
        "lunes",
        "martes",
        "miércoles",
        "jueves",
        "viernes",
        "sábado",
        "domingo",
    ];
    const DAYS_SHORT: [&str; 7] = ["lun", "mar", "mié", "jue", "vie", "sáb", "dom"];
    // US month-first patterns in use, rewritten day-first.
    let spanish = match english {
        "%b. %d, %Y - %T" => "%-d %b %Y - %T",
        "%b. %d, %Y - %T %Z" => "%-d %b %Y - %T %Z",
        "%b %-d, %H:%M" | "%b %d, %H:%M" => "%-d %b, %H:%M",
        "%b %d, %Y" => "%-d %b %Y",
        "%b %d, %Y %H:%M" => "%-d %b %Y, %H:%M",
        "%b %d, %Y %H:%M %Z" => "%-d %b %Y, %H:%M %Z",
        "%B %d, %Y" => "%-d de %B de %Y",
        other => other,
    };
    let month = date.month0() as usize;
    let day = date.weekday().num_days_from_monday() as usize;
    spanish
        .replace("%A", DAYS[day])
        .replace("%a", DAYS_SHORT[day])
        .replace("%B", MONTHS[month])
        .replace("%b", MONTHS_SHORT[month])
}

/// Rendered text that is still English on a Spanish screen, for tests. A
/// string leaks when it is an English key whose Spanish copy differs (the
/// call site skipped `copy`), or when it reads as English prose and has no
/// Spanish entry at all. Brand names, numbers and data carry no English
/// function words, so they pass.
#[doc(hidden)]
pub fn english_leaks<'a>(rendered: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    const ENGLISH: &[&str] = &[
        "the", "to", "your", "you", "is", "are", "and", "of", "this", "that", "on", "with", "for",
        "not", "be", "can", "will", "from", "it", "or", "has", "have", "was", "been", "here",
        "there", "yet", "only", "when", "before", "after", "once", "until", "isn't", "can't",
        "don't", "doesn't", "won't", "couldn't", "an", "by", "at", "into", "any",
    ];
    let spanish: std::collections::HashSet<&str> =
        SPANISH_DESKTOP.values().map(String::as_str).collect();
    let mut leaks = Vec::new();
    for value in rendered {
        let text = value.trim();
        if text.is_empty() || spanish.contains(text) {
            continue;
        }
        let keyed = SPANISH_DESKTOP
            .get(text)
            .or_else(|| SPANISH_DESKTOP.get(value))
            .is_some_and(|es| es.trim() != text);
        let words: Vec<String> = text
            .split(|c: char| !(c.is_ascii_alphabetic() || c == '\'' || c == '’'))
            .filter(|w| !w.is_empty())
            .map(|w| w.replace('’', "'").to_ascii_lowercase())
            .collect();
        let english = words.len() >= 3
            && words
                .iter()
                .filter(|w| ENGLISH.contains(&w.as_str()))
                .count()
                >= 2;
        if (keyed || english) && !leaks.iter().any(|l| l == text) {
            leaks.push(text.to_owned());
        }
    }
    leaks
}

/// Format full UI messages in the accepted language. Both templates are Rust
/// format literals, so argument types and formatting are checked at build time.
#[macro_export]
macro_rules! localized_format {
    ($english:literal, $spanish:literal $(, $($args:tt)*)?) => {{
        match $crate::locale::language() {
            $crate::locale::Language::English => format!($english $(, $($args)*)?),
            $crate::locale::Language::Spanish => format!($spanish $(, $($args)*)?),
        }
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spanish_dates_put_the_day_first_with_spanish_names() {
        let date = chrono::NaiveDate::from_ymd_opt(2026, 11, 3)
            .unwrap()
            .and_hms_opt(18, 24, 0)
            .unwrap();
        let show = |pattern: &str| date.format(&date_pattern(pattern, &date)).to_string();
        assert_eq!(show("%b %d, %Y"), "Nov 03, 2026");
        render_in(Language::Spanish, || {
            assert_eq!(show("%b %d, %Y"), "3 nov 2026");
            assert_eq!(show("%a %-d %b %Y"), "mar 3 nov 2026");
            assert_eq!(show("%A"), "martes");
            assert_eq!(show("%B %d, %Y"), "3 de noviembre de 2026");
            assert_eq!(show("%-d %b %Y, %H:%M UTC"), "3 nov 2026, 18:24 UTC");
        });
    }

    #[test]
    fn english_leaks_flags_skipped_copy_and_unkeyed_prose_only() {
        let leaks = english_leaks([
            "Backup not confirmed",                     // keyed, copy skipped
            "Copia sin confirmar",                      // already Spanish
            "This screen was never translated at all.", // no entry, English prose
            "Bitcoin mainnet",                          // product name
            "Allen Family",                             // user data
            "975,147",
        ]);
        assert_eq!(
            leaks,
            [
                "Backup not confirmed",
                "This screen was never translated at all."
            ]
        );
    }

    #[test]
    fn spanish_error_copy_preserves_network_references_and_raw_detail() {
        let error = "Your Spark wallet isn't available — Nothing was sent and your balances are unchanged. Open the Spark wallet from the dashboard, then start the transfer again. (Ref: CC-XFER-WALLET)";
        let opaque = "Review";
        assert_eq!(notice(error.to_owned()), error);
        render_in(Language::Spanish, || {
            assert_eq!(notice(error.to_owned()), "Tu cartera Spark no está disponible — No se envió nada. Tus saldos siguen intactos. Abre la cartera Spark desde el resumen y vuelve a iniciar la transferencia. (Ref.: CC-XFER-WALLET)");
            assert_eq!(
                notice(format!(
                    "{opaque} Check the amount and fee rate, then try again."
                )),
                "Revisa el importe y la tasa de comisión e inténtalo de nuevo. Detalle: Review"
            );
            assert_eq!(
                notice("SDK diagnostic 0x019af".into()),
                "SDK diagnostic 0x019af"
            );
        });
        assert_eq!(notice(error.to_owned()), error);
    }

    #[test]
    fn spanish_format_preserves_named_and_positional_values() {
        let reference = "CC-XFER-INPUT";
        let format = || {
            localized_format!(
                "Value: {:.2} BTC ({reference})",
                "Importe: {:.2} BTC ({reference})",
                0.01
            )
        };
        assert_eq!(format(), "Value: 0.01 BTC (CC-XFER-INPUT)");
        assert_eq!(
            render_in(Language::Spanish, format),
            "Importe: 0.01 BTC (CC-XFER-INPUT)"
        );
    }
}
