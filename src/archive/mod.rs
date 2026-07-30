#[cfg(feature = "archives")]
mod compression;
#[cfg(feature = "archives")]
mod containers;
mod detection;
#[cfg(feature = "archives")]
mod dispatch;

#[cfg(feature = "archives")]
use compression::{
    decode_lzma, decode_xz, search_compressed_file, search_compressed_tar, search_expanded_file,
    zstd_decoder,
};
#[cfg(feature = "archives")]
use containers::{search_tar, search_zip};
pub(crate) use detection::kind;

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
#[cfg(feature = "archives")]
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

#[cfg_attr(not(feature = "archives"), allow(clippy::unnecessary_wraps))]
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
        dispatch::ArchiveSearch::new(
            (root_index, outer_path),
            bytes,
            query,
            query_cache,
            options,
            collector,
        )
        .search(kind)
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
