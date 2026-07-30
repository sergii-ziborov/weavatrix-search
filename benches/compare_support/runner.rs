use super::{
    Arc, AtomicU64, CaseMode, FileEvidenceMode, Instant, MARKER, Ordering, Path, SearchMode,
    SearchOptions, SearchQuery, Searcher, Workload, benchmark_scan_options, black_box, env_usize,
    median, millis, profile, profile_count, profile_files, scan_only, timed,
};

pub(super) fn run(root: &Path) {
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

pub(super) fn run_literal(root: &Path) {
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
