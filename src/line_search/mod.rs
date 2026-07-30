mod diagnostics;

use crate::collector::{Collector, FileSummary};
use crate::options::{ResultMode, SearchOptions};
use crate::query::{CompiledQuery, QueryCache};
use crate::report::{ContextLine, SearchMatch, SearchWarning, SearchWarningKind};
use diagnostics::render_line;
use memchr::memchr;
use std::borrow::Cow;
use std::collections::VecDeque;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(crate) struct SearchIdentity {
    pub(crate) root_index: usize,
    pub(crate) path: String,
    pub(crate) source_bytes: u64,
    pub(crate) encoding: Cow<'static, str>,
    pub(crate) archive: bool,
    pub(crate) source_offset_base: Option<u64>,
    pub(crate) lossy: bool,
}

struct PendingMatch {
    found: SearchMatch,
    remaining: usize,
}

pub(crate) struct LineSearcher {
    query: Arc<CompiledQuery>,
    options: Arc<SearchOptions>,
    collector: Arc<Collector>,
    identity: SearchIdentity,
    pending_bytes: Vec<u8>,
    pending_offset: u64,
    next_offset: u64,
    line_number: u64,
    discarding_long_line: bool,
    before: VecDeque<ContextLine>,
    pending_matches: Vec<PendingMatch>,
    warned_lossy: bool,
    matching_lines: u64,
    occurrences: u64,
    warned_replacement_limit: bool,
}

impl LineSearcher {
    pub(crate) fn path(&self) -> &str {
        &self.identity.path
    }

    pub(crate) fn new(
        query: Arc<CompiledQuery>,
        options: Arc<SearchOptions>,
        collector: Arc<Collector>,
        identity: SearchIdentity,
    ) -> Self {
        Self {
            query,
            options,
            collector,
            identity,
            pending_bytes: Vec::new(),
            pending_offset: 0,
            next_offset: 0,
            line_number: 1,
            discarding_long_line: false,
            before: VecDeque::new(),
            pending_matches: Vec::new(),
            warned_lossy: false,
            matching_lines: 0,
            occurrences: 0,
            warned_replacement_limit: false,
        }
    }

