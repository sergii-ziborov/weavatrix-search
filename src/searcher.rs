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
use std::path::PathBuf;
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

impl Searcher {
    /// Creates a searcher with bounded, ignore-aware defaults.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>, query: SearchQuery) -> Self {
        let options = SearchOptions::default();
        let mut scan_options = ScanOptions::default()
            .metadata_only()
            .selected_files_only()
            .with_skip_hidden(true)
            .with_content_parallelism(if cfg!(windows) { 8 } else { 16 })
            .with_content_discovery(ContentDiscoveryMode::BufferedParallel)
            .with_content_validation(ContentValidationPolicy::Fast);
        // Archive inputs must reach the bounded archive reader. Scanner's
        // repository-oriented default is intentionally smaller.
        scan_options.max_file_bytes = scanner_file_limit(&options);
        Self {
            root: root.into(),
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
        self
    }

    /// Adds independent roots in insertion order.
    #[must_use]
    pub fn extend_roots(mut self, roots: impl IntoIterator<Item = impl Into<PathBuf>>) -> Self {
        self.additional_roots
            .extend(roots.into_iter().map(Into::into));
        self
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
        let query = Arc::new(self.query.compile(self.options.case)?);
        let options = Arc::new(self.options);
        let collector = Arc::new(Collector::new(
            options.max_results,
            options.max_warnings,
            options.result_mode,
        ));
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
            warnings_dropped: collected.warnings_dropped,
            scan,
        })
    }
}

pub(crate) struct IndexedContent<'a> {
    pub(crate) root_index: usize,
    pub(crate) path: &'a str,
    pub(crate) bytes: &'a [u8],
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
pub(crate) fn search_indexed(
    roots: Vec<PathBuf>,
    files: &[IndexedContent<'_>],
    query: &SearchQuery,
    options: SearchOptions,
    parallelism: usize,
    revision: String,
    indexed_files: u64,
    candidate_files: u64,
    prefiltered: bool,
) -> Result<SearchReport> {
    is_streaming_utf8(&options.encoding)?;
    let query = Arc::new(query.compile(options.case)?);
    let options = Arc::new(options);
    let collector = Arc::new(Collector::new(
        options.max_results,
        options.max_warnings,
        options.result_mode,
    ));
    let next = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(
        (0..roots.len())
            .map(|_| AtomicU64::new(0))
            .collect::<Vec<_>>(),
    );
    let emitted = Arc::new(
        (0..roots.len())
            .map(|_| AtomicU64::new(0))
            .collect::<Vec<_>>(),
    );
    let discovered = files
        .iter()
        .fold(vec![0_u64; roots.len()], |mut counts, file| {
            if let Some(count) = counts.get_mut(file.root_index) {
                *count = count.saturating_add(1);
            }
            counts
        });
    let workers = parallelism.min(files.len().max(1));
    let panicked = std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(workers);
        for _ in 0..workers {
            let query = Arc::clone(&query);
            let options = Arc::clone(&options);
            let collector = Arc::clone(&collector);
            let next = Arc::clone(&next);
            let completed = Arc::clone(&completed);
            let emitted = Arc::clone(&emitted);
            handles.push(scope.spawn(move || {
                let mut query_cache = query.create_cache();
                loop {
                    if collector.should_quit() {
                        break;
                    }
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(file) = files.get(index) else {
                        break;
                    };
                    let expected_bytes = u64::try_from(file.bytes.len()).unwrap_or(u64::MAX);
                    let mut processor = FileProcessor::new(
                        SearchIdentity {
                            root_index: file.root_index,
                            path: file.path.to_owned(),
                            encoding: "UTF-8".into(),
                            archive: false,
                            source_offset_base: Some(0),
                            lossy: false,
                        },
                        expected_bytes,
                        Arc::clone(&query),
                        Arc::clone(&options),
                        Arc::clone(&collector),
                    );
                    if let Err(error) = processor
                        .push(file.bytes, &mut query_cache)
                        .and_then(|()| processor.finish(&mut query_cache))
                    {
                        handle_error(&collector, &options, error);
                    }
                    if let (Some(count), Some(bytes)) =
                        (completed.get(file.root_index), emitted.get(file.root_index))
                    {
                        count.fetch_add(1, Ordering::Relaxed);
                        bytes.fetch_add(expected_bytes, Ordering::Relaxed);
                    }
                }
            }));
        }
        let mut panicked = false;
        for handle in handles {
            panicked |= handle.join().is_err();
        }
        panicked
    });
    if panicked {
        return Err(Error::index(
            "<memory>",
            "an indexed-search worker panicked",
        ));
    }
    if let Some(error) = collector.take_fatal() {
        return Err(error);
    }
    let stopped = collector.should_quit();
    let collected = collector.finish();
    let reports = roots
        .iter()
        .enumerate()
        .map(|(root_index, root)| {
            let completed = completed[root_index].load(Ordering::Relaxed);
            let bytes = emitted[root_index].load(Ordering::Relaxed);
            ContentVisitReport {
                mode: ContentVisitMode::Streaming,
                root: root.clone(),
                discovered: discovered[root_index],
                completed,
                opened: completed,
                chunks: completed,
                bytes_read: bytes,
                bytes_emitted: bytes,
                consumer_skipped: 0,
                stopped,
                skipped: Vec::new(),
                warnings: Vec::new(),
                ignore_sources: Vec::new(),
                revision: revision.clone(),
                complete: !stopped,
                termination: stopped.then_some(ScanTermination::Cancelled),
                portable: false,
                cache: ScanCacheStats::default(),
            }
        })
        .collect::<Vec<_>>();
    let files_searched = reports.iter().map(|report| report.completed).sum();
    let bytes_searched = reports.iter().map(|report| report.bytes_emitted).sum();
    Ok(SearchReport {
        backend: SearchBackend::PersistentIndex,
        index: Some(IndexSearchEvidence {
            revision,
            indexed_files,
            candidate_files,
            prefiltered,
        }),
        roots,
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
        warnings_dropped: collected.warnings_dropped,
        scan: MultiContentVisitReport { reports },
    })
}

