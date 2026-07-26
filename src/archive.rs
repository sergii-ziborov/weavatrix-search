use crate::collector::Collector;
#[cfg(feature = "archives")]
use crate::encoding::search_complete_bytes;
#[cfg(feature = "archives")]
use crate::error::Error;
use crate::error::Result;
#[cfg(feature = "archives")]
use crate::line_search::SearchIdentity;
use crate::options::SearchOptions;
use crate::query::{CompiledQuery, QueryCache};
use crate::report::{SearchWarning, SearchWarningKind};
#[cfg(feature = "archives")]
use std::io::{self, Cursor, Read, Write};
#[cfg(feature = "archives")]
use std::path::Component;
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArchiveKind {
    Zip,
    Tar,
    TarGzip,
    Gzip,
    TarBzip2,
    Bzip2,
    TarZstd,
    Zstd,
    TarLz4,
    Lz4,
    TarLzma,
    Lzma,
    TarXz,
    Xz,
    TarBrotli,
    Brotli,
}

pub(crate) fn kind(path: &str) -> Option<ArchiveKind> {
    let path = Path::new(path);
    let path = path.to_string_lossy();
    if has_suffix(&path, &[".tgz", ".tar.gz"]) {
        return Some(ArchiveKind::TarGzip);
    }
    if has_suffix(&path, &[".tbz", ".tbz2", ".tar.bz2", ".tar.bz"]) {
        return Some(ArchiveKind::TarBzip2);
    }
    if has_suffix(&path, &[".tzst", ".tar.zst", ".tar.zstd"]) {
        return Some(ArchiveKind::TarZstd);
    }
    if has_suffix(&path, &[".tar.lz4"]) {
        return Some(ArchiveKind::TarLz4);
    }
    if has_suffix(&path, &[".tlz", ".tar.lzma"]) {
        return Some(ArchiveKind::TarLzma);
    }
    if has_suffix(&path, &[".txz", ".tar.xz"]) {
        return Some(ArchiveKind::TarXz);
    }
    if has_suffix(&path, &[".tar.br"]) {
        return Some(ArchiveKind::TarBrotli);
    }
    if has_suffix(&path, &[".zip"]) {
        return Some(ArchiveKind::Zip);
    }
    if has_suffix(&path, &[".tar"]) {
        return Some(ArchiveKind::Tar);
    }
    if has_suffix(&path, &[".gz"]) {
        return Some(ArchiveKind::Gzip);
    }
    if has_suffix(&path, &[".bz2", ".bz"]) {
        return Some(ArchiveKind::Bzip2);
    }
    if has_suffix(&path, &[".zst", ".zstd"]) {
        return Some(ArchiveKind::Zstd);
    }
    if has_suffix(&path, &[".lz4"]) {
        return Some(ArchiveKind::Lz4);
    }
    if has_suffix(&path, &[".lzma"]) {
        return Some(ArchiveKind::Lzma);
    }
    if has_suffix(&path, &[".xz"]) {
        return Some(ArchiveKind::Xz);
    }
    if has_suffix(&path, &[".br"]) {
        return Some(ArchiveKind::Brotli);
    }
    None
}

