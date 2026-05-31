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
  flagged with a *"unify into a shared domain"* hint.
