# Liana i18n

Translations are edited in `.lang` catalogs under `i18n/`. Each one names its
language in a header, and `SupportedLocale` is generated from the set of
catalogs at build time, so a catalog is all a language needs:

```text
# Language: Français
```

English is the source catalog:

```text
coins-label => "Label"
```

Other languages keep the English text next to the translation:

```text
coins-label => "Label" => "Libelle"
```

Write the bare word `NONE` for an entry nobody has translated yet. It is never
quoted, so it cannot be confused with a translation whose text is "NONE".
Untranslated entries are left out of the generated Fluent files, so the runtime
falls back to English:

```text
coins-label => "Label" => NONE
```

When the English text of an entry changes, `sync` resets its translations to
`NONE` and keeps the one it replaced as a note, so a translator sees what the
entry said before:

```text
coins-label => "Coin label" => NONE # "Libelle"
```

Long entries may use the multiline form:

```text
coins-label
=> "Label"
=> "Libelle";;
```

A note follows the terminator in that form:

```text
coins-label
=> "Coin label"
=> NONE;; # "Libelle"
```

Use `\n` for line breaks inside values.

## Add an entry

```sh
cargo run -p liana-i18n-toolbox -- add-entry coins-label "Label"
```

This appends the English value and marks the entry untranslated in the other
catalogs.

## Add a language

Pass the locale and the language's own name, the one the UI lists:

```console
$ cargo run -p liana-i18n-toolbox -- add-lang de Deutsch
created liana-i18n/i18n/liana_de.lang
Deutsch is picked up by the next build
```

That is the whole procedure. The catalog starts out entirely `NONE`, and the
next build regenerates `SupportedLocale`, so German appears in the language
picker and `de-DE` resolves to it. Nothing else needs editing.

## Change an English string

Only the English catalog needs care while a feature is in progress: edit the
value there and let the tooling deal with the other catalogs.

Say the catalogs hold this entry, in `liana_en.lang`:

```text
common-no-label => "No label"
```

and in `liana_fr.lang`:

```text
common-no-label => "No label" => "Aucun libellé"
```

Edit the English value to `"No label yet"`, and the catalogs no longer agree.
Both the build and CI say so:

```console
$ cargo run -p liana-i18n-toolbox -- sync --verify
outdated: liana-i18n/i18n/liana_fr.lang
outdated: liana-i18n/i18n/liana_it.lang
outdated: liana-i18n/i18n/liana_pt-PT.lang

$ cargo build
error: failed to run custom build command for `liana-i18n`
  thread 'main' panicked at liana-i18n/build.rs:20:73:
  i18n/liana_fr.lang: english text changed for 'common-no-label'
```

Run the tooling to rewrite the locale catalogs:

```console
$ cargo run -p liana-i18n-toolbox -- sync
synced: liana-i18n/i18n/liana_fr.lang
synced: liana-i18n/i18n/liana_it.lang
synced: liana-i18n/i18n/liana_pt-PT.lang

$ cargo run -p liana-i18n-toolbox -- sync --verify
catalogs are synced with liana_en.lang
```

Each translation of the changed text is now `NONE`, with the translation it
replaced kept as a note:

```text
common-no-label => "No label yet" => NONE # "Aucun libellé"
```

The note holds the last translation a human wrote, so it survives further
English edits while the entry stays `NONE`, and it disappears once someone
translates the entry again. Nothing else in the catalog moves, and the app shows
English for an untranslated entry.

Before a release, translators go through every remaining `NONE` once.

## Check catalogs

CI runs both of these on the branch tip:

```sh
cargo run -p liana-i18n-toolbox -- sync --verify
cargo run -p liana-i18n-toolbox -- check-ids
```

`sync --verify` reports catalogs that have drifted from the English source; drop
the flag to rewrite them. `check-ids` fails on a `t!` id the English catalog does
not have, and on an id the catalog holds that nothing uses.

Cargo generates Fluent `.ftl` files at build time from the `.lang` catalogs, so
`cargo test -p liana-i18n` also fails on a catalog that cannot be parsed.
