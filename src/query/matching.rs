use super::{CompiledQuery, QueryCache};
use crate::error::Result;
use crate::report::MatchSpan;
use regex_automata::Input;
use regex_automata::meta::Regex;
use regex_automata::util::syntax;

impl CompiledQuery {
    pub(super) fn regex_many(patterns: &[String]) -> Result<Self> {
        let regex = Regex::builder()
            .syntax(syntax::Config::new().utf8(true))
            .build_many(patterns)?;
        Ok(Self::Regex(regex))
    }

    pub(crate) fn create_cache(&self) -> QueryCache {
        match self {
            Self::Literal { .. } => QueryCache::Literal,
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
            (Self::Literal { finder, length }, QueryCache::Literal) => {
                for start in finder.find_iter(haystack) {
                    if !visitor(MatchSpan {
                        pattern_index: 0,
                        start,
                        end: start + *length,
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
            (Self::Literal { .. }, QueryCache::Regex(_))
            | (Self::Regex(_), QueryCache::Literal) => {
                unreachable!("query cache belongs to another compiled query")
            }
        }
    }
}
