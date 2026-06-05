//! TOML configuration schema (`Config`/`Scan`/`Locales`/`Match`/`Normalize`/
//! `Output`) plus defaults. Parameterizes every pipeline stage.
//!
//! Defaults mirror the example `pockingbird.toml` exactly. Missing
//! tables/fields fall back to these defaults via container-level `serde(default)`.

use std::fmt;
use std::path::PathBuf;

use serde::Deserialize;

/// Cap on the number of leave-one-out sub-signatures per key. Validated at
/// startup so a `min_locales_agree` too low for `M` is a config error, not a
/// silent combinatorial blow-up. See [`Config::reconcile_floor`].
pub const MAX_SUBSIGNATURES: u128 = 512;

/// The resolved configuration the pipeline consumes. Built from a `RawConfig`
/// by resolving `[match]` (see [`Config::from_toml`]); not itself `Deserialize`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub scan: Scan,
    pub locales: Locales,
    pub match_: Match,
    pub output: Output,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Scan {
    pub po_patterns: Vec<String>,
    pub ignore_dirs: Vec<String>,
    pub roots: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Locales {
    pub exclude: Vec<String>,
}

/// Resolved match settings: a preset baseline with explicit file/CLI field
/// overrides layered on (see `resolve`), so downstream code always sees a
/// fully-populated struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub tiers: Vec<Tier>,
    pub fuzzy_max_distance: usize,
    pub fuzzy_min_length: usize,
    pub empty_policy: EmptyPolicy,
    pub min_locales_agree: usize,
    pub normalize: Normalize,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Normalize {
    pub case_fold: bool,
    pub collapse_whitespace: bool,
    pub strip_trailing_punct: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Output {
    pub format: Format,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "cli", derive(clap::ValueEnum))]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Exact,
    Normalized,
    Fuzzy,
}

impl Tier {
    /// Every tier in canonical order (`exact ⊂ normalized ⊂ fuzzy`). The single
    /// source of tier ordering — both the default `tiers` and the report's
    /// section order derive from this, so a new tier can't be silently omitted.
    pub const ALL: [Tier; 3] = [Tier::Exact, Tier::Normalized, Tier::Fuzzy];

    /// The tier's lowercase name, matching its serde representation. Used in both
    /// the JSON `tier` field and the text section header.
    pub fn label(self) -> &'static str {
        match self {
            Tier::Exact => "exact",
            Tier::Normalized => "normalized",
            Tier::Fuzzy => "fuzzy",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmptyPolicy {
    Own,
    Skip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "cli", derive(clap::ValueEnum))]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Text,
    Json,
}

/// A named bundle of `[match]` settings. The preset supplies the baseline for
/// every knob; any field set explicitly in `[match]` (or on the CLI) overrides
/// it. Lets a user pick a profile instead of tuning six knobs by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(feature = "cli", derive(clap::ValueEnum))]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    /// Exact tier only, no normalization — only byte-identical translations match.
    Strict,
    /// Exact + normalized + fuzzy≤2 with normalization on. The default.
    Balanced,
    /// Like `balanced`, but a wider fuzzy radius (≤3) and a lower length floor.
    Loose,
}

impl Preset {
    /// The full `Match` baseline this preset stands for. Explicit fields layer
    /// on top of this; `balanced` is the single source of the built-in defaults.
    pub fn baseline(self) -> Match {
        let normalize_all = Normalize {
            case_fold: true,
            collapse_whitespace: true,
            strip_trailing_punct: true,
        };
        match self {
            Preset::Strict => Match {
                tiers: vec![Tier::Exact],
                fuzzy_max_distance: 2,
                fuzzy_min_length: 5,
                empty_policy: EmptyPolicy::Own,
                min_locales_agree: 5,
                normalize: Normalize {
                    case_fold: false,
                    collapse_whitespace: false,
                    strip_trailing_punct: false,
                },
            },
            Preset::Balanced => Match {
                tiers: Tier::ALL.to_vec(),
                fuzzy_max_distance: 2,
                fuzzy_min_length: 5,
                empty_policy: EmptyPolicy::Own,
                min_locales_agree: 5,
                normalize: normalize_all,
            },
            Preset::Loose => Match {
                tiers: Tier::ALL.to_vec(),
                fuzzy_max_distance: 3,
                fuzzy_min_length: 4,
                empty_policy: EmptyPolicy::Own,
                min_locales_agree: 5,
                normalize: normalize_all,
            },
        }
    }
}

impl Default for Scan {
    fn default() -> Self {
        Self {
            po_patterns: vec!["**/*.po".to_string()],
            ignore_dirs: vec![
                "vendor".to_string(),
                "node_modules".to_string(),
                ".git".to_string(),
            ],
            roots: vec![PathBuf::from(".")],
        }
    }
}