    pub(crate) fn push(&mut self, mut bytes: &[u8], query_cache: &mut QueryCache) {
        if bytes.is_empty() {
            return;
        }
        if self.discarding_long_line {
            let Some(newline) = memchr(b'\n', bytes) else {
                self.next_offset = self
                    .next_offset
                    .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
                return;
            };
            let consumed = newline + 1;
            self.next_offset = self
                .next_offset
                .saturating_add(u64::try_from(consumed).unwrap_or(u64::MAX));
            self.line_number = self.line_number.saturating_add(1);
            self.pending_offset = self.next_offset;
            self.discarding_long_line = false;
            bytes = &bytes[consumed..];
        }

        if !self.pending_bytes.is_empty() {
            if let Some(newline) = memchr(b'\n', bytes) {
                let prefix = &bytes[..newline];
                if self.pending_bytes.len().saturating_add(prefix.len())
                    > self.options.max_line_bytes
                {
                    self.warn_long_line();
                } else {
                    self.pending_bytes.extend_from_slice(prefix);
                    let line = std::mem::take(&mut self.pending_bytes);
                    self.process_line(&line, self.pending_offset, query_cache);
                }
                let consumed = newline + 1;
                self.next_offset = self
                    .next_offset
                    .saturating_add(u64::try_from(consumed).unwrap_or(u64::MAX));
                self.line_number = self.line_number.saturating_add(1);
                self.pending_offset = self.next_offset;
                bytes = &bytes[consumed..];
            } else {
                if self.pending_bytes.len().saturating_add(bytes.len())
                    > self.options.max_line_bytes
                {
                    self.pending_bytes.clear();
                    self.warn_long_line();
                    self.discarding_long_line = true;
                } else {
                    self.pending_bytes.extend_from_slice(bytes);
                }
                self.next_offset = self
                    .next_offset
                    .saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));
                return;
            }
        }

        let mut start = 0;
        while let Some(relative_newline) = memchr(b'\n', &bytes[start..]) {
            let newline = start + relative_newline;
            let line = &bytes[start..newline];
            if line.len() > self.options.max_line_bytes {
                self.warn_long_line();
            } else {
                self.process_line(line, self.pending_offset, query_cache);
            }
            let consumed = newline + 1 - start;
            self.next_offset = self
                .next_offset
                .saturating_add(u64::try_from(consumed).unwrap_or(u64::MAX));
            self.line_number = self.line_number.saturating_add(1);
            self.pending_offset = self.next_offset;
            start = newline + 1;
        }
        let trailing = &bytes[start..];
        if trailing.len() > self.options.max_line_bytes {
            self.warn_long_line();
            self.discarding_long_line = true;
        } else {
            self.pending_bytes.extend_from_slice(trailing);
        }
        self.next_offset = self
            .next_offset
            .saturating_add(u64::try_from(trailing.len()).unwrap_or(u64::MAX));
    }

    pub(crate) fn finish(mut self, query_cache: &mut QueryCache) {
        let has_unterminated_line = !self.pending_bytes.is_empty() || self.discarding_long_line;
        if !self.pending_bytes.is_empty() && !self.discarding_long_line {
            let line = std::mem::take(&mut self.pending_bytes);
            self.process_line(&line, self.pending_offset, query_cache);
        }
        for pending in self.pending_matches.drain(..) {
            self.collector.retain_match(pending.found);
        }
        let total_lines = self
            .line_number
            .saturating_sub(1)
            .saturating_add(u64::from(has_unterminated_line));
        self.collector.finish_file(FileSummary {
            root_index: self.identity.root_index,
            path: self.identity.path,
            source_bytes: self.identity.source_bytes,
            total_lines,
            matching_lines: self.matching_lines,
            occurrences: self.occurrences,
            encoding: self.identity.encoding,
            lossy: self.identity.lossy,
            archive: self.identity.archive,
        });
    }

    fn process_line(&mut self, line: &[u8], offset: u64, query_cache: &mut QueryCache) {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let spans = self.query.find_spans(query_cache, line);
        if !spans.is_empty() {
            self.matching_lines = self.matching_lines.saturating_add(1);
            self.occurrences = self
                .occurrences
                .saturating_add(u64::try_from(spans.len()).unwrap_or(u64::MAX));
            match self.collector.result_mode() {
                ResultMode::Quiet => {
                    self.collector
                        .quiet_match(1, u64::try_from(spans.len()).unwrap_or(u64::MAX));
                    return;
                }
                ResultMode::Matches | ResultMode::Count | ResultMode::Files => {}
            }
        }
        if self.collector.result_mode() != ResultMode::Matches {
            if std::str::from_utf8(line).is_err() {
                self.warn_lossy();
            }
            return;
        }
        let needs_text = !spans.is_empty()
            || !self.pending_matches.is_empty()
            || self.options.before_context > 0;
        if !needs_text {
            if std::str::from_utf8(line).is_err() {
                self.warn_lossy();
            }
            return;
        }
        let replacement_preview = self.render_replacement(line, query_cache);
        let (rendered, lossy) = render_line(line);
        let lossy = lossy || self.identity.lossy;
        if lossy && !self.identity.lossy {
            self.warn_lossy();
        }
        let context = ContextLine {
            line_number: self.line_number,
            text: rendered.clone(),
            lossy,
        };

        let mut ready = Vec::new();
        for (index, pending) in self.pending_matches.iter_mut().enumerate() {
            pending.found.after.push(context.clone());
            pending.remaining = pending.remaining.saturating_sub(1);
            if pending.remaining == 0 {
                ready.push(index);
            }
        }
        for index in ready.into_iter().rev() {
            let pending = self.pending_matches.swap_remove(index);
            self.collector.retain_match(pending.found);
        }

        if !spans.is_empty() {
            let found = SearchMatch {
                root_index: self.identity.root_index,
                path: self.identity.path.clone(),
                line_number: self.line_number,
                end_line_number: self.line_number,
                decoded_byte_offset: offset,
                source_byte_offset: self
                    .identity
                    .source_offset_base
                    .map(|base| base.saturating_add(offset)),
                line: rendered,
                replacement_preview,
                spans,
                before: self.before.iter().cloned().collect(),
                after: Vec::with_capacity(self.options.after_context),
                encoding: self.identity.encoding.to_string(),
                lossy,
                archive: self.identity.archive,
            };
            if self.options.after_context == 0 {
                self.collector.retain_match(found);
            } else {
                self.pending_matches.push(PendingMatch {
                    found,
                    remaining: self.options.after_context,
                });
            }
        }

        if self.options.before_context > 0 {
            self.before.push_back(context);
            while self.before.len() > self.options.before_context {
                self.before.pop_front();
            }
        }
    }
}
