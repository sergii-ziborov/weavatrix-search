use super::{LineSearcher, QueryCache, SearchWarning, SearchWarningKind};

impl LineSearcher {
    pub(super) fn warn_long_line(&self) {
        self.collector.warn(SearchWarning {
            path: self.identity.path.clone(),
            kind: SearchWarningKind::LineTooLong,
            message: format!(
                "line {} exceeds the {} byte limit",
                self.line_number, self.options.max_line_bytes
            ),
        });
    }

    pub(super) fn warn_lossy(&mut self) {
        if self.warned_lossy {
            return;
        }
        self.warned_lossy = true;
        self.collector.warn(SearchWarning {
            path: self.identity.path.clone(),
            kind: SearchWarningKind::Encoding,
            message: format!(
                "{} contains malformed UTF-8; replacement characters were used",
                self.identity.path
            ),
        });
    }

    pub(super) fn warn_replacement_limit(&mut self) {
        if self.warned_replacement_limit {
            return;
        }
        self.warned_replacement_limit = true;
        self.collector.warn(SearchWarning {
            path: self.identity.path.clone(),
            kind: SearchWarningKind::Limit,
            message: format!(
                "replacement preview exceeds the {} byte limit",
                self.options.max_replacement_bytes
            ),
        });
    }

    pub(super) fn render_replacement(
        &mut self,
        line: &[u8],
        query_cache: &mut QueryCache,
    ) -> Option<String> {
        let replacement = self.options.replacement.clone()?;
        if let Some(preview) = self.query.replacement_preview(
            query_cache,
            line,
            0..line.len(),
            &replacement,
            self.options.max_replacement_bytes,
        ) {
            Some(String::from_utf8_lossy(&preview).into_owned())
        } else {
            self.warn_replacement_limit();
            None
        }
    }
}

pub(super) fn render_line(line: &[u8]) -> (String, bool) {
    match std::str::from_utf8(line) {
        Ok(text) => (text.to_owned(), false),
        Err(_) => (String::from_utf8_lossy(line).into_owned(), true),
    }
}
