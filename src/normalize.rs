//! Canonicalize a string per tier: exact (trim) and normalized (case-fold +
//! collapse whitespace + strip trailing punctuation), `exact ⊂ normalized`.
//!
//! Decisions (PLAN "Open details"):
//! - **Whitespace** is *collapsed* to a single space (runs → one space), not
//!   removed, so word boundaries are preserved (`"o  k" → "o k"`).
//! - **Trailing punctuation** is every Unicode Punctuation char (category `P`:
//!   `Pc Pd Ps Pe Pi Pf Po`), plus any whitespace it exposes. This catches
//!   localized punctuation (`…`, `»`, `¿`) without merging symbols (`™`, `°`).

use unicode_properties::{GeneralCategoryGroup, UnicodeGeneralCategory};

use crate::config::Normalize;

/// Exact-tier canonical: trim surrounding whitespace.
pub fn exact(value: &str) -> String {
    value.trim().to_string()
}

/// Normalized-tier canonical: trim, then apply the enabled `[match.normalize]`
/// transforms. `exact ⊂ normalized` — equal exacts stay equal here.
pub fn normalized(value: &str, config: &Normalize) -> String {
    let mut result = value.trim().to_string();
    if config.case_fold {
        result = result.to_lowercase();
    }
    if config.collapse_whitespace {
        result = result.split_whitespace().collect::<Vec<_>>().join(" ");
    }
    if config.strip_trailing_punct {
        result = strip_trailing_punct(&result);
    }
    result
}

/// Drop trailing Unicode punctuation and any whitespace it leaves exposed.
fn strip_trailing_punct(value: &str) -> String {
    value
        .trim_end_matches(|c: char| {
            c.is_whitespace() || c.general_category_group() == GeneralCategoryGroup::Punctuation
        })
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Normalize;

    fn all_on() -> Normalize {
        Normalize::default()
    }

    #[test]
    fn exact_trims() {
        assert_eq!(exact("  Ok  "), "Ok");
    }

    #[test]
    fn normalized_folds_case_punct_and_keeps_single_space() {
        let config = all_on();
        // case-fold + trailing punct + trailing whitespace all converge.
        assert_eq!(normalized("OK.", &config), "ok");
        assert_eq!(normalized("OK ", &config), "ok");
        assert_eq!(normalized("ok", &config), "ok");
        // Collapse keeps the word boundary: a space stays (single).
        assert_eq!(normalized("O  K", &config), "o k");
    }

    #[test]
    fn normalized_strips_localized_trailing_punctuation() {
        let config = all_on();
        assert_eq!(normalized("Готово…", &config), "готово");
        assert_eq!(normalized("Réponse !", &config), "réponse");
        assert_eq!(normalized("Дальше »", &config), "дальше");
    }

    #[test]
    fn normalized_keeps_non_punctuation_symbols() {
        // ™ is a Symbol (So), not Punctuation — must survive.
        let config = all_on();
        assert_eq!(normalized("Brand™", &config), "brand™");
    }

    #[test]
    fn exact_is_subset_of_normalized() {
        let config = all_on();
        // Values equal under exact remain equal under normalized.
        let a = "Save";
        let b = "Save";
        assert_eq!(exact(a), exact(b));
        assert_eq!(normalized(a, &config), normalized(b, &config));
        // Normalized is strictly coarser: these differ under exact, merge here.
        assert_ne!(exact("OK."), exact("ok"));
        assert_eq!(normalized("OK.", &config), normalized("ok", &config));
    }

    #[test]
    fn flags_are_individually_honored() {
        let only_case = Normalize {
            case_fold: true,
            collapse_whitespace: false,
            strip_trailing_punct: false,
        };
        assert_eq!(normalized("OK.", &only_case), "ok.");

        let only_punct = Normalize {
            case_fold: false,
            collapse_whitespace: false,
            strip_trailing_punct: true,
        };
        assert_eq!(normalized("OK.", &only_punct), "OK");
    }
}
