//! The `.lang` translation catalog format.
//!
//! Every catalog names its language in a header, which is where the runtime's
//! `SupportedLocale` comes from: adding a catalog adds a language.
//!
//! ```text
//! # Language: Français
//! ```
//!
//! `liana_en.lang` holds the source text, one value per entry:
//!
//! ```text
//! home-balance => "Balance"
//! ```
//!
//! Every other catalog repeats the English next to its translation, so a
//! translator sees both. `NONE` marks an entry nobody has translated yet; it is
//! a bare word, never a quoted value, so it cannot collide with a translation
//! whose text happens to be "NONE":
//!
//! ```text
//! home-balance => "Balance" => "Solde"
//! home-fiat    => "Fiat"    => NONE
//! ```
//!
//! When the English text changes, `sync` resets the translation to `NONE` and
//! keeps the one it replaced as a note, so a translator sees what the entry said
//! before:
//!
//! ```text
//! home-balance => "Total balance" => NONE # "Solde"
//! ```
//!
//! Entries too long for one line use the block form, terminated by `;;`:
//!
//! ```text
//! home-balance
//! => "Balance"
//! => "Solde";;
//! ```
//!
//! A note follows the terminator there:
//!
//! ```text
//! home-balance
//! => "Total balance"
//! => NONE;; # "Solde"
//! ```

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

/// File name of the source catalog.
pub const ENGLISH_FILE: &str = "liana_en.lang";

/// Header line carrying the language's own name.
const LANGUAGE_HEADER: &str = "# Language:";

const PREFIX: &str = "liana_";
const SUFFIX: &str = ".lang";

/// Entries longer than this are written in the block form.
const INLINE_LIMIT: usize = 85;

/// The bare word marking an entry nobody has translated yet.
const UNTRANSLATED: &str = "NONE";

/// A parsed catalog: the language it holds, and its entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    /// The language's own name, as the UI lists it, e.g. `Français`.
    pub language: String,
    pub entries: Vec<Entry>,
}

/// A catalog the runtime knows about, taken from its file name and header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Locale {
    pub code: String,
    pub language: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Translation {
    Untranslated,
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub english: String,
    /// `None` in the English catalog, which carries a single value per entry.
    pub translated: Option<Translation>,
    /// The translation an English change replaced, kept as a note next to `NONE`.
    pub previous: Option<String>,
}

impl Entry {
    pub fn english(id: impl Into<String>, english: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            english: english.into(),
            translated: None,
            previous: None,
        }
    }

    pub fn translated(
        id: impl Into<String>,
        english: impl Into<String>,
        translated: Translation,
    ) -> Self {
        Self {
            id: id.into(),
            english: english.into(),
            translated: Some(translated),
            previous: None,
        }
    }

    /// Untranslated because the English text changed. `previous` is the
    /// translation that change replaced, when there was one.
    pub fn outdated(
        id: impl Into<String>,
        english: impl Into<String>,
        previous: Option<String>,
    ) -> Self {
        Self {
            id: id.into(),
            english: english.into(),
            translated: Some(Translation::Untranslated),
            previous,
        }
    }
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// The locale code a catalog file name carries, e.g. `liana_pt-PT.lang` -> `pt-PT`.
pub fn locale_from_path(path: &Path) -> Result<&str, String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(PREFIX))
        .and_then(|name| name.strip_suffix(SUFFIX))
        .ok_or_else(|| format!("{}: not a catalog file name", path.display()))
}

