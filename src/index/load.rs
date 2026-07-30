use super::{
    BLOOM_WORDS, Error, FORMAT_VERSION, IndexEntry, IndexOptions, IndexReader, MAGIC, MAX_ROOTS,
    PLATFORM_ID, Path, PathBuf, Result, decode_path,
};

pub(super) struct IndexHeader {
    pub(super) root_count: usize,
    pub(super) entry_count: u64,
    pub(super) declared_content_bytes: u64,
    pub(super) declared_revision: [u8; 32],
}

pub(super) fn read_header(
    reader: &mut IndexReader,
    path: &Path,
    options: &IndexOptions,
) -> Result<IndexHeader> {
    if reader.bytes(8)? != MAGIC {
        return Err(Error::index(path, "invalid magic"));
    }
    let version = reader.u32()?;
    if version != FORMAT_VERSION {
        return Err(Error::index(
            path,
            format!("format version {version} is not supported"),
        ));
    }
    let platform = reader.u8()?;
    if platform != PLATFORM_ID {
        return Err(Error::index(
            path,
            format!("index platform {platform} does not match runtime {PLATFORM_ID}"),
        ));
    }
    let _flags = reader.u8()?;
    let _reserved = reader.u16()?;
    let root_count = usize::try_from(reader.u32()?)
        .map_err(|_| Error::index(path, "root count does not fit usize"))?;
    if root_count == 0 || root_count > MAX_ROOTS {
        return Err(Error::index(
            path,
            format!("invalid root count {root_count}"),
        ));
    }
    let entry_count = reader.u64()?;
    if entry_count > options.max_entries {
        return Err(Error::index(
            path,
            format!("entry count {entry_count} exceeds {}", options.max_entries),
        ));
    }
    let declared_content_bytes = reader.u64()?;
    if declared_content_bytes > options.max_content_bytes {
        return Err(Error::index(
            path,
            format!(
                "content bytes {declared_content_bytes} exceed {}",
                options.max_content_bytes
            ),
        ));
    }
    let declared_revision = reader.array::<32>()?;
    Ok(IndexHeader {
        root_count,
        entry_count,
        declared_content_bytes,
        declared_revision,
    })
}

pub(super) fn read_roots(
    reader: &mut IndexReader,
    count: usize,
    max_path_bytes: usize,
    path: &Path,
    allocation_label: &str,
) -> Result<Vec<PathBuf>> {
    let mut roots = Vec::new();
    roots.try_reserve_exact(count).map_err(|error| {
        Error::index(
            path,
            format!("{allocation_label} allocation failed: {error}"),
        )
    })?;
    for _ in 0..count {
        let encoded = reader.length_prefixed(max_path_bytes)?;
        roots.push(decode_path(&encoded).map_err(|message| Error::index(path, message))?);
    }
    Ok(roots)
}

pub(super) fn read_entries(
    reader: &mut IndexReader,
    entry_count: u64,
    roots: &[PathBuf],
    options: &IndexOptions,
    path: &Path,
) -> Result<(Vec<IndexEntry>, u64)> {
    let entry_capacity = usize::try_from(entry_count)
        .map_err(|_| Error::index(path, "entry count does not fit usize"))?;
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(entry_capacity)
        .map_err(|error| Error::index(path, format!("entry allocation failed: {error}")))?;
    let mut observed_content_bytes = 0_u64;
    for _ in 0..entry_count {
        let root_index = usize::try_from(reader.u32()?)
            .map_err(|_| Error::index(path, "root index does not fit usize"))?;
        if root_index >= roots.len() {
            return Err(Error::index(path, "entry root index is out of range"));
        }
        let encoded_path = reader.length_prefixed(options.max_path_bytes)?;
        let relative = String::from_utf8(encoded_path)
            .map_err(|_| Error::index(path, "relative path is not UTF-8"))?;
        let content_len = reader.u64()?;
        observed_content_bytes = observed_content_bytes
            .checked_add(content_len)
            .ok_or_else(|| Error::index(path, "content byte count overflow"))?;
        if observed_content_bytes > options.max_content_bytes {
            return Err(Error::index(
                path,
                format!("content exceeds {} bytes", options.max_content_bytes),
            ));
        }
        let content_hash = reader.array::<32>()?;
        let prefilterable = match reader.u8()? {
            0 => false,
            1 => true,
            value => {
                return Err(Error::index(
                    path,
                    format!("invalid prefilter flag {value}"),
                ));
            }
        };
        let mut bloom = [0_u64; BLOOM_WORDS];
        for word in &mut bloom {
            *word = reader.u64()?;
        }
        let content_size = usize::try_from(content_len)
            .map_err(|_| Error::index(path, "file content does not fit usize"))?;
        let content = reader.bytes(content_size)?;
        entries.push(IndexEntry {
            root_index,
            path: relative,
            content,
            content_hash,
            prefilterable,
            bloom,
        });
    }
    Ok((entries, observed_content_bytes))
}
