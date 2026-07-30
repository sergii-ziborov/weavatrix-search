use super::{MARKER, Path, fs};

pub(super) fn prepare(root: &Path, files: usize) {
    assert!(files > 0, "file count must be positive");
    if root.exists() {
        assert!(
            root.join(MARKER).is_file(),
            "refusing to populate an existing unmarked directory"
        );
        assert_eq!(
            fs::read_to_string(root.join(MARKER)).unwrap(),
            files.to_string(),
            "existing fixture has a different configured size"
        );
        return;
    }
    fs::create_dir_all(root).expect("create benchmark root");
    fs::write(root.join(MARKER), files.to_string()).expect("write marker");
    fs::write(root.join(".gitignore"), "group0003/\n").expect("write ignore file");
    let groups = files.div_ceil(500);
    let workers = std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .min(groups)
        .min(16);
    std::thread::scope(|scope| {
        for worker in 0..workers {
            scope.spawn(move || {
                for group in (worker..groups).step_by(workers) {
                    write_fixture_group(root, group, files);
                }
            });
        }
    });
}

fn write_fixture_group(root: &Path, group: usize, files: usize) {
    let directory = root.join(format!("group{group:04}"));
    fs::create_dir(&directory).expect("create benchmark directory");
    let start = group * 500;
    let end = start.saturating_add(500).min(files);
    for index in start..end {
        let content = if index % 20 == 0 {
            format!("pub fn needle_target_{index}() {{}}\nlet item_{index} = 42;\n")
        } else {
            format!("pub fn ordinary_{index}() {{}}\n")
        };
        fs::write(directory.join(format!("file{index:07}.rs")), content)
            .expect("write benchmark file");
    }
}

pub(super) fn verify(root: &Path, files: usize) {
    assert_eq!(
        fs::read_to_string(root.join(MARKER)).unwrap(),
        files.to_string()
    );
    let actual = fs::read_dir(root)
        .unwrap()
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| fs::read_dir(entry.path()).unwrap().count())
        .sum::<usize>();
    assert_eq!(actual, files);
}
