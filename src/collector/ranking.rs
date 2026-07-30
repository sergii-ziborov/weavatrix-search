use crate::report::{MatchedFile, SearchMatch, SearchWarning, SourceFileEvidence};
use std::cmp::Ordering;

pub(super) struct RankedMatch(pub(super) SearchMatch);

pub(super) struct RankedWarning(pub(super) SearchWarning);

pub(super) struct RankedFile(pub(super) MatchedFile);

pub(super) struct RankedEvidence(pub(super) SourceFileEvidence);

impl RankedMatch {
    pub(super) fn compare(left: &SearchMatch, right: &SearchMatch) -> Ordering {
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
    pub(super) fn compare(left: &MatchedFile, right: &MatchedFile) -> Ordering {
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
    pub(super) fn compare(left: &SourceFileEvidence, right: &SourceFileEvidence) -> Ordering {
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