#[cfg_attr(not(feature = "archives"), allow(clippy::unnecessary_wraps))]
#[allow(clippy::too_many_lines)]
pub(crate) fn search(
    kind: ArchiveKind,
    source: (usize, &str),
    bytes: &[u8],
    query: &Arc<CompiledQuery>,
    query_cache: &mut QueryCache,
    options: &Arc<SearchOptions>,
    collector: &Arc<Collector>,
) -> Result<()> {
    let (root_index, outer_path) = source;
    #[cfg(feature = "archives")]
    {
        match kind {
            ArchiveKind::Zip => search_zip(
                root_index,
                outer_path,
                bytes,
                query,
                query_cache,
                options,
                collector,
            ),
            ArchiveKind::Tar => search_tar(
                root_index,
                outer_path,
                Cursor::new(bytes),
                query,
                query_cache,
                options,
                collector,
            ),
            ArchiveKind::TarGzip => {
                let decoder = flate2::read::GzDecoder::new(Cursor::new(bytes));
                search_compressed_tar(
                    root_index,
                    outer_path,
                    decoder,
                    query,
                    query_cache,
                    options,
                    collector,
                )
            }
            ArchiveKind::Gzip => {
                let decoder = flate2::read::GzDecoder::new(Cursor::new(bytes));
                search_compressed_file(
                    (root_index, outer_path),
                    decoder,
                    &[".gz"],
                    query,
                    query_cache,
                    options,
                    collector,
                )
            }
            ArchiveKind::TarBzip2 => search_compressed_tar(
                root_index,
                outer_path,
                bzip2_rs::DecoderReader::new(Cursor::new(bytes)),
                query,
                query_cache,
                options,
                collector,
            ),
            ArchiveKind::Bzip2 => search_compressed_file(
                (root_index, outer_path),
                bzip2_rs::DecoderReader::new(Cursor::new(bytes)),
                &[".bz2", ".bz"],
                query,
                query_cache,
                options,
                collector,
            ),
            ArchiveKind::TarZstd => search_compressed_tar(
                root_index,
                outer_path,
                zstd_decoder(bytes, outer_path)?,
                query,
                query_cache,
                options,
                collector,
            ),
            ArchiveKind::Zstd => search_compressed_file(
                (root_index, outer_path),
                zstd_decoder(bytes, outer_path)?,
                &[".zstd", ".zst"],
                query,
                query_cache,
                options,
                collector,
            ),
            ArchiveKind::TarLz4 => search_compressed_tar(
                root_index,
                outer_path,
                lz4_flex::frame::FrameDecoder::new(Cursor::new(bytes)),
                query,
                query_cache,
                options,
                collector,
            ),
            ArchiveKind::Lz4 => search_compressed_file(
                (root_index, outer_path),
                lz4_flex::frame::FrameDecoder::new(Cursor::new(bytes)),
                &[".lz4"],
                query,
                query_cache,
                options,
                collector,
            ),
            ArchiveKind::TarLzma => {
                let expanded = decode_lzma(
                    bytes,
                    options.archives.max_expanded_bytes,
                    options.archives.max_decoder_memory_bytes,
                    outer_path,
                )?;
                search_tar(
                    root_index,
                    outer_path,
                    Cursor::new(expanded),
                    query,
                    query_cache,
                    options,
                    collector,
                )
            }
            ArchiveKind::Lzma => {
                let expanded = decode_lzma(
                    bytes,
                    options.archives.max_entry_bytes,
                    options.archives.max_decoder_memory_bytes,
                    outer_path,
                )?;
                search_expanded_file(
                    (root_index, outer_path),
                    &expanded,
                    &[".lzma"],
                    query,
                    query_cache,
                    options,
                    collector,
                )
            }
            ArchiveKind::TarXz => {
                let expanded = decode_xz(
                    bytes,
                    options.archives.max_expanded_bytes,
                    options.archives.max_decoder_memory_bytes,
                    outer_path,
                )?;
                search_tar(
                    root_index,
                    outer_path,
                    Cursor::new(expanded),
                    query,
                    query_cache,
                    options,
                    collector,
                )
            }
            ArchiveKind::Xz => {
                let expanded = decode_xz(
                    bytes,
                    options.archives.max_entry_bytes,
                    options.archives.max_decoder_memory_bytes,
                    outer_path,
                )?;
                search_expanded_file(
                    (root_index, outer_path),
                    &expanded,
                    &[".xz"],
                    query,
                    query_cache,
                    options,
                    collector,
                )
            }
            ArchiveKind::TarBrotli => search_compressed_tar(
                root_index,
                outer_path,
                brotli_decompressor::Decompressor::new(Cursor::new(bytes), 8 * 1024),
                query,
                query_cache,
                options,
                collector,
            ),
            ArchiveKind::Brotli => search_compressed_file(
                (root_index, outer_path),
                brotli_decompressor::Decompressor::new(Cursor::new(bytes), 8 * 1024),
                &[".br"],
                query,
                query_cache,
                options,
                collector,
            ),
        }
    }
    #[cfg(not(feature = "archives"))]
    {
        let _ = (kind, root_index, bytes, query, query_cache, options);
        collector.warn(SearchWarning {
            path: outer_path.to_owned(),
            kind: SearchWarningKind::Archive,
            message: "archive support is disabled at compile time".to_owned(),
        });
        Ok(())
    }
}

#[cfg(feature = "archives")]
fn search_compressed_tar(
    root_index: usize,
    outer_path: &str,
    decoder: impl Read,
    query: &Arc<CompiledQuery>,
    query_cache: &mut QueryCache,
    options: &Arc<SearchOptions>,
    collector: &Arc<Collector>,
) -> Result<()> {
    let expanded = read_limited(decoder, options.archives.max_expanded_bytes, outer_path)?;
    search_tar(
        root_index,
        outer_path,
        Cursor::new(expanded),
        query,
        query_cache,
        options,
        collector,
    )
}

