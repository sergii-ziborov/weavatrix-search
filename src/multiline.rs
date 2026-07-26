use crate::collector::{Collector, FileSummary};
use crate::line_search::SearchIdentity;
use crate::options::{ResultMode, SearchOptions};
use crate::query::{CompiledQuery, QueryCache};
use crate::report::{ContextLine, MatchSpan, SearchMatch};
use memchr::{memchr, memrchr};
use std::sync::Arc;

pub(crate) fn search(
    identity: SearchIdentity,
    text: &str,
    query: &Arc<CompiledQuery>,
    query_cache: &mut QueryCache,
    options: &Arc<SearchOptions>,
    collector: &Arc<Collector>,
) {
    let bytes = text.as_bytes();
    let mut cursor = LineCursor::new(bytes);
    if options.result_mode == ResultMode::Quiet {
        query.visit_spans(query_cache, bytes, |span| {
            cursor.advance_to(span.start);
            let start_line = cursor.number;
            cursor.advance_to(span.end.saturating_sub(1).max(span.start));
            collector.quiet_match(
                cursor.number.saturating_sub(start_line).saturating_add(1),
                1,
            );
            false
        });
        return;
    }

    let mut block: Option<Block> = None;
    let mut replacement_cache = options.replacement.as_ref().map(|_| query.create_cache());
    let mut matching_lines = 0_u64;
    let mut occurrences = 0_u64;
    query.visit_spans(query_cache, bytes, |span| {
        cursor.advance_to(span.start);
        let start_line = cursor.number;
        let start_byte = cursor.start;
        let end_probe = span.end.saturating_sub(1).max(span.start);
        cursor.advance_to(end_probe);
        let end_line = cursor.number;
        let end_byte = cursor.full_end.max(span.end);

        match &mut block {
            Some(current) if start_line <= current.end_line => {
                current.end_line = current.end_line.max(end_line);
                current.end_byte = current.end_byte.max(end_byte);
                current.spans.push(span);
            }
            Some(_) => {
                let (lines, matches) = finish_block(
                    block.take().expect("multiline block is present"),
                    bytes,
                    &identity,
                    query,
                    replacement_cache.as_mut(),
                    options,
                    collector,
                );
                matching_lines = matching_lines.saturating_add(lines);
                occurrences = occurrences.saturating_add(matches);
                block = Some(Block::new(start_line, end_line, start_byte, end_byte, span));
            }
            None => {
                block = Some(Block::new(start_line, end_line, start_byte, end_byte, span));
            }
        }
        true
    });
    if let Some(block) = block {
        let (lines, matches) = finish_block(
            block,
            bytes,
            &identity,
            query,
            replacement_cache.as_mut(),
            options,
            collector,
        );
        matching_lines = matching_lines.saturating_add(lines);
        occurrences = occurrences.saturating_add(matches);
    }
    collector.finish_file(FileSummary {
        root_index: identity.root_index,
        path: identity.path,
        matching_lines,
        occurrences,
        archive: identity.archive,
    });
}

struct Block {
    start_line: u64,
    end_line: u64,
    start_byte: usize,
    end_byte: usize,
    spans: Vec<MatchSpan>,
}

impl Block {
    fn new(
        start_line: u64,
        end_line: u64,
        start_byte: usize,
        end_byte: usize,
        span: MatchSpan,
    ) -> Self {
        Self {
            start_line,
            end_line,
            start_byte,
            end_byte,
            spans: vec![span],
        }
    }
}

