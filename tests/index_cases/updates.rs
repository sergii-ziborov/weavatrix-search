use super::support::{TempRepo, scan_options};
use weavatrix_search::{
    IndexOptions, PersistentIndex, SearchOptions, SearchQuery, WatchEvent, WatchEventKind,
    WatchPlan,
};

#[test]
fn incremental_update_adds_replaces_and_removes_without_full_discovery() {
    let repo = TempRepo::new("incremental");
    repo.write("changed.txt", b"old marker\n");
    repo.write("removed.txt", b"removed marker\n");
    repo.write("stable.txt", b"stable marker\n");
    let (mut index, _) = PersistentIndex::build(
        [repo.path()],
        scan_options(),
        IndexOptions::default().with_parallelism(2),
    )
    .unwrap();

    repo.write("changed.txt", b"new marker\n");
    repo.write("added.txt", b"new marker\n");
    repo.remove("removed.txt");
    let events = [
        WatchEvent::new(repo.path().join("changed.txt"), WatchEventKind::Modify),
        WatchEvent::new(repo.path().join("added.txt"), WatchEventKind::Create),
        WatchEvent::new(repo.path().join("removed.txt"), WatchEventKind::Remove),
    ];
    let update = index.update_events(0, events, scan_options()).unwrap();

    assert!(!update.full_rebuild);
    assert_eq!(
        (update.added, update.updated, update.removed),
        (1, 1, 1),
        "{update:?}"
    );
    assert!(update.changed_scan.is_some());
    let new_matches = index
        .search(SearchQuery::literal("new marker"), SearchOptions::default())
        .unwrap();
    assert_eq!(new_matches.files_with_matches, 2);
    let old_matches = index
        .search(
            SearchQuery::literal("removed marker"),
            SearchOptions::default(),
        )
        .unwrap();
    assert_eq!(old_matches.files_with_matches, 0);
}

#[test]
fn failed_incremental_limit_check_leaves_snapshot_unchanged() {
    let repo = TempRepo::new("rollback");
    repo.write("a.txt", b"aa");
    repo.write("b.txt", b"bb");
    let (mut index, _) = PersistentIndex::build(
        [repo.path()],
        scan_options(),
        IndexOptions::default()
            .with_parallelism(2)
            .with_max_content_bytes(5),
    )
    .unwrap();
    let revision = index.status().revision;

    repo.write("a.txt", b"cccc");
    let error = index
        .update_events(
            0,
            [WatchEvent::new(
                repo.path().join("a.txt"),
                WatchEventKind::Modify,
            )],
            scan_options(),
        )
        .unwrap_err();

    assert!(error.to_string().contains("content exceeds"), "{error}");
    assert_eq!(index.status().revision, revision);
    assert_eq!(
        index
            .search(SearchQuery::literal("aa"), SearchOptions::default())
            .unwrap()
            .files_with_matches,
        1
    );
}

#[test]
fn full_rebuild_counts_only_content_changes_as_updates() {
    let repo = TempRepo::new("rebuild-count");
    repo.write("a.txt", b"old\n");
    repo.write("b.txt", b"stable\n");
    let (mut index, _) =
        PersistentIndex::build([repo.path()], scan_options(), IndexOptions::default()).unwrap();
    repo.write("a.txt", b"new\n");

    let report = index
        .update(
            0,
            &WatchPlan {
                full_rescan: true,
                ..WatchPlan::default()
            },
            scan_options(),
        )
        .unwrap();

    assert!(report.full_rebuild);
    assert_eq!((report.added, report.updated, report.removed), (0, 1, 0));
}

#[test]
fn rebuild_excludes_an_index_stored_inside_the_repository() {
    let repo = TempRepo::new("self-exclusion");
    repo.write("source.txt", b"source marker\n");
    let path = repo.path().join(".weavatrix").join("search.wvx");
    let (mut index, build) = PersistentIndex::build_and_save(
        &path,
        [repo.path()],
        scan_options(),
        IndexOptions::default(),
    )
    .unwrap();

    let report = index
        .update(
            0,
            &WatchPlan {
                full_rescan: true,
                ..WatchPlan::default()
            },
            scan_options(),
        )
        .unwrap();

    assert_eq!(report.files, build.files);
    assert_eq!((report.added, report.updated, report.removed), (0, 0, 0));
    assert_eq!(
        index
            .search(SearchQuery::literal("WVXIDX01"), SearchOptions::default())
            .unwrap()
            .files_with_matches,
        0
    );

    let (rebuilt, second_build) = PersistentIndex::build_and_save(
        &path,
        [repo.path()],
        scan_options(),
        IndexOptions::default(),
    )
    .unwrap();
    assert_eq!(second_build.files, build.files);
    assert_eq!(
        rebuilt
            .search(SearchQuery::literal("WVXIDX01"), SearchOptions::default())
            .unwrap()
            .files_with_matches,
        0
    );
}

#[test]
fn index_resource_limits_are_enforced() {
    let repo = TempRepo::new("limits");
    repo.write("a.txt", b"a");
    repo.write("b.txt", b"b");

    let error = PersistentIndex::build(
        [repo.path()],
        scan_options(),
        IndexOptions::default().with_max_entries(1),
    )
    .unwrap_err();

    assert!(error.to_string().contains("entry count exceeds"), "{error}");
}