#[cfg(feature = "archives")]
fn search_compressed_file(
    source: (usize, &str),
    decoder: impl Read,
    suffixes: &[&str],
    query: &Arc<CompiledQuery>,
    query_cache: &mut QueryCache,
    options: &Arc<SearchOptions>,
    collector: &Arc<Collector>,
) -> Result<()> {
    let (root_index, outer_path) = source;
    let expanded = read_limited(decoder, options.archives.max_entry_bytes, outer_path)?;
    search_expanded_file(
        (root_index, outer_path),
        &expanded,
        suffixes,
        query,
        query_cache,
        options,
        collector,
    )
}

#[cfg(feature = "archives")]
fn search_expanded_file(
    source: (usize, &str),
    expanded: &[u8],
    suffixes: &[&str],
    query: &Arc<CompiledQuery>,
    query_cache: &mut QueryCache,
    options: &Arc<SearchOptions>,
    collector: &Arc<Collector>,
) -> Result<()> {
    let (root_index, outer_path) = source;
    let inner = strip_suffix_ignore_ascii_case(outer_path, suffixes).unwrap_or("content");
    search_complete_bytes(
        SearchIdentity {
            root_index,
            path: format!("{outer_path}!{inner}"),
            source_bytes: u64::try_from(expanded.len()).unwrap_or(u64::MAX),
            encoding: String::new().into(),
            archive: true,
            source_offset_base: Some(0),
            lossy: false,
        },
        expanded,
        Arc::clone(query),
        query_cache,
        Arc::clone(options),
        Arc::clone(collector),
    )
}

#[cfg(feature = "archives")]
fn decode_lzma(
    bytes: &[u8],
    output_limit: u64,
    memory_limit: usize,
    path: &str,
) -> Result<Vec<u8>> {
    let output_limit = usize::try_from(output_limit).unwrap_or(usize::MAX);
    let mut input = Cursor::new(bytes);
    let mut output = LimitedWriter::new(output_limit);
    let options = lzma_rs::decompress::Options {
        memlimit: Some(memory_limit),
        ..lzma_rs::decompress::Options::default()
    };
    lzma_rs::lzma_decompress_with_options(&mut input, &mut output, &options)
        .map_err(|error| Error::archive(path, error.to_string()))?;
    Ok(output.into_inner())
}

#[cfg(feature = "archives")]
fn decode_xz(bytes: &[u8], output_limit: u64, memory_limit: usize, path: &str) -> Result<Vec<u8>> {
    use xz4rust::{DICT_SIZE_MIN, XzDecoder, XzNextBlockResult};

    if memory_limit < DICT_SIZE_MIN {
        return Err(Error::limit(
            path,
            format!("XZ needs at least {DICT_SIZE_MIN} decoder bytes; configured {memory_limit}"),
        ));
    }
    let output_limit = usize::try_from(output_limit).unwrap_or(usize::MAX);
    let mut decoder = XzDecoder::in_heap_with_alloc_dict_size(DICT_SIZE_MIN, memory_limit);
    let mut output = Vec::with_capacity(output_limit.min(64 * 1024));
    let mut buffer = [0_u8; 16 * 1024];
    let mut input_position = 0_usize;

    loop {
        let result = decoder
            .decode(&bytes[input_position..], &mut buffer)
            .map_err(|error| Error::archive(path, error.to_string()))?;
        let (consumed, produced, end_of_stream) = match result {
            XzNextBlockResult::NeedMoreData(consumed, produced) => (consumed, produced, false),
            XzNextBlockResult::EndOfStream(consumed, produced) => (consumed, produced, true),
        };
        input_position = input_position
            .checked_add(consumed)
            .ok_or_else(|| Error::archive(path, "XZ input position overflow"))?;
        if produced > output_limit.saturating_sub(output.len()) {
            return Err(Error::limit(
                path,
                format!("expanded XZ content exceeds {output_limit} bytes"),
            ));
        }
        output.extend_from_slice(&buffer[..produced]);

        if !end_of_stream {
            if consumed == 0 && produced == 0 {
                return Err(Error::archive(path, "XZ decoder made no progress"));
            }
            if input_position >= bytes.len() {
                return Err(Error::archive(path, "truncated XZ stream"));
            }
            continue;
        }

        let padding_start = input_position;
        while bytes.get(input_position) == Some(&0) {
            input_position += 1;
        }
        let padding = input_position - padding_start;
        if !padding.is_multiple_of(4) {
            return Err(Error::archive(
                path,
                "XZ stream padding is not a multiple of four bytes",
            ));
        }
        if input_position == bytes.len() {
            return Ok(output);
        }
        if !bytes[input_position..].starts_with(&[0xFD, b'7', b'z', b'X', b'Z', 0]) {
            return Err(Error::archive(path, "unexpected data after XZ stream"));
        }
        decoder.reset();
    }
}

