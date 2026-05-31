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
/// silent combinatorial blow-up. See [`Config::validate`].
pub const MAX_SUBSIGNATURES: u128 = 512;

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub scan: Scan,
    pub locales: Locales,
    #[serde(rename = "match")]
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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
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
#[serde(rename_all = "lowercase")]
pub enum Format {
    Text,
    Json,
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
    fn default() -> Self {
        Self {
            tiers: Tier::ALL.to_vec(),
            fuzzy_max_distance: 2,
            fuzzy_min_length: 5,
            empty_policy: EmptyPolicy::Own,
            min_locales_agree: 5,
            normalize: Normalize::default(),
        }
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
                depth,
                subsignatures,
                cap,
            } => write!(
                f,
                "match.min_locales_agree = {min_agree} is too low for {locales} locales: \
                 leave-one-out depth T = {depth} yields {subsignatures} sub-signatures per key \
                 (cap {cap}). Raise min_locales_agree to reduce T = M - K.",
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Parse a `Config` from a TOML string. Missing fields use the defaults.
    pub fn from_toml(input: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(input)
    }

    /// Validate `min_locales_agree` against the number of active locales `M`.
    ///
    /// `T = M - K` is the leave-one-out depth; the number of sub-signatures per
    /// key is `Σ_{t=0}^{T} C(M, t)`. A `K` too low for `M` blows this up — we
    /// reject it with a clear error instead of letting grouping explode.
    pub fn validate(&self, locale_count: usize) -> Result<(), ConfigError> {
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
        Ok(())
    }
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
    fn validate_accepts_default_floor() {
        // M = 6, K = 5 -> T = 1 -> C(6,0)+C(6,1) = 7 <= cap.
        assert!(Config::default().validate(6).is_ok());
    }

    #[test]
    fn validate_rejects_floor_too_low_for_locale_count() {
        // M = 20, K = 1 -> T = 19 -> ~2^20 sub-signatures >> cap.
        let mut config = Config::default();
        config.match_.min_locales_agree = 1;
        let error = config.validate(20).unwrap_err();
        assert!(matches!(error, ConfigError::SubsignatureBlowup { .. }));
        // Message names the offending knob.
        assert!(error.to_string().contains("min_locales_agree"));
    }

    #[test]
    fn validate_rejects_zero_floor() {
        let mut config = Config::default();
        config.match_.min_locales_agree = 0;
        assert_eq!(
            config.validate(6).unwrap_err(),
            ConfigError::ZeroMinLocalesAgree
        );
    }
}
