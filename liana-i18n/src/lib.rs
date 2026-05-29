use std::{borrow::Cow, fmt, str::FromStr, sync::RwLock};

use fluent_bundle::{FluentArgs, FluentBundle, FluentResource, FluentValue};
use once_cell::sync::Lazy;
use unic_langid::LanguageIdentifier;

const EN: &str = include_str!("../locales/en.ftl");
const PT_PT: &str = include_str!("../locales/pt-PT.ftl");
const IT: &str = include_str!("../locales/it.ftl");
const FR: &str = include_str!("../locales/fr.ftl");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SupportedLocale {
    En,
    PtPt,
    It,
    Fr,
}

impl SupportedLocale {
    pub const ALL: [SupportedLocale; 4] = [
        SupportedLocale::En,
        SupportedLocale::PtPt,
        SupportedLocale::It,
        SupportedLocale::Fr,
    ];

    pub fn code(self) -> &'static str {
        match self {
            SupportedLocale::En => "en",
            SupportedLocale::PtPt => "pt-PT",
            SupportedLocale::It => "it",
            SupportedLocale::Fr => "fr",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SupportedLocale::En => "English",
            SupportedLocale::PtPt => "Português (Portugal)",
            SupportedLocale::It => "Italiano",
            SupportedLocale::Fr => "Français",
        }
    }

    fn langid(self) -> LanguageIdentifier {
        self.code().parse().expect("hardcoded locale must parse")
    }

    fn source(self) -> &'static str {
        match self {
            SupportedLocale::En => EN,
            SupportedLocale::PtPt => PT_PT,
            SupportedLocale::It => IT,
            SupportedLocale::Fr => FR,
        }
    }

    pub fn from_system() -> Self {
        sys_locale::get_locale()
            .and_then(|locale| SupportedLocale::from_str(&locale).ok())
            .unwrap_or(SupportedLocale::En)
    }
}

impl Default for SupportedLocale {
    fn default() -> Self {
        SupportedLocale::En
    }
}

impl fmt::Display for SupportedLocale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl FromStr for SupportedLocale {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let normalized = value.replace('_', "-").to_ascii_lowercase();
        if normalized == "en" || normalized.starts_with("en-") {
            Ok(SupportedLocale::En)
        } else if normalized == "pt" || normalized.starts_with("pt-") {
            // Prefer the Portugal translation for generic Portuguese until another variant exists.
            Ok(SupportedLocale::PtPt)
        } else if normalized == "it" || normalized.starts_with("it-") {
            Ok(SupportedLocale::It)
        } else if normalized == "fr" || normalized.starts_with("fr-") {
            Ok(SupportedLocale::Fr)
        } else {
            Err(())
        }
    }
}

fn bundle(locale: SupportedLocale) -> FluentBundle<FluentResource> {
    let resource = FluentResource::try_new(locale.source().to_string())
        .unwrap_or_else(|(_, errors)| panic!("invalid {} locale: {errors:?}", locale.code()));
    let mut bundle = FluentBundle::new(vec![locale.langid()]);
    bundle
        .add_resource(resource)
        .unwrap_or_else(|errors| panic!("invalid {} locale bundle: {errors:?}", locale.code()));
    bundle
}

static CURRENT_LOCALE: Lazy<RwLock<SupportedLocale>> =
    Lazy::new(|| RwLock::new(SupportedLocale::En));

pub fn init(locale: SupportedLocale) {
    set_locale(locale);
}

pub fn current_locale() -> SupportedLocale {
    CURRENT_LOCALE
        .read()
        .map(|locale| *locale)
        .unwrap_or_default()
}

pub fn set_locale(locale: SupportedLocale) {
    if let Ok(mut current) = CURRENT_LOCALE.write() {
        *current = locale;
    }
}

pub fn translate(key: &str, args: &[(&str, String)]) -> String {
    translate_with_locale(current_locale(), key, args)
        .or_else(|| translate_with_locale(SupportedLocale::En, key, args))
        .unwrap_or_else(|| key.to_string())
}

fn translate_with_locale(
    locale: SupportedLocale,
    key: &str,
    args: &[(&str, String)],
) -> Option<String> {
    let bundle = bundle(locale);
    let message = bundle.get_message(key)?;
    let pattern = message.value()?;
    let mut fluent_args = FluentArgs::new();
    for (name, value) in args {
        fluent_args.set(*name, FluentValue::String(Cow::Owned(value.clone())));
    }
    let mut errors = Vec::new();
    Some(
        bundle
            .format_pattern(pattern, Some(&fluent_args), &mut errors)
            .to_string(),
    )
}

#[macro_export]
macro_rules! t {
    ($key:literal) => {
        $crate::translate($key, &[])
    };
    ($key:literal, $($name:ident = $value:expr),+ $(,)?) => {{
        let args = &[$((stringify!($name), ($value).to_string())),+];
        $crate::translate($key, args)
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_locales_parse() {
        assert_eq!(SupportedLocale::from_str("en-US"), Ok(SupportedLocale::En));
        assert_eq!(
            SupportedLocale::from_str("pt_PT"),
            Ok(SupportedLocale::PtPt)
        );
        assert_eq!(SupportedLocale::from_str("it-IT"), Ok(SupportedLocale::It));
        assert_eq!(SupportedLocale::from_str("fr-FR"), Ok(SupportedLocale::Fr));
    }

    #[test]
    fn translates_with_fallback() {
        init(SupportedLocale::Fr);
        assert_eq!(translate("settings-language", &[]), "Langue");
        assert_eq!(translate("missing-key", &[]), "missing-key");
    }

    #[test]
    fn locale_files_have_matching_keys() {
        let english = keys(EN);
        for source in [PT_PT, IT, FR] {
            assert_eq!(english, keys(source));
        }
    }

    fn keys(source: &str) -> Vec<&str> {
        source
            .lines()
            .filter_map(|line| line.split_once('=').map(|(key, _)| key.trim()))
            .filter(|key| !key.is_empty() && !key.starts_with('#'))
            .collect()
    }
}
