//! CORE: tier-agnostic signature bucketing + leave-one-out over a canonical
//! matrix → [`CandidateGroup`]s.
//!
//! ## The canonical-matrix seam (4.1)
//!
//! Each tier is just a way to canonicalize the raw [`Matrix`] into a canonical
//! one of the same shape: `exact`/`normalized` are `Fn(&str) -> String` applied
//! per cell; `fuzzy` is `Fn(&[String]) -> Map` applied per locale column. The
//! bucketing below is *identical* for every tier — it never looks at the tier,
//! only at the canonical values.
//!
//! ## Bucketing
//!
//! A key's signature is its row of canonical tokens. For `M` active locales and
//! a floor `K = min_locales_agree`, we enumerate every sub-signature obtained by
//! retaining `n ∈ [K, M]` columns (leave-one-out: drop `t = M − n ≤ T = M − K`
//! columns). Keys sharing an identical sub-signature agree on exactly those `n`
//! locales → a candidate group at level `n`. The number of sub-signatures per
//! key is `Σ_{t=0}^{T} C(M, t)`, bounded at startup by [`crate::config`].
//!
//! ## Empty policy (4.4)
//!
//! - `own`: `Empty` is a distinct token; an all-empty sub-signature is dropped
//!   (two keys both blank in the same locales are not "duplicates").
//! - `skip`: empty cells are absent — they never enter a sub-signature, and they
//!   drop out of the denominator (`total_locales`).

use std::collections::{BTreeMap, BTreeSet};

use rayon::prelude::*;

use crate::config::{EmptyPolicy, Match, Normalize, Tier};
use crate::fuzzy::fuzzy_canonicals;
use crate::index::{Cell, KeyId, Matrix};
use crate::normalize::{exact, normalized};

/// A reported duplicate-candidate group over one tier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateGroup {
    /// The keys that collide on the shared sub-signature (sorted).
    pub keys: Vec<KeyId>,
    pub tier: Tier,
    /// Locales the keys agree on (`n`).
    pub agree_locales: usize,
    /// Denominator: `M` under `own`; under `skip`, agreeing + diverging
    /// non-empty locales (empties excluded).
    pub total_locales: usize,
    /// Retained locale → shared canonical value (`Empty` only under `own`).
    pub shared: BTreeMap<String, Cell>,
    /// Diverging / dropped locales (sorted).
    pub differ: Vec<String>,
    /// `true` if the keys span more than one domain.
    pub cross_domain: bool,
}

/// Group a single tier (canonicalize + bucket). The pipeline calls this per tier
/// so it can emit per-tier progress; running every configured tier is just a
/// loop over it (see the test helper).
pub fn group_tier(matrix: &Matrix, tier: Tier, config: &Match) -> Vec<CandidateGroup> {
    let canon = canonical_matrix(
        matrix,
        tier,
        &config.normalize,
        config.fuzzy_max_distance,
        config.fuzzy_min_length,
    );
    bucket(&canon, tier, config.empty_policy, config.min_locales_agree)
}

// ---------------------------------------------------------------------------
// Canonicalization (the seam, 4.1)
// ---------------------------------------------------------------------------

/// Canonicalize the raw matrix into the given tier's canonical matrix.
pub fn canonical_matrix(
    matrix: &Matrix,
    tier: Tier,
    normalize: &Normalize,
    fuzzy_max_distance: usize,
    fuzzy_min_length: usize,
) -> Matrix {
    match tier {
        Tier::Exact => map_cells(matrix, exact),
        Tier::Normalized => map_cells(matrix, |value| normalized(value, normalize)),
        Tier::Fuzzy => fuzzy_matrix(matrix, fuzzy_max_distance, fuzzy_min_length),
    }
}

fn map_cells(matrix: &Matrix, canon: impl Fn(&str) -> String) -> Matrix {
    let rows = matrix
        .rows
        .iter()
        .map(|(key, row)| {
            (
                key.clone(),
                row.iter().map(|cell| map_cell(cell, &canon)).collect(),
            )
        })
        .collect();
    Matrix {
        locales: matrix.locales.clone(),
        rows,
    }
}

