mod types;

pub use types::{
    ArchiveOptions, BinaryPolicy, CaseMode, EncodingMode, FileEvidenceMode, ResultMode,
    SearchErrorPolicy, SearchMode,
};

use crate::report::FileEvidenceVisitor;

/// Repository search policy and resource bounds.
#[derive(Debug, Clone)]
pub struct SearchOptions {
    /// Query case policy.
    pub case: CaseMode,
    /// Number of preceding lines retained per match.
    pub before_context: usize,
    /// Number of following lines retained per match.
    pub after_context: usize,
    /// Maximum deterministic match records retained in memory.
    pub max_results: usize,
    /// Maximum deterministic warnings retained in memory.
    pub max_warnings: usize,
    /// Controls retained per-source text metrics.
    pub file_evidence: FileEvidenceMode,
    /// Maximum deterministic per-source metric records retained in memory.
    pub max_file_evidence: usize,
    /// Optional zero-retention callback invoked for every completed source.
    pub file_evidence_visitor: Option<FileEvidenceVisitor>,
    /// Maximum bytes accepted for one ordinary source file.
    pub max_file_bytes: u64,
    /// Maximum bytes retained for one logical line.
    pub max_line_bytes: usize,
    /// Maximum source bytes buffered in multiline mode.
    pub max_multiline_bytes: u64,
    /// Optional non-mutating replacement template used to render previews.
    ///
    /// `$0`, numbered groups, named groups, and `$$` follow
    /// `regex-automata` replacement syntax.
    pub replacement: Option<String>,
    /// Maximum UTF-8 bytes retained for one replacement preview.
    pub max_replacement_bytes: usize,
    /// Line-isolated or multiline matching.
    pub mode: SearchMode,
    /// Match evidence, aggregate count, file summary, or early-exit output.
    pub result_mode: ResultMode,
    /// Input decoding policy.
    pub encoding: EncodingMode,
    /// Binary-input policy.
    pub binary: BinaryPolicy,
    /// Archive recognition and expansion policy.
    pub archives: ArchiveOptions,
    /// Per-file failure policy.
    pub error_policy: SearchErrorPolicy,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            case: CaseMode::Sensitive,
            before_context: 0,
            after_context: 0,
            max_results: 10_000,
            max_warnings: 1_000,
            file_evidence: FileEvidenceMode::None,
            max_file_evidence: 100_000,
            file_evidence_visitor: None,
            max_file_bytes: 32 * 1024 * 1024,
            max_line_bytes: 8 * 1024 * 1024,
            max_multiline_bytes: 8 * 1024 * 1024,
            replacement: None,
            max_replacement_bytes: 8 * 1024 * 1024,
            mode: SearchMode::Line,
            result_mode: ResultMode::Matches,
            encoding: EncodingMode::Auto,
            binary: BinaryPolicy::Skip,
            archives: ArchiveOptions::default(),
            error_policy: SearchErrorPolicy::Continue,
        }
    }
}

impl SearchOptions {
    /// Sets query case handling.
    #[must_use]
    pub const fn with_case(mut self, case: CaseMode) -> Self {
        self.case = case;
        self
    }

    /// Sets the number of lines retained before and after every match.
    #[must_use]
    pub const fn with_context(mut self, before: usize, after: usize) -> Self {
        self.before_context = before;
        self.after_context = after;
        self
    }

    /// Sets the deterministic retained-result limit.
    #[must_use]
    pub const fn with_max_results(mut self, max_results: usize) -> Self {
        self.max_results = max_results;
        self
    }

    /// Sets the deterministic retained-warning limit.
    #[must_use]
    pub const fn with_max_warnings(mut self, max_warnings: usize) -> Self {
        self.max_warnings = max_warnings;
        self
    }

    /// Sets deterministic per-source metric retention.
    #[must_use]
    pub const fn with_file_evidence(mut self, file_evidence: FileEvidenceMode) -> Self {
        self.file_evidence = file_evidence;
        self
    }

    /// Sets the deterministic retained per-source metric limit.
    #[must_use]
    pub const fn with_max_file_evidence(mut self, max_file_evidence: usize) -> Self {
        self.max_file_evidence = max_file_evidence;
        self
    }

    /// Streams completed per-source metrics without retaining them in memory.
    ///
    /// The callback can run concurrently on content workers and must provide
    /// any synchronization required by its state.
    #[must_use]
    pub fn with_file_evidence_visitor<F>(mut self, visitor: F) -> Self
    where
        F: Fn(&crate::SourceFileEvidence) + Send + Sync + 'static,
    {
        self.file_evidence_visitor = Some(FileEvidenceVisitor::new(visitor));
        self
    }

    /// Sets the ordinary source-file size limit.
    #[must_use]
    pub const fn with_max_file_bytes(mut self, max_file_bytes: u64) -> Self {
        self.max_file_bytes = max_file_bytes;
        self
    }

    /// Sets the per-line byte limit.
    #[must_use]
    pub const fn with_max_line_bytes(mut self, max_line_bytes: usize) -> Self {
        self.max_line_bytes = max_line_bytes;
        self
    }

    /// Sets the multiline whole-file buffer limit.
    #[must_use]
    pub const fn with_max_multiline_bytes(mut self, max_multiline_bytes: u64) -> Self {
        self.max_multiline_bytes = max_multiline_bytes;
        self
    }

    /// Enables a non-mutating replacement preview for every retained match.
    #[must_use]
    pub fn with_replacement(mut self, replacement: impl Into<String>) -> Self {
        self.replacement = Some(replacement.into());
        self
    }

    /// Sets the byte limit for one rendered replacement preview.
    #[must_use]
    pub const fn with_max_replacement_bytes(mut self, max_replacement_bytes: usize) -> Self {
        self.max_replacement_bytes = max_replacement_bytes;
        self
    }

    /// Sets line-isolated or multiline matching.
    #[must_use]
    pub const fn with_mode(mut self, mode: SearchMode) -> Self {
        self.mode = mode;
        self
    }

    /// Sets retained output and early-exit behavior.
    #[must_use]
    pub const fn with_result_mode(mut self, result_mode: ResultMode) -> Self {
        self.result_mode = result_mode;
        self
    }

    /// Sets the decoding policy.
    #[must_use]
    pub fn with_encoding(mut self, encoding: EncodingMode) -> Self {
        self.encoding = encoding;
        self
    }

    /// Sets binary-input handling.
    #[must_use]
    pub const fn with_binary_policy(mut self, binary: BinaryPolicy) -> Self {
        self.binary = binary;
        self
    }

    /// Replaces archive recognition and expansion policy.
    #[must_use]
    pub fn with_archives(mut self, archives: ArchiveOptions) -> Self {
        self.archives = archives;
        self
    }

    /// Sets per-file error handling.
    #[must_use]
    pub const fn with_error_policy(mut self, error_policy: SearchErrorPolicy) -> Self {
        self.error_policy = error_policy;
        self
    }
}
