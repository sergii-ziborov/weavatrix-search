#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![doc = include_str!("../README.md")]

mod archive;
mod collector;
mod encoding;
mod error;
mod line_search;
mod multiline;
mod options;
mod output;
mod query;
mod report;
mod searcher;

pub use error::{Error, Result};
pub use options::{
    ArchiveOptions, BinaryPolicy, CaseMode, EncodingMode, ResultMode, SearchErrorPolicy,
    SearchMode, SearchOptions,
};
pub use output::{
    ColorChoice, OutputFormat, OutputOptions, write_report, write_report_with, write_warnings,
};
pub use query::SearchQuery;
pub use report::{
    ContextLine, MatchSpan, MatchedFile, SearchMatch, SearchReport, SearchWarning,
    SearchWarningKind,
};
pub use searcher::Searcher;
pub use weavatrix_scan::{CancellationToken, ScanOptions};

/// Searches one repository with default scanner and search policies.
///
/// # Errors
///
/// Returns query-compilation, scanner, decoding, or archive errors according
/// to [`SearchOptions::error_policy`].
pub fn search(root: impl Into<std::path::PathBuf>, query: SearchQuery) -> Result<SearchReport> {
    Searcher::new(root, query).search()
}

/// Searches multiple repository roots in insertion order with default
/// scanner and search policies.
///
/// # Errors
///
/// Returns query-compilation, scanner, decoding, or archive errors according
/// to [`SearchOptions::error_policy`].
pub fn search_many(
    root: impl Into<std::path::PathBuf>,
    additional_roots: impl IntoIterator<Item = impl Into<std::path::PathBuf>>,
    query: SearchQuery,
) -> Result<SearchReport> {
    Searcher::new(root, query)
        .extend_roots(additional_roots)
        .search()
}