fn map_cell(cell: &Cell, canon: impl Fn(&str) -> String) -> Cell {
    match cell {
        Cell::Value(value) => Cell::Value(canon(value)),
        Cell::Empty => Cell::Empty,
    }
}

/// Fuzzy tier: cluster each locale column's raw values independently, then map
/// every cell to its cluster representative. Columns are clustered in parallel.
fn fuzzy_matrix(matrix: &Matrix, max_distance: usize, min_length: usize) -> Matrix {
    let width = matrix.locales.len();
    let mut columns: Vec<Vec<String>> = vec![Vec::new(); width];
    for row in matrix.rows.values() {
        for (index, cell) in row.iter().enumerate() {
            if let Cell::Value(value) = cell {
                columns[index].push(value.clone());
            }
        }
    }
    let maps: Vec<BTreeMap<String, String>> = columns
        .par_iter()
        .map(|values| {
            fuzzy_canonicals(values, max_distance, min_length)
                .into_iter()
                .collect()
        })
        .collect();

    let rows = matrix
        .rows
        .iter()
        .map(|(key, row)| {
            let canon_row = row
                .iter()
                .enumerate()
                .map(|(index, cell)| match cell {
                    Cell::Value(value) => Cell::Value(
                        maps[index]
                            .get(value)
                            .cloned()
                            .unwrap_or_else(|| value.clone()),
                    ),
                    Cell::Empty => Cell::Empty,
                })
                .collect();
            (key.clone(), canon_row)
        })
        .collect();
    Matrix {
        locales: matrix.locales.clone(),
        rows,
    }
}

// ---------------------------------------------------------------------------
// Bucketing
// ---------------------------------------------------------------------------

/// One column's value inside a sub-signature.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum SigToken {
    Empty,
    Value(String),
}

/// A sub-signature: retained `(column index, token)` pairs in column order.
type SubSig = Vec<(usize, SigToken)>;

struct RawGroup<'a> {
    subsig: SubSig,
    keys: BTreeSet<&'a KeyId>,
}

/// The grouping behavior of an empty policy, concentrated on the type so the
/// bucketing code never branches on the policy itself. Lives here (not in
/// `config`) because the projection speaks in `group.rs` terms (`SigToken`,
/// `RawGroup`, the canonical `Matrix`).
impl EmptyPolicy {
    /// Project a canonical row into the `(column, token)` pairs that take part in
    /// sub-signatures. `own` keeps every column (Empty is a token); `skip` omits
    /// empty columns entirely.
    fn present(self, row: &[Cell]) -> Vec<(usize, SigToken)> {
        match self {
            EmptyPolicy::Own => row
                .iter()
                .enumerate()
                .map(|(i, cell)| (i, token(cell)))
                .collect(),
            EmptyPolicy::Skip => row
                .iter()
                .enumerate()
                .filter_map(|(i, cell)| match cell {
                    Cell::Value(value) => Some((i, SigToken::Value(value.clone()))),
                    Cell::Empty => None,
                })
                .collect(),
        }
    }

    /// Whether a retained sub-signature is a real duplicate candidate. Under
    /// `own`, an all-empty sub-signature is not (shared emptiness isn't a match);
    /// under `skip`, empties are already absent, so every sub-signature counts.
    fn admits(self, subsig: &SubSig) -> bool {
        match self {
            EmptyPolicy::Own => subsig.iter().any(|(_, t)| !matches!(t, SigToken::Empty)),
            EmptyPolicy::Skip => true,
        }
    }

