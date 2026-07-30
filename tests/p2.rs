use serde_json::Value;
use std::fs;
#[cfg(feature = "archives")]
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use weavatrix_search::{
    ColorChoice, OutputFormat, OutputOptions, ResultMode, SearchOptions, SearchQuery,
    SearchWarningKind, Searcher, write_report, write_report_with,
};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("weavatrix-search-p2-{}-{id}", std::process::id()));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("src/people.txt"),
            "Ada Lovelace and Grace Hopper\n",
        )
        .unwrap();
        fs::write(root.join("src/other.txt"), "ordinary\n").unwrap();
        Self { root }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[cfg(feature = "archives")]
#[path = "p2_cases/archives.rs"]
mod archives;
#[path = "p2_cases/core.rs"]
mod core;

fn path_text(path: &Path) -> &str {
    path.to_str().expect("temporary path is UTF-8")
}