impl Default for Match {
    /// The `balanced` preset — the single source of the default `[match]` values.
    fn default() -> Self {
        Preset::Balanced.baseline()
    }
}

/// The deserialization target for a whole config file. `match` parses into a
/// [`MatchOverride`] (not a resolved [`Match`]); the rest is flat. The resolved
/// [`Config`] is built from this — the only thing TOML parses into.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawConfig {
    scan: Scan,
    locales: Locales,
    #[serde(rename = "match")]
    match_: MatchOverride,
    output: Output,
}

/// A draft `[match]`: every knob optional, plus a `preset`. A `None` knob means
/// "fall back to the layer below"; a `Some` knob is an explicit override.
/// `deny_unknown_fields` rejects typos. The file parses into one; the CLI builds
/// one from flags. Resolved into a populated [`Match`] by [`resolve`].
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct MatchOverride {
    pub(crate) preset: Option<Preset>,
    pub(crate) tiers: Option<Vec<Tier>>,
    pub(crate) fuzzy_max_distance: Option<usize>,
    pub(crate) fuzzy_min_length: Option<usize>,
    pub(crate) empty_policy: Option<EmptyPolicy>,
    pub(crate) min_locales_agree: Option<usize>,
    pub(crate) normalize: Option<NormalizeOverride>,
}

impl MatchOverride {
    /// Overlay this override's explicit (`Some`) fields onto `base`; `None` keeps
    /// the base value. `preset` is not a field here — it selects the base in
    /// [`resolve`], it does not overlay.
    fn overlay(self, base: Match) -> Match {
        Match {
            tiers: self.tiers.unwrap_or(base.tiers),
            fuzzy_max_distance: self.fuzzy_max_distance.unwrap_or(base.fuzzy_max_distance),
            fuzzy_min_length: self.fuzzy_min_length.unwrap_or(base.fuzzy_min_length),
            empty_policy: self.empty_policy.unwrap_or(base.empty_policy),
            min_locales_agree: self.min_locales_agree.unwrap_or(base.min_locales_agree),
            normalize: merge_normalize(base.normalize, self.normalize),
        }
    }
}

/// Deserialization shadow of [`Normalize`]: each toggle optional so it merges
/// onto the preset baseline instead of resetting omitted toggles to `false`.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct NormalizeOverride {
    pub(crate) case_fold: Option<bool>,
    pub(crate) collapse_whitespace: Option<bool>,
    pub(crate) strip_trailing_punct: Option<bool>,
}

/// Resolve `[match]` from its layers into a populated [`Match`]. Precedence,
/// high→low: a CLI field, then a file field, then the preset baseline, then the
/// built-in default. The preset is chosen by the most specific layer that names
/// one (`cli ?? file ?? balanced`); explicit fields overlay base ← file ← cli.
fn resolve(file: MatchOverride, cli: MatchOverride) -> Match {
    let preset = cli.preset.or(file.preset).unwrap_or(Preset::Balanced);
    let base = preset.baseline();
    cli.overlay(file.overlay(base))
}

/// Layer explicit normalize toggles onto the baseline (omitted → baseline).
fn merge_normalize(base: Normalize, raw: Option<NormalizeOverride>) -> Normalize {
    match raw {
        None => base,
        Some(raw) => Normalize {
            case_fold: raw.case_fold.unwrap_or(base.case_fold),
            collapse_whitespace: raw.collapse_whitespace.unwrap_or(base.collapse_whitespace),
            strip_trailing_punct: raw
                .strip_trailing_punct
                .unwrap_or(base.strip_trailing_punct),
        },
    }
}

impl Default for Normalize {
    fn default() -> Self {
        Self {
            case_fold: true,
            collapse_whitespace: true,
            strip_trailing_punct: true,
        }
    }
}

impl Default for Output {
    fn default() -> Self {
        Self {
            format: Format::Text,
        }
    }
}

