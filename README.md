# pockingbird

A CLI that finds **duplicate translation keys** in gettext `.po` catalogs.

Over the years a localization catalog accumulates keys whose translations are
identical across every locale — actual duplicates that can be collapsed into one
key. It also accumulates *near*-duplicates: keys that differ only by a trailing
punctuation mark or a one-character typo, or that agree in most locales but not
all (e.g. 5 of 6). `pockingbird` surfaces all of these in a single report.

It is **report-only**: it never edits your `.po` files. The decision to collapse
keys stays with you.

## How it works

1. Discover every `.po` file under the target path.
2. Parse each locale's catalog and build a matrix `key × locale → translation`.
3. Group keys by their translation signature, then report duplicate groups
   ranked by how many locales agree.

A key's translation vector is its signature. Two keys are duplicates when their
vectors match across locales. Matching happens at three tiers:

- **exact** — translations are byte-equal (after trimming).
- **normalized** — equal after case-folding, whitespace collapsing, and trailing
  punctuation stripping (catches `Ok` vs `OK.`).
- **fuzzy ≤2** — within Levenshtein distance 2 (catches typos and stray
  punctuation). This is a global tier: per locale, strings within edit distance
  are clustered (BK-tree + union-find) and compared by cluster.

Groups are reported in tiers by the number of agreeing locales — full matches
(`M/M`) first, then partial matches (`M−1/M`, `M−2/M`, …) down to a configurable
floor. Keys that are empty in a locale, and entire locales that are incomplete,
can be handled via configuration.

## Install

```sh
cargo install --path .
```

## Usage

```sh
# Scan the current directory, human-readable report
pockingbird find .

# Point at a locales root, JSON output for pipelines
pockingbird find ./path/to/locales --format json

# Use a config file
pockingbird find . --config pockingbird.toml
```

### Output

- `text` (default) — colored sections per tier (exact → normalized → fuzzy),
  each grouped by match level (`M/M`, `M−1/M`, …). Every group lists its keys,
  the shared translations, and the locales where they diverge. A summary at the
  bottom reports the number of groups, keys, and candidate keys (never
  "removable" — the call is yours).
- `json` — the same group structure as machine-readable data.

Exit code is always `0` — `pockingbird` reports, it does not gate.

## Configuration (`pockingbird.toml`)

```toml
[scan]
po_patterns = ["**/*.po"]
ignore_dirs = ["vendor", "node_modules", ".git"]
roots = ["."]

[locales]
exclude = []              # e.g. ["ch_CH"] to drop an incomplete locale

[match]
tiers = ["exact", "normalized", "fuzzy"]  # which tiers to compute and show
fuzzy_max_distance = 2
fuzzy_min_length = 5      # strings shorter than this skip the fuzzy tier
empty_policy = "own"      # "own": empty is its own value | "skip": ignore that locale
min_locales_agree = 5     # report tiers from M/M down to this floor

[match.normalize]
case_fold = true
collapse_whitespace = true
strip_trailing_punct = true

[output]
format = "text"           # "text" | "json"
```

## Status

Implemented and tested. The full pipeline (`walk → po → index → group →
report`) is in place: `.po` discovery, parsing, the `key × locale` matrix, all
three match tiers (exact / normalized / fuzzy), partial-match levels via
leave-one-out down to the configured floor, the `own`/`skip` empty policy, and
`text` / `json` reports. Covered by per-module unit tests plus an end-to-end test
([`tests/integration.rs`](./tests/integration.rs)); example catalogs live in
[`fixtures/`](./fixtures).

See [PLAN.md](./PLAN.md) for the design and [TODO.md](./TODO.md) for the task
breakdown.
