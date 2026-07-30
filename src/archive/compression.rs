use super::containers::{read_limited, search_tar};
use super::detection::strip_suffix_ignore_ascii_case;
use super::{
    Arc, Collector, CompiledQuery, Cursor, Error, QueryCache, Read, Result, SearchIdentity,
    SearchOptions, Write, io, search_complete_bytes,
};

#[cfg(feature = "archives")]
pub(super) fn search_compressed_tar(
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
pub(super) fn search_compressed_file(
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
pub(super) fn search_expanded_file(
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
pub(super) fn decode_lzma(
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
pub(super) fn decode_xz(
    bytes: &[u8],
    output_limit: u64,
    memory_limit: usize,
    path: &str,
) -> Result<Vec<u8>> {
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
pub(super) fn zstd_decoder<'a>(
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
