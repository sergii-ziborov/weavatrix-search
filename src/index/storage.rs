use super::{
    Error, FORMAT_VERSION, File, IndexLock, IndexOptions, IndexReader, IndexWriter, MAGIC, Mutex,
    OpenOptions, PLATFORM_ID, Path, PersistentIndex, Result, auxiliary_path, encode_path, fs, hex,
    read_entries, read_header, read_roots, replace_file, revision_bytes, validate_unique_entries,
};

impl PersistentIndex {
    /// Opens and fully validates a persistent index.
    ///
    /// # Errors
    ///
    /// Returns I/O, format, platform, checksum, allocation, or limit failures.
    pub fn open(path: impl AsRef<Path>, options: IndexOptions) -> Result<Self> {
        let path = path.as_ref();
        let storage_path = std::path::absolute(path).map_err(|source| Error::io(path, source))?;
        options.validate(path)?;
        let metadata = fs::metadata(path).map_err(|source| Error::io(path, source))?;
        if metadata.len() > options.max_index_bytes {
            return Err(Error::index(
                path,
                format!(
                    "file size {} exceeds {} bytes",
                    metadata.len(),
                    options.max_index_bytes
                ),
            ));
        }
        let file = File::open(path).map_err(|source| Error::io(path, source))?;
        let mut reader = IndexReader::new(file, options.max_index_bytes);
        let header = read_header(&mut reader, path, &options)?;
        let roots = read_roots(
            &mut reader,
            header.root_count,
            options.max_path_bytes,
            path,
            "root",
        )?;
        let event_roots = read_roots(
            &mut reader,
            header.root_count,
            options.max_path_bytes,
            path,
            "event-root",
        )?;
        let (entries, observed_content_bytes) =
            read_entries(&mut reader, header.entry_count, &roots, &options, path)?;
        let checksum = reader.finish()?;
        if observed_content_bytes != header.declared_content_bytes {
            return Err(Error::index(
                path,
                "declared content byte count does not match entries",
            ));
        }
        validate_unique_entries(&entries, path)?;
        let computed_revision = revision_bytes(&roots, &entries);
        if computed_revision != header.declared_revision {
            return Err(Error::index(
                path,
                "revision evidence does not match entries",
            ));
        }
        if !checksum.valid {
            return Err(Error::index(path, "checksum mismatch"));
        }
        Ok(Self {
            roots,
            event_roots,
            entries,
            revision: hex(&computed_revision),
            content_bytes: observed_content_bytes,
            options,
            storage_path: Mutex::new(Some(storage_path)),
        })
    }

    /// Atomically saves this index with a whole-file SHA-256 checksum.
    ///
    /// # Errors
    ///
    /// Returns path, lock, size, serialization, or I/O failures.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        self.options.validate(path)?;
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(|source| Error::io(parent, source))?;
        let _lock = IndexLock::acquire(path)?;
        let temp = auxiliary_path(path, "tmp");
        let result = self
            .write_file(&temp)
            .and_then(|()| replace_file(&temp, path));
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result?;
        let storage_path = std::path::absolute(path).map_err(|source| Error::io(path, source))?;
        *self
            .storage_path
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(storage_path);
        Ok(())
    }

    fn write_file(&self, path: &Path) -> Result<()> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|source| Error::io(path, source))?;
        let mut writer = IndexWriter::new(file, self.options.max_index_bytes);
        writer.bytes(MAGIC)?;
        writer.u32(FORMAT_VERSION)?;
        writer.u8(PLATFORM_ID)?;
        writer.u8(0)?;
        writer.u16(0)?;
        writer.u32(
            u32::try_from(self.roots.len())
                .map_err(|_| Error::index(path, "root count exceeds u32"))?,
        )?;
        writer.u64(
            u64::try_from(self.entries.len())
                .map_err(|_| Error::index(path, "entry count exceeds u64"))?,
        )?;
        writer.u64(self.content_bytes)?;
        writer.bytes(&revision_bytes(&self.roots, &self.entries))?;
        for root in &self.roots {
            writer.length_prefixed(&encode_path(root), self.options.max_path_bytes)?;
        }
        for root in &self.event_roots {
            writer.length_prefixed(&encode_path(root), self.options.max_path_bytes)?;
        }
        for entry in &self.entries {
            writer.u32(
                u32::try_from(entry.root_index)
                    .map_err(|_| Error::index(path, "root index exceeds u32"))?,
            )?;
            writer.length_prefixed(entry.path.as_bytes(), self.options.max_path_bytes)?;
            writer.u64(
                u64::try_from(entry.content.len())
                    .map_err(|_| Error::index(path, "file content exceeds u64"))?,
            )?;
            writer.bytes(&entry.content_hash)?;
            writer.u8(u8::from(entry.prefilterable))?;
            for word in entry.bloom {
                writer.u64(word)?;
            }
            writer.bytes(&entry.content)?;
        }
        writer.finish(path)
    }
}
