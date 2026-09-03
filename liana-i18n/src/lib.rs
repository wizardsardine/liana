use std::{
    borrow::Cow,
    fmt,
    str::FromStr,
    sync::{LazyLock, RwLock},
};

pub use fluent_bundle::FluentValue;
use fluent_bundle::{concurrent::FluentBundle, FluentArgs, FluentResource};
use unic_langid::LanguageIdentifier;

// The enum, its codes, labels and resources come from the catalogs in i18n/,
// so adding a catalog is all it takes to add a language.
include!(concat!(env!("OUT_DIR"), "/locales.rs"));

impl SupportedLocale {
    fn langid(self) -> LanguageIdentifier {
        self.code().parse().expect("catalog locale must parse")
    }

    pub fn from_system() -> Self {
        sys_locale::get_locale()
            .and_then(|locale| SupportedLocale::from_str(&locale).ok())
            .unwrap_or(SupportedLocale::En)
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
        let normalized = value.split(['.', '@']).next().ok_or(())?.replace('_', "-");
        let langid = normalized.parse::<LanguageIdentifier>().map_err(|_| ())?;

        // An exact code wins, then any catalog for the same language, so a
        // generic "pt" takes the Portugal catalog until another variant exists.
        Self::ALL
            .iter()
            .find(|locale| locale.langid() == langid)
            .or_else(|| {
                Self::ALL
                    .iter()
                    .find(|locale| locale.langid().language == langid.language)
            })
            .copied()
            .ok_or(())
    }
}

type Bundle = FluentBundle<FluentResource>;

fn build_bundle(locale: SupportedLocale, source: &str) -> Bundle {
    let resource = FluentResource::try_new(source.to_string())
        .unwrap_or_else(|(_, errors)| panic!("invalid {} locale: {errors:?}", locale.code()));
    let mut bundle = FluentBundle::new_concurrent(vec![locale.langid()]);
    bundle
        .add_resource(resource)
        .unwrap_or_else(|errors| panic!("invalid {} locale bundle: {errors:?}", locale.code()));
    bundle
}

static BUNDLES: LazyLock<[Bundle; SupportedLocale::ALL.len()]> =
    LazyLock::new(|| SupportedLocale::ALL.map(|locale| build_bundle(locale, locale.source())));

static CURRENT_LOCALE: RwLock<SupportedLocale> = RwLock::new(SupportedLocale::En);

pub fn init(locale: SupportedLocale) {
    LazyLock::force(&BUNDLES);
    set_locale(locale);
}

pub fn current_locale() -> SupportedLocale {
    *CURRENT_LOCALE.read().expect("locale lock poisoned")
}

pub fn set_locale(locale: SupportedLocale) {
    *CURRENT_LOCALE.write().expect("locale lock poisoned") = locale;
}

#[doc(hidden)]
pub struct FluentArgument<T>(pub T);

#[doc(hidden)]
pub trait IntoFluentArgument {
    fn into_fluent_argument(self) -> FluentValue<'static>;
}

// Native Fluent values keep their type; display-only values use the auto-borrowed fallback.
impl<'a, T: ?Sized> IntoFluentArgument for FluentArgument<&'a T>
where
    &'a T: Into<FluentValue<'a>>,
{
    fn into_fluent_argument(self) -> FluentValue<'static> {
        let value: FluentValue<'a> = self.0.into();
        value.into_owned()
    }
}

impl<T: ToString + ?Sized> IntoFluentArgument for &FluentArgument<&T> {
    fn into_fluent_argument(self) -> FluentValue<'static> {
        FluentValue::String(Cow::Owned(self.0.to_string()))
    }
}

