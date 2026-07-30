use super::{Error, Path, Result};

/// Resource and parallelism policy for persistent indexes.
#[derive(Debug, Clone)]
pub struct IndexOptions {
    /// Maximum selected files accepted by one index.
    pub max_entries: u64,
    /// Maximum raw source bytes retained by one index.
    pub max_content_bytes: u64,
    /// Maximum serialized index bytes accepted on load or save.
    pub max_index_bytes: u64,
    /// Maximum encoded bytes accepted for one root or relative path.
    pub max_path_bytes: usize,
    /// Content workers used while building or updating the index.
    pub build_parallelism: usize,
    /// Workers used to verify candidate content during a query.
    pub search_parallelism: usize,
}

impl Default for IndexOptions {
    fn default() -> Self {
        let parallelism = std::thread::available_parallelism()
            .map_or(1, usize::from)
            .clamp(1, 32);
        Self {
            max_entries: 2_000_000,
            max_content_bytes: 64 * 1024 * 1024 * 1024,
            max_index_bytes: 96 * 1024 * 1024 * 1024,
            max_path_bytes: 1024 * 1024,
            build_parallelism: parallelism,
            search_parallelism: parallelism,
        }
    }
}

impl IndexOptions {
    /// Sets build and query worker counts.
    #[must_use]
    pub fn with_parallelism(mut self, workers: usize) -> Self {
        self.build_parallelism = workers;
        self.search_parallelism = workers;
        self
    }

    /// Sets content workers used while building or updating.
    #[must_use]
    pub const fn with_build_parallelism(mut self, workers: usize) -> Self {
        self.build_parallelism = workers;
        self
    }

    /// Sets candidate-verification workers used by queries.
    #[must_use]
    pub const fn with_search_parallelism(mut self, workers: usize) -> Self {
        self.search_parallelism = workers;
        self
    }

    /// Sets the maximum selected-file count.
    #[must_use]
    pub const fn with_max_entries(mut self, max_entries: u64) -> Self {
        self.max_entries = max_entries;
        self
    }

    /// Sets the maximum retained source bytes.
    #[must_use]
    pub const fn with_max_content_bytes(mut self, max_content_bytes: u64) -> Self {
        self.max_content_bytes = max_content_bytes;
        self
    }

    /// Sets the maximum serialized index bytes.
    #[must_use]
    pub const fn with_max_index_bytes(mut self, max_index_bytes: u64) -> Self {
        self.max_index_bytes = max_index_bytes;
        self
    }

    /// Sets the maximum encoded length of one root or relative path.
    #[must_use]
    pub const fn with_max_path_bytes(mut self, max_path_bytes: usize) -> Self {
        self.max_path_bytes = max_path_bytes;
        self
    }

    pub(super) fn validate(&self, path: &Path) -> Result<()> {
        if self.build_parallelism == 0 || self.search_parallelism == 0 {
            return Err(Error::index(path, "parallelism must be greater than zero"));
        }
        if self.max_entries == 0 {
            return Err(Error::index(path, "max_entries must be greater than zero"));
        }
        if self.max_content_bytes == 0 || self.max_index_bytes == 0 {
            return Err(Error::index(
                path,
                "content and index byte limits must be greater than zero",
            ));
        }
        if self.max_path_bytes == 0 {
            return Err(Error::index(
                path,
                "max_path_bytes must be greater than zero",
            ));
        }
        Ok(())
    }
}