#[cfg(feature = "archives")]
struct LimitedWriter {
    content: Vec<u8>,
    limit: usize,
}

#[cfg(feature = "archives")]
impl LimitedWriter {
    fn new(limit: usize) -> Self {
        Self {
            content: Vec::new(),
            limit,
        }
    }

    fn into_inner(self) -> Vec<u8> {
        self.content
    }
}

#[cfg(feature = "archives")]
impl Write for LimitedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.content.len().saturating_add(bytes.len()) > self.limit {
            return Err(io::Error::other(format!(
                "expanded content exceeds the {} byte limit",
                self.limit
            )));
        }
        self.content.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(feature = "archives")]
fn zstd_decoder<'a>(
    bytes: &'a [u8],
    path: &str,
) -> Result<ruzstd::decoding::StreamingDecoder<Cursor<&'a [u8]>, ruzstd::decoding::FrameDecoder>> {
    // Prime the reusable state with a tiny frame so ruzstd's reset path
    // enforces its 100 MiB malformed-frame window ceiling before allocation.
    const PRIME: [u8; 10] = [40, 181, 47, 253, 0, 56, 9, 0, 0, 0];
    let mut decoder = ruzstd::decoding::FrameDecoder::new();
    decoder
        .init(Cursor::new(PRIME))
        .map_err(|error| Error::archive(path, error.to_string()))?;
    ruzstd::decoding::StreamingDecoder::new_with_decoder(Cursor::new(bytes), decoder)
        .map_err(|error| Error::archive(path, error.to_string()))
}

fn strip_suffix_ignore_ascii_case<'a>(path: &'a str, suffixes: &[&str]) -> Option<&'a str> {
    suffixes.iter().find_map(|suffix| {
        path.get(path.len().checked_sub(suffix.len())?..)
            .is_some_and(|tail| tail.eq_ignore_ascii_case(suffix))
            .then(|| &path[..path.len() - suffix.len()])
    })
}

fn has_suffix(path: &str, suffixes: &[&str]) -> bool {
    strip_suffix_ignore_ascii_case(path, suffixes).is_some()
}

#[cfg(feature = "archives")]
fn search_zip(
    root_index: usize,
    outer_path: &str,
    bytes: &[u8],
    query: &Arc<CompiledQuery>,
    query_cache: &mut QueryCache,
    options: &Arc<SearchOptions>,
    collector: &Arc<Collector>,
) -> Result<()> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| Error::archive(outer_path, error.to_string()))?;
    let entries = archive.len().min(options.archives.max_entries);
    if archive.len() > options.archives.max_entries {
        warn_limit(
            collector,
            outer_path,
            format!(
                "archive has {} entries; only the first {} are searched",
                archive.len(),
                options.archives.max_entries
            ),
        );
    }
    let mut expanded = 0_u64;
    for index in 0..entries {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| Error::archive(outer_path, error.to_string()))?;
        if entry.is_dir() {
            continue;
        }
        let Some(path) = entry.enclosed_name().as_deref().and_then(safe_virtual_path) else {
            collector.warn(SearchWarning {
                path: outer_path.to_owned(),
                kind: SearchWarningKind::Archive,
                message: format!("unsafe ZIP member path was skipped: {}", entry.name()),
            });
            continue;
        };
        let size = entry.size();
        if size > options.archives.max_entry_bytes
            || expanded.saturating_add(size) > options.archives.max_expanded_bytes
        {
            warn_limit(
                collector,
                &format!("{outer_path}!{path}"),
                "archive member exceeds configured expansion limits".to_owned(),
            );
            continue;
        }
        let content = read_limited(
            &mut entry,
            options.archives.max_entry_bytes,
            &format!("{outer_path}!{path}"),
        )?;
        let next_expanded =
            expanded.saturating_add(u64::try_from(content.len()).unwrap_or(u64::MAX));
        if next_expanded > options.archives.max_expanded_bytes {
            warn_limit(
                collector,
                &format!("{outer_path}!{path}"),
                "archive member exceeds configured expansion limits".to_owned(),
            );
            continue;
        }
        expanded = next_expanded;
        search_complete_bytes(
            SearchIdentity {
                root_index,
                path: format!("{outer_path}!{path}"),
                source_bytes: u64::try_from(content.len()).unwrap_or(u64::MAX),
                encoding: String::new().into(),
                archive: true,
                source_offset_base: Some(0),
                lossy: false,
            },
            &content,
            Arc::clone(query),
            query_cache,
            Arc::clone(options),
            Arc::clone(collector),
        )?;
    }
    Ok(())
}

