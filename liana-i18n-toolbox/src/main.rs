//! Maintains the `.lang` translation catalogs under `liana-i18n/i18n`.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::ExitCode,
};

use liana_i18n_toolbox::{
    locale_files, parse_file, render, sync, valid_id, write_lang, Catalog, Entry, Translation,
    ENGLISH_FILE,
};

const USAGE: &str = "\
usage:
  liana-i18n-toolbox sync [--verify]     rewrite every locale catalog from the English source
  liana-i18n-toolbox check-ids           check the i18n ids used in the Rust sources
  liana-i18n-toolbox add-entry <id> <english>
  liana-i18n-toolbox add-lang <locale> <language>";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate lives in the workspace")
        .to_path_buf()
}

fn lang_dir() -> PathBuf {
    repo_root().join("liana-i18n").join("i18n")
}

/// The path as written in the repository, so output does not depend on where
/// the checkout lives.
fn relative(path: &Path) -> &Path {
    path.strip_prefix(repo_root()).unwrap_or(path)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
    let result = match args.first().map(String::as_str) {
        Some("sync") => sync_catalogs(rest.contains(&"--verify")),
        Some("check-ids") => check_ids(),
        Some("add-entry") => match rest.as_slice() {
            [id, english] => add_entry(id, english),
            _ => usage(),
        },
        Some("add-lang") => match rest.as_slice() {
            [locale, language] => add_lang(locale, language),
            _ => usage(),
        },
        _ => usage(),
    };

    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn usage() -> Result<bool, String> {
    eprintln!("{USAGE}");
    Ok(false)
}

/// Rewrite each locale catalog from the English source, or report the ones that
/// would change. Returns whether the catalogs are (or were made) consistent.
fn sync_catalogs(verify: bool) -> Result<bool, String> {
    let dir = lang_dir();
    let en_path = dir.join(ENGLISH_FILE);
    let english = parse_file(&en_path)?;
    let mut clean = true;

    let rendered = render(&english, true);
    let current = std::fs::read_to_string(&en_path).map_err(|err| err.to_string())?;
    if current != rendered {
        clean = false;
        if verify {
            println!("outdated: {}", relative(&en_path).display());
        } else {
            write_lang(&en_path, &english, true)?;
            println!("synced: {}", relative(&en_path).display());
        }
    }

    for path in locale_files(&dir)? {
        let catalog = parse_file(&path)?;
        let synced = Catalog {
            language: catalog.language,
            entries: sync(&english.entries, &catalog.entries),
        };
        let rendered = render(&synced, false);
        let current = std::fs::read_to_string(&path).map_err(|err| err.to_string())?;
        if current == rendered {
            continue;
        }
        clean = false;
        if verify {
            println!("outdated: {}", relative(&path).display());
        } else {
            write_lang(&path, &synced, false)?;
            println!("synced: {}", relative(&path).display());
        }
    }

    if verify && clean {
        println!("catalogs are synced with {ENGLISH_FILE}");
    }
    Ok(clean || !verify)
}

/// Every `t!("…")` id must exist in the English catalog, and every catalog id
/// must be used by the sources.
fn check_ids() -> Result<bool, String> {
    let root = repo_root();
    let english = parse_file(&lang_dir().join(ENGLISH_FILE))?;
    let catalog: BTreeSet<&str> = english
        .entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect();

    let mut failures = Vec::new();
    let mut referenced = BTreeSet::new();
    let mut used = BTreeSet::new();

    for path in source_files(&root)? {
        let text = std::fs::read_to_string(&path).map_err(|err| err.to_string())?;
        let display = relative(&path).display();
        for literal in string_literals(&text) {
            // An id can reach `translate` through a `const` or a helper rather
            // than a `t!` literal, so anything spelled like an id counts as a
            // reference; only `t!` call sites are checked.
            referenced.insert(literal.value.to_string());
            if !literal.in_t_macro {
                continue;
            }
            used.insert(literal.value.to_string());
            if !valid_id(literal.value) {
                failures.push(format!(
                    "{display}:{}: invalid i18n id '{}'",
                    literal.line, literal.value
                ));
            } else if !catalog.contains(literal.value) {
                failures.push(format!(
                    "{display}:{}: missing id in {ENGLISH_FILE}: '{}'",
                    literal.line, literal.value
                ));
            }
        }
    }

    for id in catalog
        .iter()
        .copied()
        .filter(|id| !referenced.contains(*id))
    {
        failures.push(format!("unused id in {ENGLISH_FILE}: '{id}'"));
    }

    if !failures.is_empty() {
        println!("i18n id check failed:");
        for failure in &failures {
            println!("- {failure}");
        }
        return Ok(false);
    }

    println!("i18n id check passed");
    println!("ids used: {}", used.len());
    Ok(true)
}

fn add_entry(id: &str, english: &str) -> Result<bool, String> {
    if !valid_id(id) {
        return Err(format!(
            "invalid id '{id}': use lowercase letters, digits and hyphen"
        ));
    }
    let dir = lang_dir();
    let en_path = dir.join(ENGLISH_FILE);
    let mut catalog = parse_file(&en_path)?;
    if catalog.entries.iter().any(|entry| entry.id == id) {
        return Err(format!("id '{id}' already exists"));
    }
    catalog.entries.push(Entry::english(id, english));
    write_lang(&en_path, &catalog, true)?;
    sync_catalogs(false)?;
    println!("added '{id}'");
    Ok(true)
}

fn add_lang(locale: &str, language: &str) -> Result<bool, String> {
    if locale.is_empty()
        || !locale
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(format!(
            "invalid locale '{locale}': use letters, digits, hyphen or underscore"
        ));
    }
    if language.trim().is_empty() {
        return Err("the language name cannot be empty".to_string());
    }
    let dir = lang_dir();
    let path = dir.join(format!("liana_{locale}.lang"));
    if path.exists() {
        return Err(format!("{} already exists", relative(&path).display()));
    }
    let english = parse_file(&dir.join(ENGLISH_FILE))?;
    let catalog = Catalog {
        language: language.to_string(),
        entries: english
            .entries
            .iter()
            .map(|entry| Entry::translated(&entry.id, &entry.english, Translation::Untranslated))
            .collect(),
    };
    write_lang(&path, &catalog, false)?;
    println!("created {}", relative(&path).display());
    println!("{language} is picked up by the next build");
    Ok(true)
}

