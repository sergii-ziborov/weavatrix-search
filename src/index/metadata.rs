use super::{
    BLOOM_WORDS, Digest, Error, IndexEntry, IndexStatus, Path, PathBuf, PersistentIndex, Result,
    ScanOptions, Sha256, encode_path,
};

impl PersistentIndex {
    /// Returns immutable index health evidence.
    #[must_use]
    pub fn status(&self) -> IndexStatus {
        IndexStatus {
            roots: self.roots.clone(),
            files: u64::try_from(self.entries.len()).unwrap_or(u64::MAX),
            content_bytes: self.content_bytes,
            revision: self.revision.clone(),
        }
    }

    /// Returns indexed roots in stable insertion order.
    #[must_use]
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    #[cfg(feature = "live")]
    pub(crate) fn event_roots(&self) -> &[PathBuf] {
        &self.event_roots
    }

    pub(super) fn with_storage_exclusion(
        &self,
        scan_options: ScanOptions,
        storage_path: Option<&Path>,
    ) -> Result<ScanOptions> {
        let Some(storage_path) = storage_path else {
            return Ok(scan_options);
        };
        scan_options_with_storage_exclusion(&self.roots, scan_options, storage_path)
    }

    /// Returns the selected-file count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether the index contains no selected files.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns the deterministic root/path/content revision.
    #[must_use]
    pub fn revision(&self) -> &str {
        &self.revision
    }
}

pub(super) fn validate_unique_entries(entries: &[IndexEntry], path: &Path) -> Result<()> {
    for pair in entries.windows(2) {
        if pair[0].key() >= pair[1].key() {
            return Err(Error::index(path, "entries are not strictly ordered"));
        }
    }
    Ok(())
}

pub(super) fn entry_position(
    entries: &[IndexEntry],
    root_index: usize,
    path: &str,
) -> std::result::Result<usize, usize> {
    entries.binary_search_by(|entry| entry.key().cmp(&(root_index, path)))
}

pub(super) fn content_bytes(entries: &[IndexEntry], path: &Path) -> Result<u64> {
    entries.iter().try_fold(0_u64, |total, entry| {
        total
            .checked_add(u64::try_from(entry.content.len()).unwrap_or(u64::MAX))
            .ok_or_else(|| Error::index(path, "content byte count overflow"))
    })
}

pub(super) fn revision(roots: &[PathBuf], entries: &[IndexEntry]) -> String {
    hex(&revision_bytes(roots, entries))
}

pub(super) fn revision_bytes(roots: &[PathBuf], entries: &[IndexEntry]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"weavatrix-search-index-revision-v1");
    for root in roots {
        let encoded = encode_path(root);
        digest.update(
            u64::try_from(encoded.len())
                .unwrap_or(u64::MAX)
                .to_le_bytes(),
        );
        digest.update(encoded);
    }
    for entry in entries {
        digest.update(
            u64::try_from(entry.root_index)
                .unwrap_or(u64::MAX)
                .to_le_bytes(),
        );
        digest.update(
            u64::try_from(entry.path.len())
                .unwrap_or(u64::MAX)
                .to_le_bytes(),
        );
        digest.update(entry.path.as_bytes());
        digest.update(entry.content_hash);
    }
    digest.finalize().into()
}

pub(super) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}

pub(super) fn scan_options_with_storage_exclusion(
    roots: &[PathBuf],
    mut scan_options: ScanOptions,
    storage_path: &Path,
) -> Result<ScanOptions> {
    let storage_path =
        std::path::absolute(storage_path).map_err(|source| Error::io(storage_path, source))?;
    let resolved_storage = storage_path.canonicalize().unwrap_or_else(|_| {
        storage_path.parent().map_or_else(
            || storage_path.clone(),
            |parent| {
                parent
                    .canonicalize()
                    .ok()
                    .and_then(|parent| {
                        storage_path
                            .file_name()
                            .map(|file_name| parent.join(file_name))
                    })
                    .unwrap_or_else(|| storage_path.clone())
            },
        )
    });
    for root in roots {
        let resolved_root = root.canonicalize().unwrap_or_else(|_| root.clone());
        let Ok(relative) = resolved_storage.strip_prefix(&resolved_root) else {
            continue;
        };
        let relative = relative.to_str().ok_or_else(|| {
            Error::index(
                &storage_path,
                "an index inside a root must have a UTF-8 relative path",
            )
        })?;
        let escaped = escape_override_literal(relative);
        scan_options.override_rules.extend([
            format!("!/{escaped}"),
            format!("!/{escaped}.lock"),
            format!("!/{escaped}.tmp.*"),
            format!("!/{escaped}.backup.*"),
        ]);
    }
    Ok(scan_options)
}

pub(super) fn escape_override_literal(path: &str) -> String {
    let mut escaped = String::with_capacity(path.len());
    for character in path.replace('\\', "/").chars() {
        if matches!(character, '\\' | '*' | '?' | '[' | ']' | '{' | '}' | ' ') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

pub(super) fn trigram_bloom(bytes: &[u8]) -> [u64; BLOOM_WORDS] {
    let mut bloom = [0_u64; BLOOM_WORDS];
    for window in bytes.windows(3) {
        let trigram =
            (u32::from(window[0]) << 16) | (u32::from(window[1]) << 8) | u32::from(window[2]);
        let (first, second) = bloom_positions(trigram);
        bloom[first / 64] |= 1_u64 << (first % 64);
        bloom[second / 64] |= 1_u64 << (second % 64);
    }
    bloom
}

pub(super) fn bloom_contains(bloom: &[u64; BLOOM_WORDS], trigram: u32) -> bool {
    let (first, second) = bloom_positions(trigram);
    (bloom[first / 64] & (1_u64 << (first % 64))) != 0
        && (bloom[second / 64] & (1_u64 << (second % 64))) != 0
}

pub(super) fn bloom_positions(trigram: u32) -> (usize, usize) {
    const BITS: u64 = (BLOOM_WORDS * 64) as u64;
    let value = u64::from(trigram);
    let first = value.wrapping_mul(0x9e37_79b1_85eb_ca87) % BITS;
    let second = value.rotate_left(17).wrapping_mul(0xc2b2_ae3d_27d4_eb4f) % BITS;
    (
        usize::try_from(first).unwrap_or(0),
        usize::try_from(second).unwrap_or(0),
    )
}
