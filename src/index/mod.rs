mod build;
mod build_worker;
mod change_set;
mod entry;
mod failure;
mod files;
mod format;
mod load;
mod metadata;
mod options;
mod reports;
mod search;
mod storage;
mod update;
mod update_worker;

use entry::{EntryBuilder, IndexEntry};
use failure::{has_failure, set_failure, take_failure};
use files::{IndexLock, auxiliary_path, decode_path, encode_path, replace_file};
use format::{IndexReader, IndexWriter};
use load::{read_entries, read_header, read_roots};
use metadata::{
    bloom_contains, content_bytes, entry_position, hex, revision, revision_bytes,
    scan_options_with_storage_exclusion, trigram_bloom, validate_unique_entries,
};
pub use options::IndexOptions;
pub use reports::{IndexBuildReport, IndexStatus, IndexUpdateReport};

use crate::archive;
use crate::error::{Error, Result};
use crate::options::{EncodingMode, FileEvidenceMode, SearchOptions};
use crate::query::SearchQuery;
use crate::report::SearchReport;
use crate::searcher::{IndexedContent, search_indexed};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use weavatrix_scan::{
    ChangedContentVisitOutcome, ContentFileStatus, ContentVisitControl, ContentVisitEvent,
    ContentVisitReport, MultiContentVisitReport, MultiScanner, ScanOptions, Scanner, WatchEvent,
    WatchPlan, WatcherEventAdapter,
};

const MAGIC: &[u8; 8] = b"WVXIDX01";
const FORMAT_VERSION: u32 = 1;
const CHECKSUM_BYTES: u64 = 32;
// 512 bits keeps short/medium source files selective while saving 64 bytes
// per entry versus the original prototype (12.2 MiB at 200k files).
const BLOOM_WORDS: usize = 8;
const MAX_ROOTS: usize = 4_096;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[cfg(target_os = "windows")]
const PLATFORM_ID: u8 = 1;
#[cfg(target_os = "linux")]
const PLATFORM_ID: u8 = 2;
#[cfg(target_os = "macos")]
const PLATFORM_ID: u8 = 3;
#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
const PLATFORM_ID: u8 = 4;

/// Ergonomic builder for one persistent multi-root snapshot.
#[derive(Debug)]
pub struct IndexBuilder {
    roots: Vec<PathBuf>,
    scan_options: ScanOptions,
    index_options: IndexOptions,
}

/// Immutable query snapshot with explicit mutation methods for watcher deltas.
#[derive(Debug)]
pub struct PersistentIndex {
    roots: Vec<PathBuf>,
    event_roots: Vec<PathBuf>,
    entries: Vec<IndexEntry>,
    revision: String,
    content_bytes: u64,
    options: IndexOptions,
    storage_path: Mutex<Option<PathBuf>>,
}
