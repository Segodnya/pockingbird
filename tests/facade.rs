//! The library front door: `pockingbird::scan` discovers, parses, and groups in
//! one call — no manual wiring of walk/pipeline/parser. Runs without the `cli`
//! feature (the facade lives in the core).

use std::fs;
use std::path::Path;

use pockingbird::{scan, Config};

/// Write a `.po` under the gettext layout with a full, valid header.
fn write_po(root: &Path, locale: &str, entries: &[(&str, &str)]) {
    let dir = root.join(locale).join("LC_MESSAGES");
    fs::create_dir_all(&dir).unwrap();
    let mut content = format!(
        "msgid \"\"\n\
         msgstr \"\"\n\
         \"Project-Id-Version: t 1.0\\n\"\n\
         \"POT-Creation-Date: 2024-01-01 00:00+0000\\n\"\n\
         \"PO-Revision-Date: 2024-01-01 00:00+0000\\n\"\n\
         \"Last-Translator: t <t@example.com>\\n\"\n\
         \"Language-Team: {locale} <{locale}@example.com>\\n\"\n\
         \"Language: {locale}\\n\"\n\
         \"MIME-Version: 1.0\\n\"\n\
         \"Content-Type: text/plain; charset=UTF-8\\n\"\n\
         \"Content-Transfer-Encoding: 8bit\\n\"\n\
         \"Plural-Forms: nplurals=2; plural=(n != 1);\\n\"\n\n"
    );
    for (msgid, msgstr) in entries {
        content.push_str(&format!("msgid \"{msgid}\"\nmsgstr \"{msgstr}\"\n\n"));
    }
    fs::write(dir.join("messages.po"), content).unwrap();
}

#[test]
fn scan_finds_a_duplicate_in_one_call() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    // a and b share their translation in both locales → one duplicate group.
    write_po(root, "en", &[("a", "Save"), ("b", "Save")]);
    write_po(root, "ru", &[("a", "Сохранить"), ("b", "Сохранить")]);

    let config = Config::from_toml("[match]\nmin_locales_agree = 2\n").unwrap();
    let report = scan(root, &config).expect("scan runs");

    assert_eq!(report.total_keys, 2);
    assert!(
        report.groups.iter().any(|group| group.agree_locales == 2),
        "scan surfaces the full-agreement duplicate"
    );
}
