use crate::error::Error;
use crate::options::{FileEvidenceMode, ResultMode, SearchOptions};
use crate::report::{MatchedFile, SearchMatch, SearchWarning, SourceFileEvidence};
use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};

struct RankedMatch(SearchMatch);

struct RankedWarning(SearchWarning);

struct RankedFile(MatchedFile);

struct RankedEvidence(SourceFileEvidence);

impl RankedMatch {
    fn compare(left: &SearchMatch, right: &SearchMatch) -> Ordering {
        left.root_index
            .cmp(&right.root_index)
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.line_number.cmp(&right.line_number))
            .then_with(|| {
                left.spans
                    .first()
                    .map_or(0, |span| span.start)
                    .cmp(&right.spans.first().map_or(0, |span| span.start))
            })
    }
}

impl PartialEq for RankedMatch {
    fn eq(&self, other: &Self) -> bool {
        Self::compare(&self.0, &other.0) == Ordering::Equal
    }
}

impl Eq for RankedMatch {}

impl PartialOrd for RankedMatch {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RankedMatch {
    fn cmp(&self, other: &Self) -> Ordering {
        Self::compare(&self.0, &other.0)
    }
}

impl PartialEq for RankedWarning {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for RankedWarning {}

impl PartialOrd for RankedWarning {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RankedWarning {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .path
            .cmp(&other.0.path)
            .then_with(|| self.0.kind.cmp(&other.0.kind))
            .then_with(|| self.0.message.cmp(&other.0.message))
    }
}

impl RankedFile {
    fn compare(left: &MatchedFile, right: &MatchedFile) -> Ordering {
        left.root_index
            .cmp(&right.root_index)
            .then_with(|| left.path.cmp(&right.path))
    }
}

impl PartialEq for RankedFile {
    fn eq(&self, other: &Self) -> bool {
        Self::compare(&self.0, &other.0) == Ordering::Equal
    }
}

impl Eq for RankedFile {}

impl PartialOrd for RankedFile {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RankedFile {
    fn cmp(&self, other: &Self) -> Ordering {
        Self::compare(&self.0, &other.0)
    }
}

impl RankedEvidence {
    fn compare(left: &SourceFileEvidence, right: &SourceFileEvidence) -> Ordering {
        left.root_index
            .cmp(&right.root_index)
            .then_with(|| left.path.cmp(&right.path))
    }
}

impl PartialEq for RankedEvidence {
    fn eq(&self, other: &Self) -> bool {
        Self::compare(&self.0, &other.0) == Ordering::Equal
    }
}

impl Eq for RankedEvidence {}

impl PartialOrd for RankedEvidence {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RankedEvidence {
    fn cmp(&self, other: &Self) -> Ordering {
        Self::compare(&self.0, &other.0)
    }
}

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
    file_evidence_visitor: Option<crate::FileEvidenceVisitor>,
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

impl Collector {
    pub(crate) fn new(options: &SearchOptions) -> Self {
        Self {
            limit: options.max_results,
            warning_limit: options.max_warnings,
            file_evidence_mode: options.file_evidence,
            file_evidence_limit: options.max_file_evidence,
            file_evidence_visitor: options.file_evidence_visitor.clone(),
            result_mode: options.result_mode,
            matches: Mutex::new(BinaryHeap::new()),
            files: Mutex::new(BinaryHeap::new()),
            file_evidence: Mutex::new(BinaryHeap::new()),
            warnings: Mutex::new(BinaryHeap::new()),
            fatal: Mutex::new(None),
            matching_lines: AtomicU64::new(0),
            occurrences: AtomicU64::new(0),
            files_with_matches: AtomicU64::new(0),
            warnings_dropped: AtomicU64::new(0),
            truncated: AtomicBool::new(false),
            file_evidence_truncated: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            quiet_found: AtomicBool::new(false),
        }
    }

    pub(crate) fn retain_match(&self, found: SearchMatch) {
        debug_assert_eq!(self.result_mode, ResultMode::Matches);
        if self.limit == 0 {
            self.truncated.store(true, AtomicOrdering::Relaxed);
            return;
        }
        let mut matches = self
            .matches
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches.len() < self.limit {
            matches.push(RankedMatch(found));
            return;
        }
        let should_replace = matches
            .peek()
            .is_some_and(|largest| RankedMatch::compare(&found, &largest.0).is_lt());
        self.truncated.store(true, AtomicOrdering::Relaxed);
        if should_replace {
            matches.pop();
            matches.push(RankedMatch(found));
        }
    }