fn finish_block(
    mut block: Block,
    bytes: &[u8],
    identity: &SearchIdentity,
    query: &CompiledQuery,
    query_cache: Option<&mut QueryCache>,
    options: &SearchOptions,
    collector: &Collector,
) -> (u64, u64) {
    let matching_lines = block
        .end_line
        .saturating_sub(block.start_line)
        .saturating_add(1);
    let occurrences = u64::try_from(block.spans.len()).unwrap_or(u64::MAX);
    if options.result_mode != ResultMode::Matches {
        return (matching_lines, occurrences);
    }
    for span in &mut block.spans {
        span.start = span.start.saturating_sub(block.start_byte);
        span.end = span.end.saturating_sub(block.start_byte);
    }
    let line = std::str::from_utf8(&bytes[block.start_byte..block.end_byte])
        .expect("decoded multiline input is UTF-8")
        .to_owned();
    let replacement_preview = options.replacement.as_deref().and_then(|replacement| {
        if let Some(preview) = query.replacement_preview(
            query_cache.expect("replacement cache exists when replacement is requested"),
            bytes,
            block.start_byte..block.end_byte,
            replacement,
            options.max_replacement_bytes,
        ) {
            Some(
                String::from_utf8(preview)
                    .expect("replacement of decoded multiline input is UTF-8"),
            )
        } else {
            collector.warn(crate::report::SearchWarning {
                path: identity.path.clone(),
                kind: crate::report::SearchWarningKind::Limit,
                message: format!(
                    "replacement preview exceeds the {} byte limit",
                    options.max_replacement_bytes
                ),
            });
            None
        }
    });
    collector.retain_match(SearchMatch {
        root_index: identity.root_index,
        path: identity.path.clone(),
        line_number: block.start_line,
        end_line_number: block.end_line,
        decoded_byte_offset: u64::try_from(block.start_byte).unwrap_or(u64::MAX),
        source_byte_offset: identity
            .source_offset_base
            .map(|base| base.saturating_add(u64::try_from(block.start_byte).unwrap_or(u64::MAX))),
        line,
        replacement_preview,
        spans: block.spans,
        before: context_before(
            bytes,
            block.start_byte,
            block.start_line,
            options.before_context,
            identity.lossy,
        ),
        after: context_after(
            bytes,
            block.end_byte,
            block.end_line,
            options.after_context,
            identity.lossy,
        ),
        encoding: identity.encoding.clone(),
        lossy: identity.lossy,
        archive: identity.archive,
    });
    (matching_lines, occurrences)
}

#[derive(Clone)]
struct LineCursor<'a> {
    bytes: &'a [u8],
    number: u64,
    start: usize,
    full_end: usize,
}

impl<'a> LineCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            number: 1,
            start: 0,
            full_end: next_line_end(bytes, 0),
        }
    }

    fn advance_to(&mut self, offset: usize) {
        while self.full_end < self.bytes.len() && offset >= self.full_end {
            self.start = self.full_end;
            self.full_end = next_line_end(self.bytes, self.start);
            self.number = self.number.saturating_add(1);
        }
    }
}

fn next_line_end(bytes: &[u8], start: usize) -> usize {
    memchr(b'\n', &bytes[start..]).map_or(bytes.len(), |relative| start + relative + 1)
}

fn context_before(
    bytes: &[u8],
    start: usize,
    start_line: u64,
    count: usize,
    lossy: bool,
) -> Vec<ContextLine> {
    let mut context = Vec::with_capacity(count);
    let mut end = start;
    let mut line_number = start_line;
    for _ in 0..count {
        if end == 0 || line_number <= 1 {
            break;
        }
        let content_end = trim_line_end(bytes, end);
        let line_start = memrchr(b'\n', &bytes[..content_end]).map_or(0, |index| index + 1);
        line_number -= 1;
        context.push(ContextLine {
            line_number,
            text: std::str::from_utf8(&bytes[line_start..content_end])
                .expect("decoded context is UTF-8")
                .to_owned(),
            lossy,
        });
        end = line_start;
    }
    context.reverse();
    context
}

fn context_after(
    bytes: &[u8],
    start: usize,
    end_line: u64,
    count: usize,
    lossy: bool,
) -> Vec<ContextLine> {
    let mut context = Vec::with_capacity(count);
    let mut line_start = start;
    let mut line_number = end_line;
    for _ in 0..count {
        if line_start >= bytes.len() {
            break;
        }
        let full_end = next_line_end(bytes, line_start);
        let content_end = trim_line_end(bytes, full_end);
        line_number = line_number.saturating_add(1);
        context.push(ContextLine {
            line_number,
            text: std::str::from_utf8(&bytes[line_start..content_end])
                .expect("decoded context is UTF-8")
                .to_owned(),
            lossy,
        });
        line_start = full_end;
    }
    context
}

fn trim_line_end(bytes: &[u8], mut end: usize) -> usize {
    if end > 0 && bytes[end - 1] == b'\n' {
        end -= 1;
    }
    if end > 0 && bytes[end - 1] == b'\r' {
        end -= 1;
    }
    end
}
