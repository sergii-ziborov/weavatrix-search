mod matching;
mod replacement;

use crate::error::{Error, Result};
use crate::options::CaseMode;
use memchr::memmem;
use regex_automata::meta::{Cache, Regex};
use regex_syntax::hir::{Hir, HirKind};

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
            return Ok(CompiledQuery::Literal {
                finder: Box::new(memmem::Finder::new(pattern.as_bytes()).into_owned()),
                length: pattern.len(),
            });
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

    pub(crate) fn prefilter_trigrams(&self, case: CaseMode) -> Option<Vec<Vec<u32>>> {
        match self {
            Self::Literal(pattern) => {
                sensitive_trigrams(pattern, case).map(|trigrams| vec![trigrams])
            }
            Self::Regex(pattern) => {
                if is_case_insensitive(pattern, case) {
                    return None;
                }
                let hir = regex_syntax::Parser::new().parse(pattern).ok()?;
                let literal = longest_mandatory_literal(&hir)?;
                trigrams(&literal).map(|trigrams| vec![trigrams])
            }
            Self::Any(queries) => {
                let mut alternatives = Vec::new();
                for query in queries {
                    alternatives.extend(query.prefilter_trigrams(case)?);
                }
                (!alternatives.is_empty()).then_some(alternatives)
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

fn sensitive_trigrams(pattern: &str, case: CaseMode) -> Option<Vec<u32>> {
    (!is_case_insensitive(pattern, case))
        .then_some(pattern.as_bytes())
        .and_then(trigrams)
}

fn longest_mandatory_literal(hir: &Hir) -> Option<Vec<u8>> {
    match hir.kind() {
        HirKind::Literal(literal) => Some(literal.0.to_vec()),
        HirKind::Capture(capture) => longest_mandatory_literal(&capture.sub),
        HirKind::Repetition(repetition) if repetition.min > 0 => {
            longest_mandatory_literal(&repetition.sub)
        }
        HirKind::Concat(expressions) => expressions
            .iter()
            .filter_map(longest_mandatory_literal)
            .max_by_key(Vec::len),
        HirKind::Empty
        | HirKind::Class(_)
        | HirKind::Look(_)
        | HirKind::Repetition(_)
        | HirKind::Alternation(_) => None,
    }
}

fn trigrams(bytes: &[u8]) -> Option<Vec<u32>> {
    if bytes.len() < 3 {
        return None;
    }
    let mut values = bytes
        .windows(3)
        .map(|window| {
            (u32::from(window[0]) << 16) | (u32::from(window[1]) << 8) | u32::from(window[2])
        })
        .collect::<Vec<_>>();
    values.sort_unstable();
    values.dedup();
    Some(values)
}

pub(crate) enum CompiledQuery {
    Literal {
        finder: Box<memmem::Finder<'static>>,
        length: usize,
    },
    Regex(Regex),
}

pub(crate) enum QueryCache {
    Literal,
    Regex(Box<Cache>),
}
