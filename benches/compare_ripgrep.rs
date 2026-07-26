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

#[allow(clippy::too_many_lines)]
fn main() {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments
        .first()
        .is_some_and(|argument| argument == "prepare")
    {
        let root = PathBuf::from(arguments.get(1).expect("prepare requires a path"));
        let files = arguments
            .get(2)
            .expect("prepare requires a file count")
            .parse::<usize>()
            .expect("file count must be an integer");
        prepare(&root, files);
        verify(&root, files);
        println!("prepared={} files={files}", root.display());
        return;
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "verify")
    {
        let root = PathBuf::from(arguments.get(1).expect("verify requires a path"));
        let files = arguments
            .get(2)
            .expect("verify requires a file count")
            .parse::<usize>()
            .expect("file count must be an integer");
        verify(&root, files);
        println!("verified={} files={files}", root.display());
        return;
    }
    if arguments.first().is_some_and(|argument| argument == "run") {
        let root = PathBuf::from(arguments.get(1).expect("run requires a path"));
        run(&root);
        return;
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "run-literal")
    {
        run_literal(Path::new(
            arguments.get(1).expect("run-literal requires a path"),
        ));
        return;
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "run-cli")
    {
        let root = PathBuf::from(arguments.get(1).expect("run-cli requires a path"));
        run_cli(&root);
        return;
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "run-index")
    {
        let root = PathBuf::from(arguments.get(1).expect("run-index requires a path"));
        run_index(&root);
        return;
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "run-index-open")
    {
        let root = PathBuf::from(arguments.get(1).expect("run-index-open requires a root"));
        let index_path = PathBuf::from(arguments.get(2).expect("run-index-open requires an index"));
        run_index_open(&root, &index_path);
        return;
    }
    if arguments
        .first()
        .is_some_and(|argument| argument == "run-count")
    {
        let root = PathBuf::from(arguments.get(1).expect("run-count requires a path"));
        assert!(root.join(MARKER).is_file(), "benchmark marker is missing");
        profile_count(
            &root,
            env_usize("WEAVATRIX_SEARCH_BENCH_RUNS", 11),
            env_usize("WEAVATRIX_SEARCH_BENCH_WARMUPS", 2),
        );
        return;
    }
    if arguments.first().is_some_and(|argument| argument == "once") {
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
        return;
    }

    let root =
        std::env::temp_dir().join(format!("weavatrix-search-benchmark-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    prepare(&root, 6_000);
    run(&root);
    fs::remove_dir_all(&root).expect("remove temporary benchmark corpus");
}

fn prepare(root: &Path, files: usize) {
    assert!(files > 0, "file count must be positive");
    if root.exists() {
        assert!(
            root.join(MARKER).is_file(),
            "refusing to populate an existing unmarked directory"
        );
        assert_eq!(
            fs::read_to_string(root.join(MARKER)).unwrap(),
            files.to_string(),
            "existing fixture has a different configured size"
        );
        return;
    }
    fs::create_dir_all(root).expect("create benchmark root");
    fs::write(root.join(MARKER), files.to_string()).expect("write marker");
    fs::write(root.join(".gitignore"), "group0003/\n").expect("write ignore file");
    let groups = files.div_ceil(500);
    let workers = std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .min(groups)
        .min(16);
    std::thread::scope(|scope| {
        for worker in 0..workers {
            scope.spawn(move || {
                for group in (worker..groups).step_by(workers) {
                    write_fixture_group(root, group, files);
                }
            });
        }
    });
}

fn write_fixture_group(root: &Path, group: usize, files: usize) {
    let directory = root.join(format!("group{group:04}"));
    fs::create_dir(&directory).expect("create benchmark directory");
    let start = group * 500;
    let end = start.saturating_add(500).min(files);
    for index in start..end {
        let content = if index % 20 == 0 {
            format!("pub fn needle_target_{index}() {{}}\nlet item_{index} = 42;\n")
        } else {
            format!("pub fn ordinary_{index}() {{}}\n")
        };
        fs::write(directory.join(format!("file{index:07}.rs")), content)
            .expect("write benchmark file");
    }
}

fn verify(root: &Path, files: usize) {
    assert_eq!(
        fs::read_to_string(root.join(MARKER)).unwrap(),
        files.to_string()
    );
    let actual = fs::read_dir(root)
        .unwrap()
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| fs::read_dir(entry.path()).unwrap().count())
        .sum::<usize>();
    assert_eq!(actual, files);
}

fn run(root: &Path) {
    assert!(root.join(MARKER).is_file(), "benchmark marker is missing");
    let runs = env_usize("WEAVATRIX_SEARCH_BENCH_RUNS", 11);
    let warmups = env_usize("WEAVATRIX_SEARCH_BENCH_WARMUPS", 2);
    println!(
        "corpus={} statistic=median runs={runs} warmups={warmups}",
        root.display()
    );
    let mut scanner = Vec::with_capacity(runs);
    for _ in 0..warmups {
        black_box(scan_only(root));
    }
    for _ in 0..runs {
        let started = Instant::now();
        let files = scan_only(root);
        scanner.push(started.elapsed());
        black_box(files);
    }
    println!(
        "mode=content-floor engine=weavatrix-scan files={} median_ms={:.3}",
        scan_only(root),
        millis(median(&mut scanner))
    );
    profile(
        root,
        &Workload {
            mode: "literal",
            query: SearchQuery::literal("needle_target"),
            patterns: &["needle_target"],
            fixed: true,
            search_mode: SearchMode::Line,
        },
        runs,
        warmups,
    );
    profile_file_evidence(root, runs, warmups);
    profile(
        root,
        &Workload {
            mode: "regex",
            query: SearchQuery::regex(r"item_[0-9]+ = 42"),
            patterns: &[r"item_[0-9]+ = 42"],
            fixed: false,
            search_mode: SearchMode::Line,
        },
        runs,
        warmups,
    );
    profile(
        root,
        &Workload {
            mode: "multi",
            query: SearchQuery::any([
                SearchQuery::literal("needle_target"),
                SearchQuery::regex(r"item_[0-9]+ = 42"),
            ]),
            patterns: &["needle_target", r"item_[0-9]+ = 42"],
            fixed: false,
            search_mode: SearchMode::Line,
        },
        runs,
        warmups,
    );
    profile(
        root,
        &Workload {
            mode: "multiline",
            query: SearchQuery::regex(r"needle_target_[0-9]+\(\) \{\}\r?\nlet item_[0-9]+ = 42"),
            patterns: &[r"needle_target_[0-9]+\(\) \{\}\r?\nlet item_[0-9]+ = 42"],
            fixed: false,
            search_mode: SearchMode::Multiline,
        },
        runs,
        warmups,
    );
    profile_count(root, runs, warmups);
    profile_files(root, runs, warmups);
}

fn profile_file_evidence(root: &Path, runs: usize, warmups: usize) {
    let expected = evidence_search(root, false);
    let retained_expected = evidence_search(root, true);
    assert_eq!(expected, retained_expected);
    for _ in 0..warmups {
        black_box(evidence_search(root, false));
        black_box(evidence_search(root, true));
    }
    let mut streaming = Vec::with_capacity(runs);
    let mut retained = Vec::with_capacity(runs);
    for index in 0..runs {
        if index % 2 == 0 {
            streaming.push(timed(|| evidence_search(root, false), &expected));
            retained.push(timed(|| evidence_search(root, true), &expected));
        } else {
            retained.push(timed(|| evidence_search(root, true), &expected));
            streaming.push(timed(|| evidence_search(root, false), &expected));
        }
    }
    println!(
        "mode=file-evidence-stream engine=weavatrix-search files={} logical_lines={} median_ms={:.3}",
        expected.0,
        expected.1,
        millis(median(&mut streaming))
    );
    println!(
        "mode=file-evidence-retained engine=weavatrix-search files={} logical_lines={} median_ms={:.3}",
        expected.0,
        expected.1,
        millis(median(&mut retained))
    );
}

fn evidence_search(root: &Path, retain: bool) -> (u64, u64) {
    let counters = Arc::new((AtomicU64::new(0), AtomicU64::new(0)));
    let sink = Arc::clone(&counters);
    let mut options = SearchOptions::default()
        .with_case(CaseMode::Sensitive)
        .with_max_results(usize::MAX);
    if retain {
        options = options
            .with_file_evidence(FileEvidenceMode::All)
            .with_max_file_evidence(usize::MAX);
    } else {
        options = options.with_file_evidence_visitor(move |evidence| {
            sink.0.fetch_add(1, Ordering::Relaxed);
            sink.1.fetch_add(evidence.total_lines, Ordering::Relaxed);
        });
    }
    let report = Searcher::new(root, SearchQuery::literal("needle_target"))
        .scan_options(benchmark_scan_options().with_skip_hidden(true))
        .options(options)
        .search()
        .expect("file evidence benchmark search");
    if retain {
        (
            u64::try_from(report.file_evidence.len()).unwrap_or(u64::MAX),
            report
                .file_evidence
                .iter()
                .map(|evidence| evidence.total_lines)
                .sum(),
        )
    } else {
        (
            counters.0.load(Ordering::Relaxed),
            counters.1.load(Ordering::Relaxed),
        )
    }
}

fn run_literal(root: &Path) {
    assert!(root.join(MARKER).is_file(), "benchmark marker is missing");
    profile(
        root,
        &Workload {
            mode: "literal",
            query: SearchQuery::literal("needle_target"),
            patterns: &["needle_target"],
            fixed: true,
            search_mode: SearchMode::Line,
        },
        env_usize("WEAVATRIX_SEARCH_BENCH_RUNS", 11),
        env_usize("WEAVATRIX_SEARCH_BENCH_WARMUPS", 2),
    );
}

fn run_cli(root: &Path) {
    assert!(root.join(MARKER).is_file(), "benchmark marker is missing");
    let binary = release_search_binary();
    assert!(
        binary.is_file(),
        "{} is missing; run cargo build --release --all-features first",
        binary.display()
    );
    let runs = env_usize("WEAVATRIX_SEARCH_BENCH_RUNS", 7);
    let warmups = env_usize("WEAVATRIX_SEARCH_BENCH_WARMUPS", 2);
    let expected = weavatrix_cli(root, &binary);
    assert_eq!(
        expected,
        ripgrep_cli(root),
        "end-to-end CLI output differs from ripgrep"
    );
    for _ in 0..warmups {
        black_box(weavatrix_cli(root, &binary));
        black_box(ripgrep_cli(root));
    }
    let mut ours = Vec::with_capacity(runs);
    let mut ripgrep = Vec::with_capacity(runs);
    for index in 0..runs {
        if index % 2 == 0 {
            ours.push(timed(|| weavatrix_cli(root, &binary), &expected));
            ripgrep.push(timed(|| ripgrep_cli(root), &expected));
        } else {
            ripgrep.push(timed(|| ripgrep_cli(root), &expected));
            ours.push(timed(|| weavatrix_cli(root, &binary), &expected));
        }
    }
    println!(
        "mode=literal-json-cli engine=weavatrix-search-cli matching_lines={} median_ms={:.3}",
        expected.len(),
        millis(median(&mut ours))
    );
    println!(
        "mode=literal-json-cli engine=ripgrep-cli matching_lines={} median_ms={:.3}",
        expected.len(),
        millis(median(&mut ripgrep))
    );
}

fn run_index(root: &Path) {
    assert!(root.join(MARKER).is_file(), "benchmark marker is missing");
    let runs = env_usize("WEAVATRIX_SEARCH_BENCH_RUNS", 11);
    let warmups = env_usize("WEAVATRIX_SEARCH_BENCH_WARMUPS", 3);
    let index_path = std::env::temp_dir().join(format!(
        "weavatrix-search-resident-benchmark-{}.wvx",
        std::process::id()
    ));
    let _ = fs::remove_file(&index_path);
    let started = Instant::now();
    let (built_index, build_report) = PersistentIndex::build_and_save(
        &index_path,
        [root],
        benchmark_scan_options(),
        IndexOptions::default(),
    )
    .expect("build persistent benchmark index");
    let build_elapsed = started.elapsed();
    drop(built_index);
    let started = Instant::now();
    let mut index =
        PersistentIndex::open(&index_path, IndexOptions::default()).expect("open benchmark index");
    let index_bytes = fs::metadata(&index_path)
        .expect("persistent benchmark index metadata")
        .len();
    let open_elapsed = started.elapsed();
    let query = SearchQuery::literal("needle_target");
    let options = SearchOptions::default()
        .with_case(CaseMode::Sensitive)
        .with_max_results(usize::MAX);
    let expected_report = index
        .search(query.clone(), options.clone())
        .expect("search resident index");
    let expected = report_signatures(&expected_report);
    let (_, ripgrep_expected) = ripgrep(root, &["needle_target"], true, SearchMode::Line);
    assert_eq!(
        expected, ripgrep_expected,
        "resident-index output differs from ripgrep"
    );
    for _ in 0..warmups {
        black_box(
            index
                .search(query.clone(), options.clone())
                .expect("warm resident index"),
        );
        black_box(ripgrep(root, &["needle_target"], true, SearchMode::Line));
    }
    let mut resident = Vec::with_capacity(runs);
    let mut ripgrep_times = Vec::with_capacity(runs);
    for run in 0..runs {
        if run % 2 == 0 {
            resident.push(timed_index(&index, &query, &options, &expected));
            ripgrep_times.push(timed_ripgrep(root, &["needle_target"], true, SearchMode::Line).0);
        } else {
            ripgrep_times.push(timed_ripgrep(root, &["needle_target"], true, SearchMode::Line).0);
            resident.push(timed_index(&index, &query, &options, &expected));
        }
    }
    let evidence = expected_report
        .index
        .as_ref()
        .expect("indexed search evidence");
    let live_update = profile_live_update(&mut index, root, runs, warmups);
    println!(
        "mode=index-build files={} content_bytes={} index_bytes={} elapsed_ms={:.3}",
        build_report.files,
        build_report.content_bytes,
        index_bytes,
        millis(build_elapsed)
    );
    println!(
        "mode=index-open files={} elapsed_ms={:.3}",
        build_report.files,
        millis(open_elapsed)
    );
    println!(
        "mode=resident-literal engine=weavatrix-search-index indexed={} candidates={} matching_lines={} median_ms={:.3}",
        evidence.indexed_files,
        evidence.candidate_files,
        expected.len(),
        millis(median(&mut resident))
    );
    println!(
        "mode=resident-literal engine=ripgrep-cli matching_lines={} median_ms={:.3}",
        expected.len(),
        millis(median(&mut ripgrep_times))
    );
    println!(
        "mode=live-update changed_files=1 median_ms={:.3}",
        millis(live_update)
    );
    fs::remove_file(&index_path).expect("remove benchmark index");
}

fn run_index_open(root: &Path, index_path: &Path) {
    assert!(root.join(MARKER).is_file(), "benchmark marker is missing");
    let runs = env_usize("WEAVATRIX_SEARCH_BENCH_RUNS", 11);
    let warmups = env_usize("WEAVATRIX_SEARCH_BENCH_WARMUPS", 3);
    let started = Instant::now();
    let mut index =
        PersistentIndex::open(index_path, IndexOptions::default()).expect("open benchmark index");
    let open_elapsed = started.elapsed();
    let query = SearchQuery::literal("needle_target");
    let options = SearchOptions::default()
        .with_case(CaseMode::Sensitive)
        .with_max_results(usize::MAX);
    let expected = index
        .search(query.clone(), options.clone())
        .expect("search resident index");
    for _ in 0..warmups {
        black_box(
            index
                .search(query.clone(), options.clone())
                .expect("warm resident index"),
        );
    }
    let mut resident = Vec::with_capacity(runs);
    let expected_signatures = report_signatures(&expected);
    for _ in 0..runs {
        resident.push(timed_index(&index, &query, &options, &expected_signatures));
    }
    let evidence = expected.index.as_ref().expect("indexed search evidence");
    let live_update = profile_live_update(&mut index, root, runs, warmups);
    println!(
        "mode=index-open files={} index_bytes={} elapsed_ms={:.3}",
        evidence.indexed_files,
        fs::metadata(index_path).expect("index metadata").len(),
        millis(open_elapsed)
    );
    println!(
        "mode=resident-literal engine=weavatrix-search-index indexed={} candidates={} matching_lines={} median_ms={:.3}",
        evidence.indexed_files,
        evidence.candidate_files,
        expected.matching_lines,
        millis(median(&mut resident))
    );
    println!(
        "mode=live-update changed_files=1 median_ms={:.3}",
        millis(live_update)
    );
}

fn profile_live_update(
    index: &mut PersistentIndex,
    root: &Path,
    runs: usize,
    warmups: usize,
) -> Duration {
    let path = root.join("group0000").join("file0000001.rs");
    let original = fs::read(&path).expect("read live-update fixture");
    let mut durations = Vec::with_capacity(runs);
    for iteration in 0..warmups.saturating_add(runs) {
        let mut changed = original.clone();
        changed.extend_from_slice(if iteration % 2 == 0 {
            b"// live-a\n"
        } else {
            b"// live-b\n"
        });
        fs::write(&path, changed).expect("write live-update fixture");
        let started = Instant::now();
        let report = index
            .update_events(
                0,
                [WatchEvent::new(&path, WatchEventKind::Modify)],
                benchmark_scan_options(),
            )
            .expect("apply one-file live update");
        let elapsed = started.elapsed();
        assert_eq!((report.added, report.updated, report.removed), (0, 1, 0));
        if iteration >= warmups {
            durations.push(elapsed);
        }
    }
    fs::write(&path, original).expect("restore live-update fixture");
    index
        .update_events(
            0,
            [WatchEvent::new(&path, WatchEventKind::Modify)],
            benchmark_scan_options(),
        )
        .expect("restore resident index");
    median(&mut durations)
}

fn timed_index(
    index: &PersistentIndex,
    query: &SearchQuery,
    options: &SearchOptions,
    expected: &[Signature],
) -> Duration {
    let started = Instant::now();
    let report = index
        .search(query.clone(), options.clone())
        .expect("search resident index");
    let elapsed = started.elapsed();
    assert_eq!(report_signatures(&report), expected);
    elapsed
}

fn report_signatures(report: &weavatrix_search::SearchReport) -> Vec<Signature> {
    report
        .matches
        .iter()
        .map(|found| Signature {
            path: found.path.clone(),
            line: found.line_number,
            spans: found
                .spans
                .iter()
                .map(|span| (span.start, span.end))
                .collect(),
        })
        .collect()
}

fn benchmark_scan_options() -> ScanOptions {
    ScanOptions::default()
        .metadata_only()
        .selected_files_only()
        .with_content_parallelism(benchmark_content_parallelism())
        .with_content_discovery(ContentDiscoveryMode::BufferedParallel)
        .with_content_validation(ContentValidationPolicy::Fast)
}

fn benchmark_content_parallelism() -> usize {
    env_usize(
        "WEAVATRIX_SEARCH_BENCH_THREADS",
        if cfg!(windows) { 8 } else { 16 },
    )
}

fn release_search_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("WEAVATRIX_SEARCH_BIN") {
        return PathBuf::from(path);
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("release")
        .join(if cfg!(windows) {
            "weavatrix-search.exe"
        } else {
            "weavatrix-search"
        })
}

fn weavatrix_cli(root: &Path, binary: &Path) -> Vec<Signature> {
    let output = Command::new(binary)
        .current_dir(root)
        .args([
            "--json",
            "--fixed-strings",
            "--max-results",
            &usize::MAX.to_string(),
            "needle_target",
            ".",
        ])
        .output()
        .expect("run release weavatrix-search");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    parse_json_matches(&output.stdout, false)
}

fn ripgrep_cli(root: &Path) -> Vec<Signature> {
    let output = Command::new("rg")
        .current_dir(root)
        .args([
            "--json",
            "--no-messages",
            "--color",
            "never",
            "--no-require-git",
            "--fixed-strings",
            "--regexp",
            "needle_target",
            ".",
        ])
        .output()
        .expect("ripgrep is required for this benchmark");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    parse_json_matches(&output.stdout, true)
}

fn parse_json_matches(output: &[u8], ripgrep: bool) -> Vec<Signature> {
    let mut signatures = String::from_utf8_lossy(output)
        .lines()
        .filter_map(|line| {
            let value: Value = serde_json::from_str(line).expect("valid JSON Lines output");
            (value["type"] == "match").then(|| {
                let data = &value["data"];
                if ripgrep {
                    Signature {
                        path: normalize_path(data["path"]["text"].as_str().unwrap()),
                        line: data["line_number"].as_u64().unwrap(),
                        spans: data["submatches"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|matched| {
                                (
                                    usize::try_from(matched["start"].as_u64().unwrap()).unwrap(),
                                    usize::try_from(matched["end"].as_u64().unwrap()).unwrap(),
                                )
                            })
                            .collect(),
                    }
                } else {
                    Signature {
                        path: normalize_path(data["path"].as_str().unwrap()),
                        line: data["line_number"].as_u64().unwrap(),
                        spans: data["spans"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|matched| {
                                (
                                    usize::try_from(matched["start"].as_u64().unwrap()).unwrap(),
                                    usize::try_from(matched["end"].as_u64().unwrap()).unwrap(),
                                )
                            })
                            .collect(),
                    }
                }
            })
        })
        .collect::<Vec<_>>();
    signatures.sort();
    signatures
}

fn scan_only(root: &Path) -> u64 {
    Scanner::new(root)
        .options(benchmark_scan_options().with_skip_hidden(true))
        .visit_content_streaming(|_| |_| ContentVisitControl::Continue)
        .expect("scan-only content visit")
        .completed
}

fn profile(root: &Path, workload: &Workload, runs: usize, warmups: usize) {
    let expected = weavatrix(root, workload.query.clone(), workload.search_mode).1;
    let rg_expected = ripgrep(
        root,
        workload.patterns,
        workload.fixed,
        workload.search_mode,
    )
    .1;
    assert_eq!(
        expected, rg_expected,
        "{} output differs from ripgrep",
        workload.mode
    );

    for _ in 0..warmups {
        black_box(weavatrix(
            root,
            workload.query.clone(),
            workload.search_mode,
        ));
        black_box(ripgrep(
            root,
            workload.patterns,
            workload.fixed,
            workload.search_mode,
        ));
    }

    let mut ours = Vec::with_capacity(runs);
    let mut rg = Vec::with_capacity(runs);
    let mut files = 0_u64;
    for index in 0..runs {
        if index % 2 == 0 {
            let (elapsed, output, searched) =
                timed_weavatrix(root, workload.query.clone(), workload.search_mode);
            assert_eq!(output, expected);
            files = searched;
            ours.push(elapsed);
            let (elapsed, output) = timed_ripgrep(
                root,
                workload.patterns,
                workload.fixed,
                workload.search_mode,
            );
            assert_eq!(output, expected);
            rg.push(elapsed);
        } else {
            let (elapsed, output) = timed_ripgrep(
                root,
                workload.patterns,
                workload.fixed,
                workload.search_mode,
            );
            assert_eq!(output, expected);
            rg.push(elapsed);
            let (elapsed, output, searched) =
                timed_weavatrix(root, workload.query.clone(), workload.search_mode);
            assert_eq!(output, expected);
            files = searched;
            ours.push(elapsed);
        }
    }
    println!(
        "mode={} engine=weavatrix-search files={files} matching_lines={} median_ms={:.3}",
        workload.mode,
        expected.len(),
        millis(median(&mut ours))
    );
    println!(
        "mode={} engine=ripgrep-cli files={files} matching_lines={} median_ms={:.3}",
        workload.mode,
        expected.len(),
        millis(median(&mut rg))
    );
}

fn profile_count(root: &Path, runs: usize, warmups: usize) {
    let expected = weavatrix_count(root);
    assert_eq!(expected, ripgrep_count(root));
    let occurrences = expected.iter().map(|(_, count)| count).sum::<u64>();
    for _ in 0..warmups {
        black_box(weavatrix_count(root));
        black_box(ripgrep_count(root));
    }
    let mut ours = Vec::with_capacity(runs);
    let mut rg = Vec::with_capacity(runs);
    for index in 0..runs {
        if index % 2 == 0 {
            ours.push(timed(|| weavatrix_count(root), &expected));
            rg.push(timed(|| ripgrep_count(root), &expected));
        } else {
            rg.push(timed(|| ripgrep_count(root), &expected));
            ours.push(timed(|| weavatrix_count(root), &expected));
        }
    }
    println!(
        "mode=count engine=weavatrix-search matched_files={} occurrences={} median_ms={:.3}",
        expected.len(),
        occurrences,
        millis(median(&mut ours))
    );
    println!(
        "mode=count engine=ripgrep-cli matched_files={} occurrences={} median_ms={:.3}",
        expected.len(),
        occurrences,
        millis(median(&mut rg))
    );
}

fn profile_files(root: &Path, runs: usize, warmups: usize) {
    let expected = weavatrix_files(root);
    assert_eq!(expected, ripgrep_files(root));
    for _ in 0..warmups {
        black_box(weavatrix_files(root));
        black_box(ripgrep_files(root));
    }
    let mut ours = Vec::with_capacity(runs);
    let mut rg = Vec::with_capacity(runs);
    for index in 0..runs {
        if index % 2 == 0 {
            ours.push(timed(|| weavatrix_files(root), &expected));
            rg.push(timed(|| ripgrep_files(root), &expected));
        } else {
            rg.push(timed(|| ripgrep_files(root), &expected));
            ours.push(timed(|| weavatrix_files(root), &expected));
        }
    }
    println!(
        "mode=files engine=weavatrix-search matched_files={} median_ms={:.3}",
        expected.len(),
        millis(median(&mut ours))
    );
    println!(
        "mode=files engine=ripgrep-cli matched_files={} median_ms={:.3}",
        expected.len(),
        millis(median(&mut rg))
    );
}

fn timed<T: PartialEq>(operation: impl FnOnce() -> T, expected: &T) -> Duration {
    let started = Instant::now();
    let output = operation();
    let elapsed = started.elapsed();
    assert!(&output == expected);
    elapsed
}

fn weavatrix_count(root: &Path) -> Vec<(String, u64)> {
    let report = Searcher::new(root, SearchQuery::literal("needle_target"))
        .options(
            SearchOptions::default()
                .with_result_mode(ResultMode::Count)
                .with_max_results(usize::MAX),
        )
        .search()
        .expect("weavatrix count search");
    report
        .matched_files
        .into_iter()
        .map(|file| (file.path, file.occurrences))
        .collect()
}

fn ripgrep_count(root: &Path) -> Vec<(String, u64)> {
    let output = Command::new("rg")
        .current_dir(root)
        .args([
            "--count-matches",
            "--with-filename",
            "--no-messages",
            "--color",
            "never",
            "--no-require-git",
            "--fixed-strings",
            "--regexp",
            "needle_target",
            ".",
        ])
        .output()
        .expect("ripgrep is required for this benchmark");
    assert!(output.status.success());
    let mut counts = String::from_utf8(output.stdout)
        .expect("ripgrep count output is UTF-8")
        .lines()
        .filter_map(|line| line.rsplit_once(':'))
        .map(|(path, count)| {
            (
                normalize_path(path),
                count.parse::<u64>().expect("ripgrep count is numeric"),
            )
        })
        .collect::<Vec<_>>();
    counts.sort();
    counts
}

fn weavatrix_files(root: &Path) -> Vec<String> {
    Searcher::new(root, SearchQuery::literal("needle_target"))
        .options(
            SearchOptions::default()
                .with_result_mode(ResultMode::Files)
                .with_max_results(usize::MAX),
        )
        .search()
        .expect("weavatrix files search")
        .matched_files
        .into_iter()
        .map(|file| file.path)
        .collect()
}

fn ripgrep_files(root: &Path) -> Vec<String> {
    let output = Command::new("rg")
        .current_dir(root)
        .args([
            "--files-with-matches",
            "--no-messages",
            "--color",
            "never",
            "--no-require-git",
            "--fixed-strings",
            "--regexp",
            "needle_target",
            ".",
        ])
        .output()
        .expect("ripgrep is required for this benchmark");
    assert!(output.status.success());
    let mut files = String::from_utf8(output.stdout)
        .expect("ripgrep file output is UTF-8")
        .lines()
        .map(normalize_path)
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn timed_weavatrix(
    root: &Path,
    query: SearchQuery,
    search_mode: SearchMode,
) -> (Duration, Vec<Signature>, u64) {
    let started = Instant::now();
    let (files, output) = weavatrix(root, query, search_mode);
    (started.elapsed(), output, files)
}

fn weavatrix(root: &Path, query: SearchQuery, search_mode: SearchMode) -> (u64, Vec<Signature>) {
    let report = Searcher::new(root, query)
        .scan_options(benchmark_scan_options().with_skip_hidden(true))
        .options(
            SearchOptions::default()
                .with_case(CaseMode::Sensitive)
                .with_mode(search_mode)
                .with_max_results(usize::MAX),
        )
        .search()
        .expect("weavatrix search");
    let signatures = report
        .matches
        .into_iter()
        .map(|found| Signature {
            path: found.path,
            line: found.line_number,
            spans: found
                .spans
                .into_iter()
                .map(|span| (span.start, span.end))
                .collect(),
        })
        .collect();
    (report.files_searched, signatures)
}

fn timed_ripgrep(
    root: &Path,
    patterns: &[&str],
    fixed: bool,
    search_mode: SearchMode,
) -> (Duration, Vec<Signature>) {
    let started = Instant::now();
    let (_, output) = ripgrep(root, patterns, fixed, search_mode);
    (started.elapsed(), output)
}

fn ripgrep(
    root: &Path,
    patterns: &[&str],
    fixed: bool,
    search_mode: SearchMode,
) -> (u64, Vec<Signature>) {
    let mut command = Command::new("rg");
    command.current_dir(root).args([
        "--json",
        "--no-messages",
        "--color",
        "never",
        "--no-require-git",
    ]);
    if fixed {
        command.arg("--fixed-strings");
    }
    if search_mode == SearchMode::Multiline {
        command.arg("--multiline");
    }
    for pattern in patterns {
        command.arg("--regexp").arg(pattern);
    }
    let output = command
        .arg(".")
        .output()
        .expect("ripgrep is required for this benchmark; install rg and retry");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut searched = 0_u64;
    let mut signatures = Vec::new();
    for line in String::from_utf8(output.stdout).unwrap().lines() {
        let value: Value = serde_json::from_str(line).unwrap();
        if value["type"] == "match" {
            let data = &value["data"];
            signatures.push(Signature {
                path: normalize_path(data["path"]["text"].as_str().unwrap()),
                line: data["line_number"].as_u64().unwrap(),
                spans: data["submatches"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|matched| {
                        (
                            usize::try_from(matched["start"].as_u64().unwrap()).unwrap(),
                            usize::try_from(matched["end"].as_u64().unwrap()).unwrap(),
                        )
                    })
                    .collect(),
            });
        } else if value["type"] == "summary" {
            searched = value["data"]["stats"]["searches"].as_u64().unwrap_or(0);
        }
    }
    signatures.sort();
    (searched, signatures)
}

fn normalize_path(path: &str) -> String {
    let path = path.replace('\\', "/");
    path.strip_prefix("./").unwrap_or(&path).to_owned()
}

fn median(values: &mut [Duration]) -> Duration {
    values.sort_unstable();
    values[values.len() / 2]
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}