#[cfg(feature = "archives")]
fn search_tar<R: Read>(
    root_index: usize,
    outer_path: &str,
    reader: R,
    query: &Arc<CompiledQuery>,
    query_cache: &mut QueryCache,
    options: &Arc<SearchOptions>,
    collector: &Arc<Collector>,
) -> Result<()> {
    let mut archive = tar::Archive::new(reader);
    let entries = archive
        .entries()
        .map_err(|error| Error::archive(outer_path, error.to_string()))?;
    let mut expanded = 0_u64;
    for (index, entry) in entries.enumerate() {
        if index >= options.archives.max_entries {
            warn_limit(
                collector,
                outer_path,
                format!(
                    "archive entry count exceeds {}; remaining members were skipped",
                    options.archives.max_entries
                ),
            );
            break;
        }
        let mut entry = entry.map_err(|error| Error::archive(outer_path, error.to_string()))?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let member = entry
            .path()
            .map_err(|error| Error::archive(outer_path, error.to_string()))?;
        let Some(path) = safe_virtual_path(&member) else {
            collector.warn(SearchWarning {
                path: outer_path.to_owned(),
                kind: SearchWarningKind::Archive,
                message: format!(
                    "unsafe TAR member path was skipped: {}",
                    member.to_string_lossy()
                ),
            });
            continue;
        };
        let size = entry.size();
        if size > options.archives.max_entry_bytes
            || expanded.saturating_add(size) > options.archives.max_expanded_bytes
        {
            warn_limit(
                collector,
                &format!("{outer_path}!{path}"),
                "archive member exceeds configured expansion limits".to_owned(),
            );
            continue;
        }
        let content = read_limited(
            &mut entry,
            options.archives.max_entry_bytes,
            &format!("{outer_path}!{path}"),
        )?;
        let next_expanded =
            expanded.saturating_add(u64::try_from(content.len()).unwrap_or(u64::MAX));
        if next_expanded > options.archives.max_expanded_bytes {
            warn_limit(
                collector,
                &format!("{outer_path}!{path}"),
                "archive member exceeds configured expansion limits".to_owned(),
            );
            continue;
        }
        expanded = next_expanded;
        search_complete_bytes(
            SearchIdentity {
                root_index,
                path: format!("{outer_path}!{path}"),
                source_bytes: u64::try_from(content.len()).unwrap_or(u64::MAX),
                encoding: String::new().into(),
                archive: true,
                source_offset_base: Some(0),
                lossy: false,
            },
            &content,
            Arc::clone(query),
            query_cache,
            Arc::clone(options),
            Arc::clone(collector),
        )?;
    }
    Ok(())
}

#[cfg(feature = "archives")]
fn read_limited(reader: impl Read, limit: u64, path: &str) -> Result<Vec<u8>> {
    let mut content = Vec::new();
    reader
        .take(limit.saturating_add(1))
        .read_to_end(&mut content)
        .map_err(|error| Error::io(path, error))?;
    if u64::try_from(content.len()).unwrap_or(u64::MAX) > limit {
        return Err(Error::archive(
            path,
            format!("expanded content exceeds the {limit} byte limit"),
        ));
    }
    Ok(content)
}

#[cfg(feature = "archives")]
fn safe_virtual_path(path: &Path) -> Option<String> {
    let mut normalized = String::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => {
                if !normalized.is_empty() {
                    normalized.push('/');
                }
                normalized.push_str(&value.to_string_lossy());
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!normalized.is_empty()).then_some(normalized)
}

#[cfg(feature = "archives")]
fn warn_limit(collector: &Collector, path: &str, message: String) {
    collector.warn(SearchWarning {
        path: path.to_owned(),
        kind: SearchWarningKind::Limit,
        message,
    });
}
