use super::{ContentDiscoveryMode, ContentValidationPolicy, Duration, Instant, ScanOptions};

pub(super) fn benchmark_scan_options() -> ScanOptions {
    let discovery = match std::env::var("WEAVATRIX_SEARCH_BENCH_DISCOVERY").as_deref() {
        Ok("streaming") => ContentDiscoveryMode::Streaming,
        Ok("buffered") | Err(_) => ContentDiscoveryMode::BufferedParallel,
        Ok(value) => panic!("unknown benchmark discovery mode {value}"),
    };
    ScanOptions::default()
        .metadata_only()
        .selected_files_only()
        .with_content_parallelism(benchmark_content_parallelism())
        .with_content_discovery(discovery)
        .with_content_validation(ContentValidationPolicy::Fast)
}

fn benchmark_content_parallelism() -> usize {
    env_usize(
        "WEAVATRIX_SEARCH_BENCH_THREADS",
        if cfg!(windows) { 8 } else { 16 },
    )
}

pub(super) fn timed<T: PartialEq>(operation: impl FnOnce() -> T, expected: &T) -> Duration {
    let started = Instant::now();
    let output = operation();
    let elapsed = started.elapsed();
    assert!(&output == expected);
    elapsed
}

pub(super) fn normalize_path(path: &str) -> String {
    let path = path.replace('\\', "/");
    path.strip_prefix("./").unwrap_or(&path).to_owned()
}

pub(super) fn median(values: &mut [Duration]) -> Duration {
    values.sort_unstable();
    values[values.len() / 2]
}

pub(super) fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

pub(super) fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}
