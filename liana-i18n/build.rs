use std::{env, fs, path::Path};

use liana_i18n_toolbox::{
    locale_files, locale_from_path, parse_file, render_locales, verify_locale, write_ftl, Locale,
    ENGLISH_FILE,
};

fn main() {
    println!("cargo:rerun-if-changed=i18n");

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR must be set");
    let out_dir = Path::new(&out_dir);
    let i18n_dir = Path::new("i18n");

    let english = parse_file(&i18n_dir.join(ENGLISH_FILE)).expect("valid english catalog");
    write_ftl(&out_dir.join("en.ftl"), &english.entries, true).expect("write english FTL");

    let mut locales = vec![Locale {
        code: "en".to_string(),
        language: english.language.clone(),
    }];

    for path in locale_files(i18n_dir).expect("read i18n directory") {
        let origin = path.display().to_string();
        let catalog = parse_file(&path).unwrap_or_else(|err| panic!("{err}"));
        verify_locale(&english.entries, &catalog.entries, &origin)
            .unwrap_or_else(|err| panic!("{err}"));

        let locale = locale_from_path(&path).unwrap_or_else(|err| panic!("{err}"));
        write_ftl(
            &out_dir.join(format!("{locale}.ftl")),
            &catalog.entries,
            false,
        )
        .unwrap_or_else(|err| panic!("write {locale} FTL: {err}"));
        locales.push(Locale {
            code: locale.to_string(),
            language: catalog.language,
        });
    }

    fs::write(out_dir.join("locales.rs"), render_locales(&locales))
        .expect("write the SupportedLocale definition");
}
