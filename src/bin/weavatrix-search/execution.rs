use super::{
    Arguments, ContentDiscoveryMode, HELP, ImmediateAction, IndexOptions, PersistentIndex,
    SearchOptions, SearchQuery, Searcher, recommended_scan_options, same_roots,
};

pub(super) fn handle_immediate(arguments: &Arguments) -> Result<bool, String> {
    if let Some(action) = arguments.action {
        match action {
            ImmediateAction::Help => print!("{HELP}"),
            ImmediateAction::Version => println!("weavatrix-search {}", env!("CARGO_PKG_VERSION")),
        }
        return Ok(true);
    }
    let Some(path) = &arguments.index_status else {
        return Ok(false);
    };
    let index =
        PersistentIndex::open(path, IndexOptions::default()).map_err(|error| error.to_string())?;
    let status = index.status();
    println!(
        "revision={} files={} bytes={} roots={}",
        status.revision,
        status.files,
        status.content_bytes,
        status.roots.len()
    );
    for (root_index, root) in status.roots.iter().enumerate() {
        println!("root[{root_index}]={}", root.display());
    }
    Ok(true)
}

pub(super) fn search_options(arguments: &mut Arguments) -> SearchOptions {
    let mut options = SearchOptions::default()
        .with_case(arguments.case)
        .with_context(arguments.before_context, arguments.after_context)
        .with_mode(arguments.mode)
        .with_result_mode(arguments.result_mode)
        .with_encoding(std::mem::take(&mut arguments.encoding))
        .with_max_results(arguments.max_results)
        .with_max_file_bytes(arguments.max_file_bytes)
        .with_max_line_bytes(arguments.max_line_bytes)
        .with_max_multiline_bytes(arguments.max_multiline_bytes)
        .with_max_replacement_bytes(arguments.max_replacement_bytes);
    options.archives.enabled = arguments.archives;
    options.archives.max_decoder_memory_bytes = arguments.max_decoder_memory_bytes;
    if let Some(replacement) = arguments.replacement.take() {
        options = options.with_replacement(replacement);
    }
    options
}

pub(super) fn scan_options(
    arguments: &mut Arguments,
    options: &SearchOptions,
) -> weavatrix_search::ScanOptions {
    let scanner_limit = if options.archives.enabled {
        options
            .max_file_bytes
            .max(options.archives.max_archive_bytes)
    } else {
        options.max_file_bytes
    };
    let mut scan_options = recommended_scan_options(&arguments.roots, options)
        .with_skip_hidden(!arguments.hidden)
        .with_override_rules(std::mem::take(&mut arguments.globs));
    if let Some(discovery) = arguments.discovery {
        scan_options.content_discovery = discovery;
        if arguments.content_workers.is_none() {
            scan_options.content_parallelism = Some(match discovery {
                ContentDiscoveryMode::Streaming if cfg!(windows) => 32,
                ContentDiscoveryMode::Streaming | ContentDiscoveryMode::BufferedParallel => {
                    if cfg!(windows) { 8 } else { 16 }
                }
            });
        }
    }
    if let Some(content_workers) = arguments.content_workers {
        scan_options.content_parallelism = Some(content_workers);
    }
    scan_options.max_file_bytes = scanner_limit;
    scan_options
}

pub(super) fn search(
    arguments: &mut Arguments,
    query: SearchQuery,
    options: SearchOptions,
    scan_options: weavatrix_search::ScanOptions,
) -> Result<weavatrix_search::SearchReport, String> {
    let roots = std::mem::take(&mut arguments.roots);
    if let Some(index_path) = arguments.index.take() {
        let index_options = IndexOptions::default().with_parallelism(arguments.index_workers);
        let index = if arguments.rebuild_index || !index_path.exists() {
            PersistentIndex::build_and_save(&index_path, roots, scan_options, index_options)
                .map_err(|error| error.to_string())?
                .0
        } else {
            let index = PersistentIndex::open(&index_path, index_options)
                .map_err(|error| error.to_string())?;
            if arguments.roots_explicit && !same_roots(index.roots(), &roots) {
                return Err(
                    "explicit PATH arguments differ from the index; use --rebuild-index".to_owned(),
                );
            }
            index
        };
        return index
            .search(query, options)
            .map_err(|error| error.to_string());
    }
    let mut roots = roots.into_iter();
    let root = roots
        .next()
        .expect("argument parsing always supplies one root");
    Searcher::new(root, query)
        .extend_roots(roots)
        .options(options)
        .scan_options(scan_options)
        .search()
        .map_err(|error| error.to_string())
}
