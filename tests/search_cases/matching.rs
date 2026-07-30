use super::*;

#[test]
fn literal_search_streams_long_lines_and_keeps_context() {
    let repo = TempRepo::new("literal");
    let mut content = b"before\n".to_vec();
    content.extend(std::iter::repeat_n(b'x', 65_534));
    content.extend_from_slice(b"needle\n");
    content.extend_from_slice(b"after\n");
    repo.write("src/long.txt", content);

    let report = Searcher::new(repo.path(), SearchQuery::literal("needle"))
        .options(SearchOptions::default().with_context(1, 1))
        .search()
        .unwrap();

    assert_eq!(report.matching_lines, 1);
    assert_eq!(report.occurrences, 1);
    assert_eq!(report.matches[0].path, "src/long.txt");
    assert_eq!(report.matches[0].line_number, 2);
    assert_eq!(report.matches[0].before[0].text, "before");
    assert_eq!(report.matches[0].after[0].text, "after");
    assert_eq!(report.matches[0].spans[0].start, 65_534);
    assert_eq!(report.matches[0].decoded_byte_offset, 7);
    assert_eq!(report.matches[0].source_byte_offset, Some(7));
}

#[test]
fn adaptive_scan_profile_streams_broad_roots_and_buffers_repositories() {
    let broad = TempRepo::new("adaptive-broad");
    let repository = TempRepo::new("adaptive-repository");
    fs::create_dir(repository.path().join(".git")).unwrap();
    let options = SearchOptions::default();

    let broad_profile = recommended_scan_options(&[broad.path().to_path_buf()], &options);
    let repository_profile = recommended_scan_options(&[repository.path().to_path_buf()], &options);

    assert_eq!(
        broad_profile.content_discovery,
        if cfg!(windows) {
            ContentDiscoveryMode::Streaming
        } else {
            ContentDiscoveryMode::BufferedParallel
        }
    );
    assert_eq!(
        repository_profile.content_discovery,
        ContentDiscoveryMode::BufferedParallel
    );
    if cfg!(windows) {
        assert_eq!(broad_profile.content_parallelism, Some(32));
        assert_eq!(repository_profile.content_parallelism, Some(8));
    }
}

#[test]
fn file_evidence_is_single_pass_deterministic_and_streamable() {
    let repo = TempRepo::new("file-evidence");
    repo.write("z.txt", b"miss\n");
    repo.write("a.txt", b"needle\nsecond");
    repo.write("empty.txt", b"");
    let streamed = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&streamed);

    let report = Searcher::new(repo.path(), SearchQuery::literal("needle"))
        .options(
            SearchOptions::default()
                .with_file_evidence(FileEvidenceMode::All)
                .with_max_file_evidence(2)
                .with_file_evidence_visitor(move |evidence| {
                    sink.lock().unwrap().push(evidence.clone());
                }),
        )
        .search()
        .unwrap();

    assert_eq!(report.files_searched, 3);
    assert!(report.file_evidence_truncated);
    assert_eq!(
        report
            .file_evidence
            .iter()
            .map(|evidence| evidence.path.as_str())
            .collect::<Vec<_>>(),
        ["a.txt", "empty.txt"]
    );
    assert_eq!(report.file_evidence[0].source_bytes, 13);
    assert_eq!(report.file_evidence[0].total_lines, 2);
    assert_eq!(report.file_evidence[0].matching_lines, 1);
    assert_eq!(report.file_evidence[1].source_bytes, 0);
    assert_eq!(report.file_evidence[1].total_lines, 0);

    let mut streamed = Arc::try_unwrap(streamed).unwrap().into_inner().unwrap();
    streamed.sort_unstable_by(|left, right| left.path.cmp(&right.path));
    assert_eq!(streamed.len(), 3);
    assert_eq!(streamed[2].path, "z.txt");
    assert_eq!(streamed[2].total_lines, 1);

    let matched = Searcher::new(repo.path(), SearchQuery::literal("needle"))
        .options(
            SearchOptions::default()
                .with_file_evidence(FileEvidenceMode::Matched)
                .with_max_file_evidence(10),
        )
        .search()
        .unwrap();
    assert_eq!(matched.file_evidence.len(), 1);
    assert_eq!(matched.file_evidence[0].path, "a.txt");
}

#[test]
fn regex_case_and_ignore_policy_match_repository_expectations() {
    let repo = TempRepo::new("regex");
    repo.write(".gitignore", b"ignored.txt\n");
    repo.write("kept.txt", b"Alpha 123\nalpha 456\n");
    repo.write("ignored.txt", b"Alpha 999\n");

    let report = Searcher::new(repo.path(), SearchQuery::regex(r"alpha\s+\d+"))
        .options(
            SearchOptions::default()
                .with_case(CaseMode::Insensitive)
                .with_max_results(10),
        )
        .search()
        .unwrap();

    assert_eq!(report.matches.len(), 2);
    assert!(report.matches.iter().all(|found| found.path == "kept.txt"));
    assert_eq!(report.occurrences, 2);
}

#[test]
fn ordered_multi_pattern_query_uses_one_pass_and_reports_pattern_ids() {
    let repo = TempRepo::new("multi-pattern");
    repo.write("matches.txt", b"alpha 123 beta\nfoobar\n");

    let report = Searcher::new(
        repo.path(),
        SearchQuery::any([
            SearchQuery::literal("beta"),
            SearchQuery::regex(r"alpha\s+\d+"),
            SearchQuery::literal("foo"),
            SearchQuery::literal("foobar"),
        ]),
    )
    .search()
    .unwrap();

    assert_eq!(report.matching_lines, 2);
    assert_eq!(report.occurrences, 3);
    assert_eq!(
        report.matches[0]
            .spans
            .iter()
            .map(|span| (span.pattern_index, span.start, span.end))
            .collect::<Vec<_>>(),
        vec![(1, 0, 9), (0, 10, 14)]
    );
    assert_eq!(
        report.matches[1]
            .spans
            .iter()
            .map(|span| (span.pattern_index, span.start, span.end))
            .collect::<Vec<_>>(),
        vec![(2, 0, 3)]
    );
}

#[test]
fn empty_multi_pattern_query_is_rejected() {
    let repo = TempRepo::new("empty-multi");
    let error = Searcher::new(repo.path(), SearchQuery::any([]))
        .search()
        .unwrap_err();
    assert!(matches!(error, Error::EmptyQuery));
}
