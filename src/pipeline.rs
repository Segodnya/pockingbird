//! The pipeline run: the deep core behind the CLI shell. Given `.po` paths, a
//! parse adapter, config, and a progress sink, it parses, builds the Matrix,
//! validates, gates eligibility, groups every tier, and returns a [`Report`].
//!
//! This lives outside the `cli` feature so it is the test surface — the whole
//! pipeline can be exercised without a process, the filesystem, or `colored`.
//! Two seams keep it testable:
//!
//! - **Parse adapter** — the injected `Fn(&Path) -> Result<ParsedCatalog,
//!   PoError>`. Real runs pass [`crate::po::parse_po`]; tests pass an in-memory
//!   map, which is what makes the *skip-policy* (a malformed catalog is recorded
//!   and the run continues, never fatal) assertable.
//! - **Progress sink** — a one-method seam ([`ProgressSink`]); the run pushes
//!   [`PipelineEvent`]s to it. The CLI's stderr adapter renders and times; a test
//!   adapter records. Timing and IO stay out of the core.

use std::path::{Path, PathBuf};

use crate::config::{Config, ConfigError, Tier};
use crate::group::{self, CandidateGroup};
use crate::index::{build_matrix, CatalogInput};
use crate::locale;
use crate::po::{ParsedCatalog, PoError};

/// The run's data result. No timings, no formatting — rendering is the shell's
/// job.
#[derive(Debug)]
pub struct Report {
    pub groups: Vec<CandidateGroup>,
    /// Keys examined (the summary denominator): the matrix row count.
    pub total_keys: usize,
    pub skipped: Vec<Skip>,
}

/// A catalog that failed to parse. Recorded, never fatal.
#[derive(Debug)]
pub struct Skip {
    pub path: PathBuf,
    pub error: PoError,
}

/// A run that could not produce a report at all (distinct from a skipped file).
#[derive(Debug)]
pub enum PipelineError {
    /// Every path failed to parse — nothing to group.
    NoCatalogs,
    /// `min_locales_agree` is invalid for the locale count (see [`Config::validate`]).
    Config(ConfigError),
}

impl std::fmt::Display for PipelineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoCatalogs => write!(f, "no catalogs parsed"),
            Self::Config(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for PipelineError {}

/// A progress event emitted during a run. The vocabulary is here; the rendering
/// is the sink's.
pub enum PipelineEvent<'a> {
    Parsing { done: usize, total: usize },
    Skipped { path: &'a Path, error: &'a PoError },
    Matrix { locales: usize, keys: usize },
    TierStarted(Tier),
    TierDone { tier: Tier, groups: usize },
}

