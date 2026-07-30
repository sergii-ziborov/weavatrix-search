use super::{
    EncodingMode, Error, FileEvidenceMode, IndexedContent, PersistentIndex, Result, SearchOptions,
    SearchQuery, SearchReport, search_indexed,
};

impl PersistentIndex {
    /// Searches the indexed snapshot with bounded parallel verification.
    ///
    /// # Errors
    ///
    /// Returns query, decoding, archive, or resource-limit failures.
    pub fn search(&self, query: SearchQuery, options: SearchOptions) -> Result<SearchReport> {
        self.search_with_parallelism(query, options, self.options.search_parallelism)
    }

    /// Searches with an explicit worker count.
    ///
    /// # Errors
    ///
    /// Returns the same failures as [`Self::search`] and rejects zero workers.
    // Owning the query keeps temporary literals ergonomic and mirrors Searcher.
    #[allow(clippy::needless_pass_by_value)]
    pub fn search_with_parallelism(
        &self,
        query: SearchQuery,
        options: SearchOptions,
        parallelism: usize,
    ) -> Result<SearchReport> {
        if parallelism == 0 {
            return Err(Error::index(
                "<memory>",
                "search parallelism must be greater than zero",
            ));
        }
        let needs_every_file = options.file_evidence == FileEvidenceMode::All
            || options.file_evidence_visitor.is_some();
        let alternatives = if needs_every_file {
            None
        } else {
            match options.encoding {
                EncodingMode::Auto | EncodingMode::Utf8 => query.prefilter_trigrams(options.case),
                EncodingMode::Utf16Le | EncodingMode::Utf16Be | EncodingMode::Label(_) => None,
            }
        };
        let files = self
            .entries
            .iter()
            .filter(|entry| {
                alternatives
                    .as_deref()
                    .is_none_or(|trigrams| entry.may_match(trigrams))
            })
            .map(|entry| IndexedContent {
                root_index: entry.root_index,
                path: &entry.path,
                bytes: &entry.content,
            })
            .collect::<Vec<_>>();
        let candidate_files = u64::try_from(files.len()).unwrap_or(u64::MAX);
        let indexed_files = u64::try_from(self.entries.len()).unwrap_or(u64::MAX);
        search_indexed(
            self.roots.clone(),
            &files,
            &query,
            options,
            parallelism,
            self.revision.clone(),
            indexed_files,
            candidate_files,
            alternatives.is_some(),
        )
    }
}
