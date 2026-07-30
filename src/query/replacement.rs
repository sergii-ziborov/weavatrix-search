use super::{CompiledQuery, QueryCache};
use regex_automata::Input;
use regex_automata::util::captures::Captures;
use std::ops::Range;

impl CompiledQuery {
    pub(crate) fn replacement_preview(
        &self,
        cache: &mut QueryCache,
        haystack: &[u8],
        range: Range<usize>,
        replacement: &str,
        limit: usize,
    ) -> Option<Vec<u8>> {
        let mut output = Vec::with_capacity(range.len().min(limit));
        let mut copied = range.start;
        let mut exceeded = false;
        match (self, cache) {
            (Self::Literal { finder, length }, QueryCache::Literal) => {
                for start in finder.find_iter(&haystack[range.clone()]) {
                    let start = range.start + start;
                    let end = start + *length;
                    append_bounded(&mut output, &haystack[copied..start], limit, &mut exceeded);
                    append_replacement(
                        &mut output,
                        replacement,
                        haystack,
                        start..end,
                        None,
                        limit,
                        &mut exceeded,
                    );
                    copied = end;
                    if exceeded {
                        break;
                    }
                }
            }
            (Self::Regex(regex), QueryCache::Regex(cache)) => {
                let mut captures = regex.create_captures();
                let input = Input::new(haystack).range(range.clone());
                let mut searcher = regex_automata::util::iter::Searcher::new(input);
                while let Some(matched) = searcher.advance(|input| {
                    regex.search_captures_with(cache.as_mut(), input, &mut captures);
                    Ok(captures.get_match())
                }) {
                    append_bounded(
                        &mut output,
                        &haystack[copied..matched.start()],
                        limit,
                        &mut exceeded,
                    );
                    append_replacement(
                        &mut output,
                        replacement,
                        haystack,
                        matched.start()..matched.end(),
                        Some(&captures),
                        limit,
                        &mut exceeded,
                    );
                    copied = matched.end();
                    if exceeded {
                        break;
                    }
                }
            }
            (Self::Literal { .. }, QueryCache::Regex(_))
            | (Self::Regex(_), QueryCache::Literal) => {
                unreachable!("query cache belongs to another compiled query")
            }
        }
        append_bounded(
            &mut output,
            &haystack[copied..range.end],
            limit,
            &mut exceeded,
        );
        (!exceeded).then_some(output)
    }
}

fn append_bounded(output: &mut Vec<u8>, bytes: &[u8], limit: usize, exceeded: &mut bool) {
    if output.len().saturating_add(bytes.len()) > limit {
        *exceeded = true;
        return;
    }
    output.extend_from_slice(bytes);
}

fn append_replacement(
    output: &mut Vec<u8>,
    replacement: &str,
    haystack: &[u8],
    matched: Range<usize>,
    captures: Option<&Captures>,
    limit: usize,
    exceeded: &mut bool,
) {
    let bytes = replacement.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() && !*exceeded {
        let Some(relative) = memchr::memchr(b'$', &bytes[cursor..]) else {
            append_bounded(output, &bytes[cursor..], limit, exceeded);
            break;
        };
        let dollar = cursor + relative;
        append_bounded(output, &bytes[cursor..dollar], limit, exceeded);
        if *exceeded {
            break;
        }
        if bytes.get(dollar + 1) == Some(&b'$') {
            append_bounded(output, b"$", limit, exceeded);
            cursor = dollar + 2;
            continue;
        }

        let (reference, next) = if bytes.get(dollar + 1) == Some(&b'{') {
            let start = dollar + 2;
            let Some(end_relative) = memchr::memchr(b'}', &bytes[start..]) else {
                append_bounded(output, b"$", limit, exceeded);
                cursor = dollar + 1;
                continue;
            };
            let end = start + end_relative;
            (&replacement[start..end], end + 1)
        } else {
            let start = dollar + 1;
            let mut end = start;
            while bytes
                .get(end)
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
            {
                end += 1;
            }
            if start == end {
                append_bounded(output, b"$", limit, exceeded);
                cursor = dollar + 1;
                continue;
            }
            (&replacement[start..end], end)
        };
        if let Some(group) = capture_range(reference, &matched, captures) {
            append_bounded(output, &haystack[group], limit, exceeded);
        }
        cursor = next;
    }
}

fn capture_range(
    reference: &str,
    matched: &Range<usize>,
    captures: Option<&Captures>,
) -> Option<Range<usize>> {
    if let Ok(index) = reference.parse::<usize>() {
        if let Some(captures) = captures {
            return captures.get_group(index).map(|span| span.start..span.end);
        }
        return (index == 0).then(|| matched.clone());
    }
    captures
        .and_then(|captures| captures.get_group_by_name(reference))
        .map(|span| span.start..span.end)
}
