use super::support::{TempRepo, scan_options};
use std::time::Duration;
use weavatrix_search::{
    IndexOptions, LiveIndexBuilder, LiveIndexOptions, PersistentIndex, SearchOptions, SearchQuery,
    WatchEvent, WatchEventKind,
};

#[test]
fn live_index_applies_hosted_events_and_persists_the_new_generation() {
    let repo = TempRepo::new("live");
    repo.write("source.txt", b"old live marker\n");
    let path = repo.path().join("search.wvx");
    let _ = PersistentIndex::build_and_save(
        &path,
        [repo.path()],
        scan_options(),
        IndexOptions::default().with_parallelism(2),
    )
    .unwrap();
    let live = LiveIndexBuilder::new(&path, repo.path())
        .scan_options(scan_options())
        .index_options(IndexOptions::default().with_parallelism(2))
        .live_options(
            LiveIndexOptions::default()
                .with_debounce(Duration::from_millis(20))
                .trust_existing_snapshot(),
        )
        .start()
        .unwrap();

    repo.write("source.txt", b"new live marker\n");
    let report = live
        .apply_events(
            0,
            [WatchEvent::new(
                repo.path().join("source.txt"),
                WatchEventKind::Modify,
            )],
        )
        .unwrap();
    assert_eq!(report.updated, 1);
    assert_eq!(
        live.search(
            SearchQuery::literal("new live marker"),
            SearchOptions::default()
        )
        .unwrap()
        .files_with_matches,
        1
    );
    let status = live.status();
    assert!(status.generation >= 1);
    assert!(status.dirty);
    live.stop().unwrap();

    let reopened = PersistentIndex::open(&path, IndexOptions::default()).unwrap();
    assert_eq!(
        reopened
            .search(
                SearchQuery::literal("new live marker"),
                SearchOptions::default()
            )
            .unwrap()
            .files_with_matches,
        1
    );
}

#[test]
fn live_index_observes_native_filesystem_changes() {
    let repo = TempRepo::new("native-live");
    repo.write("source.txt", b"old native marker\n");
    let path = repo.path().join("search.wvx");
    let _ = PersistentIndex::build_and_save(
        &path,
        [repo.path()],
        scan_options(),
        IndexOptions::default().with_parallelism(2),
    )
    .unwrap();
    let live = LiveIndexBuilder::new(&path, repo.path())
        .scan_options(scan_options())
        .index_options(IndexOptions::default().with_parallelism(2))
        .live_options(
            LiveIndexOptions::default()
                .with_debounce(Duration::from_millis(20))
                .trust_existing_snapshot(),
        )
        .start()
        .unwrap();
    let generation = live.status().generation;

    repo.write("source.txt", b"new native marker\n");
    assert!(
        live.wait_for_update(generation, Duration::from_secs(10)),
        "{:?}",
        live.status()
    );
    assert_eq!(
        live.search(
            SearchQuery::literal("new native marker"),
            SearchOptions::default()
        )
        .unwrap()
        .files_with_matches,
        1
    );
    live.stop().unwrap();
}
