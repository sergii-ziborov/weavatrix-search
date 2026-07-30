use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use weavatrix_scan::{ContentValidationPolicy, ScanOptions};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub(super) struct TempRepo {
    root: PathBuf,
}

impl TempRepo {
    pub(super) fn new(name: &str) -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "weavatrix-search-index-{name}-{}-{id}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        Self { root }
    }

    pub(super) fn write(&self, relative: &str, bytes: impl AsRef<[u8]>) {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, bytes).unwrap();
    }

    pub(super) fn remove(&self, relative: &str) {
        fs::remove_file(self.root.join(relative)).unwrap();
    }

    pub(super) fn path(&self) -> &Path {
        &self.root
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

pub(super) fn scan_options() -> ScanOptions {
    ScanOptions::default()
        .metadata_only()
        .selected_files_only()
        .with_content_parallelism(2)
        .with_content_validation(ContentValidationPolicy::Fast)
}

pub(super) fn matched_paths(
    report: &weavatrix_search::SearchReport,
) -> Vec<(usize, String, u64, String)> {
    report
        .matches
        .iter()
        .map(|found| {
            (
                found.root_index,
                found.path.clone(),
                found.line_number,
                found.line.clone(),
            )
        })
        .collect()
}
