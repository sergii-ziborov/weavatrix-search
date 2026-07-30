use super::*;

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
                .with_file_evidence(FileEvidenceMode::All)
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
    assert_eq!(report.file_evidence[0].encoding, "UTF-16LE");
    assert_eq!(report.file_evidence[0].source_bytes, 38);
    assert_eq!(report.file_evidence[0].total_lines, 2);
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
        .options(
            SearchOptions::default()
                .with_file_evidence(FileEvidenceMode::All)
                .with_max_file_evidence(10),
        )
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
    assert_eq!(report.file_evidence.len(), 3);
    assert!(
        report
            .file_evidence
            .iter()
            .all(|evidence| evidence.archive && evidence.total_lines >= 1)
    );
}

#[cfg(feature = "archives")]
#[test]
fn archive_evidence_preserves_multi_root_identity() {
    let first = TempRepo::new("archive-root-first");
    let second = TempRepo::new("archive-root-second");
    first.write("plain.txt", b"unrelated\n");
    let archive_path = second.path().join("source.zip");
    let archive_file = fs::File::create(archive_path).unwrap();
    let mut archive = zip::ZipWriter::new(archive_file);
    archive
        .start_file("member.txt", zip::write::SimpleFileOptions::default())
        .unwrap();
    archive.write_all(b"needle\n").unwrap();
    archive.finish().unwrap();

    let report = Searcher::new(first.path(), SearchQuery::literal("needle"))
        .add_root(second.path())
        .options(SearchOptions::default().with_file_evidence(FileEvidenceMode::All))
        .search()
        .unwrap();

    let member = report
        .file_evidence
        .iter()
        .find(|evidence| evidence.archive)
        .unwrap();
    assert_eq!(member.root_index, 1);
    assert_eq!(member.total_lines, 1);
}
