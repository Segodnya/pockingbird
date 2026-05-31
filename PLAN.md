# Implementation plan — pockingbird

A staged plan to build a report-only CLI that finds duplicate translation keys
across gettext `.po` catalogs. Each stage is independently buildable and testable.

## Goal

Parse all `.po` files under a target path, build a `key × locale → translation`
matrix, and report groups of duplicate keys ranked by how many locales agree and
at which match tier (exact / normalized / fuzzy ≤2). Report only — never edit
`.po` files.

## Reused approach

The structure: `polib` for parsing,
`ignore`/`globset` for file discovery, `rayon` for parallelism, `clap` for the
CLI (under a `cli` feature), `colored` + `serde_json` for output, and temp-dir
fixtures for integration tests. There is **no source
scanning** — no tree-sitter — because we only read `.po` files.

## Dependencies

- `polib` — `.po` parsing.
- `ignore` + `globset` — discover `**/*.po`.
- `rayon` — parallel parsing and comparisons.
- `strsim` — Levenshtein distance for the fuzzy tier.
- BK-tree — small in-house module (or the `bk-tree` crate) for "all strings
  within edit distance ≤ d"; union-find — trivial in-house.
- `clap` (derive, `cli` feature), `colored` (`cli` feature).
- `serde` + `serde_json`, `toml`.

## Module layout

```
src/
  lib.rs        // pipeline: walk -> po -> index -> group -> report
  config.rs     // TOML schema (Config/Match/Locales/Output) + defaults
  walk.rs       // discover .po files (adapted walk)
  locale.rs     // locale id from path (.../<locale>/LC_MESSAGES/*.po); path fallback
  po.rs         // parse a .po via polib -> Catalog; domain from filename stem
  normalize.rs  // canonicalize a string per tier (trim/case/whitespace/punct)
  fuzzy.rs      // BK-tree + union-find -> per-locale fuzzy string clusters
  index.rs      // matrix KeyId -> [per locale: Cell]
  group.rs      // CORE: signature bucketing + leave-one-out (tier-agnostic)
  report.rs     // render text(colored)/json
  cli.rs        // clap: find <path> --config --format
  main.rs       // entrypoint, exit code
tests/integration.rs  // run the binary over synthetic .po fixtures
fixtures/             // .po sets (en/ru/es/pt/tr/id) per case
pockingbird.toml      // example config
```

## Data model

- `KeyId = { domain, msgctxt: Option<String>, msgid: String, msgid_plural: Option<String> }`.
  `domain` comes from the `.po` filename stem (`messages`, `django`, …). It makes
  `en/messages.po:X` and `en/django.po:X` **distinct keys**, so the same
  `msgctxt+msgid` across domains never collides in one locale column — and a pair
  that turns out identical becomes a cross-domain candidate (see Output).
- **Locale id (column axis).** Derived from the path `.../<locale>/LC_MESSAGES/*.po`.
  When the layout doesn't match, **fall back to the file path / parent dir as the
  id**. The report shows whichever id was used (locale or path).
- Plural forms: all `msgstr[0..n]` joined into one per-locale string
  (separator `\u{1}`).
- `Cell = Some(canonical) | Empty` — a key's value in a locale.
- `Matrix`: `KeyId -> Vec<Cell>` over a fixed order of active locales
  `L = all_locales \ exclude`, `M = |L|`.
- **Eligibility guard.** A key participates in grouping only if it has
  `≥ min_locales_agree` **non-empty** cells. A key with nothing (or almost
  nothing) translated has nothing to match on; this kills the degenerate
  "everything empty" bucket under both `own` and `skip`.

## Core algorithm (group.rs + fuzzy.rs)

The tier decides a cell's **canonical value**; bucketing and leave-one-out are
tier-agnostic and run on top of those canonicals.

Cell canonicalization per tier:

- **exact** — trim.
- **normalized** — case-fold + collapse whitespace + strip trailing punctuation
  (`exact ⊂ normalized`).
- **fuzzy ≤2** (global tier) — per locale, build a BK-tree over all non-empty
  values, find each string's neighbors within Levenshtein ≤ threshold, union-find
  them into clusters, and use the cluster representative (the lexicographically
  smallest member, for determinism) as the cell canonical. This reduces global
  fuzzy matching to a hashable canonical and reuses the bucketing pipeline below.
  Note: edit distance is not transitive, so union-find clusters are approximate
  (a chain of ≤2 steps can drift further apart). This is documented and
  acceptable for duplicate hunting.
  **Min-length guard:** strings shorter than `fuzzy_min_length` (default 5) skip
  the fuzzy tier entirely (their canonical is the normalized value) — distance ≤2
  on short strings merges genuinely different words (`Save`/`Same`, `On`/`Off`).

Bucketing (identical for every tier):

1. **Full groups (M/M).** Bucket keys by the full signature (tuple of canonicals
   over `L`). A bucket of size ≥2 is a duplicate group.
2. **Partial groups (M−t/M), t = 1..T.** Leave-one-out: generate sub-signatures
   with `t` locales dropped (`C(M,t)` of them); a bucket groups keys that agree
   in the remaining `M−t` locales, with the dropped locales being where they
   diverge. `T = M − K`, where `K = min_locales_agree`.
3. **Empty policy (cell-level).** `own`: `Empty` is a distinct token in the
   signature. `skip`: an empty cell drops out of the sub-signature (via the same
   leave-one-out machinery) and out of the denominator. Dropping a whole locale is
   a separate mechanism — `[locales].exclude`, applied before the matrix is built —
   not an empty policy.
4. **Level dedup (within a tier) — kept.** A full `M/M` group also satisfies its
   own `M−1`, `M−2`, … sub-signatures (leave-one-out), so a group is shown only at
   its **highest** agreement level; lower levels suppress any group whose agreeing
   set is a subset of one already shown above. Buckets of size 1 are ignored.
