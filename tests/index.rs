use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use weavatrix_scan::{ContentValidationPolicy, ScanOptions};
use weavatrix_search::{
    CaseMode, IndexBuilder, IndexOptions, PersistentIndex, SearchBackend, SearchOptions,
    SearchQuery, Searcher, WatchEvent, WatchEventKind, WatchPlan,
};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct TempRepo {
    root: PathBuf,
}

impl TempRepo {
    fn new(name: &str) -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "weavatrix-search-index-{name}-{}-{id}",
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

    fn remove(&self, relative: &str) {
        fs::remove_file(self.root.join(relative)).unwrap();
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

fn scan_options() -> ScanOptions {
    ScanOptions::default()
        .metadata_only()
        .selected_files_only()
        .with_content_parallelism(2)
        .with_content_validation(ContentValidationPolicy::Fast)
}

fn matched_paths(report: &weavatrix_search::SearchReport) -> Vec<(usize, String, u64, String)> {
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

#[test]
fn persistent_index_round_trips_and_preserves_filesystem_results() {
    let repo = TempRepo::new("round-trip");
    repo.write(".gitignore", b"ignored.txt\n");
    repo.write("src/match.txt", b"before\nstable needle\n");
    repo.write("src/miss.txt", b"unrelated content\n");
    repo.write("notes.txt", b"another unrelated file\n");
    repo.write("ignored.txt", b"stable needle\n");
    let path = repo.path().join("search.wvx");
    let query = SearchQuery::literal("stable needle");
    let options = SearchOptions::default();

    let filesystem = Searcher::new(repo.path(), query.clone())
        .options(options.clone())
        .scan_options(scan_options())
        .search()
        .unwrap();
    let (index, build) = IndexBuilder::new(repo.path())
        .scan_options(scan_options())
        .index_options(IndexOptions::default().with_parallelism(2))
        .build_and_save(&path)
        .unwrap();
    let indexed = index.search(query.clone(), options.clone()).unwrap();

    assert_eq!(indexed.backend, SearchBackend::PersistentIndex);
    assert_eq!(matched_paths(&indexed), matched_paths(&filesystem));
    assert_eq!(build.files, index.status().files);
    assert_eq!(build.revision, index.status().revision);
    let evidence = indexed.index.as_ref().unwrap();
    assert_eq!(evidence.indexed_files, build.files);
    assert!(evidence.prefiltered);
    assert!(evidence.candidate_files < evidence.indexed_files);

    let reopened =
        PersistentIndex::open(&path, IndexOptions::default().with_parallelism(2)).unwrap();
    assert_eq!(reopened.status(), index.status());
    assert_eq!(
        matched_paths(&reopened.search(query, options).unwrap()),
        matched_paths(&filesystem)
    );
}

#[test]
fn index_checksum_rejects_corruption() {
    let repo = TempRepo::new("checksum");
    repo.write("source.txt", b"checksum needle\n");
    let path = repo.path().join("search.wvx");
    PersistentIndex::build_and_save(
        &path,
        [repo.path()],
        scan_options(),
        IndexOptions::default(),
    )
    .unwrap();

    let mut bytes = fs::read(&path).unwrap();
    let position = bytes.len() / 2;
    bytes[position] ^= 0x5a;
    fs::write(&path, bytes).unwrap();

    let error = PersistentIndex::open(&path, IndexOptions::default()).unwrap_err();
    assert!(
        error.to_string().contains("checksum")
            || error.to_string().contains("revision")
            || error.to_string().contains("UTF-8"),
        "{error}"
    );
}

#[test]
fn index_prefilter_never_changes_regex_case_or_utf16_results() {
    let repo = TempRepo::new("prefilter-parity");
    repo.write("sensitive.txt", b"prefixfooneedle\n");
    repo.write("upper.txt", b"PREFIXFOONEEDLE\n");
    repo.write("alternative.txt", b"left branch\n");
    let mut utf16 = vec![0xff, 0xfe];
    for unit in "utf16 needle\n".encode_utf16() {
        utf16.extend_from_slice(&unit.to_le_bytes());
    }
    repo.write("utf16.txt", utf16);
    let (index, _) =
        PersistentIndex::build([repo.path()], scan_options(), IndexOptions::default()).unwrap();
    let cases = [
        (
            SearchQuery::regex(r"prefix(?:foo|bar)needle"),
            SearchOptions::default(),
        ),
        (
            SearchQuery::literal("prefixfooneedle"),
            SearchOptions::default().with_case(CaseMode::Insensitive),
        ),
        (
            SearchQuery::regex(r"left branch|right branch"),
            SearchOptions::default(),
        ),
        (
            SearchQuery::literal("utf16 needle"),
            SearchOptions::default(),
        ),
    ];

    for (query, options) in cases {
        let filesystem = Searcher::new(repo.path(), query.clone())
            .options(options.clone())
            .scan_options(scan_options())
            .search()
            .unwrap();
        let indexed = index.search(query, options).unwrap();
        assert_eq!(matched_paths(&indexed), matched_paths(&filesystem));
    }
}

#[test]
fn incremental_update_adds_replaces_and_removes_without_full_discovery() {
    let repo = TempRepo::new("incremental");
    repo.write("changed.txt", b"old marker\n");
    repo.write("removed.txt", b"removed marker\n");
    repo.write("stable.txt", b"stable marker\n");
    let (mut index, _) = PersistentIndex::build(
        [repo.path()],
        scan_options(),
        IndexOptions::default().with_parallelism(2),
    )
    .unwrap();

    repo.write("changed.txt", b"new marker\n");
    repo.write("added.txt", b"new marker\n");
    repo.remove("removed.txt");
    let events = [
        WatchEvent::new(repo.path().join("changed.txt"), WatchEventKind::Modify),
        WatchEvent::new(repo.path().join("added.txt"), WatchEventKind::Create),
        WatchEvent::new(repo.path().join("removed.txt"), WatchEventKind::Remove),
    ];
    let update = index.update_events(0, events, scan_options()).unwrap();

    assert!(!update.full_rebuild);
    assert_eq!(
        (update.added, update.updated, update.removed),
        (1, 1, 1),
        "{update:?}"
    );
    assert!(update.changed_scan.is_some());
    let new_matches = index
        .search(SearchQuery::literal("new marker"), SearchOptions::default())
        .unwrap();
    assert_eq!(new_matches.files_with_matches, 2);
    let old_matches = index
        .search(
            SearchQuery::literal("removed marker"),
            SearchOptions::default(),
        )
        .unwrap();
    assert_eq!(old_matches.files_with_matches, 0);
}

#[test]
fn failed_incremental_limit_check_leaves_snapshot_unchanged() {
    let repo = TempRepo::new("rollback");
    repo.write("a.txt", b"aa");
    repo.write("b.txt", b"bb");
    let (mut index, _) = PersistentIndex::build(
        [repo.path()],
        scan_options(),
        IndexOptions::default()
            .with_parallelism(2)
            .with_max_content_bytes(5),
    )
    .unwrap();
    let revision = index.status().revision;

    repo.write("a.txt", b"cccc");
    let error = index
        .update_events(
            0,
            [WatchEvent::new(
                repo.path().join("a.txt"),
                WatchEventKind::Modify,
            )],
            scan_options(),
        )
        .unwrap_err();

    assert!(error.to_string().contains("content exceeds"), "{error}");
    assert_eq!(index.status().revision, revision);
    assert_eq!(
        index
            .search(SearchQuery::literal("aa"), SearchOptions::default())
            .unwrap()
            .files_with_matches,
        1
    );
}

#[test]
fn full_rebuild_counts_only_content_changes_as_updates() {
    let repo = TempRepo::new("rebuild-count");
    repo.write("a.txt", b"old\n");
    repo.write("b.txt", b"stable\n");
    let (mut index, _) =
        PersistentIndex::build([repo.path()], scan_options(), IndexOptions::default()).unwrap();
    repo.write("a.txt", b"new\n");

    let report = index
        .update(
            0,
            &WatchPlan {
                full_rescan: true,
                ..WatchPlan::default()
            },
            scan_options(),
        )
        .unwrap();

    assert!(report.full_rebuild);
    assert_eq!((report.added, report.updated, report.removed), (0, 1, 0));
}

#[test]
fn rebuild_excludes_an_index_stored_inside_the_repository() {
    let repo = TempRepo::new("self-exclusion");
    repo.write("source.txt", b"source marker\n");
    let path = repo.path().join(".weavatrix").join("search.wvx");
    let (mut index, build) = PersistentIndex::build_and_save(
        &path,
        [repo.path()],
        scan_options(),
        IndexOptions::default(),
    )
    .unwrap();

    let report = index
        .update(
            0,
            &WatchPlan {
                full_rescan: true,
                ..WatchPlan::default()
            },
            scan_options(),
        )
        .unwrap();

    assert_eq!(report.files, build.files);
    assert_eq!((report.added, report.updated, report.removed), (0, 0, 0));
    assert_eq!(
        index
            .search(SearchQuery::literal("WVXIDX01"), SearchOptions::default())
            .unwrap()
            .files_with_matches,
        0
    );

    let (rebuilt, second_build) = PersistentIndex::build_and_save(
        &path,
        [repo.path()],
        scan_options(),
        IndexOptions::default(),
    )
    .unwrap();
    assert_eq!(second_build.files, build.files);
    assert_eq!(
        rebuilt
            .search(SearchQuery::literal("WVXIDX01"), SearchOptions::default())
            .unwrap()
            .files_with_matches,
        0
    );
}

#[test]
fn index_resource_limits_are_enforced() {
    let repo = TempRepo::new("limits");
    repo.write("a.txt", b"a");
    repo.write("b.txt", b"b");

    let error = PersistentIndex::build(
        [repo.path()],
        scan_options(),
        IndexOptions::default().with_max_entries(1),
    )
    .unwrap_err();

    assert!(error.to_string().contains("entry count exceeds"), "{error}");
}

#[test]
fn cli_builds_reuses_and_reports_a_persistent_index() {
    let repo = TempRepo::new("cli");
    repo.write("src/match.txt", b"cli index needle\n");
    repo.write("src/miss.txt", b"ordinary\n");
    let path = repo.path().join(".weavatrix").join("search.wvx");
    let binary = env!("CARGO_BIN_EXE_weavatrix-search");

    let first = Command::new(binary)
        .args([
            "--fixed-strings",
            "--index",
            path.to_str().unwrap(),
            "cli index needle",
            repo.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(path.is_file());
    assert!(String::from_utf8_lossy(&first.stdout).contains("src/match.txt"));

    let reused = Command::new(binary)
        .current_dir(repo.path())
        .args([
            "--fixed-strings",
            "--stats",
            "--index",
            path.to_str().unwrap(),
            "cli index needle",
        ])
        .output()
        .unwrap();
    assert!(
        reused.status.success(),
        "{}",
        String::from_utf8_lossy(&reused.stderr)
    );
    let stderr = String::from_utf8_lossy(&reused.stderr);
    assert!(stderr.contains("indexed"), "{stderr}");
    assert!(stderr.contains("candidates"), "{stderr}");

    let status = Command::new(binary)
        .args(["--index-status", path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(status.status.success());
    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(stdout.contains("revision="), "{stdout}");
    assert!(stdout.contains("files="), "{stdout}");
}

#[cfg(feature = "live")]
#[test]
fn live_index_applies_hosted_events_and_persists_the_new_generation() {
    use std::time::Duration;
    use weavatrix_search::{LiveIndexBuilder, LiveIndexOptions};

    let repo = TempRepo::new("live");
    repo.write("source.txt", b"old live marker\n");
    let path = repo.path().join("search.wvx");
    let _ = PersistentIndex::build_and_save(
        &path,
        [repo.path()],
        scan_options(),
        IndexOptions::default().with_parallelism(2),
    )
    .unwrap();
    let live = LiveIndexBuilder::new(&path, repo.path())
        .scan_options(scan_options())
        .index_options(IndexOptions::default().with_parallelism(2))
        .live_options(
            LiveIndexOptions::default()
                .with_debounce(Duration::from_millis(20))
                .trust_existing_snapshot(),
        )
        .start()
        .unwrap();

    repo.write("source.txt", b"new live marker\n");
    let report = live
        .apply_events(
            0,
            [WatchEvent::new(
                repo.path().join("source.txt"),
                WatchEventKind::Modify,
            )],
        )
        .unwrap();
    assert_eq!(report.updated, 1);
    assert_eq!(
        live.search(
            SearchQuery::literal("new live marker"),
            SearchOptions::default()
        )
        .unwrap()
        .files_with_matches,
        1
    );
    let status = live.status();
    assert!(status.generation >= 1);
    assert!(status.dirty);
    live.stop().unwrap();

    let reopened = PersistentIndex::open(&path, IndexOptions::default()).unwrap();
    assert_eq!(
        reopened
            .search(
                SearchQuery::literal("new live marker"),
                SearchOptions::default()
            )
            .unwrap()
            .files_with_matches,
        1
    );
}

#[cfg(feature = "live")]
#[test]
fn live_index_observes_native_filesystem_changes() {
    use std::time::Duration;
    use weavatrix_search::{LiveIndexBuilder, LiveIndexOptions};

    let repo = TempRepo::new("native-live");
    repo.write("source.txt", b"old native marker\n");
    let path = repo.path().join("search.wvx");
    let _ = PersistentIndex::build_and_save(
        &path,
        [repo.path()],
        scan_options(),
        IndexOptions::default().with_parallelism(2),
    )
    .unwrap();
    let live = LiveIndexBuilder::new(&path, repo.path())
        .scan_options(scan_options())
        .index_options(IndexOptions::default().with_parallelism(2))
        .live_options(
            LiveIndexOptions::default()
                .with_debounce(Duration::from_millis(20))
                .trust_existing_snapshot(),
        )
        .start()
        .unwrap();
    let generation = live.status().generation;

    repo.write("source.txt", b"new native marker\n");
    assert!(
        live.wait_for_update(generation, Duration::from_secs(10)),
        "{:?}",
        live.status()
    );
    assert_eq!(
        live.search(
            SearchQuery::literal("new native marker"),
            SearchOptions::default()
        )
        .unwrap()
        .files_with_matches,
        1
    );
    live.stop().unwrap();
}