/// Every catalog in `dir` except the English source, sorted by locale.
pub fn locale_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(dir).map_err(|err| format!("{}: {err}", dir.display()))? {
        let path = entry
            .map_err(|err| format!("{}: {err}", dir.display()))?
            .path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if name != ENGLISH_FILE && name.starts_with(PREFIX) && name.ends_with(SUFFIX) {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

pub fn parse_file(path: &Path) -> Result<Catalog, String> {
    let source = fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    parse(&source, &path.display().to_string())
}

pub fn parse(source: &str, origin: &str) -> Result<Catalog, String> {
    let lines: Vec<&str> = source.lines().collect();
    let language = lines
        .iter()
        .find_map(|line| line.trim().strip_prefix(LANGUAGE_HEADER))
        .map(str::trim)
        .filter(|language| !language.is_empty())
        .ok_or_else(|| format!("{origin}: no '{LANGUAGE_HEADER}' header"))?
        .to_string();
    let mut entries = Vec::new();
    let mut ids = BTreeSet::new();
    let mut index = 0;

    while index < lines.len() {
        let (line, note) = split_note(lines[index].trim());
        let start = index + 1;
        index += 1;
        if line.is_empty() {
            continue;
        }

        let (id, values, note) = match line.split_once("=>") {
            Some((id, rest)) => (
                id.trim().to_string(),
                parse_inline_values(rest).map_err(|err| format!("{origin}:{start}: {err}"))?,
                note,
            ),
            None => {
                let id = line.to_string();
                let mut values = Vec::new();
                let note = loop {
                    let Some(raw) = lines.get(index) else {
                        return Err(format!("{origin}:{start}: unterminated block entry"));
                    };
                    let (value_line, note) = split_note(raw.trim());
                    index += 1;
                    if value_line.is_empty() {
                        continue;
                    }
                    let end = value_line.ends_with(";;");
                    let source = if end {
                        value_line.trim_end_matches(";;").trim_end()
                    } else {
                        value_line
                    };
                    values.push(
                        parse_block_value(source)
                            .map_err(|err| format!("{origin}:{index}: {err}"))?,
                    );
                    if end {
                        break note;
                    }
                    if note.is_some() {
                        return Err(format!("{origin}:{index}: a note follows the last value"));
                    }
                };
                (id, values, note)
            }
        };
        let previous = note
            .map(parse_note)
            .transpose()
            .map_err(|err| format!("{origin}:{start}: {err}"))?;

        if !valid_id(&id) {
            return Err(format!("{origin}:{start}: invalid id '{id}'"));
        }
        if !ids.insert(id.clone()) {
            return Err(format!("{origin}:{start}: duplicate id '{id}'"));
        }
        let mut values = values.into_iter();
        let english = match values.next() {
            Some(Translation::Text(text)) => text,
            Some(Translation::Untranslated) => {
                return Err(format!(
                    "{origin}:{start}: english cannot be {UNTRANSLATED}"
                ))
            }
            None => return Err(format!("{origin}:{start}: expected one or two values")),
        };
        let translated = values.next();
        if values.next().is_some() {
            return Err(format!("{origin}:{start}: expected one or two values"));
        }
        if previous.is_some() && translated != Some(Translation::Untranslated) {
            return Err(format!(
                "{origin}:{start}: only an {UNTRANSLATED} entry carries a note"
            ));
        }
        entries.push(Entry {
            id,
            english,
            translated,
            previous,
        });
    }

    Ok(Catalog { language, entries })
}

/// Splits a line at the `#` starting its note, if any. A `#` inside a quoted
/// value belongs to the value.
fn split_note(line: &str) -> (&str, Option<&str>) {
    let mut quoted = false;
    let mut escaped = false;
    for (offset, ch) in line.char_indices() {
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            quoted = !quoted;
        } else if ch == '#' && !quoted {
            return (line[..offset].trim_end(), Some(&line[offset + 1..]));
        }
    }
    (line, None)
}

/// The quoted translation a note carries.
fn parse_note(source: &str) -> Result<String, String> {
    let (text, rest) = parse_quoted(source.trim_start())?;
    if rest.trim().is_empty() {
        Ok(text)
    } else {
        Err("unexpected trailing text".to_string())
    }
}

fn parse_inline_values(mut source: &str) -> Result<Vec<Translation>, String> {
    let mut values = Vec::new();
    loop {
        let (value, rest) = parse_value(source.trim_start())?;
        values.push(value);
        source = rest.trim_start();
        match source.strip_prefix("=>") {
            Some(rest) => source = rest,
            None if source.is_empty() => return Ok(values),
            None => return Err("unexpected trailing text".to_string()),
        }
    }
}

fn parse_block_value(source: &str) -> Result<Translation, String> {
    let source = source
        .trim_start()
        .strip_prefix("=>")
        .ok_or("expected '=>' value")?;
    let (value, rest) = parse_value(source.trim_start())?;
    if rest.trim().is_empty() {
        Ok(value)
    } else {
        Err("unexpected trailing text".to_string())
    }
}

/// A quoted string, or the bare `NONE` marker.
fn parse_value(source: &str) -> Result<(Translation, &str), String> {
    if let Some(rest) = source.strip_prefix(UNTRANSLATED) {
        if rest.trim_start().is_empty() || rest.starts_with("=>") {
            return Ok((Translation::Untranslated, rest));
        }
    }
    let (text, rest) = parse_quoted(source)?;
    Ok((Translation::Text(text), rest))
}

fn parse_quoted(source: &str) -> Result<(String, &str), String> {
    if !source.starts_with('"') {
        return Err(format!("expected a quoted value or {UNTRANSLATED}"));
    }

    let mut out = String::new();
    let mut escaped = false;

    for (offset, ch) in source[1..].char_indices() {
        if escaped {
            match ch {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                other => out.push(other),
            }
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Ok((out, &source[1 + offset + ch.len_utf8()..]));
        } else {
            out.push(ch);
        }
    }

    if escaped {
        return Err("unfinished escape".to_string());
    }
    Err("unterminated quoted value".to_string())
}

fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
        .replace('"', "\\\"")
}

