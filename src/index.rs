//! Build the `KeyId × locale → Cell` matrix over a fixed order of active
//! locales, with the eligibility guard (`≥ K` non-empty cells to participate).
//!
//! The matrix holds **raw** joined values; tier canonicalization happens later
//! in [`crate::group`] over the canonical-matrix seam. A cell is `Empty` when
//! its value is blank (untranslated, or whitespace-only).

use std::collections::{BTreeMap, BTreeSet};

use crate::po::ParsedCatalog;

/// Identity of a translatable key. `domain` is part of the identity, so
/// `messages.po:X` and `django.po:X` are distinct keys. Hashable and ordered
/// for stable, deterministic bucketing.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeyId {
    pub domain: String,
    pub msgctxt: Option<String>,
    pub msgid: String,
    pub msgid_plural: Option<String>,
}

/// A key's value in one locale.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Cell {
    Value(String),
    Empty,
}

impl Cell {
    pub fn is_empty(&self) -> bool {
        matches!(self, Cell::Empty)
    }
}

/// One parsed catalog tagged with its locale id (column axis).
#[derive(Debug, Clone)]
pub struct CatalogInput {
    pub locale: String,
    pub catalog: ParsedCatalog,
}

/// `KeyId → Vec<Cell>` over a fixed locale order. Every row has one cell per
/// active locale, aligned to `locales`.
#[derive(Debug, Clone)]
pub struct Matrix {
    pub locales: Vec<String>,
    pub rows: BTreeMap<KeyId, Vec<Cell>>,
}

impl Matrix {
    /// Number of non-empty cells in a row.
    pub fn non_empty_count(row: &[Cell]) -> usize {
        row.iter().filter(|cell| !cell.is_empty()).count()
    }

    /// Drop keys with fewer than `min_locales_agree` non-empty cells — they
    /// have nothing to match on (eligibility guard).
    pub fn retain_eligible(&mut self, min_locales_agree: usize) {
        self.rows
            .retain(|_, row| Matrix::non_empty_count(row) >= min_locales_agree);
    }
}

/// Build the matrix from tagged catalogs. Active locales `L = all \ exclude`
/// in sorted (stable) order; absent and blank cells are `Empty`.
pub fn build_matrix(inputs: &[CatalogInput], exclude: &[String]) -> Matrix {
    let excluded: BTreeSet<&str> = exclude.iter().map(String::as_str).collect();

    let mut locale_set: BTreeSet<&str> = BTreeSet::new();
    for input in inputs {
        if !excluded.contains(input.locale.as_str()) {
            locale_set.insert(input.locale.as_str());
        }
    }
    let locales: Vec<String> = locale_set.iter().map(|l| l.to_string()).collect();
    let column: BTreeMap<&str, usize> = locales
        .iter()
        .enumerate()
        .map(|(index, locale)| (locale.as_str(), index))
        .collect();
    let width = locales.len();

    let mut rows: BTreeMap<KeyId, Vec<Cell>> = BTreeMap::new();
    for input in inputs {
        let Some(&col) = column.get(input.locale.as_str()) else {
            continue;
        };
        for entry in &input.catalog.entries {
            let key = KeyId {
                domain: input.catalog.domain.clone(),
                msgctxt: entry.msgctxt.clone(),
                msgid: entry.msgid.clone(),
                msgid_plural: entry.msgid_plural.clone(),
            };
            let cell = if entry.value.trim().is_empty() {
                Cell::Empty
            } else {
                Cell::Value(entry.value.clone())
            };
            let row = rows.entry(key).or_insert_with(|| vec![Cell::Empty; width]);
            row[col] = cell;
        }
    }

    Matrix { locales, rows }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::po::PoEntry;

    fn entry(msgid: &str, value: &str) -> PoEntry {
        PoEntry {
            msgctxt: None,
            msgid: msgid.to_string(),
            msgid_plural: None,
            value: value.to_string(),
        }
    }

    fn input(locale: &str, domain: &str, entries: Vec<PoEntry>) -> CatalogInput {
        CatalogInput {
            locale: locale.to_string(),
            catalog: ParsedCatalog {
                domain: domain.to_string(),
                entries,
            },
        }
    }

    fn key(domain: &str, msgid: &str) -> KeyId {
        KeyId {
            domain: domain.to_string(),
            msgctxt: None,
            msgid: msgid.to_string(),
            msgid_plural: None,
        }
    }

    #[test]
    fn keyid_distinguishes_domains() {
        let a = key("messages", "X");
        let b = key("django", "X");
        assert_ne!(a, b);
        let mut set = std::collections::HashSet::new();
        set.insert(a);
        set.insert(b);
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn row_has_one_cell_per_locale_in_stable_order() {
        let inputs = vec![
            input("ru", "messages", vec![entry("Save", "Сохранить")]),
            input("en", "messages", vec![entry("Save", "Save")]),
            input("es", "messages", vec![entry("Save", "Guardar")]),
        ];
        let matrix = build_matrix(&inputs, &[]);

        let locales: Vec<&str> = matrix.locales.iter().map(String::as_str).collect();
        assert_eq!(locales, ["en", "es", "ru"]); // sorted, stable

        let row = &matrix.rows[&key("messages", "Save")];
        assert_eq!(row.len(), 3);
        assert_eq!(row[0], Cell::Value("Save".to_string())); // en
        assert_eq!(row[1], Cell::Value("Guardar".to_string())); // es
        assert_eq!(row[2], Cell::Value("Сохранить".to_string())); // ru
    }

    #[test]
    fn exclude_removes_a_column() {
        let inputs = vec![
            input("en", "messages", vec![entry("Save", "Save")]),
            input("ru", "messages", vec![entry("Save", "Сохранить")]),
            input("es", "messages", vec![entry("Save", "Guardar")]),
        ];
        let matrix = build_matrix(&inputs, &["es".to_string()]);

        let locales: Vec<&str> = matrix.locales.iter().map(String::as_str).collect();
        assert_eq!(locales, ["en", "ru"]);
        assert_eq!(matrix.rows[&key("messages", "Save")].len(), 2);
    }

    #[test]
    fn empty_msgstr_becomes_empty_cell() {
        let inputs = vec![
            input("en", "messages", vec![entry("Save", "Save")]),
            input("ru", "messages", vec![entry("Save", "   ")]), // whitespace-only
        ];
        let matrix = build_matrix(&inputs, &[]);
        let row = &matrix.rows[&key("messages", "Save")];
        assert_eq!(row[0], Cell::Value("Save".to_string())); // en
        assert_eq!(row[1], Cell::Empty); // ru
    }

    #[test]
    fn eligibility_guard_filters_sparse_keys() {
        // M = 3 locales, K = 2. Key "a" has 3 non-empty; "b" has 1.
        let inputs = vec![
            input("en", "m", vec![entry("a", "A"), entry("b", "B")]),
            input("ru", "m", vec![entry("a", "А"), entry("b", "")]),
            input("es", "m", vec![entry("a", "A2"), entry("b", "")]),
        ];
        let mut matrix = build_matrix(&inputs, &[]);
        matrix.retain_eligible(2);

        assert!(matrix.rows.contains_key(&key("m", "a")));
        assert!(!matrix.rows.contains_key(&key("m", "b")));
    }
}
