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

/// Controls deterministic retention of completed per-source text metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FileEvidenceMode {
    /// Do not retain per-source metrics in the final report.
    #[default]
    None,
    /// Retain metrics only for sources containing at least one match.
    Matched,
    /// Retain metrics for every source whose text search completed.
    All,
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