pub fn render(catalog: &Catalog, english_only: bool) -> String {
    let format = if english_only {
        r#"# Format: id => "English""#
    } else {
        r#"# Format: id => "English" => "Translation""#
    };
    let mut lines = vec![
        "# Translator-friendly catalog".to_string(),
        format!("{LANGUAGE_HEADER} {}", catalog.language),
        format.to_string(),
        "# Use \\n for line breaks".to_string(),
        String::new(),
    ];
    let mut entries: Vec<&Entry> = catalog.entries.iter().collect();
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    for entry in entries {
        if !lines.last().is_some_and(String::is_empty) {
            lines.push(String::new());
        }
        lines.extend(render_entry(entry, english_only));
    }
    lines.push(String::new());
    lines.join("\n")
}

fn render_entry(entry: &Entry, english_only: bool) -> Vec<String> {
    let english = escape(&entry.english);
    if english_only {
        let inline = format!("{} => \"{english}\"", entry.id);
        if inline.chars().count() <= INLINE_LIMIT {
            return vec![inline];
        }
        return vec![entry.id.clone(), format!("=> \"{english}\";;")];
    }

    let translated = match entry.translated.as_ref() {
        Some(Translation::Text(text)) => format!("\"{}\"", escape(text)),
        _ => UNTRANSLATED.to_string(),
    };
    let note = match entry.previous.as_ref() {
        Some(previous) => format!(" # \"{}\"", escape(previous)),
        None => String::new(),
    };
    // The note is left out of the width, so it never changes an entry's shape.
    let inline = format!("{} => \"{english}\" => {translated}", entry.id);
    if inline.chars().count() <= INLINE_LIMIT {
        return vec![format!("{inline}{note}")];
    }
    vec![
        entry.id.clone(),
        format!("=> \"{english}\""),
        format!("=> {translated};;{note}"),
    ]
}

pub fn write_lang(path: &Path, catalog: &Catalog, english_only: bool) -> Result<(), String> {
    fs::write(path, render(catalog, english_only))
        .map_err(|err| format!("{}: {err}", path.display()))
}

