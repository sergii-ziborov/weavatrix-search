use serde_json::Value;
use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use weavatrix_scan::{
    ContentDiscoveryMode, ContentValidationPolicy, ContentVisitControl, ScanOptions, Scanner,
};
use weavatrix_search::{
    CaseMode, FileEvidenceMode, IndexOptions, PersistentIndex, ResultMode, SearchMode,
    SearchOptions, SearchQuery, Searcher, WatchEvent, WatchEventKind,
};

#[path = "compare_support/cli.rs"]
mod cli;
#[path = "compare_support/common.rs"]
mod common;
#[path = "compare_support/fixture.rs"]
mod fixture;
#[path = "compare_support/index.rs"]
mod index;
#[path = "compare_support/profiles.rs"]
mod profiles;
#[path = "compare_support/queries.rs"]
mod queries;
#[path = "compare_support/runner.rs"]
mod runner;

use cli::run_cli;
use common::{benchmark_scan_options, env_usize, median, millis, normalize_path, timed};
use fixture::{prepare, verify};
use index::{run_index, run_index_open};
use profiles::{profile, profile_count, profile_files, scan_only};
use queries::{
    ripgrep, ripgrep_count, ripgrep_files, timed_ripgrep, timed_weavatrix, weavatrix,
    weavatrix_count, weavatrix_files,
};
use runner::{run, run_literal};

const MARKER: &str = ".weavatrix-search-benchmark";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Signature {
    path: String,
    line: u64,
    spans: Vec<(usize, usize)>,
}

struct Workload {
    mode: &'static str,
    query: SearchQuery,
    patterns: &'static [&'static str],
    fixed: bool,
    search_mode: SearchMode,
}

fn main() {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if dispatch(&arguments) {
        return;
    }
    run_default();
}

fn dispatch(arguments: &[String]) -> bool {
    match arguments.first().map(String::as_str) {
        Some("prepare") => {
            let root = PathBuf::from(arguments.get(1).expect("prepare requires a path"));
            let files = arguments
                .get(2)
                .expect("prepare requires a file count")
                .parse::<usize>()
                .expect("file count must be an integer");
            prepare(&root, files);
            verify(&root, files);
            println!("prepared={} files={files}", root.display());
        }
        Some("verify") => {
            let root = PathBuf::from(arguments.get(1).expect("verify requires a path"));
            let files = arguments
                .get(2)
                .expect("verify requires a file count")
                .parse::<usize>()
                .expect("file count must be an integer");
            verify(&root, files);
            println!("verified={} files={files}", root.display());
        }
        Some("run") => {
            let root = PathBuf::from(arguments.get(1).expect("run requires a path"));
            run(&root);
        }
        Some("run-literal") => {
            run_literal(Path::new(
                arguments.get(1).expect("run-literal requires a path"),
            ));
        }
        Some("run-cli") => {
            let root = PathBuf::from(arguments.get(1).expect("run-cli requires a path"));
            run_cli(&root);
        }
        Some("run-index") => {
            let root = PathBuf::from(arguments.get(1).expect("run-index requires a path"));
            run_index(&root);
        }
        Some("run-index-open") => {
            let root = PathBuf::from(arguments.get(1).expect("run-index-open requires a root"));
            let index_path =
                PathBuf::from(arguments.get(2).expect("run-index-open requires an index"));
            run_index_open(&root, &index_path);
        }
        Some("run-count") => {
            let root = PathBuf::from(arguments.get(1).expect("run-count requires a path"));
            assert!(root.join(MARKER).is_file(), "benchmark marker is missing");
            profile_count(
                &root,
                env_usize("WEAVATRIX_SEARCH_BENCH_RUNS", 11),
                env_usize("WEAVATRIX_SEARCH_BENCH_WARMUPS", 2),
            );
        }
        Some("once") => {
            let root = PathBuf::from(arguments.get(1).expect("once requires a path"));
            let mode = arguments.get(2).expect("once requires literal or regex");
            let query = match mode.as_str() {
                "literal" => SearchQuery::literal("needle_target"),
                "regex" => SearchQuery::regex(r"item_[0-9]+ = 42"),
                _ => panic!("once mode must be literal or regex"),
            };
            let started = Instant::now();
            let (files, matches) = weavatrix(&root, query, SearchMode::Line);
            println!(
                "mode={mode} files={files} matching_lines={} elapsed_ms={:.3}",
                matches.len(),
                millis(started.elapsed())
            );
        }
        _ => return false,
    }
    true
}

fn run_default() {
    let root =
        std::env::temp_dir().join(format!("weavatrix-search-benchmark-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    prepare(&root, 6_000);
    run(&root);
    fs::remove_dir_all(&root).expect("remove temporary benchmark corpus");
}
