use super::*;

#[test]
fn replacement_preview_supports_named_captures_without_writing() {
    let fixture = Fixture::new();
    let source = fixture.root.join("src/people.txt");
    let original = fs::read(&source).unwrap();
    let report = Searcher::new(
        &fixture.root,
        SearchQuery::regex(r"(?P<first>[A-Z][a-z]+) (?P<last>[A-Z][a-z]+)"),
    )
    .options(SearchOptions::default().with_replacement("$last, $first"))
    .search()
    .unwrap();

    assert_eq!(1, report.matches.len());
    assert_eq!(
        Some("Lovelace, Ada and Hopper, Grace"),
        report.matches[0].replacement_preview.as_deref()
    );
    assert_eq!(original, fs::read(source).unwrap());
}

#[test]
fn literal_replacement_and_preview_limit_are_bounded() {
    let fixture = Fixture::new();
    let report = Searcher::new(&fixture.root, SearchQuery::literal("Ada"))
        .options(SearchOptions::default().with_replacement("<$0> $$"))
        .search()
        .unwrap();
    assert_eq!(
        Some("<Ada> $ Lovelace and Grace Hopper"),
        report.matches[0].replacement_preview.as_deref()
    );

    let limited = Searcher::new(&fixture.root, SearchQuery::literal("Ada"))
        .options(
            SearchOptions::default()
                .with_replacement("replacement")
                .with_max_replacement_bytes(4),
        )
        .search()
        .unwrap();
    assert_eq!(None, limited.matches[0].replacement_preview);
    assert!(
        limited
            .warnings
            .iter()
            .any(|warning| warning.kind == SearchWarningKind::Limit)
    );
}

#[test]
fn text_and_json_adapters_preserve_mode_and_evidence() {
    let fixture = Fixture::new();
    let count = Searcher::new(&fixture.root, SearchQuery::literal("Ada"))
        .options(
            SearchOptions::default()
                .with_result_mode(ResultMode::Count)
                .with_max_results(10),
        )
        .search()
        .unwrap();
    assert_eq!(ResultMode::Count, count.result_mode);
    assert_eq!(1, count.matched_files.len());

    let mut text = Vec::new();
    write_report(&count, OutputFormat::Text, &mut text).unwrap();
    assert_eq!("src/people.txt:1\n", String::from_utf8(text).unwrap());

    let mut json = Vec::new();
    write_report(&count, OutputFormat::JsonLines, &mut json).unwrap();
    let records = String::from_utf8(json)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!("file", records[0]["type"]);
    assert_eq!("summary", records.last().unwrap()["type"]);
    assert_eq!(2, records.last().unwrap()["data"]["files_searched"]);
}

#[test]
fn cli_has_stable_output_and_exit_codes() {
    let fixture = Fixture::new();
    let binary = env!("CARGO_BIN_EXE_weavatrix-search");

    let matched = Command::new(binary)
        .args([
            "--fixed-strings",
            "--replace",
            "<$0>",
            "Ada",
            path_text(&fixture.root),
        ])
        .output()
        .unwrap();
    assert_eq!(Some(0), matched.status.code());
    assert_eq!(
        "src/people.txt:1:<Ada> Lovelace and Grace Hopper\n",
        String::from_utf8(matched.stdout).unwrap()
    );

    let missing = Command::new(binary)
        .args(["--fixed-strings", "absent", path_text(&fixture.root)])
        .output()
        .unwrap();
    assert_eq!(Some(1), missing.status.code());

    let invalid = Command::new(binary)
        .args(["(", path_text(&fixture.root)])
        .output()
        .unwrap();
    assert_eq!(Some(2), invalid.status.code());
    assert!(
        String::from_utf8(invalid.stderr)
            .unwrap()
            .contains("invalid regular expression")
    );
}

#[test]
fn cli_exposes_discovery_and_reader_controls() {
    let fixture = Fixture::new();
    let binary = env!("CARGO_BIN_EXE_weavatrix-search");

    for discovery in ["adaptive", "streaming", "buffered"] {
        let output = Command::new(binary)
            .args([
                "--fixed-strings",
                "--discovery",
                discovery,
                "--content-workers",
                "2",
                "Ada",
                path_text(&fixture.root),
            ])
            .output()
            .unwrap();
        assert_eq!(Some(0), output.status.code(), "{discovery}");
    }

    let invalid = Command::new(binary)
        .args(["--content-workers", "0", "Ada", path_text(&fixture.root)])
        .output()
        .unwrap();
    assert_eq!(Some(2), invalid.status.code());
    assert!(
        String::from_utf8(invalid.stderr)
            .unwrap()
            .contains("must be greater than zero")
    );
}

#[test]
fn multi_root_cli_and_library_preserve_root_identity() {
    let first = Fixture::new();
    let second = Fixture::new();
    fs::write(second.root.join("src/people.txt"), "Ada Byron\n").unwrap();

    let report = Searcher::new(&first.root, SearchQuery::literal("Ada"))
        .add_root(&second.root)
        .search()
        .unwrap();
    assert_eq!(2, report.roots.len());
    assert_eq!(
        vec![0, 1],
        report
            .matches
            .iter()
            .map(|found| found.root_index)
            .collect::<Vec<_>>()
    );
    assert_eq!(2, report.scan.reports.len());

    let output = Command::new(env!("CARGO_BIN_EXE_weavatrix-search"))
        .args([
            "--fixed-strings",
            "-e",
            "Ada",
            path_text(&first.root),
            path_text(&second.root),
        ])
        .output()
        .unwrap();
    assert_eq!(Some(0), output.status.code());
    let stdout = String::from_utf8(output.stdout).unwrap().replace('\\', "/");
    let first_path = first
        .root
        .join("src/people.txt")
        .to_string_lossy()
        .replace('\\', "/");
    let second_path = second
        .root
        .join("src/people.txt")
        .to_string_lossy()
        .replace('\\', "/");
    assert_eq!(
        format!("{first_path}:1:Ada Lovelace and Grace Hopper\n{second_path}:1:Ada Byron\n"),
        stdout
    );
}

#[test]
fn extended_text_output_supports_heading_column_only_match_color_and_null() {
    let fixture = Fixture::new();
    let report = Searcher::new(&fixture.root, SearchQuery::regex("Ada|Grace"))
        .search()
        .unwrap();
    let mut text = Vec::new();
    write_report_with(
        &report,
        &OutputOptions {
            heading: true,
            column: true,
            only_matching: true,
            color: ColorChoice::Never,
            ..OutputOptions::default()
        },
        &mut text,
    )
    .unwrap();
    assert_eq!(
        "src/people.txt\n1:1:Ada\n1:18:Grace\n",
        String::from_utf8(text).unwrap()
    );

    let files = Searcher::new(&fixture.root, SearchQuery::literal("Ada"))
        .options(SearchOptions::default().with_result_mode(ResultMode::Files))
        .search()
        .unwrap();
    let mut nul = Vec::new();
    write_report_with(
        &files,
        &OutputOptions {
            null: true,
            ..OutputOptions::default()
        },
        &mut nul,
    )
    .unwrap();
    assert_eq!(b"src/people.txt\0", nul.as_slice());
}