fn source_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).map_err(|err| format!("{}: {err}", dir.display()))? {
            let path = entry
                .map_err(|err| format!("{}: {err}", dir.display()))?
                .path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if path.is_dir() {
                if name != "target" && name != ".git" {
                    stack.push(path);
                }
            } else if name.ends_with(".rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

struct Literal<'a> {
    value: &'a str,
    line: usize,
    in_t_macro: bool,
}

/// The string literals of a Rust source, with whether each is the first
/// argument of a `t!` call. Comments and raw strings are skipped, so a call
/// site quoted in a doc comment is not mistaken for a real one. Escapes are not
/// decoded: an i18n id never contains one, and a literal that does simply will
/// not match a catalog id.
fn string_literals(text: &str) -> Vec<Literal<'_>> {
    let bytes = text.as_bytes();
    let mut literals = Vec::new();
    let mut line = 1;
    let mut index = 0;

    while index < bytes.len() {
        let rest = &bytes[index..];
        if rest.starts_with(b"//") {
            index += rest
                .iter()
                .position(|byte| *byte == b'\n')
                .unwrap_or(rest.len());
        } else if rest.starts_with(b"/*") {
            let (skipped, newlines) = skip_block_comment(rest);
            line += newlines;
            index += skipped;
        } else if let Some(hashes) = raw_string_hashes(rest) {
            let (skipped, newlines) = skip_raw_string(rest, hashes);
            line += newlines;
            index += skipped;
        } else if bytes[index] == b'"' {
            let start = index + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end] != b'"' {
                end += if bytes[end] == b'\\' { 2 } else { 1 };
            }
            if end > bytes.len() {
                break;
            }
            let value = &text[start..end];
            literals.push(Literal {
                value,
                line,
                in_t_macro: opens_t_macro(&text[..index]),
            });
            line += value.matches('\n').count();
            index = end + 1;
        } else {
            if bytes[index] == b'\n' {
                line += 1;
            }
            index += 1;
        }
    }

    literals
}

/// Length of the `/* */` comment at the front of `bytes`, and the newlines in
/// it. Rust nests block comments, so the depth is tracked.
fn skip_block_comment(bytes: &[u8]) -> (usize, usize) {
    let mut depth = 0;
    let mut newlines = 0;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index..].starts_with(b"/*") {
            depth += 1;
            index += 2;
        } else if bytes[index..].starts_with(b"*/") {
            depth -= 1;
            index += 2;
            if depth == 0 {
                break;
            }
        } else {
            if bytes[index] == b'\n' {
                newlines += 1;
            }
            index += 1;
        }
    }
    (index, newlines)
}

/// The number of `#` in the raw string opening `bytes`, if it opens one.
fn raw_string_hashes(bytes: &[u8]) -> Option<usize> {
    let rest = bytes.strip_prefix(b"r")?;
    let hashes = rest.iter().take_while(|byte| **byte == b'#').count();
    rest.get(hashes)
        .is_some_and(|byte| *byte == b'"')
        .then_some(hashes)
}

/// Length of the raw string at the front of `bytes`, and the newlines in it.
fn skip_raw_string(bytes: &[u8], hashes: usize) -> (usize, usize) {
    let mut terminator = vec![b'"'];
    terminator.extend(std::iter::repeat_n(b'#', hashes));
    let mut newlines = 0;
    let mut index = 2 + hashes;
    while index < bytes.len() {
        if bytes[index..].starts_with(&terminator) {
            return (index + terminator.len(), newlines);
        }
        if bytes[index] == b'\n' {
            newlines += 1;
        }
        index += 1;
    }
    (bytes.len(), newlines)
}

/// Whether the text right before a literal is a `t!(` waiting for its id.
fn opens_t_macro(before: &str) -> bool {
    let before = before.trim_end();
    let Some(before) = before.strip_suffix('(') else {
        return false;
    };
    let Some(before) = before.trim_end().strip_suffix("t!") else {
        return false;
    };
    !before
        .chars()
        .next_back()
        .is_some_and(|ch| ch.is_alphanumeric() || ch == '_')
}
