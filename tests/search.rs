use std::fs;
#[cfg(feature = "archives")]
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use weavatrix_search::{
    CaseMode, ContentDiscoveryMode, EncodingMode, Error, FileEvidenceMode, ResultMode, SearchMode,
    SearchOptions, SearchQuery, SearchWarningKind, Searcher, recommended_scan_options,
};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct TempRepo {
    root: PathBuf,
}

impl TempRepo {
    fn new(name: &str) -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "weavatrix-search-{name}-{}-{id}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    fn write(&self, relative: &str, bytes: impl AsRef<[u8]>) {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, bytes).unwrap();
    }

    fn path(&self) -> &Path {
        &self.root
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[path = "search_cases/encoding_archives.rs"]
mod encoding_archives;
#[path = "search_cases/matching.rs"]
mod matching;
#[path = "search_cases/result_modes.rs"]
mod result_modes;
