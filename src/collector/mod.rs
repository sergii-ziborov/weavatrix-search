mod operations;
mod ranking;

use crate::error::Error;
use crate::options::{FileEvidenceMode, ResultMode, SearchOptions};
use crate::report::{MatchedFile, SearchMatch, SearchWarning, SourceFileEvidence};
use ranking::{RankedEvidence, RankedFile, RankedMatch, RankedWarning};
use std::borrow::Cow;
use std::collections::BinaryHeap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};

pub(crate) struct FileSummary {
    pub(crate) root_index: usize,
    pub(crate) path: String,
    pub(crate) source_bytes: u64,
    pub(crate) total_lines: u64,
    pub(crate) matching_lines: u64,
    pub(crate) occurrences: u64,
    pub(crate) encoding: Cow<'static, str>,
    pub(crate) lossy: bool,
    pub(crate) archive: bool,
}

pub(crate) struct Collector {
    limit: usize,
    warning_limit: usize,
    file_evidence_mode: FileEvidenceMode,
    file_evidence_limit: usize,
    file_evidence_visitor: Mutex<Option<crate::FileEvidenceVisitor>>,
    file_evidence_visitor_enabled: AtomicBool,
    result_mode: ResultMode,
    matches: Mutex<BinaryHeap<RankedMatch>>,
    files: Mutex<BinaryHeap<RankedFile>>,
    file_evidence: Mutex<BinaryHeap<RankedEvidence>>,
    warnings: Mutex<BinaryHeap<RankedWarning>>,
    fatal: Mutex<Option<Error>>,
    matching_lines: AtomicU64,
    occurrences: AtomicU64,
    files_with_matches: AtomicU64,
    warnings_dropped: AtomicU64,
    truncated: AtomicBool,
    file_evidence_truncated: AtomicBool,
    failed: AtomicBool,
    quiet_found: AtomicBool,
}

pub(crate) struct Collected {
    pub(crate) matches: Vec<SearchMatch>,
    pub(crate) files: Vec<MatchedFile>,
    pub(crate) file_evidence: Vec<SourceFileEvidence>,
    pub(crate) warnings: Vec<SearchWarning>,
    pub(crate) matching_lines: u64,
    pub(crate) occurrences: u64,
    pub(crate) files_with_matches: u64,
    pub(crate) warnings_dropped: u64,
    pub(crate) truncated: bool,
    pub(crate) file_evidence_truncated: bool,
}
