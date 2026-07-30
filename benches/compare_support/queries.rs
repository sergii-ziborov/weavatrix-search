use super::{
    CaseMode, Command, Duration, Instant, Path, ResultMode, SearchMode, SearchOptions, SearchQuery,
    Searcher, Signature, Value, benchmark_scan_options, normalize_path,
};

pub(super) fn weavatrix_count(root: &Path) -> Vec<(String, u64)> {
    let report = Searcher::new(root, SearchQuery::literal("needle_target"))
        .options(
            SearchOptions::default()
                .with_result_mode(ResultMode::Count)
                .with_max_results(usize::MAX),
        )
        .search()
        .expect("weavatrix count search");
    report
        .matched_files
        .into_iter()
        .map(|file| (file.path, file.occurrences))
        .collect()
}

pub(super) fn ripgrep_count(root: &Path) -> Vec<(String, u64)> {
    let output = Command::new("rg")
        .current_dir(root)
        .args([
            "--count-matches",
            "--with-filename",
            "--no-messages",
            "--color",
            "never",
            "--no-require-git",
            "--fixed-strings",
            "--regexp",
            "needle_target",
            ".",
        ])
        .output()
        .expect("ripgrep is required for this benchmark");
    assert!(output.status.success());
    let mut counts = String::from_utf8(output.stdout)
        .expect("ripgrep count output is UTF-8")
        .lines()
        .filter_map(|line| line.rsplit_once(':'))
        .map(|(path, count)| {
            (
                normalize_path(path),
                count.parse::<u64>().expect("ripgrep count is numeric"),
            )
        })
        .collect::<Vec<_>>();
    counts.sort();
    counts
}

pub(super) fn weavatrix_files(root: &Path) -> Vec<String> {
    Searcher::new(root, SearchQuery::literal("needle_target"))
        .options(
            SearchOptions::default()
                .with_result_mode(ResultMode::Files)
                .with_max_results(usize::MAX),
        )
        .search()
        .expect("weavatrix files search")
        .matched_files
        .into_iter()
        .map(|file| file.path)
        .collect()
}

pub(super) fn ripgrep_files(root: &Path) -> Vec<String> {
    let output = Command::new("rg")
        .current_dir(root)
        .args([
            "--files-with-matches",
            "--no-messages",
            "--color",
            "never",
            "--no-require-git",
            "--fixed-strings",
            "--regexp",
            "needle_target",
            ".",
        ])
        .output()
        .expect("ripgrep is required for this benchmark");
    assert!(output.status.success());
    let mut files = String::from_utf8(output.stdout)
        .expect("ripgrep file output is UTF-8")
        .lines()
        .map(normalize_path)
        .collect::<Vec<_>>();
    files.sort();
    files
}

pub(super) fn timed_weavatrix(
    root: &Path,
    query: SearchQuery,
    search_mode: SearchMode,
) -> (Duration, Vec<Signature>, u64) {
    let started = Instant::now();
    let (files, output) = weavatrix(root, query, search_mode);
    (started.elapsed(), output, files)
}

pub(super) fn weavatrix(
    root: &Path,
    query: SearchQuery,
    search_mode: SearchMode,
) -> (u64, Vec<Signature>) {
    let report = Searcher::new(root, query)
        .scan_options(benchmark_scan_options().with_skip_hidden(true))
        .options(
            SearchOptions::default()
                .with_case(CaseMode::Sensitive)
                .with_mode(search_mode)
                .with_max_results(usize::MAX),
        )
        .search()
        .expect("weavatrix search");
    let signatures = report
        .matches
        .into_iter()
        .map(|found| Signature {
            path: found.path,
            line: found.line_number,
            spans: found
                .spans
                .into_iter()
                .map(|span| (span.start, span.end))
                .collect(),
        })
        .collect();
    (report.files_searched, signatures)
}

pub(super) fn timed_ripgrep(
    root: &Path,
    patterns: &[&str],
    fixed: bool,
    search_mode: SearchMode,
) -> (Duration, Vec<Signature>) {
    let started = Instant::now();
    let (_, output) = ripgrep(root, patterns, fixed, search_mode);
    (started.elapsed(), output)
}

pub(super) fn ripgrep(
    root: &Path,
    patterns: &[&str],
    fixed: bool,
    search_mode: SearchMode,
) -> (u64, Vec<Signature>) {
    let mut command = Command::new("rg");
    command.current_dir(root).args([
        "--json",
        "--no-messages",
        "--color",
        "never",
        "--no-require-git",
    ]);
    if fixed {
        command.arg("--fixed-strings");
    }
    if search_mode == SearchMode::Multiline {
        command.arg("--multiline");
    }
    for pattern in patterns {
        command.arg("--regexp").arg(pattern);
    }
    let output = command
        .arg(".")
        .output()
        .expect("ripgrep is required for this benchmark; install rg and retry");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut searched = 0_u64;
    let mut signatures = Vec::new();
    for line in String::from_utf8(output.stdout).unwrap().lines() {
        let value: Value = serde_json::from_str(line).unwrap();
        if value["type"] == "match" {
            let data = &value["data"];
            signatures.push(Signature {
                path: normalize_path(data["path"]["text"].as_str().unwrap()),
                line: data["line_number"].as_u64().unwrap(),
                spans: data["submatches"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|matched| {
                        (
                            usize::try_from(matched["start"].as_u64().unwrap()).unwrap(),
                            usize::try_from(matched["end"].as_u64().unwrap()).unwrap(),
                        )
                    })
                    .collect(),
            });
        } else if value["type"] == "summary" {
            searched = value["data"]["stats"]["searches"].as_u64().unwrap_or(0);
        }
    }
    signatures.sort();
    (searched, signatures)
}
