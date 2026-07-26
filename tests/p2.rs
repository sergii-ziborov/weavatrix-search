use serde_json::Value;
use std::fs;
#[cfg(feature = "archives")]
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use weavatrix_search::{
    ColorChoice, OutputFormat, OutputOptions, ResultMode, SearchOptions, SearchQuery,
    SearchWarningKind, Searcher, write_report, write_report_with,
};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("weavatrix-search-p2-{}-{id}", std::process::id()));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("src/people.txt"),
            "Ada Lovelace and Grace Hopper\n",
        )
        .unwrap();
        fs::write(root.join("src/other.txt"), "ordinary\n").unwrap();
        Self { root }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

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

#[cfg(feature = "archives")]
#[test]
fn searches_pure_rust_bzip2_zstd_lz4_lzma_xz_and_brotli_streams() {
    let fixture = Fixture::new();
    let payload = b"compressed needle with enough symbols 1234567890\n";

    let bzip_path = fixture.root.join("data.bz2");
    let bzip_source = fixture.root.join("bzip-source.txt");
    fs::write(&bzip_source, payload).unwrap();
    banzai::encode(
        BufReader::new(fs::File::open(&bzip_source).unwrap()),
        BufWriter::new(fs::File::create(&bzip_path).unwrap()),
        1,
    )
    .unwrap();
    fs::remove_file(bzip_source).unwrap();

    let zstd = ruzstd::encoding::compress_to_vec(
        payload.as_slice(),
        ruzstd::encoding::CompressionLevel::Fastest,
    );
    fs::write(fixture.root.join("data.zst"), zstd).unwrap();

    let lz4_path = fixture.root.join("data.lz4");
    let mut lz4 = lz4_flex::frame::FrameEncoder::new(fs::File::create(lz4_path).unwrap());
    lz4.write_all(payload).unwrap();
    lz4.finish().unwrap();

    let mut lzma = Vec::new();
    lzma_rs::lzma_compress(&mut std::io::Cursor::new(payload), &mut lzma).unwrap();
    fs::write(fixture.root.join("data.lzma"), lzma).unwrap();

    let mut xz = Vec::new();
    lzma_rs::xz_compress(&mut std::io::Cursor::new(payload), &mut xz).unwrap();
    fs::write(fixture.root.join("data.xz"), xz).unwrap();

    let brotli_path = fixture.root.join("data.br");
    let mut brotli =
        brotli::CompressorWriter::new(fs::File::create(brotli_path).unwrap(), 4096, 5, 22);
    brotli.write_all(payload).unwrap();
    drop(brotli);

    let report = Searcher::new(&fixture.root, SearchQuery::literal("needle"))
        .options(SearchOptions::default().with_max_results(20))
        .search()
        .unwrap();
    let paths = report
        .matches
        .iter()
        .map(|found| found.path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        vec![
            "data.br!data",
            "data.bz2!data",
            "data.lz4!data",
            "data.lzma!data",
            "data.xz!data",
            "data.zst!data",
        ],
        paths
    );
}

#[cfg(feature = "archives")]
#[test]
fn xz_decoder_memory_is_bounded_before_allocation() {
    let fixture = Fixture::new();
    let mut xz = Vec::new();
    lzma_rs::xz_compress(&mut std::io::Cursor::new(b"bounded needle\n"), &mut xz).unwrap();
    fs::write(fixture.root.join("data.xz"), xz).unwrap();

    let mut options = SearchOptions::default();
    options.archives.max_decoder_memory_bytes = 1024;
    let report = Searcher::new(&fixture.root, SearchQuery::literal("needle"))
        .options(options)
        .search()
        .unwrap();
    assert!(report.matches.is_empty());
    assert!(
        report
            .warnings
            .iter()
            .any(|warning| warning.kind == SearchWarningKind::Limit)
    );
}

#[cfg(feature = "archives")]
#[test]
fn searches_concatenated_xz_streams_and_tar_xz_members() {
    let fixture = Fixture::new();
    let mut first = Vec::new();
    let mut second = Vec::new();
    lzma_rs::xz_compress(&mut std::io::Cursor::new(b"first needle\n"), &mut first).unwrap();
    lzma_rs::xz_compress(&mut std::io::Cursor::new(b"second needle\n"), &mut second).unwrap();
    first.extend_from_slice(&[0, 0, 0, 0]);
    first.extend_from_slice(&second);
    fs::write(fixture.root.join("joined.xz"), first).unwrap();

    let mut tar = tar::Builder::new(Vec::new());
    let content = b"tar xz needle\n";
    let mut header = tar::Header::new_gnu();
    header.set_size(u64::try_from(content.len()).unwrap());
    header.set_mode(0o644);
    header.set_cksum();
    tar.append_data(&mut header, "nested/code.txt", content.as_slice())
        .unwrap();
    let tar = tar.into_inner().unwrap();
    let mut tar_xz = Vec::new();
    lzma_rs::xz_compress(&mut std::io::Cursor::new(tar), &mut tar_xz).unwrap();
    fs::write(fixture.root.join("bundle.tar.xz"), tar_xz).unwrap();

    let report = Searcher::new(&fixture.root, SearchQuery::literal("needle"))
        .search()
        .unwrap();
    assert_eq!(
        2,
        report
            .matches
            .iter()
            .filter(|found| found.path == "joined.xz!joined")
            .count()
    );
    assert!(
        report
            .matches
            .iter()
            .any(|found| found.path == "bundle.tar.xz!nested/code.txt")
    );
}

fn path_text(path: &Path) -> &str {
    path.to_str().unwrap()
}