/// Write the Fluent resource a catalog compiles to. Untranslated entries are
/// left out so the runtime falls back to the English bundle.
pub fn write_ftl(path: &Path, entries: &[Entry], english: bool) -> Result<(), String> {
    let mut output = String::new();
    for entry in entries {
        let value = if english {
            &entry.english
        } else {
            match entry.translated.as_ref() {
                Some(Translation::Text(text)) => text,
                Some(Translation::Untranslated) => continue,
                None => return Err(format!("missing translation for '{}'", entry.id)),
            }
        };
        output.push_str(&entry.id);
        output.push_str(" = ");
        // Fluent needs the continuation lines of a multi-line value indented.
        output.push_str(&value.replace('\n', "\n    "));
        output.push('\n');
    }
    fs::write(path, output).map_err(|err| format!("{}: {err}", path.display()))
}

/// The variant name a locale code takes, e.g. `pt-PT` -> `PtPt`.
fn variant(code: &str) -> String {
    code.split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => {
                    first.to_ascii_uppercase().to_string() + &chars.as_str().to_lowercase()
                }
                None => String::new(),
            }
        })
        .collect()
}

/// The `SupportedLocale` data the runtime compiles in, one variant per catalog.
/// Only data: the runtime keeps the behaviour that reads it.
pub fn render_locales(locales: &[Locale]) -> String {
    let variants: Vec<String> = locales.iter().map(|locale| variant(&locale.code)).collect();
    let arms = |value: &dyn Fn(&Locale) -> String| -> String {
        variants
            .iter()
            .zip(locales)
            .map(|(variant, locale)| {
                format!(
                    "            SupportedLocale::{variant} => {},\n",
                    value(locale)
                )
            })
            .collect()
    };

    let declarations: String = variants
        .iter()
        .zip(locales)
        .map(|(variant, locale)| {
            let default = if locale.code == "en" {
                "    #[default]\n"
            } else {
                ""
            };
            format!("{default}    {variant},\n")
        })
        .collect();
    let all: String = variants
        .iter()
        .map(|variant| format!("        SupportedLocale::{variant},\n"))
        .collect();

    format!(
        "// Generated from the catalogs in i18n/ by liana-i18n-toolbox. Do not edit.\n\
         \n\
         #[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]\n\
         #[repr(usize)]\n\
         pub enum SupportedLocale {{\n\
         {declarations}}}\n\
         \n\
         impl SupportedLocale {{\n\
         \x20   pub const ALL: [SupportedLocale; {count}] = [\n\
         {all}    ];\n\
         \n\
         \x20   /// The locale code, as the catalog file name carries it.\n\
         \x20   pub fn code(self) -> &'static str {{\n\
         \x20       match self {{\n\
         {codes}        }}\n\
         \x20   }}\n\
         \n\
         \x20   /// The language's own name.\n\
         \x20   pub fn label(self) -> &'static str {{\n\
         \x20       match self {{\n\
         {labels}        }}\n\
         \x20   }}\n\
         \n\
         \x20   fn source(self) -> &'static str {{\n\
         \x20       match self {{\n\
         {sources}        }}\n\
         \x20   }}\n\
         }}\n",
        count = locales.len(),
        codes = arms(&|locale| format!("{:?}", locale.code)),
        labels = arms(&|locale| format!("{:?}", locale.language)),
        sources = arms(&|locale| format!(
            "include_str!(concat!(env!(\"OUT_DIR\"), \"/{}.ftl\"))",
            locale.code
        )),
    )
}

/// A locale catalog must carry the same ids in the same order as the English
/// source, with the same English text next to each.
pub fn verify_locale(english: &[Entry], locale: &[Entry], origin: &str) -> Result<(), String> {
    if english.len() != locale.len()
        || english
            .iter()
            .zip(locale)
            .any(|(source, entry)| source.id != entry.id)
    {
        return Err(format!("{origin} is not synced with {ENGLISH_FILE}"));
    }
    for (source, entry) in english.iter().zip(locale) {
        if source.english != entry.english {
            return Err(format!("{origin}: english text changed for '{}'", entry.id));
        }
    }
    Ok(())
}

