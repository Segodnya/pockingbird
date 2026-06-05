# Context — pockingbird

Domain vocabulary for `pockingbird`, a report-only CLI that finds duplicate
translation keys across gettext `.po` catalogs. Use these terms exactly; they name
the seams the design is built around.

## Core terms

- **Key** — a translatable entry, identified by
  `KeyId = { domain, msgctxt, msgid, msgid_plural }`. The `domain` is part of the
  identity: `messages.po:X` and `django.po:X` are **different keys**.
- **Domain** — the `.po` filename stem (`messages`, `django`, …). Disambiguates a
  key within a locale, so two domains never collide in one column.
- **Locale id (column)** — the matrix column axis, derived from the path
  `.../<locale>/LC_MESSAGES/*.po`. When the layout doesn't match, the **path itself
  is the id** (path fallback). The report shows whichever id was used.
- **Matrix** — `KeyId -> Vec<Cell>` over a fixed locale order. The rows are keys,
  the columns are locale ids.
- **Cell** — a key's value in one locale: `Some(canonical)` or `Empty`.
- **Signature** — a key's vector of canonical cells. Two keys are duplicate
  candidates when their signatures match (fully, or in `M−t` columns).

## Matching

- **Tier** — the rule that turns a cell into its canonical value: **exact**
  (trim), **normalized** (case-fold + collapse whitespace + strip trailing punct,
  `exact ⊂ normalized`), **fuzzy ≤2** (per-locale BK-tree + union-find clustering;
  the cluster representative is the canonical). Tiers nest.
- **Canonical** — the comparable form of a cell under a tier.
- **Canonical Matrix (the seam)** — `group.rs` consumes a matrix of canonicals, not
  a per-cell function. `exact`/`normalized` are pure per-cell `Fn(&str)->String`;
  `fuzzy` is per-locale `Fn(&[String])->Map`. All three produce a canonical matrix,
  so bucketing stays tier-agnostic. **The interface is the matrix, not the tier.**
- **Match level** — how many columns agree: full `M/M`, then partial `M−1/M`, …
  down to `min_locales_agree` (the floor `K`). `T = M − K` is the leave-one-out
  depth; it is small by design ("almost all" locales), and validated at startup.

## Eligibility and policies

- **Eligibility guard** — a key joins grouping only with `≥ K` **non-empty** cells.
  Untranslated keys have nothing to match on; this kills the all-`Empty` bucket.
- **Empty policy (cell-level)** — `own`: `Empty` is a distinct signature token;
  `skip`: an empty cell drops out of the sub-signature and the denominator.
- **Locale exclude** — dropping a whole locale before the matrix is built
  (`[locales].exclude`). A separate mechanism from the empty policy.
- **Fuzzy min-length** — strings shorter than `fuzzy_min_length` skip the fuzzy
  tier (distance ≤2 on short strings merges different words).

## Reporting

- **Candidate** — a reported duplicate group. Never called "removable": with no
  source scan the tool can't prove call-sites are interchangeable; the human
  decides whether to collapse.
- **Level dedup (kept)** — a group is shown only at its highest agreement level.
- **Cross-tier dedup (dropped)** — an exact duplicate may reappear in the
  normalized and fuzzy sections; tiers are self-contained.
- **Cross-domain candidate** — a group whose keys span more than one domain,
  flagged with a _"unify into a shared domain"_ hint.

## Pipeline

- **Pipeline run** — the deep core (`pipeline.rs`, no `cli` feature): given `.po`
  paths, a parse adapter, config, and a progress sink, it parses, builds the
  Matrix, reconciles the floor, gates eligibility, groups every tier, and returns a
  **Report**. This is the test surface — the seam the CLI shell wraps.
- **Parse adapter** — the injected `Fn(&Path) -> Result<ParsedCatalog, PoError>`.
  Real adapter reads the file (`po::parse_po`); the in-memory adapter feeds tests
  without the filesystem. The seam that makes the skip-policy testable.
- **Skip-policy** — a catalog that fails to parse is recorded in `Report.skipped`
  and the run continues; never fatal (report-only). Lives in the pipeline run.
- **Progress sink** — a one-method seam (`emit(PipelineEvent)`) the run pushes
  events to. The stderr adapter renders and times; the collector adapter records
  events for assertions. Keeps timing/IO out of the core.
- **Report** — the run's data result: the candidate groups, the total key count,
  and the skipped list. No timings, no formatting — rendering is the shell's job.
- **Facade (`scan`)** — the one-call library door: `scan(root, &Config) -> Report`
  discovers, parses, and runs the pipeline. The CLI is a thin shell over it (it
  does not re-wire walk/pipeline itself). `scan_with` takes a progress sink.

## Configuration

- **RawConfig** — the deserialization target (`scan`, `locales`, `match`, `output`),
  where `match` is a **MatchOverride**, not a resolved `Match`. `deny_unknown_fields`
  lives here. The only thing TOML parses into.
- **MatchOverride** — a draft `[match]`: every knob `Option`, plus a `preset`. A
  missing knob means "fall back"; a present knob is an explicit override. The CLI
  builds one from flags (`--tier []` → `None`); the file parses into one.
- **Resolve seam** — `resolve(file: MatchOverride, cli: MatchOverride) -> Match`,
  the single place precedence is applied: `preset = cli.preset ?? file.preset ??
Balanced`, then overlay `base ← file ← cli` field by field. \*\*CLI > file > preset
  > default\**, per field. `Config::from_toml` resolves with an empty CLI override;
  > the CLI passes its own. `Config` itself is the *resolved\* product (not
  > `Deserialize`); its `match` is always a fully-populated `Match`.
- **FloorDecision** — the result of `reconcile_floor(M)`: the **effective** floor
  and whether it was **clamped**. One call folds both floor checks — too low for
  `M` is a hard `ConfigError` (sub-signature blow-up); too high is clamped down to
  `M` (a warning, never a silently-empty report). The two never collide: a floor
  above `M` has leave-one-out depth `0`, so it cannot blow up.
