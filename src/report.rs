/// Backend that supplied bytes for one search execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchBackend {
    /// Ignore-aware filesystem traversal and one-pass content delivery.
    Filesystem,
    /// An opened persistent content index.
    PersistentIndex,
    /// A resident index maintained from filesystem watcher events.
    LiveIndex,
}

/// Evidence for a persistent or live index query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexSearchEvidence {
    /// Deterministic revision of indexed roots, paths, and content hashes.
    pub revision: String,
    /// Selected files stored in the complete index.
    pub indexed_files: u64,
    /// Files retained after the conservative trigram prefilter.
    pub candidate_files: u64,
    /// Whether the prefilter could safely narrow this query.
    pub prefiltered: bool,
}

/// A half-open byte range in the decoded matching block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MatchSpan {
    /// Zero-based index in the flattened ordered query set.
    pub pattern_index: usize,
    /// Inclusive decoded byte offset.
    pub start: usize,
    /// Exclusive decoded byte offset.
    pub end: usize,
}

/// One rendered context line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextLine {
    /// One-based line number.
    pub line_number: u64,
    /// Decoded line text without its newline.
    pub text: String,
    /// Whether malformed input required replacement characters.
    pub lossy: bool,
}

/// One matching line or merged multiline block and its evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMatch {
    /// Root index reserved for multi-root consumers.
    pub root_index: usize,
    /// Normalized path, or `archive!member` virtual path.
    pub path: String,
    /// One-based line number.
    pub line_number: u64,
    /// One-based final line covered by the matching block.
    pub end_line_number: u64,
    /// Line start in decoded UTF-8 bytes.
    pub decoded_byte_offset: u64,
    /// Exact line start in source bytes when a lossless mapping is available.
    pub source_byte_offset: Option<u64>,
    /// Decoded matching block; multiline mode preserves embedded terminators.
    pub line: String,
    /// Non-mutating replacement preview of `line`, when requested and within
    /// the configured byte limit.
    pub replacement_preview: Option<String>,
    /// Non-overlapping decoded byte ranges matched in `line`.
    pub spans: Vec<MatchSpan>,
    /// Preceding context in source order.
    pub before: Vec<ContextLine>,
    /// Following context in source order.
    pub after: Vec<ContextLine>,
    /// Effective decoder name.
    pub encoding: String,
    /// Whether malformed input required replacement characters.
    pub lossy: bool,
    /// Whether `path` addresses a virtual archive member.
    pub archive: bool,
}

/// Category of a non-fatal search warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SearchWarningKind {
    /// Binary input was skipped.
    Binary,
    /// Input decoding was malformed or unsupported.
    Encoding,
    /// A logical line exceeded its configured bound.
    LineTooLong,
    /// An archive could not be inspected safely.
    Archive,
    /// A configured resource bound was reached.
    Limit,
}

/// A deterministic non-fatal search warning.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SearchWarning {
    /// Normalized source or virtual member path.
    pub path: String,
    /// Stable warning category.
    pub kind: SearchWarningKind,
    /// Human-readable detail.
    pub message: String,
}

/// Aggregate evidence for one matched source or archive member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedFile {
    /// Root index reserved for multi-root consumers.
    pub root_index: usize,
    /// Normalized source or virtual member path.
    pub path: String,
    /// Distinct physical lines covered by matches.
    pub matching_lines: u64,
    /// Non-overlapping match occurrences.
    pub occurrences: u64,
    /// Whether `path` addresses a virtual archive member.
    pub archive: bool,
}

/// Deterministic matches, aggregate counters, warnings, and scan evidence.
#[derive(Debug)]
pub struct SearchReport {
    /// Content backend used for this execution.
    pub backend: SearchBackend,
    /// Index evidence when `backend` is not [`SearchBackend::Filesystem`].
    pub index: Option<IndexSearchEvidence>,
    /// Search roots in insertion order.
    pub roots: Vec<std::path::PathBuf>,
    /// Output mode used to construct this report.
    pub result_mode: crate::ResultMode,
    /// Retained matches sorted by root, path, line, and first span.
    ///
    /// Empty in count, files, and quiet result modes.
    pub matches: Vec<SearchMatch>,
    /// Total matching lines, or the first observed match range in quiet mode.
    pub matching_lines: u64,
    /// Total occurrences, or one observed occurrence in quiet mode.
    pub occurrences: u64,
    /// Total matched sources, or one observed source in quiet mode.
    pub files_with_matches: u64,
    /// Files whose content pipeline completed.
    pub files_searched: u64,
    /// Source bytes emitted by the scanner.
    pub bytes_searched: u64,
    /// Whether match or matched-file records were omitted by `max_results`.
    pub truncated: bool,
    /// Retained warnings in deterministic order.
    pub warnings: Vec<SearchWarning>,
    /// Retained per-file summaries in deterministic order for count and files
    /// modes.
    pub matched_files: Vec<MatchedFile>,
    /// Number of warnings omitted by `max_warnings`.
    pub warnings_dropped: u64,
    /// Per-root scanner evidence and cancellation state in insertion order.
    pub scan: weavatrix_scan::MultiContentVisitReport,
}