5. **Cross-tier dedup — dropped.** Tiers nest (`exact ⊂ normalized ⊂ fuzzy`), so an
   exact duplicate also appears in the normalized and fuzzy sections. We **do not**
   reconcile across tiers: each tier section is self-contained and the reader scans
   from the strongest. This removes the edge-reconciliation logic from `group.rs`.

```
CandidateGroup {
  keys, agree_locales, total_locales,
  shared:  Map<Locale, String>,
  differ:  Map<Locale, Vec<(KeyId, String)>>,
  tier:    Exact | Normalized | Fuzzy,
  cross_domain: bool,   // keys span >1 domain -> "unify into a shared domain"
}
```

The `group.rs` interface is the **canonical `Matrix`**, not a per-cell
canonicalize function: `exact`/`normalized` are pure per-cell `Fn(&str)->String`,
`fuzzy` is per-locale `Fn(&[String])->Map`; all three produce a canonical matrix
that bucketing consumes, so `group.rs` stays genuinely tier-agnostic.

Complexity: bucketing is `O(n · C(M,T))` hashes (17k × `C(7, 1..2)` is cheap);
fuzzy is BK-tree queries per locale (`≈ n·log n` + neighbors), not a global
`O(n²)`. Parallelize with rayon; results are deterministic (sorted by key).

Partial matching is meant for "almost all" locales, so `T = M − K` is small by
design. At startup we validate `Σ C(M, 0..T)` against a cap (a few hundred
sub-signatures per key); a `min_locales_agree` too low for `M` is a **config error**
with a clear message, not a silent combinatorial blow-up.

## Configuration (pockingbird.toml)

```toml
[scan]
po_patterns = ["**/*.po"]
ignore_dirs = ["vendor", "node_modules", ".git"]
roots = ["."]

[locales]
exclude = []              # drop whole locales BEFORE the matrix (one column gone)

[match]
tiers = ["exact", "normalized", "fuzzy"]
fuzzy_max_distance = 2
fuzzy_min_length = 5      # strings shorter than this skip the fuzzy tier
empty_policy = "own"      # own | skip — applies to an empty CELL only
min_locales_agree = 5     # report tiers from M/M down to this floor; validated
                          # against M so T = M - K stays small (see Core algorithm)

[match.normalize]
case_fold = true
collapse_whitespace = true
strip_trailing_punct = true

[output]
format = "text"           # text | json
```

## Output

- **text** (colored): sections per tier (exact → normalized → fuzzy), each
  grouped by match level (`M/M`, `M−1/M`, …). Each group lists keys, shared
  values, and diverging columns (locale id, or path when the locale couldn't be
  derived). Cross-domain groups are flagged with a *"unify into a shared domain"*
  hint. A summary reports groups / keys / **candidate** keys (never "removable" —
  with no source scan the tool can't prove call-sites are interchangeable).
- **json**: the same group structure, machine-readable.
- Exit code: always `0` (`--fail-on-dupes` is out of MVP scope — YAGNI).

## Stages

1. **Skeleton.** Cargo.toml, lib.rs (pipeline doc), stub modules, clap CLI
   `find <path> --config --format`, exit code. Builds clean.
2. **Parsing.** `walk.rs` (find `.po`), `locale.rs` (id from path), `po.rs`
   (polib → Catalog), `config.rs` (TOML + defaults). Unit-test parsing on a fixture.
3. **Matrix + normalization.** `normalize.rs` (tiers), `index.rs` (KeyId × locale).
   Unit-test canonicalization.
4. **Fuzzy clusters.** `fuzzy.rs`: BK-tree + union-find → per-locale clusters.
   Unit-test cluster formation and determinism.
5. **Grouping core.** `group.rs`: full groups → leave-one-out → empty policies →
   tier dedup; tier-agnostic over canonicals. Unit-test each case.
6. **Reports.** `report.rs`: text(colored) / json + summary.
7. **Integration + fixtures.** `fixtures/` with synthetic `.po` sets, integration
   test running the binary.

## Fixtures and tests

Synthetic `.po` files (6 locales `en/ru/es/pt/tr/id`) crafted to exercise:

- (a) a full duplicate across all locales;
- (b) an `M−1` match with one diverging locale;
- (c) a fuzzy/punctuation pair (`Save` vs `Save.`);
- (d) empty `msgstr` under both `own` and `skip` policies;
- (e) plural forms.

- **Integration**: temp-dir →
  write fixtures + config → run the binary with `--format json` → assert group
  membership and tiers.
- **Unit** per module: `normalize` (tiers), `fuzzy` (BK-tree/union-find clusters),
  `group` (bucketing/leave-one-out/empty), `locale` (id derivation).

## Verification

- `cargo test` — unit + integration green.
- `cargo run -- find ./fixtures --format text` — eyeball the tier sections.
- Run against a real locales directory (path passed as an argument, outside the
  repo) with `--format json | head` — expected groups appear and it does not
  choke on ~17k keys × 7 locales. Real-project output is never committed.

## Open details (non-blocking)

- Exact `normalize` rules (which punctuation characters to strip).
- BK-tree: in-house module vs the `bk-tree` crate.
- gettext flags: whether to skip obsolete (`#~`) and fuzzy-flagged (`#, fuzzy`)
  entries, or feed them into the matrix. Decide before the first real-data run.
- Source-locale empty `msgstr` (where the value is implicitly the `msgid`):
  currently treated as `Empty`. Possible later refinement: treat as `= msgid`.
- Short common strings (`OK`×50) are valid candidates by definition and are
  reported as-is; no frequency/length noise filter in the MVP.
