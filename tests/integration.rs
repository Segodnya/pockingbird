//! End-to-end: write catalogs + config to a temp dir, run the built binary with
//! `--format json`, and assert group membership and tiers. Behind `cli` (the
//! binary needs that feature).
#![cfg(feature = "cli")]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// Write a `.po` with a full, valid header (polib unwraps required header keys).
fn write_po(root: &Path, locale: &str, domain: &str, entries: &[(&str, &str)]) {
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
    fs::write(dir.join(format!("{domain}.po")), content).unwrap();
}

fn write_config(root: &Path) -> PathBuf {
    let path = root.join("pockingbird.toml");
    // Small floor so M = 3 locales can form M-1 groups; rest are defaults.
    fs::write(
        &path,
        "[match]\nmin_locales_agree = 2\nempty_policy = \"own\"\n",
    )
    .unwrap();
    path
}

fn keyset(pairs: &[(&str, &str)]) -> BTreeSet<(String, String)> {
    pairs
        .iter()
        .map(|(d, m)| (d.to_string(), m.to_string()))
        .collect()
}

fn group_keys(group: &Value) -> BTreeSet<(String, String)> {
    group["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|key| {
            (
                key["domain"].as_str().unwrap().to_string(),
                key["msgid"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn find_group<'a>(
    groups: &'a [Value],
    tier: &str,
    keys: &BTreeSet<(String, String)>,
) -> Option<&'a Value> {
    groups
        .iter()
        .find(|group| group["tier"].as_str() == Some(tier) && group_keys(group) == *keys)
}

#[test]
fn end_to_end_groups_and_tiers() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // M = 3 locales. a/b full dup; c/d agree on en+ru, differ in es (M-1);
    // p1/p2 differ only by trailing punctuation (normalized, not exact);
    // messages:x and other:x are a cross-domain duplicate.
    let messages = [
        (
            "en",
            vec![
                ("a", "Save"),
                ("b", "Save"),
                ("c", "Delete"),
                ("d", "Delete"),
                ("x", "Shared"),
                ("p1", "Done."),
                ("p2", "Done"),
            ],
        ),
        (
            "ru",
            vec![
                ("a", "Сохранить"),
                ("b", "Сохранить"),
                ("c", "Удалить"),
                ("d", "Удалить"),
                ("x", "Общий"),
                ("p1", "Готово."),
                ("p2", "Готово"),
            ],
        ),
        (
            "es",
            vec![
                ("a", "Guardar"),
                ("b", "Guardar"),
                ("c", "Eliminar"),
                ("d", "Distinto"), // diverges from c only in es
                ("x", "Compartido"),
                ("p1", "Hecho."),
                ("p2", "Hecho"),
            ],
        ),
    ];

    for (locale, entries) in &messages {
        write_po(root, locale, "messages", entries);
        // other domain: only the shared key x, identical values.
        let x_value = entries.iter().find(|(id, _)| *id == "x").unwrap().1;
        write_po(root, locale, "other", &[("x", x_value)]);
    }

    let config = write_config(root);
    let output = Command::new(env!("CARGO_BIN_EXE_pockingbird"))
        .arg("find")
        .arg(root)
        .arg("--config")
        .arg(&config)
        .arg("--format")
        .arg("json")
        .output()
        .unwrap();

    assert!(output.status.success(), "binary should exit 0");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let report: Value = serde_json::from_str(&stdout).expect("valid json report");
    let groups = report["groups"].as_array().expect("groups array");

    // Full duplicate a/b under exact, all 3 locales.
    let full = find_group(
        groups,
        "exact",
        &keyset(&[("messages", "a"), ("messages", "b")]),
    )
    .expect("a/b exact full duplicate");
    assert_eq!(full["agree_locales"].as_u64(), Some(3));
    assert_eq!(full["total_locales"].as_u64(), Some(3));
    assert_eq!(full["cross_domain"].as_bool(), Some(false));

    // M-1 partial group c/d under exact.
    let partial = find_group(
        groups,
        "exact",
        &keyset(&[("messages", "c"), ("messages", "d")]),
    )
    .expect("c/d M-1 group");
    assert_eq!(partial["agree_locales"].as_u64(), Some(2));
    assert_eq!(partial["total_locales"].as_u64(), Some(3));
    assert_eq!(partial["differ"][0].as_str(), Some("es"));

    // p1/p2 collide only after normalization (trailing punct), never under exact.
    let normalized = find_group(
        groups,
        "normalized",
        &keyset(&[("messages", "p1"), ("messages", "p2")]),
    )
    .expect("p1/p2 normalized group");
    assert_eq!(normalized["agree_locales"].as_u64(), Some(3));
    assert!(
        find_group(
            groups,
            "exact",
            &keyset(&[("messages", "p1"), ("messages", "p2")])
        )
        .is_none(),
        "p1/p2 must not appear under exact"
    );

    // Cross-domain duplicate: messages:x == other:x.
    let cross = find_group(
        groups,
        "exact",
        &keyset(&[("messages", "x"), ("other", "x")]),
    )
    .expect("cross-domain x group");
    assert_eq!(cross["cross_domain"].as_bool(), Some(true));
    assert_eq!(cross["agree_locales"].as_u64(), Some(3));
}
