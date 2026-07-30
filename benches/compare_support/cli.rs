use super::{
    Command, MARKER, Path, PathBuf, Signature, Value, black_box, env_usize, median, millis,
    normalize_path, timed,
};

pub(super) fn run_cli(root: &Path) {
    assert!(root.join(MARKER).is_file(), "benchmark marker is missing");
    let binary = release_search_binary();
    assert!(
        binary.is_file(),
        "{} is missing; run cargo build --release --all-features first",
        binary.display()
    );
    let runs = env_usize("WEAVATRIX_SEARCH_BENCH_RUNS", 7);
    let warmups = env_usize("WEAVATRIX_SEARCH_BENCH_WARMUPS", 2);
    let expected = weavatrix_cli(root, &binary);
    assert_eq!(
        expected,
        ripgrep_cli(root),
        "end-to-end CLI output differs from ripgrep"
    );
    for _ in 0..warmups {
        black_box(weavatrix_cli(root, &binary));
        black_box(ripgrep_cli(root));
    }
    let mut ours = Vec::with_capacity(runs);
    let mut ripgrep = Vec::with_capacity(runs);
    for index in 0..runs {
        if index % 2 == 0 {
            ours.push(timed(|| weavatrix_cli(root, &binary), &expected));
            ripgrep.push(timed(|| ripgrep_cli(root), &expected));
        } else {
            ripgrep.push(timed(|| ripgrep_cli(root), &expected));
            ours.push(timed(|| weavatrix_cli(root, &binary), &expected));
        }
    }
    println!(
        "mode=literal-json-cli engine=weavatrix-search-cli matching_lines={} median_ms={:.3}",
        expected.len(),
        millis(median(&mut ours))
    );
    println!(
        "mode=literal-json-cli engine=ripgrep-cli matching_lines={} median_ms={:.3}",
        expected.len(),
        millis(median(&mut ripgrep))
    );
}

fn release_search_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("WEAVATRIX_SEARCH_BIN") {
        return PathBuf::from(path);
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("release")
        .join(if cfg!(windows) {
            "weavatrix-search.exe"
        } else {
            "weavatrix-search"
        })
}

fn weavatrix_cli(root: &Path, binary: &Path) -> Vec<Signature> {
    let output = Command::new(binary)
        .current_dir(root)
        .args([
            "--json",
            "--fixed-strings",
            "--max-results",
            &usize::MAX.to_string(),
            "needle_target",
            ".",
        ])
        .output()
        .expect("run release weavatrix-search");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    parse_json_matches(&output.stdout, false)
}

fn ripgrep_cli(root: &Path) -> Vec<Signature> {
    let output = Command::new("rg")
        .current_dir(root)
        .args([
            "--json",
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
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    parse_json_matches(&output.stdout, true)
}

fn parse_json_matches(output: &[u8], ripgrep: bool) -> Vec<Signature> {
    let mut signatures = String::from_utf8_lossy(output)
        .lines()
        .filter_map(|line| {
            let value: Value = serde_json::from_str(line).expect("valid JSON Lines output");
            (value["type"] == "match").then(|| {
                let data = &value["data"];
                if ripgrep {
                    Signature {
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
                    }
                } else {
                    Signature {
                        path: normalize_path(data["path"].as_str().unwrap()),
                        line: data["line_number"].as_u64().unwrap(),
                        spans: data["spans"]
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
                    }
                }
            })
        })
        .collect::<Vec<_>>();
    signatures.sort();
    signatures
}
