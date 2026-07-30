use super::{
    CaseMode, Duration, IndexOptions, Instant, MARKER, Path, PersistentIndex, SearchMode,
    SearchOptions, SearchQuery, Signature, WatchEvent, WatchEventKind, benchmark_scan_options,
    black_box, env_usize, fs, median, millis, ripgrep, timed_ripgrep,
};

pub(super) fn run_index(root: &Path) {
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

pub(super) fn run_index_open(root: &Path, index_path: &Path) {
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
