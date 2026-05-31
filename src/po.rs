//! Parse a `.po` file via polib into a catalog of keys and per-locale values.
//! `domain` comes from the filename stem; plural forms are joined with `\u{1}`.
//!
//! ## gettext flag policy (PLAN "Open detail", TODO 1.6)
//!
//! - **Obsolete (`#~`)** entries are skipped. polib's parser already drops them
//!   (an `#~` line starts with `#` but matches no known comment prefix), so no
//!   extra handling is needed — documented here so the behavior is intentional.
//! - **Fuzzy (`#, fuzzy`)** entries are skipped. A fuzzy translation is
//!   unconfirmed and gettext does not use it at runtime (`msgfmt` excludes it by
//!   default); treating it as an authoritative value would seed false duplicates.
//!   We drop the entry, so its locale cell becomes `Empty` downstream.
//! - The **metadata header** (empty `msgid`) is not a key and is skipped.

use std::fmt;
use std::path::{Path, PathBuf};

use polib::po_file::{self, POParseError};

/// Delimiter joining a key's plural forms into a single per-locale value.
pub const PLURAL_SEPARATOR: &str = "\u{1}";

/// One translatable entry extracted from a `.po` file. `value` is the joined
/// translation; an empty string means untranslated (→ `Empty` cell downstream).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoEntry {
    pub msgctxt: Option<String>,
    pub msgid: String,
    pub msgid_plural: Option<String>,
    pub value: String,
}

/// A parsed catalog: its `domain` (filename stem) and its entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCatalog {
    pub domain: String,
    pub entries: Vec<PoEntry>,
}

#[derive(Debug)]
pub enum PoError {
    Parse(POParseError),
    NoDomain(PathBuf),
}

impl fmt::Display for PoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(error) => write!(f, "{error}"),
            Self::NoDomain(path) => {
                write!(f, "cannot derive domain from path: {}", path.display())
            }
        }
    }
}

impl std::error::Error for PoError {}

/// Parse a single `.po` file into a [`ParsedCatalog`].
pub fn parse_po(path: &Path) -> Result<ParsedCatalog, PoError> {
    let domain = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| PoError::NoDomain(path.to_path_buf()))?
        .to_string();

    let catalog = po_file::parse(path).map_err(PoError::Parse)?;
    let mut entries = Vec::new();

    for message in catalog.messages() {
        // Skip the metadata header and any fuzzy entry (see module docs).
        if message.msgid().is_empty() || message.is_fuzzy() {
            continue;
        }

        let value = if !message.is_translated() {
            String::new()
        } else if message.is_plural() {
            message
                .msgstr_plural()
                .expect("plural message has plural msgstr")
                .join(PLURAL_SEPARATOR)
        } else {
            message
                .msgstr()
                .expect("singular message has singular msgstr")
                .to_string()
        };

        let msgctxt = match message.msgctxt() {
            "" => None,
            context => Some(context.to_string()),
        };
        let msgid_plural = if message.is_plural() {
            Some(
                message
                    .msgid_plural()
                    .expect("plural message has msgid_plural")
                    .to_string(),
            )
        } else {
            None
        };

        entries.push(PoEntry {
            msgctxt,
            msgid: message.msgid().to_string(),
            msgid_plural,
            value,
        });
    }

    Ok(ParsedCatalog { domain, entries })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // A full, valid header is required: polib's metadata parser unwraps these
    // keys and panics if any is missing.
    const FIXTURE: &str = r#"msgid ""
msgstr ""
"Project-Id-Version: test 1.0\n"
"POT-Creation-Date: 2024-01-01 00:00+0000\n"
"PO-Revision-Date: 2024-01-01 00:00+0000\n"
"Last-Translator: t <t@example.com>\n"
"Language-Team: ru <ru@example.com>\n"
"Language: ru\n"
"MIME-Version: 1.0\n"
"Content-Type: text/plain; charset=UTF-8\n"
"Content-Transfer-Encoding: 8bit\n"
"Plural-Forms: nplurals=2; plural=(n != 1);\n"

msgid "Save"
msgstr "Сохранить"

msgctxt "button"
msgid "Open"
msgstr "Открыть"

msgid "untranslated"
msgstr ""

#, fuzzy
msgid "fuzzyentry"
msgstr "примерно"

msgid "%d file"
msgid_plural "%d files"
msgstr[0] "%d файл"
msgstr[1] "%d файла"

#~ msgid "obsoletekey"
#~ msgstr "старое"
"#;

    fn parse_fixture() -> ParsedCatalog {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("messages.po");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(FIXTURE.as_bytes()).unwrap();
        parse_po(&path).unwrap()
    }

    fn find<'a>(catalog: &'a ParsedCatalog, msgid: &str) -> Option<&'a PoEntry> {
        catalog.entries.iter().find(|entry| entry.msgid == msgid)
    }

    #[test]
    fn domain_comes_from_filename_stem() {
        assert_eq!(parse_fixture().domain, "messages");
    }

    #[test]
    fn reads_msgid_msgstr_and_msgctxt() {
        let catalog = parse_fixture();
        let save = find(&catalog, "Save").expect("Save present");
        assert_eq!(save.value, "Сохранить");
        assert_eq!(save.msgctxt, None);

        let open = find(&catalog, "Open").expect("Open present");
        assert_eq!(open.msgctxt, Some("button".to_string()));
        assert_eq!(open.value, "Открыть");
    }

    #[test]
    fn untranslated_has_empty_value() {
        let catalog = parse_fixture();
        assert_eq!(find(&catalog, "untranslated").unwrap().value, "");
    }

    #[test]
    fn plural_forms_joined_with_separator() {
        let catalog = parse_fixture();
        let plural = find(&catalog, "%d file").expect("plural present");
        assert_eq!(plural.msgid_plural, Some("%d files".to_string()));
        assert_eq!(plural.value, format!("%d файл{PLURAL_SEPARATOR}%d файла"));
    }

    #[test]
    fn fuzzy_and_obsolete_entries_are_skipped() {
        let catalog = parse_fixture();
        assert!(find(&catalog, "fuzzyentry").is_none());
        assert!(find(&catalog, "obsoletekey").is_none());
    }
}
