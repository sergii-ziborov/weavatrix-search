use super::{
    Arc, AtomicU64, AtomicUsize, Collector, CompiledQuery, ContentVisitMode, ContentVisitReport,
    Error, FileProcessor, IndexSearchEvidence, MultiContentVisitReport, Ordering, PathBuf, Result,
    ScanCacheStats, ScanTermination, SearchBackend, SearchIdentity, SearchOptions, SearchQuery,
    SearchReport, handle_error, is_streaming_utf8,
};

pub(crate) struct IndexedContent<'a> {
    pub(crate) root_index: usize,
    pub(crate) path: &'a str,
    pub(crate) bytes: &'a [u8],
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn search_indexed(
    roots: Vec<PathBuf>,
    files: &[IndexedContent<'_>],
    query: &SearchQuery,
    options: SearchOptions,
    parallelism: usize,
    revision: String,
    indexed_files: u64,
    candidate_files: u64,
    prefiltered: bool,
) -> Result<SearchReport> {
    is_streaming_utf8(&options.encoding)?;
    let mut options = options;
    let query = Arc::new(query.compile(options.case)?);
    let collector = Arc::new(Collector::new(&options));
    options.file_evidence_visitor = None;
    let options = Arc::new(options);
    let completed = Arc::new(
        (0..roots.len())
            .map(|_| AtomicU64::new(0))
            .collect::<Vec<_>>(),
    );
    let emitted = Arc::new(
        (0..roots.len())
            .map(|_| AtomicU64::new(0))
            .collect::<Vec<_>>(),
    );
    let discovered = files
        .iter()
        .fold(vec![0_u64; roots.len()], |mut counts, file| {
            if let Some(count) = counts.get_mut(file.root_index) {
                *count = count.saturating_add(1);
            }
            counts
        });
    let workers = parallelism.min(files.len().max(1));
    run_workers(
        files, workers, &query, &options, &collector, &completed, &emitted,
    )?;
    collector.clear_file_evidence_visitor();
    if let Some(error) = collector.take_fatal() {
        return Err(error);
    }
    let stopped = collector.should_quit();
    let collected = collector.finish();
    let reports = build_reports(
        &roots,
        &discovered,
        &completed,
        &emitted,
        stopped,
        &revision,
    );
    let files_searched = reports.iter().map(|report| report.completed).sum();
    let bytes_searched = reports.iter().map(|report| report.bytes_emitted).sum();
    Ok(SearchReport {
        backend: SearchBackend::PersistentIndex,
        index: Some(IndexSearchEvidence {
            revision,
            indexed_files,
            candidate_files,
            prefiltered,
        }),
        roots,
        result_mode: options.result_mode,
        matches: collected.matches,
        matching_lines: collected.matching_lines,
        occurrences: collected.occurrences,
        files_with_matches: collected.files_with_matches,
        files_searched,
        bytes_searched,
        truncated: collected.truncated,
        warnings: collected.warnings,
        matched_files: collected.files,
        file_evidence: collected.file_evidence,
        file_evidence_truncated: collected.file_evidence_truncated,
        warnings_dropped: collected.warnings_dropped,
        scan: MultiContentVisitReport { reports },
    })
}

#[allow(clippy::too_many_arguments)]
fn run_workers(
    files: &[IndexedContent<'_>],
    workers: usize,
    query: &Arc<CompiledQuery>,
    options: &Arc<SearchOptions>,
    collector: &Arc<Collector>,
    completed: &Arc<Vec<AtomicU64>>,
    emitted: &Arc<Vec<AtomicU64>>,
) -> Result<()> {
    let next = Arc::new(AtomicUsize::new(0));
    let panicked = std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(workers);
        for _ in 0..workers {
            let query = Arc::clone(query);
            let options = Arc::clone(options);
            let collector = Arc::clone(collector);
            let next = Arc::clone(&next);
            let completed = Arc::clone(completed);
            let emitted = Arc::clone(emitted);
            handles.push(scope.spawn(move || {
                let mut query_cache = query.create_cache();
                loop {
                    if collector.should_quit() {
                        break;
                    }
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(file) = files.get(index) else {
                        break;
                    };
                    let expected_bytes = u64::try_from(file.bytes.len()).unwrap_or(u64::MAX);
                    let mut processor = FileProcessor::new(
                        SearchIdentity {
                            root_index: file.root_index,
                            path: file.path.to_owned(),
                            source_bytes: expected_bytes,
                            encoding: "UTF-8".into(),
                            archive: false,
                            source_offset_base: Some(0),
                            lossy: false,
                        },
                        expected_bytes,
                        Arc::clone(&query),
                        Arc::clone(&options),
                        Arc::clone(&collector),
                    );
                    if let Err(error) = processor
                        .push(file.bytes, &mut query_cache)
                        .and_then(|()| processor.finish(&mut query_cache))
                    {
                        handle_error(&collector, &options, error);
                    }
                    if let (Some(count), Some(bytes)) =
                        (completed.get(file.root_index), emitted.get(file.root_index))
                    {
                        count.fetch_add(1, Ordering::Relaxed);
                        bytes.fetch_add(expected_bytes, Ordering::Relaxed);
                    }
                }
            }));
        }
        let mut panicked = false;
        for handle in handles {
            panicked |= handle.join().is_err();
        }
        panicked
    });
    if panicked {
        return Err(Error::index(
            "<memory>",
            "an indexed-search worker panicked",
        ));
    }
    Ok(())
}

fn build_reports(
    roots: &[PathBuf],
    discovered: &[u64],
    completed: &[AtomicU64],
    emitted: &[AtomicU64],
    stopped: bool,
    revision: &str,
) -> Vec<ContentVisitReport> {
    roots
        .iter()
        .enumerate()
        .map(|(root_index, root)| {
            let completed = completed[root_index].load(Ordering::Relaxed);
            let bytes = emitted[root_index].load(Ordering::Relaxed);
            ContentVisitReport {
                mode: ContentVisitMode::Streaming,
                root: root.clone(),
                discovered: discovered[root_index],
                completed,
                opened: completed,
                chunks: completed,
                bytes_read: bytes,
                bytes_emitted: bytes,
                consumer_skipped: 0,
                stopped,
                skipped: Vec::new(),
                warnings: Vec::new(),
                ignore_sources: Vec::new(),
                revision: revision.to_owned(),
                complete: !stopped,
                termination: stopped.then_some(ScanTermination::Cancelled),
                portable: false,
                cache: ScanCacheStats::default(),
            }
        })
        .collect::<Vec<_>>()
}
