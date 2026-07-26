use std::fs;
#[cfg(feature = "archives")]
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use weavatrix_search::{
    CaseMode, EncodingMode, Error, ResultMode, SearchMode, SearchOptions, SearchQuery,
    SearchWarningKind, Searcher,
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

#[test]
fn auto_encoding_detects_utf16_bom() {
    let repo = TempRepo::new("utf16");
    let mut bytes = vec![0xFF, 0xFE];
    for unit in "first\nneedle here\n".encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    repo.write("utf16.txt", bytes);

    let report = Searcher::new(repo.path(), SearchQuery::literal("needle"))
        .options(
            SearchOptions::default()
                .with_encoding(EncodingMode::Auto)
                .with_context(1, 0),
        )
        .search()
        .unwrap();

    assert_eq!(report.matches.len(), 1);
    assert_eq!(report.matches[0].line_number, 2);
    assert_eq!(report.matches[0].encoding, "UTF-16LE");
    assert_eq!(report.matches[0].before[0].text, "first");
    assert_eq!(report.matches[0].decoded_byte_offset, 6);
    assert_eq!(report.matches[0].source_byte_offset, None);
}

#[test]
fn utf8_bom_preserves_exact_source_offset() {
    let repo = TempRepo::new("utf8-bom");
    repo.write("bom.txt", b"\xEF\xBB\xBFneedle\n");

    let report = Searcher::new(repo.path(), SearchQuery::literal("needle"))
        .search()
        .unwrap();

    assert_eq!(report.matches[0].decoded_byte_offset, 0);
    assert_eq!(report.matches[0].source_byte_offset, Some(3));
}

#[test]
fn binary_files_are_skipped_with_typed_warning() {
    let repo = TempRepo::new("binary");
    repo.write("binary.bin", b"needle\0more");

    let report = Searcher::new(repo.path(), SearchQuery::literal("needle"))
        .search()
        .unwrap();

    assert!(report.matches.is_empty());
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.kind == SearchWarningKind::Binary)
    );
}

#[test]
fn warning_limit_is_bounded_and_deterministic() {
    let repo = TempRepo::new("warning-limit");
    repo.write("z.bin", b"\0");
    repo.write("a.bin", b"\0");
    repo.write("m.bin", b"\0");

    let report = Searcher::new(repo.path(), SearchQuery::literal("needle"))
        .options(SearchOptions::default().with_max_warnings(1))
        .search()
        .unwrap();

    assert_eq!(report.warnings.len(), 1);
    assert_eq!(report.warnings[0].path, "a.bin");
    assert_eq!(report.warnings_dropped, 2);
}

#[cfg(feature = "archives")]
#[test]
fn searches_zip_tar_and_gzip_without_extracting() {
    let repo = TempRepo::new("archives");

    let zip_path = repo.path().join("sample.zip");
    let zip_file = fs::File::create(&zip_path).unwrap();
    let mut zip = zip::ZipWriter::new(zip_file);
    zip.start_file("nested/code.txt", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(b"zip needle\n").unwrap();
    zip.start_file(
        "nested/binary.bin",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    zip.write_all(b"needle\0binary").unwrap();
    zip.finish().unwrap();

    let tar_path = repo.path().join("sample.tar");
    let tar_file = fs::File::create(&tar_path).unwrap();
    let mut tar = tar::Builder::new(tar_file);
    // Keep the outer archive above Scanner's repository-oriented 1.5 MiB
    // default so Searcher's archive limit is exercised as well.
    let mut body = vec![b'x'; 2 * 1024 * 1024];
    body.extend_from_slice(b"tar needle\n");
    let mut header = tar::Header::new_gnu();
    header.set_size(u64::try_from(body.len()).unwrap());
    header.set_mode(0o644);
    header.set_cksum();
    tar.append_data(&mut header, "nested/code.txt", body.as_slice())
        .unwrap();
    tar.finish().unwrap();
    drop(tar);

    let gzip_path = repo.path().join("sample.txt.gz");
    let gzip_file = fs::File::create(&gzip_path).unwrap();
    let mut gzip = flate2::write::GzEncoder::new(gzip_file, flate2::Compression::fast());
    gzip.write_all(b"gzip needle\n").unwrap();
    gzip.finish().unwrap();

    let report = Searcher::new(repo.path(), SearchQuery::literal("needle"))
        .search()
        .unwrap();

    let paths = report
        .matches
        .iter()
        .map(|found| found.path.as_str())
        .collect::<Vec<_>>();
    assert!(
        paths
            .iter()
            .any(|path| path.contains("sample.zip!nested/code.txt"))
    );
    assert!(
        paths
            .iter()
            .any(|path| path.contains("sample.tar!nested/code.txt")),
        "{paths:?}; warnings={:?}",
        report.warnings
    );
    assert!(
        paths
            .iter()
            .any(|path| path.contains("sample.txt.gz!sample.txt"))
    );
    assert!(paths.iter().all(|path| !path.contains("nested/binary.bin")));
    assert!(report.warnings.iter().any(|warning| {
        warning.kind == SearchWarningKind::Binary
            && warning.path.contains("sample.zip!nested/binary.bin")
    }));
    assert!(report.matches.iter().all(|found| found.archive));
}