    pub(crate) fn finish_file(&self, summary: FileSummary) {
        let retain_evidence = match self.file_evidence_mode {
            FileEvidenceMode::None => false,
            FileEvidenceMode::Matched => summary.occurrences > 0,
            FileEvidenceMode::All => true,
        };
        if retain_evidence || self.file_evidence_visitor.is_some() {
            let evidence = SourceFileEvidence {
                root_index: summary.root_index,
                path: summary.path.clone(),
                source_bytes: summary.source_bytes,
                total_lines: summary.total_lines,
                matching_lines: summary.matching_lines,
                occurrences: summary.occurrences,
                encoding: summary.encoding.into_owned(),
                lossy: summary.lossy,
                archive: summary.archive,
            };
            if let Some(visitor) = &self.file_evidence_visitor {
                visitor.visit(&evidence);
            }
            if retain_evidence {
                self.retain_file_evidence(evidence);
            }
        }

        if summary.occurrences == 0 || self.result_mode == ResultMode::Quiet {
            return;
        }
        self.matching_lines
            .fetch_add(summary.matching_lines, AtomicOrdering::Relaxed);
        self.occurrences
            .fetch_add(summary.occurrences, AtomicOrdering::Relaxed);
        self.files_with_matches
            .fetch_add(1, AtomicOrdering::Relaxed);
        if !matches!(self.result_mode, ResultMode::Count | ResultMode::Files) {
            return;
        }
        let file = MatchedFile {
            root_index: summary.root_index,
            path: summary.path,
            matching_lines: summary.matching_lines,
            occurrences: summary.occurrences,
            archive: summary.archive,
        };
        if self.limit == 0 {
            self.truncated.store(true, AtomicOrdering::Relaxed);
            return;
        }
        let mut files = self
            .files
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if files.len() < self.limit {
            files.push(RankedFile(file));
            return;
        }
        let should_replace = files
            .peek()
            .is_some_and(|largest| RankedFile::compare(&file, &largest.0).is_lt());
        self.truncated.store(true, AtomicOrdering::Relaxed);
        if should_replace {
            files.pop();
            files.push(RankedFile(file));
        }
    }

    pub(crate) fn needs_file_evidence(&self, occurrences: u64) -> bool {
        self.file_evidence_visitor.is_some()
            || self.file_evidence_mode == FileEvidenceMode::All
            || (self.file_evidence_mode == FileEvidenceMode::Matched && occurrences > 0)
    }

    fn retain_file_evidence(&self, evidence: SourceFileEvidence) {
        if self.file_evidence_limit == 0 {
            self.file_evidence_truncated
                .store(true, AtomicOrdering::Relaxed);
            return;
        }
        let mut retained = self
            .file_evidence
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if retained.len() < self.file_evidence_limit {
            retained.push(RankedEvidence(evidence));
            return;
        }
        let should_replace = retained
            .peek()
            .is_some_and(|largest| RankedEvidence::compare(&evidence, &largest.0).is_lt());
        self.file_evidence_truncated
            .store(true, AtomicOrdering::Relaxed);
        if should_replace {
            retained.pop();
            retained.push(RankedEvidence(evidence));
        }
    }

    pub(crate) fn quiet_match(&self, matching_lines: u64, occurrences: u64) {
        if self
            .quiet_found
            .compare_exchange(false, true, AtomicOrdering::AcqRel, AtomicOrdering::Acquire)
            .is_ok()
        {
            self.matching_lines
                .store(matching_lines, AtomicOrdering::Relaxed);
            self.occurrences.store(occurrences, AtomicOrdering::Relaxed);
            self.files_with_matches.store(1, AtomicOrdering::Relaxed);
        }
    }

    pub(crate) const fn result_mode(&self) -> ResultMode {
        self.result_mode
    }

    pub(crate) fn warn(&self, warning: SearchWarning) {
        if self.warning_limit == 0 {
            self.warnings_dropped.fetch_add(1, AtomicOrdering::Relaxed);
            return;
        }
        let mut warnings = self
            .warnings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if warnings.len() < self.warning_limit {
            warnings.push(RankedWarning(warning));
            return;
        }
        let should_replace = warnings.peek().is_some_and(|largest| warning < largest.0);
        self.warnings_dropped.fetch_add(1, AtomicOrdering::Relaxed);
        if should_replace {
            warnings.pop();
            warnings.push(RankedWarning(warning));
        }
    }

    pub(crate) fn fail(&self, error: Error) {
        let mut fatal = self
            .fatal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if fatal.is_none() {
            *fatal = Some(error);
            self.failed.store(true, AtomicOrdering::Release);
        }
    }

    pub(crate) fn has_failed(&self) -> bool {
        self.failed.load(AtomicOrdering::Acquire)
    }

    pub(crate) fn should_quit(&self) -> bool {
        self.has_failed()
            || (self.result_mode == ResultMode::Quiet
                && self.quiet_found.load(AtomicOrdering::Acquire))
    }

    pub(crate) fn take_fatal(&self) -> Option<Error> {
        self.fatal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    pub(crate) fn finish(&self) -> Collected {
        let mut matches = self
            .matches
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain()
            .map(|ranked| ranked.0)
            .collect::<Vec<_>>();
        matches.sort_unstable_by(RankedMatch::compare);
        let mut files = self
            .files
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain()
            .map(|ranked| ranked.0)
            .collect::<Vec<_>>();
        files.sort_unstable_by(RankedFile::compare);
        let mut warnings = self
            .warnings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain()
            .map(|ranked| ranked.0)
            .collect::<Vec<_>>();
        warnings.sort_unstable_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.kind.cmp(&right.kind))
                .then_with(|| left.message.cmp(&right.message))
        });
        let mut file_evidence = self
            .file_evidence
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain()
            .map(|ranked| ranked.0)
            .collect::<Vec<_>>();
        file_evidence.sort_unstable_by(RankedEvidence::compare);
        Collected {
            matches,
            files,
            file_evidence,
            warnings,
            matching_lines: self.matching_lines.load(AtomicOrdering::Relaxed),
            occurrences: self.occurrences.load(AtomicOrdering::Relaxed),
            files_with_matches: self.files_with_matches.load(AtomicOrdering::Relaxed),
            warnings_dropped: self.warnings_dropped.load(AtomicOrdering::Relaxed),
            truncated: self.truncated.load(AtomicOrdering::Relaxed),
            file_evidence_truncated: self.file_evidence_truncated.load(AtomicOrdering::Relaxed),
        }
    }
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
