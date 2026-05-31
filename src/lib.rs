//! # pockingbird
//!
//! Report-only CLI that finds duplicate translation keys across gettext `.po`
//! catalogs.
//!
//! ## Pipeline
//!
//! ```text
//! walk → po → index → group → report
//! ```
//!
//! - [`walk`] — discover `.po` files under the configured roots.
//! - [`po`] — parse each catalog (polib) into keys and per-locale values;
//!   [`locale`] derives the locale id from the path.
//! - [`index`] — build the `KeyId × locale → Cell` matrix; [`normalize`] and
//!   [`fuzzy`] canonicalize cells per tier.
//! - [`group`] — tier-agnostic signature bucketing + leave-one-out over the
//!   canonical matrix → [`CandidateGroup`](group::CandidateGroup)s.
//! - [`report`] — render the groups as text (colored) or json.
//!
//! [`pipeline`] is the deep core wiring these stages into a single testable run;
//! [`config`] holds the TOML schema and defaults that parameterize every stage.

pub mod config;
pub mod fuzzy;
pub mod group;
pub mod index;
pub mod locale;
pub mod normalize;
pub mod pipeline;
pub mod po;
pub mod report;
pub mod walk;

#[cfg(feature = "cli")]
pub mod cli;
