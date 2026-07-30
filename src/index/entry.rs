use super::{
    BLOOM_WORDS, ContentFileStatus, Digest, Error, Result, Sha256, archive, bloom_contains,
    trigram_bloom,
};

#[derive(Debug)]
pub(super) struct IndexEntry {
    pub(super) root_index: usize,
    pub(super) path: String,
    pub(super) content: Vec<u8>,
    pub(super) content_hash: [u8; 32],
    pub(super) prefilterable: bool,
    pub(super) bloom: [u64; BLOOM_WORDS],
}

impl IndexEntry {
    pub(super) fn key(&self) -> (usize, &str) {
        (self.root_index, &self.path)
    }

    pub(super) fn from_parts(root_index: usize, path: String, content: Vec<u8>) -> Self {
        let content_hash: [u8; 32] = Sha256::digest(&content).into();
        let prefilterable = std::str::from_utf8(&content).is_ok() && archive::kind(&path).is_none();
        let bloom = if prefilterable {
            trigram_bloom(&content)
        } else {
            [0; BLOOM_WORDS]
        };
        Self {
            root_index,
            path,
            content,
            content_hash,
            prefilterable,
            bloom,
        }
    }

    pub(super) fn may_match(&self, alternatives: &[Vec<u32>]) -> bool {
        !self.prefilterable
            || alternatives.iter().any(|trigrams| {
                trigrams
                    .iter()
                    .all(|trigram| bloom_contains(&self.bloom, *trigram))
            })
    }
}

pub(super) struct EntryBuilder {
    pub(super) root_index: usize,
    pub(super) path: String,
    pub(super) expected_bytes: u64,
    pub(super) content: Vec<u8>,
}

impl EntryBuilder {
    pub(super) fn new(root_index: usize, path: &str, expected_bytes: u64) -> Result<Self> {
        let capacity = usize::try_from(expected_bytes).map_err(|_| {
            Error::index(
                path,
                format!("{expected_bytes} source bytes do not fit in memory"),
            )
        })?;
        let mut content = Vec::new();
        content
            .try_reserve_exact(capacity)
            .map_err(|error| Error::index(path, format!("content allocation failed: {error}")))?;
        Ok(Self {
            root_index,
            path: path.to_owned(),
            expected_bytes,
            content,
        })
    }

    pub(super) fn push(&mut self, bytes: &[u8]) -> Result<()> {
        self.content.try_reserve(bytes.len()).map_err(|error| {
            Error::index(&self.path, format!("content allocation failed: {error}"))
        })?;
        self.content.extend_from_slice(bytes);
        Ok(())
    }

    pub(super) fn finish(self, status: ContentFileStatus, bytes_read: u64) -> Option<IndexEntry> {
        if status == ContentFileStatus::Changed
            || bytes_read != self.expected_bytes
            || bytes_read != u64::try_from(self.content.len()).unwrap_or(u64::MAX)
        {
            return None;
        }
        Some(IndexEntry::from_parts(
            self.root_index,
            self.path,
            self.content,
        ))
    }
}
