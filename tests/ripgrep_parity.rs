use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use weavatrix_search::{CaseMode, SearchMode, SearchOptions, SearchQuery, Searcher};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Signature {
    path: String,
    line: u64,
    text: String,
    spans: Vec<(usize, usize)>,
}

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "weavatrix-search-rg-parity-{}-{id}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("ignored")).unwrap();
        fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
        fs::write(
            root.join("src/alpha.rs"),
            "before\nNeedle target 123\nafter\n",
        )
        .unwrap();
        fs::write(root.join("src/beta.rs"), "needle target 456\nordinary\n").unwrap();
        fs::write(root.join("ignored/hidden.rs"), "needle target 999\n").unwrap();
        Self { root }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn literal_and_regex_outputs_match_ripgrep() {
    if Command::new("rg").arg("--version").output().is_err() {
        eprintln!("ripgrep is not installed; parity test skipped");
        return;
    }
    let fixture = Fixture::new();

    let ours_literal = ours(
        &fixture.root,
        SearchQuery::literal("needle"),
        CaseMode::Insensitive,
    );
    let rg_literal = ripgrep(&fixture.root, &["needle"], true, true, false);
    assert_eq!(ours_literal, rg_literal);

    let ours_regex = ours(
        &fixture.root,
        SearchQuery::regex(r"target\s+\d+"),
        CaseMode::Sensitive,
    );
    let rg_regex = ripgrep(&fixture.root, &[r"target\s+\d+"], false, false, false);
    assert_eq!(ours_regex, rg_regex);

    let ours_multi = ours(
        &fixture.root,
        SearchQuery::any([
            SearchQuery::literal("Needle"),
            SearchQuery::regex(r"target\s+\d+"),
        ]),
        CaseMode::Sensitive,
    );
    let rg_multi = ripgrep(
        &fixture.root,
        &["Needle", r"target\s+\d+"],
        false,
        false,
        false,
    );
    assert_eq!(ours_multi, rg_multi);

    let ours_multiline = ours_mode(
        &fixture.root,
        SearchQuery::regex("before\\nNeedle"),
        CaseMode::Sensitive,
        SearchMode::Multiline,
    );
    let rg_multiline = ripgrep(&fixture.root, &["before\\nNeedle"], false, false, true);
    assert_eq!(ours_multiline, rg_multiline);
}

#[test]
fn cli_replacement_count_and_file_modes_match_ripgrep() {
    if Command::new("rg").arg("--version").output().is_err() {
        eprintln!("ripgrep is not installed; parity test skipped");
        return;
    }
    let fixture = Fixture::new();
    let binary = env!("CARGO_BIN_EXE_weavatrix-search");
    let root = fixture.root.to_str().unwrap();

    let ours_replacement =
        command_lines(Command::new(binary).args(["--replace", "id=$1", r"target\s+(\d+)", root]));
    let rg_replacement = command_lines(Command::new("rg").current_dir(&fixture.root).args([
        "--no-messages",
        "--color",
        "never",
        "--no-require-git",
        "--line-number",
        "--replace",
        "id=$1",
        "--regexp",
        r"target\s+(\d+)",
        ".",
    ]));
    assert_eq!(ours_replacement, rg_replacement);

    let ours_count = command_lines(Command::new(binary).args(["--count", "target", root]));
    let rg_count = command_lines(Command::new("rg").current_dir(&fixture.root).args([
        "--count",
        "--no-messages",
        "--color",
        "never",
        "--no-require-git",
        "target",
        ".",
    ]));
    assert_eq!(ours_count, rg_count);

    let ours_files =
        command_lines(Command::new(binary).args(["--files-with-matches", "target", root]));
    let rg_files = command_lines(Command::new("rg").current_dir(&fixture.root).args([
        "--files-with-matches",
        "--no-messages",
        "--color",
        "never",
        "--no-require-git",
        "target",
        ".",
    ]));
    assert_eq!(ours_files, rg_files);
}

fn ours(root: &Path, query: SearchQuery, case: CaseMode) -> Vec<Signature> {
    ours_mode(root, query, case, SearchMode::Line)
}

fn ours_mode(root: &Path, query: SearchQuery, case: CaseMode, mode: SearchMode) -> Vec<Signature> {
    let report = Searcher::new(root, query)
        .options(
            SearchOptions::default()
                .with_case(case)
                .with_mode(mode)
                .with_max_results(usize::MAX),
        )
        .search()
        .unwrap();
    report
        .matches
        .into_iter()
        .map(|found| {
            let mut text = found.line;
            while text.ends_with(['\n', '\r']) {
                text.pop();
            }
            Signature {
                path: found.path,
                line: found.line_number,
                text,
                spans: found
                    .spans
                    .into_iter()
                    .map(|span| (span.start, span.end))
                    .collect(),
            }
        })
        .collect()
}

fn ripgrep(
    root: &Path,
    patterns: &[&str],
    fixed: bool,
    insensitive: bool,
    multiline: bool,
) -> Vec<Signature> {
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
    if insensitive {
        command.arg("--ignore-case");
    }
    if multiline {
        command.arg("--multiline");
    }
    for pattern in patterns {
        command.arg("--regexp").arg(pattern);
    }
    let output = command.arg(".").output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut signatures = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .filter_map(|line| {
            let value: Value = serde_json::from_str(line).unwrap();
            (value["type"] == "match").then(|| {
                let data = &value["data"];
                let path = data["path"]["text"].as_str().unwrap();
                let mut text = data["lines"]["text"].as_str().unwrap().to_owned();
                while text.ends_with(['\n', '\r']) {
                    text.pop();
                }
                let spans = data["submatches"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|matched| {
                        (
                            usize::try_from(matched["start"].as_u64().unwrap()).unwrap(),
                            usize::try_from(matched["end"].as_u64().unwrap()).unwrap(),
                        )
                    })
                    .collect();
                Signature {
                    path: normalize_path(path),
                    line: data["line_number"].as_u64().unwrap(),
                    text,
                    spans,
                }
            })
        })
        .collect::<Vec<_>>();
    signatures.sort();
    signatures
}

fn normalize_path(path: &str) -> String {
    let path = path.replace('\\', "/");
    path.strip_prefix("./").unwrap_or(&path).to_owned()
}

fn command_lines(command: &mut Command) -> Vec<String> {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut lines = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(normalize_path)
        .collect::<Vec<_>>();
    lines.sort();
    lines
}