fn scanner_file_limit(options: &SearchOptions) -> u64 {
    if options.archives.enabled {
        options
            .max_file_bytes
            .max(options.archives.max_archive_bytes)
    } else {
        options.max_file_bytes
    }
}

// Keeping the streaming state inline avoids one heap allocation for every
// ordinary file, which matters more than stack size on 200k-file repositories.
#[allow(clippy::large_enum_variant)]
enum FileMode {
    Undecided(Vec<u8>),
    Streaming(LineSearcher),
    Buffered(Vec<u8>),
    Skipped,
}

struct FileProcessor {
    identity: Option<SearchIdentity>,
    expected_bytes: u64,
    archive: Option<ArchiveKind>,
    query: Arc<CompiledQuery>,
    options: Arc<SearchOptions>,
    collector: Arc<Collector>,
    mode: FileMode,
    inspected_binary_bytes: usize,
}

impl FileProcessor {
    fn new(
        identity: SearchIdentity,
        expected_bytes: u64,
        query: Arc<CompiledQuery>,
        options: Arc<SearchOptions>,
        collector: Arc<Collector>,
    ) -> Self {
        let mut identity = Some(identity);
        let archive = options
            .archives
            .enabled
            .then(|| archive::kind(&identity.as_ref().expect("identity exists").path))
            .flatten();
        let mode = if archive.is_some() {
            if expected_bytes > options.archives.max_archive_bytes {
                collector.warn(SearchWarning {
                    path: identity.as_ref().expect("identity exists").path.clone(),
                    kind: SearchWarningKind::Limit,
                    message: format!(
                        "archive is {expected_bytes} bytes; limit is {}",
                        options.archives.max_archive_bytes
                    ),
                });
                FileMode::Skipped
            } else {
                FileMode::Buffered(Vec::with_capacity(
                    usize::try_from(expected_bytes).unwrap_or(0),
                ))
            }
        } else if options.mode == SearchMode::Multiline {
            if expected_bytes > options.max_multiline_bytes {
                collector.warn(SearchWarning {
                    path: identity.as_ref().expect("identity exists").path.clone(),
                    kind: SearchWarningKind::Limit,
                    message: format!(
                        "multiline source is {expected_bytes} bytes; limit is {}",
                        options.max_multiline_bytes
                    ),
                });
                FileMode::Skipped
            } else {
                FileMode::Buffered(Vec::with_capacity(
                    usize::try_from(expected_bytes).unwrap_or(0),
                ))
            }
        } else {
            match is_streaming_utf8(&options.encoding) {
                Ok(true) if options.encoding == EncodingMode::Auto => {
                    FileMode::Undecided(Vec::new())
                }
                Ok(true) => FileMode::Streaming(LineSearcher::new(
                    Arc::clone(&query),
                    Arc::clone(&options),
                    Arc::clone(&collector),
                    identity.take().expect("identity exists"),
                )),
                Ok(false) => FileMode::Buffered(Vec::with_capacity(
                    usize::try_from(expected_bytes).unwrap_or(0),
                )),
                Err(error) => {
                    handle_error(&collector, &options, error);
                    FileMode::Skipped
                }
            }
        };
        Self {
            identity,
            expected_bytes,
            archive,
            query,
            options,
            collector,
            mode,
            inspected_binary_bytes: 0,
        }
    }

