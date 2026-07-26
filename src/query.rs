use crate::error::{Error, Result};
use crate::options::CaseMode;
use crate::report::MatchSpan;
use memchr::memmem;
use regex_automata::Input;
use regex_automata::meta::{Cache, Regex};
use regex_automata::util::captures::Captures;
use regex_automata::util::syntax;
use std::ops::Range;

/// A compiled-on-execution repository query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchQuery {
    /// Matches the supplied text literally.
    Literal(String),
    /// Matches with Rust regular-expression syntax.
    Regex(String),
    /// Matches the ordered union of several queries in one content pass.
    ///
    /// Query order resolves alternatives that begin at the same byte, like
    /// repeated `-e` arguments in ripgrep.
    Any(Vec<SearchQuery>),
}

impl SearchQuery {
    /// Creates a literal query.
    #[must_use]
    pub fn literal(value: impl Into<String>) -> Self {
        Self::Literal(value.into())
    }

    /// Creates a regular-expression query.
    #[must_use]
    pub fn regex(value: impl Into<String>) -> Self {
        Self::Regex(value.into())
    }

    /// Creates an ordered multi-pattern query.
    #[must_use]
    pub fn any(queries: impl IntoIterator<Item = SearchQuery>) -> Self {
        Self::Any(queries.into_iter().collect())
    }

    pub(crate) fn compile(&self, case: CaseMode) -> Result<CompiledQuery> {
        let mut patterns = Vec::new();
        self.collect_patterns(case, &mut patterns)?;
        if patterns.is_empty() {
            return Err(Error::EmptyQuery);
        }
        if let [PreparedPattern::SensitiveLiteral(pattern)] = patterns.as_slice() {
            return Ok(CompiledQuery::Literal(
                pattern.as_bytes().to_vec().into_boxed_slice(),
            ));
        }
        CompiledQuery::regex_many(
            &patterns
                .into_iter()
                .map(PreparedPattern::into_regex)
                .collect::<Vec<_>>(),
        )
    }

    fn collect_patterns(&self, case: CaseMode, patterns: &mut Vec<PreparedPattern>) -> Result<()> {
        match self {
            Self::Literal(pattern) | Self::Regex(pattern) if pattern.is_empty() => {
                Err(Error::EmptyQuery)
            }
            Self::Literal(pattern) => {
                let insensitive = is_case_insensitive(pattern, case);
                if insensitive {
                    patterns.push(PreparedPattern::Regex(format!(
                        "(?i:{})",
                        regex_syntax::escape(pattern)
                    )));
                } else {
                    patterns.push(PreparedPattern::SensitiveLiteral(pattern.clone()));
                }
                Ok(())
            }
            Self::Regex(pattern) => {
                let pattern = if is_case_insensitive(pattern, case) {
                    format!("(?i:{pattern})")
                } else {
                    pattern.clone()
                };
                patterns.push(PreparedPattern::Regex(pattern));
                Ok(())
            }
            Self::Any(queries) => {
                for query in queries {
                    query.collect_patterns(case, patterns)?;
                }
                Ok(())
            }
        }
    }
}

enum PreparedPattern {
    SensitiveLiteral(String),
    Regex(String),
}

impl PreparedPattern {
    fn into_regex(self) -> String {
        match self {
            Self::SensitiveLiteral(pattern) => regex_syntax::escape(&pattern),
            Self::Regex(pattern) => pattern,
        }
    }
}

fn is_case_insensitive(pattern: &str, case: CaseMode) -> bool {
    match case {
        CaseMode::Sensitive => false,
        CaseMode::Insensitive => true,
        CaseMode::Smart => !pattern.chars().any(char::is_uppercase),
    }
}

pub(crate) enum CompiledQuery {
    Literal(Box<[u8]>),
    Regex(Regex),
}

pub(crate) enum QueryCache {
    Literal,
    Regex(Box<Cache>),
}

impl CompiledQuery {
    fn regex_many(patterns: &[String]) -> Result<Self> {
        let regex = Regex::builder()
            .syntax(syntax::Config::new().utf8(true))
            .build_many(patterns)?;
        Ok(Self::Regex(regex))
    }

    pub(crate) fn create_cache(&self) -> QueryCache {
        match self {
            Self::Literal(_) => QueryCache::Literal,
            Self::Regex(regex) => QueryCache::Regex(Box::new(regex.create_cache())),
        }
    }

    pub(crate) fn find_spans(&self, cache: &mut QueryCache, haystack: &[u8]) -> Vec<MatchSpan> {
        let mut spans = Vec::new();
        self.visit_spans(cache, haystack, |span| {
            spans.push(span);
            true
        });
        spans
    }

    pub(crate) fn visit_spans(
        &self,
        cache: &mut QueryCache,
        haystack: &[u8],
        mut visitor: impl FnMut(MatchSpan) -> bool,
    ) {
        match (self, cache) {
            (Self::Literal(needle), QueryCache::Literal) => {
                for start in memmem::find_iter(haystack, needle) {
                    if !visitor(MatchSpan {
                        pattern_index: 0,
                        start,
                        end: start + needle.len(),
                    }) {
                        break;
                    }
                }
            }
            (Self::Regex(regex), QueryCache::Regex(cache)) => {
                let mut searcher = regex_automata::util::iter::Searcher::new(Input::new(haystack));
                while let Some(matched) =
                    searcher.advance(|input| Ok(regex.search_with(cache.as_mut(), input)))
                {
                    if !visitor(MatchSpan {
                        pattern_index: matched.pattern().as_usize(),
                        start: matched.start(),
                        end: matched.end(),
                    }) {
                        break;
                    }
                }
            }
            (Self::Literal(_), QueryCache::Regex(_)) | (Self::Regex(_), QueryCache::Literal) => {
                unreachable!("query cache belongs to another compiled query")
            }
        }
    }

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
            (Self::Literal(needle), QueryCache::Literal) => {
                for start in memmem::find_iter(&haystack[range.clone()], needle) {
                    let start = range.start + start;
                    let end = start + needle.len();
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
            (Self::Literal(_), QueryCache::Regex(_)) | (Self::Regex(_), QueryCache::Literal) => {
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
