use super::{
    Arc, ArchiveKind, BinaryPolicy, Collector, CompiledQuery, EncodingMode, Error, LineSearcher,
    QueryCache, Result, SearchErrorPolicy, SearchIdentity, SearchMode, SearchOptions,
    SearchWarning, SearchWarningKind, archive, auto_is_utf16, is_streaming_utf8,
    search_complete_bytes, utf8_bom_len,
};

pub(super) fn scanner_file_limit(options: &SearchOptions) -> u64 {
    if options.archives.enabled {
        options
            .max_file_bytes
            .max(options.archives.max_archive_bytes)
    } else {
        options.max_file_bytes
    }
}

// Keeping the streaming state inline avoids one heap allocation for every
// ordinary file, which matters more than stack size on 200k-file repositories.
#[allow(clippy::large_enum_variant)]
enum FileMode {
    Undecided(Vec<u8>),
    Streaming(LineSearcher),
    Buffered(Vec<u8>),
    Skipped,
}

pub(super) struct FileProcessor {
    identity: Option<SearchIdentity>,
    expected_bytes: u64,
    archive: Option<ArchiveKind>,
    query: Arc<CompiledQuery>,
    options: Arc<SearchOptions>,
    collector: Arc<Collector>,
    mode: FileMode,
    inspected_binary_bytes: usize,
}

impl FileProcessor {
    pub(super) fn new(
        identity: SearchIdentity,
        expected_bytes: u64,
        query: Arc<CompiledQuery>,
        options: Arc<SearchOptions>,
        collector: Arc<Collector>,
    ) -> Self {
        let mut identity = Some(identity);
        let archive = options
            .archives
            .enabled
            .then(|| archive::kind(&identity.as_ref().expect("identity exists").path))
            .flatten();
        let mode = if archive.is_some() {
            if expected_bytes > options.archives.max_archive_bytes {
                collector.warn(SearchWarning {
                    path: identity.as_ref().expect("identity exists").path.clone(),
                    kind: SearchWarningKind::Limit,
                    message: format!(
                        "archive is {expected_bytes} bytes; limit is {}",
                        options.archives.max_archive_bytes
                    ),
                });
                FileMode::Skipped
            } else {
                FileMode::Buffered(Vec::with_capacity(
                    usize::try_from(expected_bytes).unwrap_or(0),
                ))
            }
        } else if options.mode == SearchMode::Multiline {
            if expected_bytes > options.max_multiline_bytes {
                collector.warn(SearchWarning {
                    path: identity.as_ref().expect("identity exists").path.clone(),
                    kind: SearchWarningKind::Limit,
                    message: format!(
                        "multiline source is {expected_bytes} bytes; limit is {}",
                        options.max_multiline_bytes
                    ),
                });
                FileMode::Skipped
            } else {
                FileMode::Buffered(Vec::with_capacity(
                    usize::try_from(expected_bytes).unwrap_or(0),
                ))
            }
        } else {
            match is_streaming_utf8(&options.encoding) {
                Ok(true) if options.encoding == EncodingMode::Auto => {
                    FileMode::Undecided(Vec::new())
                }
                Ok(true) => FileMode::Streaming(LineSearcher::new(
                    Arc::clone(&query),
                    Arc::clone(&options),
                    Arc::clone(&collector),
                    identity.take().expect("identity exists"),
                )),
                Ok(false) => FileMode::Buffered(Vec::with_capacity(
                    usize::try_from(expected_bytes).unwrap_or(0),
                )),
                Err(error) => {
                    handle_error(&collector, &options, error);
                    FileMode::Skipped
                }
            }
        };
        Self {
            identity,
            expected_bytes,
            archive,
            query,
            options,
            collector,
            mode,
            inspected_binary_bytes: 0,
        }
    }

