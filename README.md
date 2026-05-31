# pockingbird

[![CI](https://github.com/Segodnya/pockingbird/actions/workflows/ci.yml/badge.svg)](https://github.com/Segodnya/pockingbird/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)

A CLI that finds **duplicate translation keys** in gettext `.po` catalogs.

Over time a catalog grows keys whose translations are identical across every
locale — real duplicates that can collapse into one key — plus *near*-duplicates
that differ only by trailing punctuation, a typo, or one disagreeing locale.
`pockingbird` surfaces all of them in a single report.

It is **report-only**: it never edits your `.po` files. The decision to collapse
keys stays with you.

## How it works

1. Discover every `.po` file under the target path.
2. Parse each locale into a matrix `key × locale → translation`.
3. Group keys with matching translation signatures, ranked by how many locales
   agree.

A key's translation vector is its signature; two keys match when their vectors
agree. Matching runs at three tiers:

- **exact** — byte-equal after trimming.
- **normalized** — equal after case-folding, whitespace collapsing, and trailing
  punctuation stripping (`Ok` vs `OK.`).
- **fuzzy ≤2** — within Levenshtein distance 2 (typos, stray punctuation). Per
  locale, near strings are clustered (BK-tree + union-find) and compared by
  cluster.

Within each tier, groups are reported by agreement level — full `M/M` first,
then `M−1/M`, `M−2/M`, … down to a configurable floor (`min_locales_agree`).
Empty cells and whole incomplete locales are handled via config.

## Install

```sh
cargo install --path .
```

## Usage

```sh
pockingbird find .                              # current dir, text report
pockingbird find ./locales --format json        # JSON for pipelines
pockingbird find . --config pockingbird.toml    # with a config file
```

- `text` (default) — colored sections per tier, each grouped by level. Every
  group lists its keys, shared translations, and diverging locales; cross-domain
  groups get a *"unify into a shared domain"* hint.
- `json` — the same structure as machine-readable data.

Exit code is always `0` — it reports, it does not gate.

## Configuration (`pockingbird.toml`)

```toml
[scan]
po_patterns = ["**/*.po"]
ignore_dirs = ["vendor", "node_modules", ".git"]
roots = ["."]

[locales]
exclude = []             # e.g. ["ch_CH"] to drop an incomplete locale

[match]
tiers = ["exact", "normalized", "fuzzy"]  # which tiers to compute
fuzzy_max_distance = 2
fuzzy_min_length = 5      # shorter strings skip the fuzzy tier
empty_policy = "own"      # "own": empty is a value | "skip": ignore that cell
min_locales_agree = 5     # report from M/M down to this floor

[match.normalize]
case_fold = true
collapse_whitespace = true
strip_trailing_punct = true

[output]
format = "text"           # "text" | "json"
```

Domain terms are defined in [CONTEXT.md](./CONTEXT.md). Example catalogs live in
[`fixtures/`](./fixtures); an end-to-end test in
[`tests/integration.rs`](./tests/integration.rs).
