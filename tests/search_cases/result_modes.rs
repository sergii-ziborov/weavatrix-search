use super::*;

#[test]
fn multiline_query_merges_overlapping_line_blocks_and_keeps_context() {
    let repo = TempRepo::new("multiline");
    repo.write("multi.txt", b"before\nfoo\nbar baz\nafter\n");

    let report = Searcher::new(
        repo.path(),
        SearchQuery::any([SearchQuery::regex("foo\\nbar"), SearchQuery::literal("baz")]),
    )
    .options(
        SearchOptions::default()
            .with_mode(SearchMode::Multiline)
            .with_file_evidence(FileEvidenceMode::All)
            .with_context(1, 1),
    )
    .search()
    .unwrap();

    assert_eq!(report.matching_lines, 2);
    assert_eq!(report.occurrences, 2);
    assert_eq!(report.matches.len(), 1);
    let found = &report.matches[0];
    assert_eq!(found.line_number, 2);
    assert_eq!(found.end_line_number, 3);
    assert_eq!(found.decoded_byte_offset, 7);
    assert_eq!(found.source_byte_offset, Some(7));
    assert_eq!(found.line, "foo\nbar baz\n");
    assert_eq!(
        found
            .spans
            .iter()
            .map(|span| (span.pattern_index, span.start, span.end))
            .collect::<Vec<_>>(),
        vec![(0, 0, 7), (1, 8, 11)]
    );
    assert_eq!(found.before[0].text, "before");
    assert_eq!(found.after[0].text, "after");
    assert_eq!(report.file_evidence[0].total_lines, 4);
    assert_eq!(report.file_evidence[0].source_bytes, 25);
}

#[test]
fn multiline_buffer_limit_skips_large_sources_with_typed_warning() {
    let repo = TempRepo::new("multiline-limit");
    repo.write("large.txt", b"needle\n");

    let report = Searcher::new(repo.path(), SearchQuery::literal("needle"))
        .options(
            SearchOptions::default()
                .with_mode(SearchMode::Multiline)
                .with_max_multiline_bytes(4),
        )
        .search()
        .unwrap();

    assert!(report.matches.is_empty());
    assert!(report.warnings.iter().any(|warning| {
        warning.path == "large.txt" && warning.kind == SearchWarningKind::Limit
    }));
}

#[test]
fn count_mode_keeps_complete_aggregates_without_match_records() {
    let repo = TempRepo::new("count-mode");
    repo.write("a.txt", b"hit\nhit hit\n");
    repo.write("b.txt", b"ordinary\n");
    repo.write("c.txt", b"hit\n");

    let report = Searcher::new(repo.path(), SearchQuery::literal("hit"))
        .options(
            SearchOptions::default()
                .with_result_mode(ResultMode::Count)
                .with_max_results(1),
        )
        .search()
        .unwrap();

    assert!(report.matches.is_empty());
    assert_eq!(report.matched_files.len(), 1);
    assert_eq!(report.matched_files[0].path, "a.txt");
    assert_eq!(report.files_with_matches, 2);
    assert_eq!(report.matching_lines, 3);
    assert_eq!(report.occurrences, 4);
    assert!(report.truncated);
}

#[test]
fn files_mode_retains_deterministic_bounded_file_summaries() {
    let repo = TempRepo::new("files-mode");
    repo.write("z.txt", b"hit\n");
    repo.write("a.txt", b"hit\nhit hit\n");

    let report = Searcher::new(repo.path(), SearchQuery::literal("hit"))
        .options(
            SearchOptions::default()
                .with_result_mode(ResultMode::Files)
                .with_max_results(1),
        )
        .search()
        .unwrap();

    assert!(report.matches.is_empty());
    assert_eq!(report.files_with_matches, 2);
    assert_eq!(report.matching_lines, 3);
    assert_eq!(report.occurrences, 4);
    assert_eq!(report.matched_files.len(), 1);
    assert_eq!(report.matched_files[0].path, "a.txt");
    assert_eq!(report.matched_files[0].matching_lines, 2);
    assert_eq!(report.matched_files[0].occurrences, 3);
    assert!(report.truncated);
}

#[test]
fn quiet_mode_stops_after_one_observed_match() {
    let repo = TempRepo::new("quiet-mode");
    repo.write("a.txt", b"hit\n");
    repo.write("z.txt", b"hit\n");

    let report = Searcher::new(repo.path(), SearchQuery::literal("hit"))
        .options(SearchOptions::default().with_result_mode(ResultMode::Quiet))
        .search()
        .unwrap();

    assert!(report.matches.is_empty());
    assert!(report.matched_files.is_empty());
    assert_eq!(report.files_with_matches, 1);
    assert_eq!(report.matching_lines, 1);
    assert_eq!(report.occurrences, 1);
}

#[test]
fn deterministic_result_limit_keeps_lexically_first_match() {
    let repo = TempRepo::new("limit");
    repo.write("z.txt", b"hit\n");
    repo.write("a.txt", b"hit\n");
    repo.write("m.txt", b"hit\n");

    for _ in 0..5 {
        let report = Searcher::new(repo.path(), SearchQuery::literal("hit"))
            .options(SearchOptions::default().with_max_results(1))
            .search()
            .unwrap();
        assert_eq!(report.matches.len(), 1);
        assert_eq!(report.matches[0].path, "a.txt");
        assert_eq!(report.matching_lines, 3);
        assert!(report.truncated);
    }
}
