mod indexed;
mod processor;

pub(crate) use indexed::{IndexedContent, search_indexed};
use processor::{FileProcessor, handle_error, scanner_file_limit};

use crate::archive::{self, ArchiveKind};
use crate::collector::Collector;
use crate::encoding::{auto_is_utf16, is_streaming_utf8, search_complete_bytes, utf8_bom_len};
use crate::error::{Error, Result};
use crate::line_search::{LineSearcher, SearchIdentity};
use crate::options::{BinaryPolicy, EncodingMode, SearchErrorPolicy, SearchMode, SearchOptions};
use crate::query::{CompiledQuery, QueryCache, SearchQuery};
use crate::report::{
    IndexSearchEvidence, SearchBackend, SearchReport, SearchWarning, SearchWarningKind,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use weavatrix_scan::{
    ContentDiscoveryMode, ContentValidationPolicy, ContentVisitControl, ContentVisitEvent,
    ContentVisitMode, ContentVisitReport, MultiContentVisitReport, MultiScanner, ScanCacheStats,
    ScanOptions, ScanTermination,
};

/// Configures and executes a search across one or more repository roots.
pub struct Searcher {
    root: PathBuf,
    additional_roots: Vec<PathBuf>,
    query: SearchQuery,
    options: SearchOptions,
    scan_options: ScanOptions,
    scan_options_custom: bool,
}

/// Builds the adaptive, ignore-aware scanner profile used by [`Searcher`].
///
/// Repository roots keep the low-latency parallel buffered traversal. Broad
/// Windows roots and filesystem roots use overlapped constant-memory discovery
/// and a deeper content queue, avoiding retention of millions of paths before
/// matching starts. Other Unix directories retain the faster buffered
/// traversal unless the caller explicitly requests streaming.
#[must_use]
pub fn recommended_scan_options(roots: &[PathBuf], options: &SearchOptions) -> ScanOptions {
    let repository_roots = !roots.is_empty() && roots.iter().all(|root| has_git_marker(root));
    let includes_filesystem_root = roots.iter().any(|root| root.parent().is_none());
    let discovery = if !repository_roots && (cfg!(windows) || includes_filesystem_root) {
        ContentDiscoveryMode::Streaming
    } else {
        ContentDiscoveryMode::BufferedParallel
    };
    let content_parallelism = if discovery == ContentDiscoveryMode::Streaming {
        if cfg!(windows) { 32 } else { 16 }
    } else if cfg!(windows) {
        8
    } else {
        16
    };
    let mut scan_options = ScanOptions::default()
        .metadata_only()
        .selected_files_only()
        .with_skip_hidden(true)
        .with_content_parallelism(content_parallelism)
        .with_content_discovery(discovery)
        .with_content_validation(ContentValidationPolicy::Fast);
    scan_options.max_file_bytes = scanner_file_limit(options);
    scan_options
}

fn has_git_marker(root: &Path) -> bool {
    root.join(".git").try_exists().unwrap_or(false)
}

impl Searcher {
    /// Creates a searcher with bounded, ignore-aware defaults.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>, query: SearchQuery) -> Self {
        let root = root.into();
        let options = SearchOptions::default();
        let scan_options = recommended_scan_options(std::slice::from_ref(&root), &options);
        Self {
            root,
            additional_roots: Vec::new(),
            query,
            options,
            scan_options,
            scan_options_custom: false,
        }
    }

    /// Replaces search and content-processing policy.
    #[must_use]
    pub fn options(mut self, options: SearchOptions) -> Self {
        if !self.scan_options_custom {
            self.scan_options.max_file_bytes = scanner_file_limit(&options);
        }
        self.options = options;
        self
    }

    /// Adds another independent root to the same bounded parallel search.
    ///
    /// Match evidence retains each root's insertion-order index. Text output
    /// prefixes relative paths with their root when more than one root is
    /// searched.
    #[must_use]
    pub fn add_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.additional_roots.push(root.into());
        if !self.scan_options_custom {
            let roots = std::iter::once(&self.root)
                .chain(self.additional_roots.iter())
                .cloned()
                .collect::<Vec<_>>();
            self.scan_options = recommended_scan_options(&roots, &self.options);
        }
        self
    }

    /// Adds independent roots in insertion order.
    #[must_use]
    pub fn extend_roots(self, roots: impl IntoIterator<Item = impl Into<PathBuf>>) -> Self {
        roots.into_iter().fold(self, Self::add_root)
    }

    /// Replaces repository discovery and content-delivery policy.
    ///
    /// A custom scanner file-size limit is preserved rather than inferred
    /// from [`SearchOptions`].
    #[must_use]
    pub fn scan_options(mut self, scan_options: ScanOptions) -> Self {
        self.scan_options = scan_options;
        self.scan_options_custom = true;
        self
    }

    /// Executes the query through the scanner's bounded parallel content
    /// pipeline.
    ///
    /// # Errors
    ///
    /// Returns query, scanner, decoding, or archive errors according to the
    /// configured policy.
    pub fn search(self) -> Result<SearchReport> {
        is_streaming_utf8(&self.options.encoding)?;
        let mut options = self.options;
        let query = Arc::new(self.query.compile(options.case)?);
        let collector = Arc::new(Collector::new(&options));
        options.file_evidence_visitor = None;
        let options = Arc::new(options);
        let worker_query = Arc::clone(&query);
        let worker_options = Arc::clone(&options);
        let worker_collector = Arc::clone(&collector);
        let mut report_roots = Vec::with_capacity(self.additional_roots.len().saturating_add(1));
        report_roots.push(self.root.clone());
        report_roots.extend(self.additional_roots.iter().cloned());
        let scanner = self.additional_roots.into_iter().fold(
            MultiScanner::new(self.root).options(self.scan_options),
            MultiScanner::add_root,
        );

        let scan = scanner.visit_content_streaming(move |_, _| {
            let query = Arc::clone(&worker_query);
            let options = Arc::clone(&worker_options);
            let collector = Arc::clone(&worker_collector);
            let mut query_cache = query.create_cache();
            let mut file = None;
            move |event| {
                if collector.should_quit() {
                    return ContentVisitControl::Quit;
                }
                match event {
                    ContentVisitEvent::FileStart { file: opened, .. } => {
                        file = Some(FileProcessor::new(
                            SearchIdentity {
                                root_index: opened.root_index,
                                path: opened.relative.to_owned(),
                                source_bytes: opened.bytes,
                                encoding: "UTF-8".into(),
                                archive: false,
                                source_offset_base: Some(0),
                                lossy: false,
                            },
                            opened.bytes,
                            Arc::clone(&query),
                            Arc::clone(&options),
                            Arc::clone(&collector),
                        ));
                    }
                    ContentVisitEvent::Chunk { bytes, .. } => {
                        if let Some(processor) = &mut file
                            && let Err(error) = processor.push(bytes, &mut query_cache)
                        {
                            handle_error(&collector, &options, error);
                        }
                    }
                    ContentVisitEvent::FileEnd { .. } => {
                        if let Some(processor) = file.take()
                            && let Err(error) = processor.finish(&mut query_cache)
                        {
                            handle_error(&collector, &options, error);
                        }
                    }
                }
                if collector.should_quit() {
                    ContentVisitControl::Quit
                } else {
                    ContentVisitControl::Continue
                }
            }
        })?;

        collector.clear_file_evidence_visitor();
        if let Some(error) = collector.take_fatal() {
            return Err(error);
        }
        let collected = collector.finish();
        let files_searched = scan.reports.iter().map(|report| report.completed).sum();
        let bytes_searched = scan.reports.iter().map(|report| report.bytes_emitted).sum();
        Ok(SearchReport {
            backend: crate::report::SearchBackend::Filesystem,
            index: None,
            roots: report_roots,
            result_mode: options.result_mode,
            matches: collected.matches,
            matching_lines: collected.matching_lines,
            occurrences: collected.occurrences,
            files_with_matches: collected.files_with_matches,
            files_searched,
            bytes_searched,
            truncated: collected.truncated,
            warnings: collected.warnings,
            matched_files: collected.files,
            file_evidence: collected.file_evidence,
            file_evidence_truncated: collected.file_evidence_truncated,
            warnings_dropped: collected.warnings_dropped,
            scan,
        })
    }
}