    /// The diverging columns and the `total_locales` denominator for a group.
    /// Under `own` every non-retained column diverges and the denominator is the
    /// full width. Under `skip` only columns where every member is non-empty
    /// diverge; empties drop out of the denominator entirely.
    fn differ_total(
        self,
        retained: &BTreeSet<usize>,
        width: usize,
        group: &RawGroup,
        canon: &Matrix,
    ) -> (Vec<usize>, usize) {
        match self {
            EmptyPolicy::Own => {
                let differ = (0..width)
                    .filter(|index| !retained.contains(index))
                    .collect();
                (differ, width)
            }
            EmptyPolicy::Skip => {
                let differ: Vec<usize> = (0..width)
                    .filter(|index| !retained.contains(index))
                    .filter(|index| {
                        group
                            .keys
                            .iter()
                            .all(|key| !canon.rows[*key][*index].is_empty())
                    })
                    .collect();
                let total = group.subsig.len() + differ.len();
                (differ, total)
            }
        }
    }
}

fn bucket(
    canon: &Matrix,
    tier: Tier,
    policy: EmptyPolicy,
    min_agree: usize,
) -> Vec<CandidateGroup> {
    let width = canon.locales.len();
    if min_agree == 0 || width == 0 {
        return Vec::new();
    }

    // 1. Per-key sub-signatures (parallel); sort makes order independent of rayon.
    let rows: Vec<(&KeyId, &Vec<Cell>)> = canon.rows.iter().collect();
    let mut pairs: Vec<(SubSig, &KeyId)> = rows
        .par_iter()
        .flat_map_iter(|(key, row)| {
            let key: &KeyId = key;
            subsigs(&row[..], policy, min_agree)
                .into_iter()
                .map(move |subsig| (subsig, key))
        })
        .collect();
    pairs.sort();

    // 2. Buckets of ≥2 distinct keys sharing a sub-signature.
    let mut raw: Vec<RawGroup> = Vec::new();
    let mut start = 0;
    while start < pairs.len() {
        let mut end = start + 1;
        while end < pairs.len() && pairs[end].0 == pairs[start].0 {
            end += 1;
        }
        let keys: BTreeSet<&KeyId> = pairs[start..end].iter().map(|(_, key)| *key).collect();
        if keys.len() >= 2 {
            raw.push(RawGroup {
                subsig: pairs[start].0.clone(),
                keys,
            });
        }
        start = end;
    }

    // 3. Level dedup (4.5): keep each group at its highest level; a group whose
    //    key set is a subset of an already-kept group is suppressed. Process
    //    high level first, larger key set first.
    raw.sort_by(|a, b| {
        b.subsig
            .len()
            .cmp(&a.subsig.len())
            .then(b.keys.len().cmp(&a.keys.len()))
            .then(a.subsig.cmp(&b.subsig))
    });
    let mut kept: Vec<RawGroup> = Vec::new();
    for candidate in raw {
        if kept
            .iter()
            .any(|group| candidate.keys.is_subset(&group.keys))
        {
            continue;
        }
        kept.push(candidate);
    }

    // 4. Assemble + deterministic order.
    let mut groups: Vec<CandidateGroup> = kept
        .into_iter()
        .map(|group| build_group(group, canon, tier, policy, width))
        .collect();
    groups.sort_by(|a, b| {
        a.keys
            .cmp(&b.keys)
            .then(b.agree_locales.cmp(&a.agree_locales))
    });
    groups
}

/// Every valid sub-signature for one key's canonical row.
fn subsigs(row: &[Cell], policy: EmptyPolicy, min_agree: usize) -> Vec<SubSig> {
    let present = policy.present(row);
    let count = present.len();
    if count < min_agree {
        return Vec::new();
    }
    let max_drop = count - min_agree;

    let mut out = Vec::new();
    for size in 0..=max_drop {
        for dropset in combinations(count, size) {
            let dropped: BTreeSet<usize> = dropset.into_iter().collect();
            let retained: SubSig = present
                .iter()
                .enumerate()
                .filter(|(position, _)| !dropped.contains(position))
                .map(|(_, pair)| pair.clone())
                .collect();
            if !policy.admits(&retained) {
                continue;
            }
            out.push(retained);
        }
    }
    out
}

fn token(cell: &Cell) -> SigToken {
    match cell {
        Cell::Value(value) => SigToken::Value(value.clone()),
        Cell::Empty => SigToken::Empty,
    }
}

