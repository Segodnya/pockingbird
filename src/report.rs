//! Render candidate groups as text (colored) or json, with identical structure.
//!
//! Tiers are reported independently (a duplicate that holds under several tiers
//! appears once per section) — there is no cross-tier dedup. JSON lives in the
//! library core (`serde_json` is always available); the colored text renderer is
//! behind the `cli` feature, alongside the `colored` dependency it needs.
//!
//! ## JSON schema
//!
//! ```json
//! {
//!   "groups": [
//!     {
//!       "tier": "exact",
//!       "agree_locales": 3,
//!       "total_locales": 3,
//!       "cross_domain": false,
//!       "keys": [{"domain": "messages", "msgctxt": null,
//!                 "msgid": "Save", "msgid_plural": null}],
//!       "shared": {"en": "Save", "ru": "Сохранить"},
//!       "differ": []
//!     }
//!   ],
//!   "summary": {"groups": 1, "keys": 120, "candidate_keys": 2}
//! }
//! ```

use std::collections::BTreeSet;

use serde::Serialize;

use crate::config::Tier;
use crate::group::CandidateGroup;
use crate::index::{Cell, KeyId};

// ---------------------------------------------------------------------------
// JSON (5.1)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct JsonReport<'a> {
    groups: Vec<JsonGroup<'a>>,
    summary: JsonSummary,
}

#[derive(Serialize)]
struct JsonGroup<'a> {
    tier: &'static str,
    agree_locales: usize,
    total_locales: usize,
    cross_domain: bool,
    keys: Vec<JsonKey<'a>>,
    /// Retained locale → shared value (`null` for an empty cell under `own`).
    shared: std::collections::BTreeMap<&'a str, Option<&'a str>>,
    differ: &'a [String],
}

#[derive(Serialize)]
struct JsonKey<'a> {
    domain: &'a str,
    msgctxt: Option<&'a str>,
    msgid: &'a str,
    msgid_plural: Option<&'a str>,
}

#[derive(Serialize)]
struct JsonSummary {
    groups: usize,
    keys: usize,
    candidate_keys: usize,
}

/// Serialize groups to pretty JSON. `total_keys` is the number of keys examined
/// (the summary denominator). Order is preserved as given (deterministic).
pub fn to_json(groups: &[CandidateGroup], total_keys: usize) -> String {
    let json_groups = groups.iter().map(json_group).collect();
    let report = JsonReport {
        groups: json_groups,
        summary: JsonSummary {
            groups: groups.len(),
            keys: total_keys,
            candidate_keys: candidate_key_count(groups),
        },
    };
    serde_json::to_string_pretty(&report).expect("report serializes")
}

fn json_group(group: &CandidateGroup) -> JsonGroup<'_> {
    JsonGroup {
        tier: tier_name(group.tier),
        agree_locales: group.agree_locales,
        total_locales: group.total_locales,
        cross_domain: group.cross_domain,
        keys: group.keys.iter().map(json_key).collect(),
        shared: group
            .shared
            .iter()
            .map(|(locale, cell)| (locale.as_str(), cell_value(cell)))
            .collect(),
        differ: &group.differ,
    }
}

fn json_key(key: &KeyId) -> JsonKey<'_> {
    JsonKey {
        domain: &key.domain,
        msgctxt: key.msgctxt.as_deref(),
        msgid: &key.msgid,
        msgid_plural: key.msgid_plural.as_deref(),
    }
}