    pub(super) fn push(&mut self, bytes: &[u8], query_cache: &mut QueryCache) -> Result<()> {
        let mode = std::mem::replace(&mut self.mode, FileMode::Skipped);
        self.mode = match mode {
            FileMode::Undecided(mut prefix) => {
                if prefix.is_empty() && bytes.len() >= 3 {
                    self.start_auto(bytes, query_cache)
                } else {
                    prefix.extend_from_slice(bytes);
                    if prefix.len() < 3
                        && u64::try_from(prefix.len()).unwrap_or(u64::MAX) < self.expected_bytes
                    {
                        FileMode::Undecided(prefix)
                    } else {
                        self.start_auto(&prefix, query_cache)
                    }
                }
            }
            FileMode::Streaming(mut lines) => {
                if binary_skip(
                    &self.options,
                    &self.collector,
                    &mut self.inspected_binary_bytes,
                    lines.path(),
                    bytes,
                ) {
                    FileMode::Skipped
                } else {
                    lines.push(bytes, query_cache);
                    FileMode::Streaming(lines)
                }
            }
            FileMode::Buffered(mut buffer) => {
                let limit = if self.archive.is_some() {
                    self.options.archives.max_archive_bytes
                } else if self.options.mode == SearchMode::Multiline {
                    self.options.max_multiline_bytes
                } else {
                    self.expected_bytes.max(1)
                };
                if u64::try_from(buffer.len().saturating_add(bytes.len())).unwrap_or(u64::MAX)
                    > limit
                {
                    let message = format!("buffered input exceeds the {limit} byte limit");
                    return Err(if self.archive.is_some() {
                        Error::archive(
                            &self.identity.as_ref().expect("identity exists").path,
                            message,
                        )
                    } else {
                        Error::limit(
                            &self.identity.as_ref().expect("identity exists").path,
                            message,
                        )
                    });
                }
                buffer.extend_from_slice(bytes);
                FileMode::Buffered(buffer)
            }
            FileMode::Skipped => FileMode::Skipped,
        };
        Ok(())
    }

    fn start_auto(&mut self, bytes: &[u8], query_cache: &mut QueryCache) -> FileMode {
        if auto_is_utf16(bytes) {
            FileMode::Buffered(bytes.to_vec())
        } else {
            let skip = utf8_bom_len(bytes);
            if binary_skip(
                &self.options,
                &self.collector,
                &mut self.inspected_binary_bytes,
                &self.identity.as_ref().expect("identity exists").path,
                &bytes[skip..],
            ) {
                return FileMode::Skipped;
            }
            let mut identity = self.identity.take().expect("identity exists");
            identity.source_offset_base =
                Some(u64::try_from(skip).expect("UTF-8 BOM length fits in u64"));
            let mut lines = LineSearcher::new(
                Arc::clone(&self.query),
                Arc::clone(&self.options),
                Arc::clone(&self.collector),
                identity,
            );
            lines.push(&bytes[skip..], query_cache);
            FileMode::Streaming(lines)
        }
    }

    pub(super) fn finish(self, query_cache: &mut QueryCache) -> Result<()> {
        match self.mode {
            FileMode::Undecided(bytes) => search_complete_bytes(
                self.identity.expect("identity exists"),
                &bytes,
                self.query,
                query_cache,
                self.options,
                self.collector,
            ),
            FileMode::Streaming(lines) => {
                lines.finish(query_cache);
                Ok(())
            }
            FileMode::Buffered(bytes) => {
                if let Some(kind) = self.archive {
                    archive::search(
                        kind,
                        (
                            self.identity.as_ref().expect("identity exists").root_index,
                            &self.identity.as_ref().expect("identity exists").path,
                        ),
                        &bytes,
                        &self.query,
                        query_cache,
                        &self.options,
                        &self.collector,
                    )
                } else {
                    search_complete_bytes(
                        self.identity.expect("identity exists"),
                        &bytes,
                        self.query,
                        query_cache,
                        self.options,
                        self.collector,
                    )
                }
            }
            FileMode::Skipped => Ok(()),
        }
    }
}

fn binary_skip(
    options: &SearchOptions,
    collector: &Collector,
    inspected_binary_bytes: &mut usize,
    path: &str,
    bytes: &[u8],
) -> bool {
    if options.binary == BinaryPolicy::Search || *inspected_binary_bytes >= 8 * 1024 {
        return false;
    }
    let remaining = 8 * 1024 - *inspected_binary_bytes;
    let inspected = &bytes[..bytes.len().min(remaining)];
    *inspected_binary_bytes += inspected.len();
    if memchr::memchr(0, inspected).is_some() {
        collector.warn(SearchWarning {
            path: path.to_owned(),
            kind: SearchWarningKind::Binary,
            message: "binary file skipped after NUL-byte detection".to_owned(),
        });
        true
    } else {
        false
    }
}

pub(super) fn handle_error(collector: &Collector, options: &SearchOptions, error: Error) {
    if options.error_policy == SearchErrorPolicy::Abort {
        collector.fail(error);
    } else {
        let (path, kind) = match &error {
            Error::Archive { path, .. } => (path.clone(), SearchWarningKind::Archive),
            Error::Limit { path, .. } => (path.clone(), SearchWarningKind::Limit),
            Error::InvalidEncoding(label) => (label.clone(), SearchWarningKind::Encoding),
            Error::Io { path, .. } => (
                path.to_string_lossy().into_owned(),
                SearchWarningKind::Archive,
            ),
            Error::EmptyQuery | Error::Regex(_) | Error::Scan(_) | Error::Index { .. } => {
                ("<search>".to_owned(), SearchWarningKind::Archive)
            }
        };
        collector.warn(SearchWarning {
            path,
            kind,
            message: error.to_string(),
        });
    }
}
