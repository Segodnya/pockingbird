# Implementation plan — pockingbird

A staged plan to build a report-only CLI that finds duplicate translation keys
across gettext `.po` catalogs. Each stage is independently buildable and testable.

## Goal

Parse all `.po` files under a target path, build a `key × locale → translation`
matrix, and report groups of duplicate keys ranked by how many locales agree and
at which match tier (exact / normalized / fuzzy ≤2). Report only — never edit
`.po` files.

## Reused approach

The structure mirrors the sibling crate `dead-poets`: `polib` for parsing,
`ignore`/`globset` for file discovery, `rayon` for parallelism, `clap` for the
CLI (under a `cli` feature), `colored` + `serde_json` for output, and temp-dir
fixtures for integration tests. Unlike `dead-poets`, there is **no source
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
  walk.rs       // discover .po files (adapted from dead-poets walk)
  locale.rs     // derive locale id from path (.../<locale>/LC_MESSAGES/*.po)
  po.rs         // parse a locale catalog via polib -> Catalog
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

- `KeyId = { msgctxt: Option<String>, msgid: String, msgid_plural: Option<String> }`.
- Plural forms: all `msgstr[0..n]` joined into one per-locale string
  (separator `\u{1}`).
- `Cell = Some(canonical) | Empty` — a key's value in a locale.
- `Matrix`: `KeyId -> Vec<Cell>` over a fixed order of active locales
  `L = all_locales \ exclude`, `M = |L|`.

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

Bucketing (identical for every tier):

1. **Full groups (M/M).** Bucket keys by the full signature (tuple of canonicals
   over `L`). A bucket of size ≥2 is a duplicate group.
2. **Partial groups (M−t/M), t = 1..T.** Leave-one-out: generate sub-signatures
   with `t` locales dropped (`C(M,t)` of them); a bucket groups keys that agree
   in the remaining `M−t` locales, with the dropped locales being where they
   diverge. `T = M − K`, where `K = min_locales_agree`.
3. **Empty policy.** `own`: `Empty` is a distinct token in the signature.
   `skip`: an empty locale drops out of the sub-signature (via the same
   leave-one-out machinery) and out of the denominator. `exclude` removes locales
   before the matrix is built.
4. **Tier/level dedup.** Each key is attributed to its strongest tier and highest
   match level; a partial section only shows links not already covered above.
   Buckets of size 1 are ignored.

```
DuplicateGroup {
  keys, agree_locales, total_locales,
  shared:  Map<Locale, String>,
  differ:  Map<Locale, Vec<(KeyId, String)>>,
  tier:    Exact | Normalized | Fuzzy,
}
```

Complexity: bucketing is `O(n · C(M,T))` hashes (17k × `C(7, 1..2)` is cheap);
fuzzy is BK-tree queries per locale (`≈ n·log n` + neighbors), not a global
`O(n²)`. Parallelize with rayon; results are deterministic (sorted by key), as in
`dead-poets`.

## Configuration (pockingbird.toml)

```toml
[scan]
po_patterns = ["**/*.po"]
ignore_dirs = ["vendor", "node_modules", ".git"]
roots = ["."]

[locales]
exclude = []

[match]
tiers = ["exact", "normalized", "fuzzy"]
fuzzy_max_distance = 2
empty_policy = "own"      # own | skip
min_locales_agree = 5

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
  values, and diverging locales. A summary reports groups / keys / potentially
  removable keys.
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

- **Integration** (mirrors `dead-poets/tests/integration.rs`): temp-dir →
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