    fn push(&mut self, bytes: &[u8], query_cache: &mut QueryCache) -> Result<()> {
        let mode = std::mem::replace(&mut self.mode, FileMode::Skipped);
        self.mode = match mode {
            FileMode::Undecided(mut prefix) => {
                if prefix.is_empty() && bytes.len() >= 3 {
                    self.start_auto(bytes, query_cache)
                } else {
                    prefix.extend_from_slice(bytes);
                    if prefix.len() < 3
                        && u64::try_from(prefix.len()).unwrap_or(u64::MAX) < self.expected_bytes
                    {
                        FileMode::Undecided(prefix)
                    } else {
                        self.start_auto(&prefix, query_cache)
                    }
                }
            }
            FileMode::Streaming(mut lines) => {
                if binary_skip(
                    &self.options,
                    &self.collector,
                    &mut self.inspected_binary_bytes,
                    lines.path(),
                    bytes,
                ) {
                    FileMode::Skipped
                } else {
                    lines.push(bytes, query_cache);
                    FileMode::Streaming(lines)
                }
            }
            FileMode::Buffered(mut buffer) => {
                let limit = if self.archive.is_some() {
                    self.options.archives.max_archive_bytes
                } else if self.options.mode == SearchMode::Multiline {
                    self.options.max_multiline_bytes
                } else {
                    self.expected_bytes.max(1)
                };
                if u64::try_from(buffer.len().saturating_add(bytes.len())).unwrap_or(u64::MAX)
                    > limit
                {
                    let message = format!("buffered input exceeds the {limit} byte limit");
                    return Err(if self.archive.is_some() {
                        Error::archive(
                            &self.identity.as_ref().expect("identity exists").path,
                            message,
                        )
                    } else {
                        Error::limit(
                            &self.identity.as_ref().expect("identity exists").path,
                            message,
                        )
                    });
                }
                buffer.extend_from_slice(bytes);
                FileMode::Buffered(buffer)
            }
            FileMode::Skipped => FileMode::Skipped,
        };
        Ok(())
    }

    fn start_auto(&mut self, bytes: &[u8], query_cache: &mut QueryCache) -> FileMode {
        if auto_is_utf16(bytes) {
            FileMode::Buffered(bytes.to_vec())
        } else {
            let skip = utf8_bom_len(bytes);
            if binary_skip(
                &self.options,
                &self.collector,
                &mut self.inspected_binary_bytes,
                &self.identity.as_ref().expect("identity exists").path,
                &bytes[skip..],
            ) {
                return FileMode::Skipped;
            }
            let mut identity = self.identity.take().expect("identity exists");
            identity.source_offset_base =
                Some(u64::try_from(skip).expect("UTF-8 BOM length fits in u64"));
            let mut lines = LineSearcher::new(
                Arc::clone(&self.query),
                Arc::clone(&self.options),
                Arc::clone(&self.collector),
                identity,
            );
            lines.push(&bytes[skip..], query_cache);
            FileMode::Streaming(lines)
        }
    }

    fn finish(self, query_cache: &mut QueryCache) -> Result<()> {
        match self.mode {
            FileMode::Undecided(bytes) => search_complete_bytes(
                self.identity.expect("identity exists"),
                &bytes,
                self.query,
                query_cache,
                self.options,
                self.collector,
            ),
            FileMode::Streaming(lines) => {
                lines.finish(query_cache);
                Ok(())
            }
            FileMode::Buffered(bytes) => {
                if let Some(kind) = self.archive {
                    archive::search(
                        kind,
                        &self.identity.as_ref().expect("identity exists").path,
                        &bytes,
                        &self.query,
                        query_cache,
                        &self.options,
                        &self.collector,
                    )
                } else {
                    search_complete_bytes(
                        self.identity.expect("identity exists"),
                        &bytes,
                        self.query,
                        query_cache,
                        self.options,
                        self.collector,
                    )
                }
            }
            FileMode::Skipped => Ok(()),
        }
    }
}

fn binary_skip(
    options: &SearchOptions,
    collector: &Collector,
    inspected_binary_bytes: &mut usize,
    path: &str,
    bytes: &[u8],
) -> bool {
    if options.binary == BinaryPolicy::Search || *inspected_binary_bytes >= 8 * 1024 {
        return false;
    }
    let remaining = 8 * 1024 - *inspected_binary_bytes;
    let inspected = &bytes[..bytes.len().min(remaining)];
    *inspected_binary_bytes += inspected.len();
    if memchr::memchr(0, inspected).is_some() {
        collector.warn(SearchWarning {
            path: path.to_owned(),
            kind: SearchWarningKind::Binary,
            message: "binary file skipped after NUL-byte detection".to_owned(),
        });
        true
    } else {
        false
    }
}

fn handle_error(collector: &Collector, options: &SearchOptions, error: Error) {
    if options.error_policy == SearchErrorPolicy::Abort {
        collector.fail(error);
    } else {
        let (path, kind) = match &error {
            Error::Archive { path, .. } => (path.clone(), SearchWarningKind::Archive),
            Error::Limit { path, .. } => (path.clone(), SearchWarningKind::Limit),
            Error::InvalidEncoding(label) => (label.clone(), SearchWarningKind::Encoding),
            Error::Io { path, .. } => (
                path.to_string_lossy().into_owned(),
                SearchWarningKind::Archive,
            ),
            Error::EmptyQuery | Error::Regex(_) | Error::Scan(_) | Error::Index { .. } => {
                ("<search>".to_owned(), SearchWarningKind::Archive)
            }
        };
        collector.warn(SearchWarning {
            path,
            kind,
            message: error.to_string(),
        });
    }
}
