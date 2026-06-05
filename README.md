# pockingbird

[![CI](https://github.com/Segodnya/pockingbird/actions/workflows/ci.yml/badge.svg)](https://github.com/Segodnya/pockingbird/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)

A CLI that finds **duplicate translation keys** in gettext `.po` catalogs.

Over time a catalog grows keys whose translations are identical across every
locale — real duplicates that can collapse into one key — plus _near_-duplicates
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
pockingbird find . --preset strict              # exact-match only, no config file
pockingbird find . --min-agree 3 --tier exact   # override knobs from the CLI
pockingbird find . --config pockingbird.toml    # with a config file
pockingbird init                                # write a starter pockingbird.toml
```

- `text` (default) — colored sections per tier, each grouped by level. Every
  group lists its keys, shared translations, and diverging locales; cross-domain
  groups get a _"unify into a shared domain"_ hint.
- `json` — the same structure as machine-readable data.

Exit code is always `0` — it reports, it does not gate. If `min_locales_agree`
exceeds the number of locales found, it is lowered to that count (with a warning)
so the report is never silently empty.

## Configuration

Run `pockingbird init` to drop a commented [`pockingbird.toml`](./pockingbird.toml)
that mirrors the built-in defaults. The fastest way to configure matching is a
**preset**; tune individual knobs on top only if you need to:

```toml
[match]
preset = "balanced"       # strict | balanced | loose
# any explicit field below overrides the preset baseline:
# min_locales_agree = 5
```

- `strict` — exact tier only, no normalization (byte-identical translations).
- `balanced` (default) — exact + normalized + fuzzy≤2, normalization on.
- `loose` — balanced with a wider fuzzy radius (≤3) and a lower length floor.

CLI flags (`--preset`, `--min-agree`, `--tier`, `--exclude`, `--format`) override
the config file, which overrides the preset, which overrides the built-in
defaults. Precedence is resolved **per field**: a CLI `--preset` re-bases the
baseline, but any knob you set explicitly in the file still wins over it. See
[`pockingbird.toml`](./pockingbird.toml) for every knob.

## Library

```rust
use std::path::Path;
use pockingbird::{scan, Config};

let report = scan(Path::new("./locales"), &Config::default())?;
let json = pockingbird::report::to_json(&report.groups, report.total_keys);
```

Domain terms are defined in [CONTEXT.md](./CONTEXT.md). Example catalogs live in
[`fixtures/`](./fixtures); an end-to-end test in
[`tests/integration.rs`](./tests/integration.rs).
