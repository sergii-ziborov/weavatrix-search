use super::support::{TempRepo, matched_paths, scan_options};
use std::fs;
use weavatrix_search::{
    CaseMode, FileEvidenceMode, IndexBuilder, IndexOptions, PersistentIndex, SearchBackend,
    SearchOptions, SearchQuery, Searcher,
};

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
fn all_file_evidence_disables_index_prefilter_without_losing_metrics() {
    let repo = TempRepo::new("file-evidence");
    repo.write("match.txt", b"needle\nsecond");
    repo.write("miss.txt", b"unrelated\n");
    repo.write("empty.txt", b"");
    let (index, build) =
        PersistentIndex::build([repo.path()], scan_options(), IndexOptions::default()).unwrap();

    let report = index
        .search(
            SearchQuery::literal("needle"),
            SearchOptions::default()
                .with_file_evidence(FileEvidenceMode::All)
                .with_max_file_evidence(10),
        )
        .unwrap();

    let index_evidence = report.index.as_ref().unwrap();
    assert!(!index_evidence.prefiltered);
    assert_eq!(index_evidence.candidate_files, build.files);
    assert_eq!(report.file_evidence.len(), 3);
    assert_eq!(
        report
            .file_evidence
            .iter()
            .map(|evidence| (evidence.path.as_str(), evidence.total_lines))
            .collect::<Vec<_>>(),
        [("empty.txt", 0), ("match.txt", 2), ("miss.txt", 1)]
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
