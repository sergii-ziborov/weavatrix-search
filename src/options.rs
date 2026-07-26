/// Controls case handling for literal and regular-expression queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaseMode {
    /// Match case exactly.
    #[default]
    Sensitive,
    /// Match without case sensitivity.
    Insensitive,
    /// Ignore case unless the query contains an uppercase character.
    Smart,
}

/// Controls how file bytes are decoded before matching.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum EncodingMode {
    /// Detect UTF-8/UTF-16 BOMs and otherwise assume UTF-8.
    #[default]
    Auto,
    /// Decode as UTF-8.
    Utf8,
    /// Decode as little-endian UTF-16.
    Utf16Le,
    /// Decode as big-endian UTF-16.
    Utf16Be,
    /// Decode with an `encoding_rs` label such as `windows-1252`.
    Label(String),
}

/// Controls handling of inputs with a NUL byte near the beginning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BinaryPolicy {
    /// Skip the input and retain a typed warning.
    #[default]
    Skip,
    /// Search binary input as decoded text.
    Search,
}

/// Controls whether per-file search failures stop the whole operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchErrorPolicy {
    /// Retain a typed warning and continue with other files.
    #[default]
    Continue,
    /// Cancel remaining work and return the first failure.
    Abort,
}

/// Controls whether matching is isolated to logical lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchMode {
    /// Search each logical line independently with streaming content.
    #[default]
    Line,
    /// Permit matches across line boundaries under a whole-file byte limit.
    Multiline,
}

/// Controls retained output while preserving aggregate counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResultMode {
    /// Retain bounded line or multiline match evidence.
    #[default]
    Matches,
    /// Retain no match records and return aggregate counters only.
    Count,
    /// Retain bounded per-file summaries without line text.
    Files,
    /// Stop all workers after the first observed match.
    Quiet,
}

/// Resource and expansion limits for archive search.
#[derive(Debug, Clone)]
pub struct ArchiveOptions {
    /// Enables archive recognition and member search.
    pub enabled: bool,
    /// Maximum compressed/source bytes accepted for one archive.
    pub max_archive_bytes: u64,
    /// Maximum expanded bytes accepted for one member.
    pub max_entry_bytes: u64,
    /// Maximum cumulative expanded bytes accepted for one archive.
    pub max_expanded_bytes: u64,
    /// Maximum members visited in one archive.
    pub max_entries: usize,
    /// Maximum decoder dictionary/scratch memory when the format exposes a
    /// configurable bound.
    pub max_decoder_memory_bytes: usize,
}

impl Default for ArchiveOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            max_archive_bytes: 32 * 1024 * 1024,
            max_entry_bytes: 16 * 1024 * 1024,
            max_expanded_bytes: 128 * 1024 * 1024,
            max_entries: 10_000,
            max_decoder_memory_bytes: 64 * 1024 * 1024,
        }
    }
}

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
