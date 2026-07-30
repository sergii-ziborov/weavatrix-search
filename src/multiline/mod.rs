mod context;

use crate::collector::{Collector, FileSummary};
use crate::line_search::SearchIdentity;
use crate::options::{ResultMode, SearchOptions};
use crate::query::{CompiledQuery, QueryCache};
use crate::report::{MatchSpan, SearchMatch};
use context::{LineCursor, context_after, context_before};
use memchr::memchr_iter;
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
    let total_lines = if collector.needs_file_evidence(occurrences) {
        logical_line_count(bytes)
    } else {
        0
    };
    collector.finish_file(FileSummary {
        root_index: identity.root_index,
        path: identity.path,
        source_bytes: identity.source_bytes,
        total_lines,
        matching_lines,
        occurrences,
        encoding: identity.encoding,
        lossy: identity.lossy,
        archive: identity.archive,
    });
}

fn logical_line_count(bytes: &[u8]) -> u64 {
    if bytes.is_empty() {
        return 0;
    }
    let terminated = bytes.last() == Some(&b'\n');
    u64::try_from(memchr_iter(b'\n', bytes).count())
        .unwrap_or(u64::MAX)
        .saturating_add(u64::from(!terminated))
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
        encoding: identity.encoding.to_string(),
        lossy: identity.lossy,
        archive: identity.archive,
    });
    (matching_lines, occurrences)
}