/// A configuration error surfaced at startup with a clear message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// `min_locales_agree` must be at least 1 (0 would group everything).
    ZeroMinLocalesAgree,
    /// `min_locales_agree` too low for the locale count → too many
    /// leave-one-out sub-signatures.
    SubsignatureBlowup {
        locales: usize,
        min_agree: usize,
        depth: usize,
        subsignatures: u128,
        cap: u128,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroMinLocalesAgree => {
                write!(f, "match.min_locales_agree must be >= 1")
            }
            Self::SubsignatureBlowup {
                locales,
                min_agree,
                cap,
                ..
            } => {
                let suggested = suggested_floor(*locales, *cap);
                write!(
                    f,
                    "match.min_locales_agree = {min_agree} is too low for {locales} locales — \
                     the near-duplicate search would blow up combinatorially. \
                     Raise match.min_locales_agree to at least {suggested}.",
                )
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// The floor to actually group at, reconciled against the active locale count
/// `M` (see [`Config::reconcile_floor`]). `effective` is what the pipeline uses;
/// `clamped` is `true` when the configured floor exceeded `M` and was lowered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FloorDecision {
    pub effective: usize,
    pub clamped: bool,
}

impl Config {
    /// Parse a `Config` from a TOML string. Missing fields use the defaults;
    /// `[match]` resolves from its preset baseline plus explicit file overrides.
    pub fn from_toml(input: &str) -> Result<Self, toml::de::Error> {
        Self::from_toml_with(input, MatchOverride::default())
    }

    /// Parse a `Config`, layering CLI `[match]` overrides on top of the file's.
    /// The single resolution point: `[match]` precedence (CLI > file > preset >
    /// default) is applied here, once.
    pub(crate) fn from_toml_with(input: &str, cli: MatchOverride) -> Result<Self, toml::de::Error> {
        let raw: RawConfig = toml::from_str(input)?;
        Ok(Config {
            scan: raw.scan,
            locales: raw.locales,
            match_: resolve(raw.match_, cli),
            output: raw.output,
        })
    }

    /// Reconcile `min_locales_agree` against the number of active locales `M`,
    /// folding both floor checks into one decision.
    ///
    /// `T = M - K` is the leave-one-out depth; the number of sub-signatures per
    /// key is `Σ_{t=0}^{T} C(M, t)`.
    /// - A `K` too *low* for `M` blows that up — a hard [`ConfigError`].
    /// - A `K` too *high* (above `M`) would drop every key and leave the report
    ///   silently empty — we clamp it down to `M` instead (`clamped = true`).
    ///
    /// The two never collide: a floor above `M` has depth `0`, so it cannot blow
    /// up. The blow-up check runs on the *configured* floor; the clamp is applied
    /// after.
    pub fn reconcile_floor(&self, locale_count: usize) -> Result<FloorDecision, ConfigError> {
        let min_agree = self.match_.min_locales_agree;
        if min_agree == 0 {
            return Err(ConfigError::ZeroMinLocalesAgree);
        }
        let depth = locale_count.saturating_sub(min_agree);
        let subsignatures = subsignature_count(locale_count, depth);
        if subsignatures > MAX_SUBSIGNATURES {
            return Err(ConfigError::SubsignatureBlowup {
                locales: locale_count,
                min_agree,
                depth,
                subsignatures,
                cap: MAX_SUBSIGNATURES,
            });
        }
        let effective = min_agree.min(locale_count);
        Ok(FloorDecision {
            effective,
            clamped: effective < min_agree,
        })
    }
}

/// The smallest `min_locales_agree` whose sub-signature count stays within `cap`
/// for `m` locales. Raising the floor lowers `T = m - floor`, so the count falls
/// monotonically — the first floor that fits is the minimum. Suggested in the
/// blow-up error so the user gets a concrete number, not just "raise it".
fn suggested_floor(m: usize, cap: u128) -> usize {
    (1..=m)
        .find(|&floor| subsignature_count(m, m - floor) <= cap)
        .unwrap_or(m)
}

/// `Σ_{t=0}^{T} C(m, t)`, saturating at `u128::MAX` so a huge count can't
/// overflow before the cap comparison rejects it.
fn subsignature_count(m: usize, t: usize) -> u128 {
    let mut total: u128 = 1; // C(m, 0)
    let mut current: u128 = 1;
    for i in 1..=t {
        // C(m, i) = C(m, i-1) * (m - i + 1) / i
        current = current.saturating_mul((m - i + 1) as u128) / (i as u128);
        total = total.saturating_add(current);
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mirrors the example pockingbird.toml in README.md.
    const EXAMPLE: &str = r#"
[scan]
po_patterns = ["**/*.po"]
ignore_dirs = ["vendor", "node_modules", ".git"]
roots = ["."]

[locales]
exclude = []

[match]
tiers = ["exact", "normalized", "fuzzy"]
fuzzy_max_distance = 2
fuzzy_min_length = 5
empty_policy = "own"
min_locales_agree = 5

[match.normalize]
case_fold = true
collapse_whitespace = true
strip_trailing_punct = true

[output]
format = "text"
"#;

    #[test]
    fn default_matches_expected_values() {
        let config = Config::default();
        assert_eq!(config.scan.po_patterns, vec!["**/*.po".to_string()]);
        assert_eq!(
            config.scan.ignore_dirs,
            vec![
                "vendor".to_string(),
                "node_modules".to_string(),
                ".git".to_string()
            ]
        );
        assert_eq!(config.scan.roots, vec![PathBuf::from(".")]);
        assert!(config.locales.exclude.is_empty());
        assert_eq!(
            config.match_.tiers,
            vec![Tier::Exact, Tier::Normalized, Tier::Fuzzy]
        );
        assert_eq!(config.match_.fuzzy_max_distance, 2);
        assert_eq!(config.match_.fuzzy_min_length, 5);
        assert_eq!(config.match_.empty_policy, EmptyPolicy::Own);
        assert_eq!(config.match_.min_locales_agree, 5);
        assert!(config.match_.normalize.case_fold);
        assert!(config.match_.normalize.collapse_whitespace);
        assert!(config.match_.normalize.strip_trailing_punct);
        assert_eq!(config.output.format, Format::Text);
    }

    #[test]
    fn example_config_parses_to_defaults() {
        let config = Config::from_toml(EXAMPLE).expect("example config parses");
        assert_eq!(config, Config::default());
    }

    #[test]
    fn empty_toml_uses_all_defaults() {
        assert_eq!(Config::from_toml("").unwrap(), Config::default());
    }

    #[test]
    fn unknown_field_is_rejected() {
        let result = Config::from_toml("[scan]\nbogus = 1\n");
        assert!(result.is_err());
    }

    #[test]
    fn reconcile_accepts_default_floor() {
        // M = 6, K = 5 -> T = 1 -> C(6,0)+C(6,1) = 7 <= cap; floor fits, no clamp.
        let decision = Config::default().reconcile_floor(6).unwrap();
        assert_eq!(decision.effective, 5);
        assert!(!decision.clamped);
    }

    #[test]
    fn reconcile_rejects_floor_too_low_for_locale_count() {
        // M = 20, K = 1 -> T = 19 -> ~2^20 sub-signatures >> cap.
        let mut config = Config::default();
        config.match_.min_locales_agree = 1;
        let error = config.reconcile_floor(20).unwrap_err();
        assert!(matches!(error, ConfigError::SubsignatureBlowup { .. }));
        // Message names the offending knob.
        assert!(error.to_string().contains("min_locales_agree"));
    }

    #[test]
    fn reconcile_rejects_zero_floor() {
        let mut config = Config::default();
        config.match_.min_locales_agree = 0;
        assert_eq!(
            config.reconcile_floor(6).unwrap_err(),
            ConfigError::ZeroMinLocalesAgree
        );
    }

    #[test]
    fn reconcile_clamps_floor_above_locale_count() {
        // Default floor 5 but only 2 locales: clamp to 2 instead of dropping every
        // key. A floor above M has depth 0, so it can't blow up first.
        let decision = Config::default().reconcile_floor(2).unwrap();
        assert_eq!(decision.effective, 2);
        assert!(decision.clamped);
    }

    // --- presets (2a) ---

    #[test]
    fn balanced_preset_equals_default() {
        // The default `[match]` IS the balanced baseline — no drift between them.
        assert_eq!(Preset::Balanced.baseline(), Match::default());
    }

    #[test]
    fn preset_selects_a_baseline() {
        let strict = Config::from_toml("[match]\npreset = \"strict\"\n").unwrap();
        assert_eq!(strict.match_.tiers, vec![Tier::Exact]);
        assert!(!strict.match_.normalize.case_fold);

        let loose = Config::from_toml("[match]\npreset = \"loose\"\n").unwrap();
        assert_eq!(loose.match_.tiers, Tier::ALL.to_vec());
        assert_eq!(loose.match_.fuzzy_max_distance, 3);
        assert_eq!(loose.match_.fuzzy_min_length, 4);
    }

    #[test]
    fn explicit_field_overrides_preset() {
        // strict baseline is exact-only; an explicit `tiers` wins, but the
        // untouched knobs (normalize toggles) keep the strict baseline.
        let config = Config::from_toml(
            "[match]\npreset = \"strict\"\ntiers = [\"exact\", \"normalized\"]\n",
        )
        .unwrap();
        assert_eq!(config.match_.tiers, vec![Tier::Exact, Tier::Normalized]);
        assert!(!config.match_.normalize.collapse_whitespace); // still strict
    }

    #[test]
    fn explicit_normalize_toggle_merges_onto_preset() {
        // balanced baseline has all toggles on; turning one off must not reset
        // the others to false (the merge, not wholesale replace).
        let config = Config::from_toml("[match.normalize]\ncase_fold = false\n").unwrap();
        assert!(!config.match_.normalize.case_fold);
        assert!(config.match_.normalize.collapse_whitespace);
        assert!(config.match_.normalize.strip_trailing_punct);
    }

    #[test]
    fn suggested_floor_is_within_cap() {
        // M = 20 with min 1 blows up; the suggestion must itself fit the cap.
        let suggested = suggested_floor(20, MAX_SUBSIGNATURES);
        assert!(subsignature_count(20, 20 - suggested) <= MAX_SUBSIGNATURES);
        // And it must be the *minimum* such floor (one lower would overflow).
        assert!(subsignature_count(20, 20 - (suggested - 1)) > MAX_SUBSIGNATURES);
    }
}