fn cell_value(cell: &Cell) -> Option<&str> {
    match cell {
        Cell::Value(value) => Some(value),
        Cell::Empty => None,
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn tier_name(tier: Tier) -> &'static str {
    match tier {
        Tier::Exact => "exact",
        Tier::Normalized => "normalized",
        Tier::Fuzzy => "fuzzy",
    }
}

/// Distinct keys appearing in at least one group — the *candidate* keys (never
/// "removable": the tool reports, the human decides).
fn candidate_key_count(groups: &[CandidateGroup]) -> usize {
    groups
        .iter()
        .flat_map(|group| group.keys.iter())
        .collect::<BTreeSet<&KeyId>>()
        .len()
}

// ---------------------------------------------------------------------------
// Text (5.2–5.4) — behind `cli`, needs `colored`
// ---------------------------------------------------------------------------

/// Tiers in display order; sections render in this sequence.
#[cfg(feature = "cli")]
const TIER_ORDER: [Tier; 3] = [Tier::Exact, Tier::Normalized, Tier::Fuzzy];

/// Render groups as colored text: a section per tier (exact→normalized→fuzzy),
/// within each ordered by match level; a footer summary. Empty cell under `own`
/// shows as `∅`; cross-domain groups carry a unify hint.
#[cfg(feature = "cli")]
pub fn to_text(groups: &[CandidateGroup], total_keys: usize) -> String {
    use std::fmt::Write;

    use colored::Colorize;

    let mut out = String::new();
    for tier in TIER_ORDER {
        let mut section: Vec<&CandidateGroup> =
            groups.iter().filter(|group| group.tier == tier).collect();
        if section.is_empty() {
            continue;
        }
        // Highest agreement level first.
        section.sort_by(|a, b| {
            b.agree_locales
                .cmp(&a.agree_locales)
                .then(a.keys.cmp(&b.keys))
        });

        let _ = writeln!(out, "{}", tier_name(tier).to_uppercase().bold().cyan());
        let mut last_level = None;
        for group in section {
            let level = (group.agree_locales, group.total_locales);
            if last_level != Some(level) {
                let _ = writeln!(
                    out,
                    "  {}",
                    format!("{}/{}", level.0, level.1).yellow().bold()
                );
                last_level = Some(level);
            }
            let keys = group
                .keys
                .iter()
                .map(format_key)
                .collect::<Vec<_>>()
                .join(" | ");
            let _ = writeln!(out, "    {} {keys}", "•".dimmed());
            let _ = writeln!(out, "      {}: {}", "shared".green(), format_shared(group));
            if !group.differ.is_empty() {
                let _ = writeln!(out, "      {}: {}", "differ".red(), group.differ.join(", "));
            }
            if group.cross_domain {
                let _ = writeln!(
                    out,
                    "      {}",
                    "⚠ cross-domain — unify into a shared domain".magenta()
                );
            }
        }
        out.push('\n');
    }

    let summary = format!(
        "Summary: {} groups · {} keys · {} candidate keys",
        groups.len(),
        total_keys,
        candidate_key_count(groups)
    );
    let _ = writeln!(out, "{}", summary.bold());
    out
}

#[cfg(feature = "cli")]
fn format_key(key: &KeyId) -> String {
    let mut text = format!("{}:{}", key.domain, key.msgid);
    if let Some(context) = &key.msgctxt {
        text.push_str(&format!(" [ctx:{context}]"));
    }
    if let Some(plural) = &key.msgid_plural {
        text.push_str(&format!(" [plural:{plural}]"));
    }
    text
}

#[cfg(feature = "cli")]
fn format_shared(group: &CandidateGroup) -> String {
    group
        .shared
        .iter()
        .map(|(locale, cell)| match cell {
            Cell::Value(value) => format!("{locale}={value}"),
            Cell::Empty => format!("{locale}=∅"),
        })
        .collect::<Vec<_>>()
        .join("  ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn key(domain: &str, msgid: &str) -> KeyId {
        KeyId {
            domain: domain.to_string(),
            msgctxt: None,
            msgid: msgid.to_string(),
            msgid_plural: None,
        }
    }

    fn shared(pairs: &[(&str, Cell)]) -> BTreeMap<String, Cell> {
        pairs
            .iter()
            .map(|(l, c)| (l.to_string(), c.clone()))
            .collect()
    }

    fn full_group() -> CandidateGroup {
        CandidateGroup {
            keys: vec![key("messages", "Save"), key("django", "Save")],
            tier: Tier::Exact,
            agree_locales: 2,
            total_locales: 2,
            shared: shared(&[
                ("en", Cell::Value("Save".to_string())),
                ("ru", Cell::Value("Сохранить".to_string())),
            ]),
            differ: vec![],
            cross_domain: true,
        }
    }

    fn partial_group() -> CandidateGroup {
        CandidateGroup {
            keys: vec![key("messages", "a"), key("messages", "b")],
            tier: Tier::Normalized,
            agree_locales: 1,
            total_locales: 2,
            shared: shared(&[("en", Cell::Value("save".to_string()))]),
            differ: vec!["ru".to_string()],
            cross_domain: false,
        }
    }

    // --- 5.1 json ---

    #[test]
    fn json_is_valid_and_structured() {
        let groups = vec![full_group()];
        let text = to_json(&groups, 120);
        let value: serde_json::Value = serde_json::from_str(&text).expect("valid json");

        assert_eq!(value["groups"][0]["tier"], "exact");
        assert_eq!(value["groups"][0]["agree_locales"], 2);
        assert_eq!(value["groups"][0]["cross_domain"], true);
        assert_eq!(value["groups"][0]["keys"][0]["domain"], "messages");
        assert_eq!(value["groups"][0]["shared"]["ru"], "Сохранить");
        assert_eq!(value["summary"]["groups"], 1);
        assert_eq!(value["summary"]["keys"], 120);
        assert_eq!(value["summary"]["candidate_keys"], 2);
    }

    #[test]
    fn json_empty_cell_is_null() {
        let mut group = partial_group();
        group.shared.insert("es".to_string(), Cell::Empty);
        let value: serde_json::Value =
            serde_json::from_str(&to_json(&[group], 1)).expect("valid json");
        assert!(value["groups"][0]["shared"]["es"].is_null());
    }

    // --- 5.2 text ---

    #[cfg(feature = "cli")]
    #[test]
    fn text_has_tier_sections_and_levels() {
        colored::control::set_override(false);
        let out = to_text(&[full_group(), partial_group()], 50);

        assert!(out.contains("EXACT"));
        assert!(out.contains("NORMALIZED"));
        assert!(out.contains("2/2")); // full level
        assert!(out.contains("1/2")); // partial level
        assert!(out.contains("messages:Save | django:Save"));
        assert!(out.contains("shared: en=Save  ru=Сохранить"));
        assert!(out.contains("differ: ru"));
    }

    // --- 5.3 cross-domain hint ---

    #[cfg(feature = "cli")]
    #[test]
    fn text_flags_cross_domain() {
        colored::control::set_override(false);
        let out = to_text(&[full_group()], 1);
        assert!(out.contains("cross-domain"));
        assert!(out.contains("unify into a shared domain"));
        // A single-domain group carries no hint.
        let plain = to_text(&[partial_group()], 1);
        assert!(!plain.contains("cross-domain"));
    }

    // --- 5.4 summary ---

    #[cfg(feature = "cli")]
    #[test]
    fn text_summary_counts_groups_and_candidates() {
        colored::control::set_override(false);
        // Two groups, four key slots but "Save" keys repeat across them? No —
        // here all four keys are distinct → 4 candidate keys.
        let out = to_text(&[full_group(), partial_group()], 200);
        assert!(out.contains("Summary: 2 groups · 200 keys · 4 candidate keys"));
        assert!(!out.contains("removable"));
    }

    #[test]
    fn candidate_keys_are_deduplicated_across_groups() {
        // The same key in two groups counts once.
        let mut second = full_group();
        second.tier = Tier::Fuzzy;
        assert_eq!(candidate_key_count(&[full_group(), second]), 2);
    }
}
