use super::*;

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