/// The one-method progress seam. Implement it to render, time, or record.
pub trait ProgressSink {
    fn emit(&mut self, event: PipelineEvent<'_>);
}

/// Silent sink — for runs (and tests) that don't care about progress.
impl ProgressSink for () {
    fn emit(&mut self, _event: PipelineEvent<'_>) {}
}

/// Run the pipeline over `paths`, parsing each via `parse`. Returns the report,
/// or a [`PipelineError`] if nothing parsed or the config is invalid for the
/// locale count. Catalogs that fail to parse are recorded in [`Report::skipped`]
/// and the run continues.
pub fn run<P, S>(
    paths: &[PathBuf],
    parse: P,
    config: &Config,
    sink: &mut S,
) -> Result<Report, PipelineError>
where
    P: Fn(&Path) -> Result<ParsedCatalog, PoError>,
    S: ProgressSink + ?Sized,
{
    let total = paths.len();
    let mut inputs = Vec::new();
    let mut skipped = Vec::new();
    for (index, path) in paths.iter().enumerate() {
        match parse(path) {
            Ok(catalog) => inputs.push(CatalogInput {
                locale: locale::locale_id(path),
                catalog,
            }),
            Err(error) => {
                sink.emit(PipelineEvent::Skipped {
                    path,
                    error: &error,
                });
                skipped.push(Skip {
                    path: path.clone(),
                    error,
                });
            }
        }
        sink.emit(PipelineEvent::Parsing {
            done: index + 1,
            total,
        });
    }
    if inputs.is_empty() {
        return Err(PipelineError::NoCatalogs);
    }

    let mut matrix = build_matrix(&inputs, &config.locales.exclude);
    let total_keys = matrix.rows.len();
    sink.emit(PipelineEvent::Matrix {
        locales: matrix.locales.len(),
        keys: total_keys,
    });

    config
        .validate(matrix.locales.len())
        .map_err(PipelineError::Config)?;
    matrix.retain_eligible(config.match_.min_locales_agree);

    let mut groups = Vec::new();
    for &tier in &config.match_.tiers {
        sink.emit(PipelineEvent::TierStarted(tier));
        let tier_groups = group::group_tier(&matrix, tier, &config.match_);
        sink.emit(PipelineEvent::TierDone {
            tier,
            groups: tier_groups.len(),
        });
        groups.extend(tier_groups);
    }

    Ok(Report {
        groups,
        total_keys,
        skipped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::po::PoEntry;

    fn entry(msgid: &str, value: &str) -> PoEntry {
        PoEntry {
            msgctxt: None,
            msgid: msgid.to_string(),
            msgid_plural: None,
            value: value.to_string(),
        }
    }

    fn catalog(domain: &str, entries: Vec<PoEntry>) -> ParsedCatalog {
        ParsedCatalog {
            domain: domain.to_string(),
            entries,
        }
    }

    /// A path under the gettext layout so `locale_id` yields `<locale>`.
    fn po_path(locale: &str) -> PathBuf {
        PathBuf::from(format!("{locale}/LC_MESSAGES/messages.po"))
    }

    /// Config with a floor low enough for a two-locale fixture.
    fn config(min_agree: usize) -> Config {
        let mut config = Config::default();
        config.match_.min_locales_agree = min_agree;
        config
    }

    /// An in-memory parse adapter: known paths parse, unknown paths fail.
    fn parser(
        catalogs: HashMap<PathBuf, ParsedCatalog>,
    ) -> impl Fn(&Path) -> Result<ParsedCatalog, PoError> {
        move |path: &Path| {
            catalogs
                .get(path)
                .cloned()
                .ok_or_else(|| PoError::NoDomain(path.to_path_buf()))
        }
    }

    #[test]
    fn groups_duplicates_across_locales() {
        // a and b share their translation in every locale → one exact group.
        let en = po_path("en");
        let ru = po_path("ru");
        let catalogs = HashMap::from([
            (
                en.clone(),
                catalog("messages", vec![entry("a", "Save"), entry("b", "Save")]),
            ),
            (
                ru.clone(),
                catalog(
                    "messages",
                    vec![entry("a", "Сохранить"), entry("b", "Сохранить")],
                ),
            ),
        ]);

        let report = run(&[en, ru], parser(catalogs), &config(2), &mut ()).expect("runs");

        assert!(report.skipped.is_empty());
        assert_eq!(report.total_keys, 2);
        assert!(
            report.groups.iter().any(|g| g.agree_locales == 2),
            "expected a full-agreement group"
        );
    }

    #[test]
    fn malformed_catalog_is_skipped_not_fatal() {
        let en = po_path("en");
        let ru = po_path("ru");
        let broken = PathBuf::from("broken/LC_MESSAGES/messages.po");
        let catalogs = HashMap::from([
            (
                en.clone(),
                catalog("messages", vec![entry("a", "Save"), entry("b", "Save")]),
            ),
            (
                ru.clone(),
                catalog("messages", vec![entry("a", "Save"), entry("b", "Save")]),
            ),
        ]);

        // `broken` isn't in the map → its parse fails, but the run continues.
        let report = run(
            &[en, broken.clone(), ru],
            parser(catalogs),
            &config(2),
            &mut (),
        )
        .expect("runs despite one bad file");

        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].path, broken);
        assert!(!report.groups.is_empty(), "good catalogs still grouped");
    }

    #[test]
    fn all_unparseable_yields_no_catalogs() {
        let paths = [po_path("en"), po_path("ru")];
        let result = run(&paths, parser(HashMap::new()), &config(2), &mut ());
        assert!(matches!(result, Err(PipelineError::NoCatalogs)));
    }

    /// Records the event stream for ordering/cardinality assertions.
    #[derive(Default)]
    struct Recorder {
        parsing: usize,
        matrix: usize,
        tiers_done: usize,
    }

    impl ProgressSink for Recorder {
        fn emit(&mut self, event: PipelineEvent<'_>) {
            match event {
                PipelineEvent::Parsing { .. } => self.parsing += 1,
                PipelineEvent::Matrix { .. } => self.matrix += 1,
                PipelineEvent::TierDone { .. } => self.tiers_done += 1,
                _ => {}
            }
        }
    }

    #[test]
    fn run_drives_the_progress_sink() {
        let en = po_path("en");
        let ru = po_path("ru");
        let catalogs = HashMap::from([
            (en.clone(), catalog("messages", vec![entry("a", "Save")])),
            (ru.clone(), catalog("messages", vec![entry("a", "Save")])),
        ]);
        let mut recorder = Recorder::default();
        let cfg = config(2);

        run(&[en, ru], parser(catalogs), &cfg, &mut recorder).expect("runs");

        assert_eq!(recorder.parsing, 2, "one Parsing event per path");
        assert_eq!(recorder.matrix, 1);
        assert_eq!(recorder.tiers_done, cfg.match_.tiers.len());
    }
}
