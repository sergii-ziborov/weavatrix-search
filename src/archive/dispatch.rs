use super::{
    Arc, ArchiveKind, Collector, CompiledQuery, Cursor, QueryCache, Read, Result, SearchOptions,
    decode_lzma, decode_xz, search_compressed_file, search_compressed_tar, search_expanded_file,
    search_tar, search_zip, zstd_decoder,
};

pub(super) struct ArchiveSearch<'a> {
    source: (usize, &'a str),
    bytes: &'a [u8],
    query: &'a Arc<CompiledQuery>,
    query_cache: &'a mut QueryCache,
    options: &'a Arc<SearchOptions>,
    collector: &'a Arc<Collector>,
}

impl<'a> ArchiveSearch<'a> {
    pub(super) fn new(
        source: (usize, &'a str),
        bytes: &'a [u8],
        query: &'a Arc<CompiledQuery>,
        query_cache: &'a mut QueryCache,
        options: &'a Arc<SearchOptions>,
        collector: &'a Arc<Collector>,
    ) -> Self {
        Self {
            source,
            bytes,
            query,
            query_cache,
            options,
            collector,
        }
    }

    pub(super) fn search(&mut self, kind: ArchiveKind) -> Result<()> {
        match kind {
            ArchiveKind::Zip => self.zip(),
            ArchiveKind::Tar => self.tar(Cursor::new(self.bytes)),
            ArchiveKind::TarGzip => {
                self.compressed_tar(flate2::read::GzDecoder::new(Cursor::new(self.bytes)))
            }
            ArchiveKind::Gzip => self.compressed_file(
                flate2::read::GzDecoder::new(Cursor::new(self.bytes)),
                &[".gz"],
            ),
            ArchiveKind::TarBzip2 => {
                self.compressed_tar(bzip2_rs::DecoderReader::new(Cursor::new(self.bytes)))
            }
            ArchiveKind::Bzip2 => self.compressed_file(
                bzip2_rs::DecoderReader::new(Cursor::new(self.bytes)),
                &[".bz2", ".bz"],
            ),
            ArchiveKind::TarZstd => self.compressed_tar(zstd_decoder(self.bytes, self.source.1)?),
            ArchiveKind::Zstd => {
                self.compressed_file(zstd_decoder(self.bytes, self.source.1)?, &[".zstd", ".zst"])
            }
            ArchiveKind::TarLz4 => {
                self.compressed_tar(lz4_flex::frame::FrameDecoder::new(Cursor::new(self.bytes)))
            }
            ArchiveKind::Lz4 => self.compressed_file(
                lz4_flex::frame::FrameDecoder::new(Cursor::new(self.bytes)),
                &[".lz4"],
            ),
            ArchiveKind::TarLzma => self.lzma(true),
            ArchiveKind::Lzma => self.lzma(false),
            ArchiveKind::TarXz => self.xz(true),
            ArchiveKind::Xz => self.xz(false),
            ArchiveKind::TarBrotli => self.compressed_tar(brotli_decompressor::Decompressor::new(
                Cursor::new(self.bytes),
                8 * 1024,
            )),
            ArchiveKind::Brotli => self.compressed_file(
                brotli_decompressor::Decompressor::new(Cursor::new(self.bytes), 8 * 1024),
                &[".br"],
            ),
        }
    }

    fn zip(&mut self) -> Result<()> {
        search_zip(
            self.source.0,
            self.source.1,
            self.bytes,
            self.query,
            self.query_cache,
            self.options,
            self.collector,
        )
    }

    fn tar(&mut self, reader: impl Read) -> Result<()> {
        search_tar(
            self.source.0,
            self.source.1,
            reader,
            self.query,
            self.query_cache,
            self.options,
            self.collector,
        )
    }

    fn compressed_tar(&mut self, reader: impl Read) -> Result<()> {
        search_compressed_tar(
            self.source.0,
            self.source.1,
            reader,
            self.query,
            self.query_cache,
            self.options,
            self.collector,
        )
    }

    fn compressed_file(&mut self, reader: impl Read, suffixes: &[&str]) -> Result<()> {
        search_compressed_file(
            self.source,
            reader,
            suffixes,
            self.query,
            self.query_cache,
            self.options,
            self.collector,
        )
    }

    fn lzma(&mut self, tar: bool) -> Result<()> {
        let expanded = decode_lzma(
            self.bytes,
            self.output_limit(tar),
            self.options.archives.max_decoder_memory_bytes,
            self.source.1,
        )?;
        self.expanded(tar, expanded, &[".lzma"])
    }

    fn xz(&mut self, tar: bool) -> Result<()> {
        let expanded = decode_xz(
            self.bytes,
            self.output_limit(tar),
            self.options.archives.max_decoder_memory_bytes,
            self.source.1,
        )?;
        self.expanded(tar, expanded, &[".xz"])
    }

    fn output_limit(&self, tar: bool) -> u64 {
        if tar {
            self.options.archives.max_expanded_bytes
        } else {
            self.options.archives.max_entry_bytes
        }
    }

    fn expanded(&mut self, tar: bool, expanded: Vec<u8>, suffixes: &[&str]) -> Result<()> {
        if tar {
            self.tar(Cursor::new(expanded))
        } else {
            search_expanded_file(
                self.source,
                &expanded,
                suffixes,
                self.query,
                self.query_cache,
                self.options,
                self.collector,
            )
        }
    }
}
