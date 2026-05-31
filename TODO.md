# TODO — pockingbird

Step-by-step decomposition of [PLAN.md](./PLAN.md). Each task is atomic with a
verifiable result. Terminology follows [CONTEXT.md](./CONTEXT.md).

Verification convention: commands are run by the **user** (per CLAUDE.md — the
agent does not run `cargo build`/`commit`). The "Verify" line is the
done-criterion to run manually.

Standing quality gate (run after each phase, alongside the per-task Verify):

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
```

`build` keeps warnings non-fatal (stub phases compile); the gate above is where
warnings are denied.

---

## Phase 0 — Skeleton

Goal: project builds, CLI responds, stub modules in place.

- [ ] **0.1 Cargo.toml.** Manifest: name `pockingbird`, edition 2021,
  `[features] cli`, binary behind the feature. Dependencies from PLAN (polib,
  ignore, globset, rayon, strsim, clap, colored, serde, serde_json, toml).
  _Verify:_ `cargo metadata` resolves the package without errors.
- [ ] **0.2 Module stubs.** Create `src/{lib,config,walk,locale,po,normalize,fuzzy,index,group,report,cli,main}.rs`
  with empty/`todo!()` signatures. In `lib.rs` — a doc comment of the pipeline
  `walk → po → index → group → report`.
  _Verify:_ `cargo build` is green.
- [ ] **0.3 CLI scaffold.** `clap` (derive): subcommand `find <path>`,
  flags `--config <path>`, `--format text|json`. Exit code always `0`.
  _Verify:_ `cargo run -- find . --format json` runs, `--help` shows all flags,
  exit code `0`.

---

## Phase 1 — Parsing & config

Goal: discover `.po`, parse them, derive locale id and domain, read TOML.

- [ ] **1.1 config.rs — schema.** Structs `Config/Scan/Locales/Match/Normalize/Output`
  with `serde` + defaults exactly as in PLAN. Parse from a TOML string.
  _Verify:_ unit test: default config == expected values; the example
  `pockingbird.toml` parses without errors.
- [ ] **1.2 config.rs — validate `min_locales_agree`.** At startup check
  `Σ C(M, 0..T)` against a cap; a `K` too low for `M` → a clear config error,
  not a panic.
  _Verify:_ unit test: valid `K` is OK; a deliberately low `K` → `Err` with a
  clear message.
- [ ] **1.3 walk.rs — discover `.po`.** Via `ignore`+`globset` walk `roots`,
  apply `po_patterns` and `ignore_dirs`.
  _Verify:_ unit test on a temp-dir: finds nested `.po`, skips
  `vendor/node_modules/.git`.
- [ ] **1.4 locale.rs — id from path.** From `.../<locale>/LC_MESSAGES/*.po`
  extract `<locale>`; on layout mismatch — fall back to path/parent dir.
  _Verify:_ unit test: standard path → `ru`; non-standard → path fallback.
- [ ] **1.5 po.rs — parse catalog.** polib → `Catalog`; `domain` from the file
  stem. Plurals `msgstr[0..n]` joined with `\u{1}`.
  _Verify:_ unit test on a fixture: msgid/msgstr/msgctxt read, domain correct,
  plurals joined.
- [ ] **1.6 gettext flags decision.** Fix (in code + comment): whether to skip
  obsolete (`#~`) and `#, fuzzy` entries. (Open detail from PLAN.)
  _Verify:_ unit test confirms the chosen behavior on a fixture with such
  entries.

---

## Phase 2 — Matrix & normalization

Goal: have `KeyId × locale → Cell` and exact/normalized canonicalization.

- [ ] **2.1 normalize.rs — exact.** `trim`.
  _Verify:_ unit test: `"  Ok  " → "Ok"`.
- [ ] **2.2 normalize.rs — normalized.** case-fold + collapse whitespace +
  strip trailing punct, configurable via `[match.normalize]` flags.
  Fix the set of stripped punctuation (Open detail).
  _Verify:_ unit test: `"OK."`/`"ok"`/`"O K" → "ok"`; `exact ⊂ normalized`.
- [ ] **2.3 index.rs — KeyId.** Type `KeyId{domain,msgctxt,msgid,msgid_plural}`,
  hashable/ordered.
  _Verify:_ unit test: `messages.po:X` ≠ `django.po:X`.
- [ ] **2.4 index.rs — matrix.** Build `Matrix: KeyId → Vec<Cell>` over a fixed
  order of active locales `L = all \ exclude`. Empty msgstr → `Empty`.
  _Verify:_ unit test: vector length == `M`; columns in stable order; exclude
  removes a column.
- [ ] **2.5 index.rs — eligibility guard.** A key joins grouping only with
  `≥ K` non-empty cells.
  _Verify:_ unit test: a key with `< K` non-empty cells is filtered out.

---

## Phase 3 — Fuzzy clusters

Goal: per-locale fuzzy ≤2 canonicalization via BK-tree + union-find.

- [ ] **3.1 BK-tree.** Decide in-house vs the `bk-tree` crate (Open detail),
  build the module: insert + "all within ≤ d" query (Levenshtein via `strsim`).
  _Verify:_ unit test: query returns the correct neighbors within the radius.
- [ ] **3.2 union-find.** Trivial structure with path compression.
  _Verify:_ unit test: union/find produce correct classes.
- [ ] **3.3 fuzzy.rs — per-locale clusters.** For non-empty values of a locale:
  BK-tree → neighbors ≤ `fuzzy_max_distance` → union-find → representative =
  lexicographically smallest member.
  _Verify:_ unit test: `Save`/`Save.` in one cluster; representative
  deterministic.
- [ ] **3.4 fuzzy.rs — min-length guard.** Strings shorter than
  `fuzzy_min_length` skip fuzzy (canonical = normalized).
  _Verify:_ unit test: `On`/`Off` do not merge at the default threshold.

---

## Phase 4 — Grouping core

Goal: tier-agnostic bucketing over the canonical matrix.

- [ ] **4.1 Canonical matrix as the seam.** `group.rs` consumes a ready
  canonical `Matrix`, not a per-cell function. exact/normalized are
  `Fn(&str)->String`, fuzzy is `Fn(&[String])->Map`; all three produce a
  canonical matrix.
  _Verify:_ unit test: the same bucketing runs over each tier.
- [ ] **4.2 Full groups (M/M).** Bucket by the full signature; a bucket ≥2 → a
  group.
  _Verify:_ unit test: a pair of full duplicates lands in one group.
- [ ] **4.3 Partial groups (M−t/M).** Leave-one-out over `t=1..T`, `T=M−K`;
  diverging locales = dropped columns.
  _Verify:_ unit test: a pair agreeing in `M−1` locales groups at level `M−1`.
- [ ] **4.4 Empty policy.** `own`: `Empty` is a distinct token; `skip`: an empty
  cell drops out of the sub-signature and the denominator.
  _Verify:_ unit test: one fixture with an empty msgstr yields different results
  under `own` vs `skip`.
- [ ] **4.5 Level dedup (kept).** A group is shown only at its highest agreement
  level; lower levels suppress subsets. Buckets of size 1 ignored.
  _Verify:_ unit test: a full group is not duplicated at `M−1`.
- [ ] **4.6 CandidateGroup + cross_domain.** Assemble the struct
  `{keys, agree_locales, total_locales, shared, differ, tier, cross_domain}`;
  `cross_domain=true` if keys span >1 domain.
  _Verify:_ unit test: a group with two domains is flagged `cross_domain`.
- [ ] **4.7 Determinism + rayon.** Results sorted by key; parallelism does not
  change the output.
  _Verify:_ unit test: two runs produce identical ordering.

---

## Phase 5 — Reports

Goal: text(colored) and json with identical group structure.

- [ ] **5.1 report.rs — json.** Serialize groups (serde_json), stable order.
  _Verify:_ unit/snapshot test: valid JSON, structure == `CandidateGroup`.
- [ ] **5.2 report.rs — text.** Sections per tier (exact→normalized→fuzzy),
  within each by match level (`M/M`, `M−1/M`…); each group: keys, shared values,
  diverging columns (locale id or path).
  _Verify:_ eyeball `cargo run -- find ./fixtures --format text` — sections
  readable, colors present.
- [ ] **5.3 Cross-domain hint.** In text, flag cross-domain groups with the
  *"unify into a shared domain"* hint.
  _Verify:_ the hint appears in the output on a cross-domain fixture.
- [ ] **5.4 Summary.** Footer: number of groups / keys / **candidate** keys
  (never "removable").
  _Verify:_ counters match the number of groups in the fixture.

---

## Phase 6 — Integration & fixtures

Goal: synthetic `.po` sets and an e2e binary test.

- [ ] **6.1 Fixtures.** `fixtures/` over 6 locales (en/ru/es/pt/tr/id), covering
  cases: (a) full duplicate; (b) `M−1`; (c) fuzzy/punctuation `Save`/`Save.`;
  (d) empty msgstr under `own`/`skip`; (e) plurals.
  _Verify:_ all files parse via `cargo run -- find ./fixtures`.
- [ ] **6.2 pockingbird.toml.** Example config at the repo root (per README/PLAN).
  _Verify:_ `cargo run -- find ./fixtures --config pockingbird.toml` is OK.
- [ ] **6.3 Integration test.** `tests/integration.rs`: temp-dir → write
  fixtures + config → run the binary with `--format json` → assert group
  membership and tiers.
  _Verify:_ `cargo test --test integration` is green.

---

## Phase 7 — Final verification

- [ ] **7.1 Full test run.** `cargo test` — unit + integration green.
- [ ] **7.2 Eyeball text output.** `cargo run -- find ./fixtures --format text`
  — all tier sections correct.
- [ ] **7.3 Real data.** Run against a real locales directory (path as an
  argument, outside the repo) `--format json | head`: expected groups appear, it
  does not choke on ~17k keys × 7 locales. Real-project output is never
  committed.
- [ ] **7.4 README status.** Update the Status section from "Design stage" to the
  current state.