/// The locale catalog as it should be: English text refreshed from the source,
/// entries the locale is missing appended as untranslated, entries the source
/// dropped removed, all in the source's order. A translation of English text
/// that changed is dropped to `NONE` and kept as a note, so only the English
/// catalog needs care during a dev phase and translators review every `NONE`
/// once before a release.
pub fn sync(english: &[Entry], locale: &[Entry]) -> Vec<Entry> {
    english
        .iter()
        .map(|source| {
            let Some(entry) = locale.iter().find(|entry| entry.id == source.id) else {
                return Entry::translated(&source.id, &source.english, Translation::Untranslated);
            };
            match (&entry.translated, entry.english == source.english) {
                (Some(Translation::Text(text)), true) => {
                    Entry::translated(&source.id, &source.english, Translation::Text(text.clone()))
                }
                (Some(Translation::Text(text)), false) => {
                    Entry::outdated(&source.id, &source.english, Some(text.clone()))
                }
                // Already untranslated: keep the note, which holds the last
                // translation a human wrote however often the English changes.
                _ => Entry::outdated(&source.id, &source.english, entry.previous.clone()),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> Translation {
        Translation::Text(value.to_string())
    }

    fn note(value: &str) -> Option<String> {
        Some(value.to_string())
    }

    fn catalog(entries: Vec<Entry>) -> Catalog {
        Catalog {
            language: "Français".to_string(),
            entries,
        }
    }

    /// A catalog source, with the header every catalog carries.
    fn source(body: &str) -> String {
        format!("# Language: Français\n{body}")
    }

    #[test]
    fn parses_the_english_source() {
        let parsed = parse(&source("home-balance => \"Balance\"\n"), "en").unwrap();
        assert_eq!(
            parsed,
            catalog(vec![Entry::english("home-balance", "Balance")])
        );
    }

    #[test]
    fn parses_inline_and_block_forms_alike() {
        let body = concat!(
            "# a comment\n",
            "home-balance => \"Balance\" => \"Solde\"\n",
            "\n",
            "home-long\n",
            "=> \"English\"\n",
            "=> \"Traduction\";;\n",
        );
        assert_eq!(
            parse(&source(body), "fr").unwrap(),
            catalog(vec![
                Entry::translated("home-balance", "Balance", text("Solde")),
                Entry::translated("home-long", "English", text("Traduction")),
            ])
        );
    }

    #[test]
    fn none_is_a_bare_word_not_a_translation() {
        let entries = parse(
            &source("home-balance => \"Balance\" => NONE\nhome-fiat => \"Fiat\" => \"NONE\"\n"),
            "fr",
        )
        .unwrap()
        .entries;
        assert_eq!(entries[0].translated, Some(Translation::Untranslated));
        assert_eq!(entries[1].translated, Some(text("NONE")));
    }

    #[test]
    fn none_survives_a_round_trip_in_both_forms() {
        let long = "e".repeat(INLINE_LIMIT);
        let entries = catalog(vec![
            Entry::translated("short", "Balance", Translation::Untranslated),
            Entry::translated("long", &long, Translation::Untranslated),
        ]);
        let rendered = render(&entries, false);
        assert!(rendered.contains("short => \"Balance\" => NONE\n"));
        assert!(rendered.contains("=> NONE;;\n"));
        assert_eq!(
            parse(&rendered, "fr").unwrap(),
            catalog(vec![
                Entry::translated("long", &long, Translation::Untranslated),
                Entry::translated("short", "Balance", Translation::Untranslated),
            ])
        );
    }

    #[test]
    fn render_sorts_entries_by_id() {
        let entries = catalog(vec![
            Entry::english("second", "Second"),
            Entry::english("first", "First"),
        ]);

        let rendered = render(&entries, true);

        assert!(rendered.find("first =>").unwrap() < rendered.find("second =>").unwrap());
    }

    #[test]
    fn round_trips_escapes() {
        let entries = catalog(vec![Entry::translated(
            "escaped",
            "a\nb\tc\"d\\e",
            text("f\ng\th\"i\\j"),
        )]);
        assert_eq!(parse(&render(&entries, false), "fr").unwrap(), entries);
    }

    #[test]
    fn untranslated_entries_are_left_out_of_the_fluent_resource() {
        let dir = std::env::temp_dir().join("liana-i18n-toolbox-ftl");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fr.ftl");
        write_ftl(
            &path,
            &[
                Entry::translated("kept", "Balance", text("Solde")),
                Entry::translated("skipped", "Fiat", Translation::Untranslated),
            ],
            false,
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "kept = Solde\n");
    }

    #[test]
    fn rejects_malformed_entries() {
        for body in [
            "home-balance => \"Balance\" => \"Solde\" => \"extra\"\n",
            "Home-Balance => \"Balance\"\n",
            "home-balance => \"Balance\"\nhome-balance => \"Again\"\n",
            "home-balance => \"unterminated\n",
            "home-balance\n=> \"no terminator\"\n",
            "home-balance => NONE\n",
            "home-balance => \"Balance\" => \"Solde\" # \"Ancien\"\n",
            "home-balance => \"Balance\" # \"Ancien\"\n",
            "home-balance => \"Balance\" => NONE # Solde\n",
            "home-balance => \"Balance\" => NONE # \"unterminated\n",
            "home-balance\n=> \"Balance\" # \"Ancien\"\n=> NONE;;\n",
        ] {
            assert!(parse(&source(body), "fr").is_err(), "accepted {body:?}");
        }
        assert!(
            parse("home-balance => \"Balance\"\n", "fr").is_err(),
            "accepted a catalog with no language header"
        );
    }

    #[test]
    fn a_note_survives_a_round_trip_in_both_forms() {
        let long = "e".repeat(INLINE_LIMIT);
        let entries = catalog(vec![
            Entry::outdated("short", "Balance", note("Solde")),
            Entry::outdated("long", &long, note("Traduction")),
        ]);
        let rendered = render(&entries, false);
        assert!(rendered.contains("short => \"Balance\" => NONE # \"Solde\"\n"));
        assert!(rendered.contains("=> NONE;; # \"Traduction\"\n"));
        assert_eq!(
            parse(&rendered, "fr").unwrap(),
            catalog(vec![
                Entry::outdated("long", &long, note("Traduction")),
                Entry::outdated("short", "Balance", note("Solde")),
            ])
        );
    }

    #[test]
    fn round_trips_a_note_with_marks_and_line_breaks() {
        let entries = catalog(vec![Entry::outdated(
            "escaped",
            "Balance",
            note("a#b\"c\nd"),
        )]);
        assert_eq!(parse(&render(&entries, false), "fr").unwrap(), entries);
    }

    #[test]
    fn a_hash_inside_a_value_is_not_a_note() {
        let parsed = parse(
            &source("home-balance => \"Bloc #1\" => \"Bloc #1\"\n"),
            "fr",
        )
        .unwrap();
        assert_eq!(
            parsed,
            catalog(vec![Entry::translated(
                "home-balance",
                "Bloc #1",
                text("Bloc #1")
            )])
        );
    }

    #[test]
    fn sync_refreshes_english_and_fills_gaps() {
        let english = vec![
            Entry::english("kept", "Balance"),
            Entry::english("added", "Fiat"),
        ];
        let locale = vec![
            Entry::translated("kept", "Balance", text("Solde")),
            Entry::translated("dropped", "Gone", text("Parti")),
        ];
        assert_eq!(
            sync(&english, &locale),
            vec![
                Entry::translated("kept", "Balance", text("Solde")),
                Entry::translated("added", "Fiat", Translation::Untranslated),
            ]
        );
    }

    #[test]
    fn sync_resets_the_translation_of_changed_english() {
        let english = vec![
            Entry::english("translated", "Total balance"),
            Entry::english("untranslated", "Total fiat"),
            Entry::english("noted", "Total coins"),
        ];
        let locale = vec![
            Entry::translated("translated", "Balance", text("Solde")),
            Entry::translated("untranslated", "Fiat", Translation::Untranslated),
            Entry::outdated("noted", "Coins", note("Pieces")),
        ];
        assert_eq!(
            sync(&english, &locale),
            vec![
                Entry::outdated("translated", "Total balance", note("Solde")),
                Entry::outdated("untranslated", "Total fiat", None),
                // A second English change keeps the last translation a human wrote.
                Entry::outdated("noted", "Total coins", note("Pieces")),
            ]
        );
    }

    #[test]
    fn sync_drops_the_note_once_the_entry_is_translated_again() {
        let english = vec![Entry::english("noted", "Total balance")];
        let locale = vec![Entry {
            previous: note("Solde"),
            ..Entry::translated("noted", "Total balance", text("Solde totale"))
        }];
        assert_eq!(
            sync(&english, &locale),
            vec![Entry::translated(
                "noted",
                "Total balance",
                text("Solde totale")
            )]
        );
    }

    #[test]
    fn generates_one_variant_per_catalog() {
        let locales = [
            Locale {
                code: "en".to_string(),
                language: "English".to_string(),
            },
            Locale {
                code: "pt-PT".to_string(),
                language: "Português (Portugal)".to_string(),
            },
        ];
        assert_eq!(
            render_locales(&locales),
            concat!(
                "// Generated from the catalogs in i18n/ by liana-i18n-toolbox. Do not edit.\n",
                "\n",
                "#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]\n",
                "#[repr(usize)]\n",
                "pub enum SupportedLocale {\n",
                "    #[default]\n",
                "    En,\n",
                "    PtPt,\n",
                "}\n",
                "\n",
                "impl SupportedLocale {\n",
                "    pub const ALL: [SupportedLocale; 2] = [\n",
                "        SupportedLocale::En,\n",
                "        SupportedLocale::PtPt,\n",
                "    ];\n",
                "\n",
                "    /// The locale code, as the catalog file name carries it.\n",
                "    pub fn code(self) -> &'static str {\n",
                "        match self {\n",
                "            SupportedLocale::En => \"en\",\n",
                "            SupportedLocale::PtPt => \"pt-PT\",\n",
                "        }\n",
                "    }\n",
                "\n",
                "    /// The language's own name.\n",
                "    pub fn label(self) -> &'static str {\n",
                "        match self {\n",
                "            SupportedLocale::En => \"English\",\n",
                "            SupportedLocale::PtPt => \"Português (Portugal)\",\n",
                "        }\n",
                "    }\n",
                "\n",
                "    fn source(self) -> &'static str {\n",
                "        match self {\n",
                "            SupportedLocale::En => include_str!(concat!(env!(\"OUT_DIR\"), \"/en.ftl\")),\n",
                "            SupportedLocale::PtPt => include_str!(concat!(env!(\"OUT_DIR\"), \"/pt-PT.ftl\")),\n",
                "        }\n",
                "    }\n",
                "}\n",
            )
        );
    }

    #[test]
    fn verify_locale_catches_drift() {
        let english = vec![Entry::english("kept", "Balance")];
        assert!(verify_locale(&english, &[], "fr").is_err());
        assert!(verify_locale(
            &english,
            &[Entry::translated("kept", "stale", text("Solde"))],
            "fr"
        )
        .is_err());
        assert!(verify_locale(
            &english,
            &[Entry::translated("kept", "Balance", text("Solde"))],
            "fr"
        )
        .is_ok());
    }
}
