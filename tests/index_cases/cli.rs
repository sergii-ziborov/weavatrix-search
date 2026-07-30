use super::support::TempRepo;
use std::process::Command;

#[test]
fn cli_builds_reuses_and_reports_a_persistent_index() {
    let repo = TempRepo::new("cli");
    repo.write("src/match.txt", b"cli index needle\n");
    repo.write("src/miss.txt", b"ordinary\n");
    let path = repo.path().join(".weavatrix").join("search.wvx");
    let binary = env!("CARGO_BIN_EXE_weavatrix-search");

    let first = Command::new(binary)
        .args([
            "--fixed-strings",
            "--index",
            path.to_str().unwrap(),
            "cli index needle",
            repo.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(path.is_file());
    assert!(String::from_utf8_lossy(&first.stdout).contains("src/match.txt"));

    let reused = Command::new(binary)
        .current_dir(repo.path())
        .args([
            "--fixed-strings",
            "--stats",
            "--index",
            path.to_str().unwrap(),
            "cli index needle",
        ])
        .output()
        .unwrap();
    assert!(
        reused.status.success(),
        "{}",
        String::from_utf8_lossy(&reused.stderr)
    );
    let stderr = String::from_utf8_lossy(&reused.stderr);
    assert!(stderr.contains("indexed"), "{stderr}");
    assert!(stderr.contains("candidates"), "{stderr}");

    let status = Command::new(binary)
        .args(["--index-status", path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(status.status.success());
    let stdout = String::from_utf8_lossy(&status.stdout);
    assert!(stdout.contains("revision="), "{stdout}");
    assert!(stdout.contains("files="), "{stdout}");
}