/// All `k`-combinations of indices `0..n`, lexicographically.
fn combinations(n: usize, k: usize) -> Vec<Vec<usize>> {
    let mut result = Vec::new();
    if k > n {
        return result;
    }
    let mut combo: Vec<usize> = (0..k).collect();
    loop {
        result.push(combo.clone());
        let mut index = k;
        let mut advanced = false;
        while index > 0 {
            index -= 1;
            if combo[index] != index + n - k {
                advanced = true;
                break;
            }
        }
        if !advanced {
            break;
        }
        combo[index] += 1;
        for next in (index + 1)..k {
            combo[next] = combo[next - 1] + 1;
        }
    }
    result
}

fn build_group(
    group: RawGroup,
    canon: &Matrix,
    tier: Tier,
    policy: EmptyPolicy,
    width: usize,
) -> CandidateGroup {
    let retained: BTreeSet<usize> = group.subsig.iter().map(|(index, _)| *index).collect();
    let keys: Vec<KeyId> = group.keys.iter().map(|key| (*key).clone()).collect();

    let mut shared = BTreeMap::new();
    for (index, tok) in &group.subsig {
        let cell = match tok {
            SigToken::Empty => Cell::Empty,
            SigToken::Value(value) => Cell::Value(value.clone()),
        };
        shared.insert(canon.locales[*index].clone(), cell);
    }

    let (differ_cols, total) = policy.differ_total(&retained, width, &group, canon);
    let differ = differ_cols
        .into_iter()
        .map(|index| canon.locales[index].clone())
        .collect();

    let cross_domain = keys
        .iter()
        .map(|key| &key.domain)
        .collect::<BTreeSet<_>>()
        .len()
        > 1;

    CandidateGroup {
        keys,
        tier,
        agree_locales: group.subsig.len(),
        total_locales: total,
        shared,
        differ,
        cross_domain,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(domain: &str, msgid: &str) -> KeyId {
        KeyId {
            domain: domain.to_string(),
            msgctxt: None,
            msgid: msgid.to_string(),
            msgid_plural: None,
        }
    }

    fn val(value: &str) -> Cell {
        Cell::Value(value.to_string())
    }

    fn matrix(locales: &[&str], rows: Vec<(KeyId, Vec<Cell>)>) -> Matrix {
        Matrix {
            locales: locales.iter().map(|l| l.to_string()).collect(),
            rows: rows.into_iter().collect(),
        }
    }

    fn config(tiers: Vec<Tier>, min_agree: usize) -> Match {
        Match {
            tiers,
            min_locales_agree: min_agree,
            ..Match::default()
        }
    }

    /// Run grouping over every configured tier — what the pipeline does inline.
    fn group(matrix: &Matrix, config: &Match) -> Vec<CandidateGroup> {
        let mut all = Vec::new();
        for &tier in &config.tiers {
            all.extend(group_tier(matrix, tier, config));
        }
        all
    }

    // --- 4.1 the same bucketing runs over each tier ---

    #[test]
    fn bucketing_is_tier_agnostic() {
        let m = matrix(
            &["en", "ru"],
            vec![
                (key("messages", "a"), vec![val("Save"), val("OK")]),
                (key("messages", "b"), vec![val("Save"), val("OK")]),
            ],
        );
        for tier in [Tier::Exact, Tier::Normalized, Tier::Fuzzy] {
            let groups = group(&m, &config(vec![tier], 2));
            assert_eq!(groups.len(), 1, "tier {tier:?} should find one group");
            assert_eq!(groups[0].agree_locales, 2);
        }
    }

    // --- 4.2 full groups (M/M) ---

    #[test]
    fn full_duplicates_form_one_group() {
        let m = matrix(
            &["en", "es", "ru"],
            vec![
                (
                    key("m", "a"),
                    vec![val("Save"), val("Guardar"), val("Сохранить")],
                ),
                (
                    key("m", "b"),
                    vec![val("Save"), val("Guardar"), val("Сохранить")],
                ),
            ],
        );
        let groups = group(&m, &config(vec![Tier::Exact], 2));
        assert_eq!(groups.len(), 1);
        let g = &groups[0];
        assert_eq!(g.agree_locales, 3);
        assert_eq!(g.total_locales, 3);
        assert!(g.differ.is_empty());
        assert_eq!(g.keys, vec![key("m", "a"), key("m", "b")]);
    }

    // --- 4.3 partial groups (M-1/M) ---

    #[test]
    fn agreement_in_m_minus_one_groups_at_that_level() {
        // differ only in "ru" (index 2).
        let m = matrix(
            &["en", "es", "ru"],
            vec![
                (
                    key("m", "a"),
                    vec![val("Save"), val("Guardar"), val("Сохранить")],
                ),
                (
                    key("m", "b"),
                    vec![val("Save"), val("Guardar"), val("Сохрани")],
                ),
            ],
        );
        let groups = group(&m, &config(vec![Tier::Exact], 2));
        assert_eq!(groups.len(), 1);
        let g = &groups[0];
        assert_eq!(g.agree_locales, 2);
        assert_eq!(g.total_locales, 3);
        assert_eq!(g.differ, vec!["ru".to_string()]);
    }

    // --- 4.4 empty policy: own vs skip differ ---

    #[test]
    fn empty_policy_changes_results() {
        // a,b agree on en/es; "ru" has a value for a, empty for b.
        let rows = || {
            vec![
                (
                    key("m", "a"),
                    vec![val("Save"), val("Guardar"), val("Сохранить")],
                ),
                (
                    key("m", "b"),
                    vec![val("Save"), val("Guardar"), Cell::Empty],
                ),
            ]
        };

        let own = config(vec![Tier::Exact], 2);
        let groups_own = group(&matrix(&["en", "es", "ru"], rows()), &own);
        assert_eq!(groups_own.len(), 1);
        assert_eq!(groups_own[0].total_locales, 3); // ru counts (value vs ∅)
        assert_eq!(groups_own[0].differ, vec!["ru".to_string()]);

        let mut skip = config(vec![Tier::Exact], 2);
        skip.empty_policy = EmptyPolicy::Skip;
        let groups_skip = group(&matrix(&["en", "es", "ru"], rows()), &skip);
        assert_eq!(groups_skip.len(), 1);
        assert_eq!(groups_skip[0].total_locales, 2); // ru drops out
        assert!(groups_skip[0].differ.is_empty());
    }

    // --- 4.5 level dedup ---

    #[test]
    fn full_group_not_duplicated_at_lower_level() {
        let m = matrix(
            &["en", "es", "ru"],
            vec![
                (
                    key("m", "a"),
                    vec![val("Save"), val("Guardar"), val("Сохранить")],
                ),
                (
                    key("m", "b"),
                    vec![val("Save"), val("Guardar"), val("Сохранить")],
                ),
            ],
        );
        // K = 2 → leave-one-out would also bucket {a,b} at level 2, but dedup
        // suppresses it under the level-3 group.
        let groups = group(&m, &config(vec![Tier::Exact], 2));
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].agree_locales, 3);
    }

    // --- 4.6 cross_domain ---

    #[test]
    fn cross_domain_is_flagged() {
        let m = matrix(
            &["en", "ru"],
            vec![
                (key("messages", "x"), vec![val("Save"), val("Сохранить")]),
                (key("django", "x"), vec![val("Save"), val("Сохранить")]),
            ],
        );
        let groups = group(&m, &config(vec![Tier::Exact], 2));
        assert_eq!(groups.len(), 1);
        assert!(groups[0].cross_domain);
    }

    #[test]
    fn single_domain_is_not_cross_domain() {
        let m = matrix(
            &["en", "ru"],
            vec![
                (key("m", "a"), vec![val("Save"), val("Сохранить")]),
                (key("m", "b"), vec![val("Save"), val("Сохранить")]),
            ],
        );
        let groups = group(&m, &config(vec![Tier::Exact], 2));
        assert!(!groups[0].cross_domain);
    }

    // --- 4.7 determinism ---

    #[test]
    fn results_are_deterministic() {
        let m = matrix(
            &["en", "es", "ru"],
            vec![
                (
                    key("m", "a"),
                    vec![val("Save"), val("Guardar"), val("Сохранить")],
                ),
                (
                    key("m", "b"),
                    vec![val("Save"), val("Guardar"), val("Сохранить")],
                ),
                (
                    key("m", "c"),
                    vec![val("Save"), val("Guardar"), val("Other")],
                ),
            ],
        );
        let cfg = config(vec![Tier::Exact, Tier::Normalized], 2);
        assert_eq!(group(&m, &cfg), group(&m, &cfg));
    }

    // --- combinations helper ---

    #[test]
    fn combinations_are_correct() {
        assert_eq!(combinations(3, 0), vec![Vec::<usize>::new()]);
        assert_eq!(combinations(3, 2), vec![vec![0, 1], vec![0, 2], vec![1, 2]]);
        assert_eq!(combinations(2, 3).len(), 0);
    }

    // --- 4.1 fuzzy canonicalization, through the public seam ---

    fn fuzzy_canon(m: &Matrix) -> Matrix {
        // distance 2, min length 5 (the defaults).
        canonical_matrix(m, Tier::Fuzzy, &Normalize::default(), 2, 5)
    }

    #[test]
    fn fuzzy_clusters_per_column_without_transpose() {
        // "delte" sits with "delta" in column 0 (distance 1) so it canonicalizes
        // to the cluster representative "delta"; in column 1 the same "delte" is
        // alone (its neighbour is far away) and stays "delte". One assertion thus
        // proves clustering is column-local AND the axes aren't swapped: a swap
        // would put "delte" in column 0 and "delta" in column 1.
        let a = key("m", "a");
        let b = key("m", "b");
        let m = matrix(
            &["c0", "c1"],
            vec![
                (a.clone(), vec![val("delte"), val("delte")]),
                (b.clone(), vec![val("delta"), val("zzzzz")]),
            ],
        );
        let canon = fuzzy_canon(&m);
        assert_eq!(canon.rows[&a][0], val("delta"), "column 0 clusters");
        assert_eq!(canon.rows[&a][1], val("delte"), "column 1 leaves it alone");
        assert_eq!(canon.rows[&b][0], val("delta"));
        assert_eq!(canon.rows[&b][1], val("zzzzz"));
    }

    #[test]
    fn fuzzy_representative_is_lexicographically_smallest() {
        // Three mutually-near values collapse onto the smallest, "gamma".
        let a = key("m", "a");
        let b = key("m", "b");
        let c = key("m", "c");
        let m = matrix(
            &["en"],
            vec![
                (a.clone(), vec![val("gammc")]),
                (b.clone(), vec![val("gammb")]),
                (c.clone(), vec![val("gamma")]),
            ],
        );
        let canon = fuzzy_canon(&m);
        assert_eq!(canon.rows[&a][0], val("gamma"));
        assert_eq!(canon.rows[&b][0], val("gamma"));
        assert_eq!(canon.rows[&c][0], val("gamma"));
    }

    #[test]
    fn fuzzy_min_length_keeps_short_strings_distinct() {
        // "on"/"off" are under the 5-char floor: they skip fuzzy and stay apart,
        // even though their edit distance is within range.
        let a = key("m", "a");
        let b = key("m", "b");
        let m = matrix(
            &["en"],
            vec![(a.clone(), vec![val("on")]), (b.clone(), vec![val("off")])],
        );
        let canon = fuzzy_canon(&m);
        assert_eq!(canon.rows[&a][0], val("on"));
        assert_eq!(canon.rows[&b][0], val("off"));
    }

    #[test]
    fn fuzzy_preserves_shape_and_empties() {
        let a = key("m", "a");
        let m = matrix(
            &["en", "ru"],
            vec![(a.clone(), vec![val("hello"), Cell::Empty])],
        );
        let canon = fuzzy_canon(&m);
        assert_eq!(canon.locales, m.locales);
        assert_eq!(canon.rows.len(), 1);
        assert_eq!(canon.rows[&a].len(), 2);
        assert_eq!(canon.rows[&a][1], Cell::Empty, "empties pass through");
    }
}