pub fn translate(key: &str, args: &[(&str, FluentValue<'static>)]) -> String {
    translate_with_fallback(
        bundle(current_locale()),
        bundle(SupportedLocale::En),
        key,
        args,
    )
    .unwrap_or_else(|| key.to_string())
}

fn bundle(locale: SupportedLocale) -> &'static Bundle {
    &BUNDLES[locale as usize]
}

fn translate_with_fallback(
    bundle: &Bundle,
    fallback: &Bundle,
    key: &str,
    args: &[(&str, FluentValue<'static>)],
) -> Option<String> {
    format_message(bundle, key, args).or_else(|| format_message(fallback, key, args))
}

fn format_message(
    bundle: &Bundle,
    key: &str,
    args: &[(&str, FluentValue<'static>)],
) -> Option<String> {
    let message = bundle.get_message(key)?;
    let pattern = message.value()?;
    let mut fluent_args = FluentArgs::with_capacity(args.len());
    for (name, value) in args {
        fluent_args.set(*name, value.clone());
    }
    let mut errors = Vec::new();
    let translation = bundle.format_pattern(pattern, Some(&fluent_args), &mut errors);
    errors.is_empty().then(|| translation.into_owned())
}

#[macro_export]
macro_rules! t {
    ($key:literal) => {
        $crate::translate($key, &[])
    };
    ($key:literal, $($name:ident = $value:expr),+ $(,)?) => {{
        let args = &[$((stringify!($name), {
            use $crate::IntoFluentArgument as _;
            $crate::FluentArgument(&($value)).into_fluent_argument()
        })),+];
        $crate::translate($key, args)
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn supported_locales_parse() {
        assert_eq!(SupportedLocale::from_str("en-US"), Ok(SupportedLocale::En));
        assert_eq!(SupportedLocale::from_str("it-IT"), Ok(SupportedLocale::It));
        assert_eq!(SupportedLocale::from_str("en--US"), Err(()));
        assert_eq!(SupportedLocale::from_str("english"), Err(()));
        assert_eq!(SupportedLocale::from_str("de-DE"), Err(()));
    }

    #[test]
    fn locale_resources_are_valid() {
        LazyLock::force(&BUNDLES);
    }

    #[test]
    fn translates_with_english_fallback() {
        let english = build_bundle(SupportedLocale::En, "message = English\n");
        let missing = build_bundle(SupportedLocale::En, "other = Other\n");
        let formatting_error = build_bundle(SupportedLocale::En, "message = { $missing }\n");

        assert_eq!(
            translate_with_fallback(&missing, &english, "message", &[]),
            Some("English".to_string())
        );
        assert_eq!(
            translate_with_fallback(&formatting_error, &english, "message", &[]),
            Some("English".to_string())
        );
        assert_eq!(translate("missing-key", &[]), "missing-key");
    }

    // What `t!` does with its arguments, minus the global catalog it reads: the
    // catalog holds the application's strings, not fixtures for these tests.
    #[test]
    fn formats_zero_one_and_other_plurals() {
        let bundle = build_bundle(
            SupportedLocale::En,
            "signatures = { $count ->\n    [0] none\n    [one] one more\n   *[other] {$count} more\n}\n",
        );
        let signatures = |count: i32| {
            let args = &[("count", FluentArgument(&count).into_fluent_argument())];
            format_message(&bundle, "signatures", args)
        };

        assert_eq!(signatures(0), Some("none".to_string()));
        assert_eq!(signatures(1), Some("one more".to_string()));
        assert_eq!(signatures(2), Some("\u{2068}2\u{2069} more".to_string()));
    }

    #[test]
    fn formats_string_arguments() {
        let bundle = build_bundle(SupportedLocale::En, "wallet = My Liana {$network} wallet\n");
        let args = &[("network", FluentArgument(&"testnet").into_fluent_argument())];

        assert_eq!(
            format_message(&bundle, "wallet", args),
            Some("My Liana \u{2068}testnet\u{2069} wallet".to_string())
        );
    }

    // A locale carries only the entries somebody translated: the rest is left out
    // of its resource and falls back to English.
    #[test]
    fn translated_entries_match_the_english_schema() {
        let english = schema(SupportedLocale::En.source());
        for locale in SupportedLocale::ALL
            .into_iter()
            .filter(|locale| *locale != SupportedLocale::En)
        {
            for (message, variables) in schema(locale.source()) {
                assert_eq!(
                    english.get(&message),
                    Some(&variables),
                    "{} schema for {message}",
                    locale.code()
                );
            }
        }
    }

    #[test]
    fn an_untranslated_entry_is_not_a_schema_mismatch() {
        let english = schema("kept = Balance\nskipped = { $count } coins\n");
        let locale = schema("kept = Solde\n");

        assert!(locale
            .iter()
            .all(|(message, variables)| english.get(message) == Some(variables)));
        assert_ne!(
            schema("kept = { $total } coins\n").get("kept"),
            english.get("kept"),
            "a translation using another variable is a mismatch"
        );
    }

    fn schema(source: &str) -> BTreeMap<String, BTreeSet<String>> {
        let mut schema = BTreeMap::<String, BTreeSet<String>>::new();
        let mut current = None;

        for line in source.lines() {
            if line.trim_start().len() == line.len() {
                current = line.split_once('=').and_then(|(key, _)| {
                    let key = key.trim();
                    (!key.is_empty() && !key.starts_with('#')).then(|| key.to_string())
                });
                if let Some(key) = &current {
                    schema.entry(key.clone()).or_default();
                }
            }

            if let Some(key) = &current {
                schema
                    .get_mut(key)
                    .expect("current message is in the schema")
                    .extend(variables(line).into_iter().map(str::to_string));
            }
        }

        schema
    }

    fn variables(line: &str) -> Vec<&str> {
        let bytes = line.as_bytes();
        let mut variables = Vec::new();
        let mut index = 0;

        while index < bytes.len() {
            if bytes[index] != b'$' {
                index += 1;
                continue;
            }

            let start = index + 1;
            let mut end = start;
            while end < bytes.len()
                && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'-' || bytes[end] == b'_')
            {
                end += 1;
            }
            if end > start {
                variables.push(&line[start..end]);
            }
            index = end;
        }

        variables
    }
}
