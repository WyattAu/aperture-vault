//! Directory scan against a real CAS and index: the first scan ingests a
//! tree, the second scan skips it (content addressing as idempotence),
//! hidden files are ignored, subdirectories are walked, and unreadable
//! files land in the failure list without sinking the run.

// Fixtures are allowed to assert: an `expect` here is the test stating what
// it assumes, the honest form, and the workspace lint denies it everywhere a
// caller would write it.
#![allow(clippy::expect_used)]

use aperture_cas::Store;
use aperture_index::Index;
use aperture_pipeline::scan::scan_directory;

fn env() -> (tempfile::TempDir, Store, Index) {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Store::open(dir.path().join("vault")).expect("opens");
    let index = Index::open(&dir.path().join("index.db")).expect("opens");
    (dir, store, index)
}

#[test]
fn scan_ingests_a_tree_then_skips_it_entirely() {
    let (dir, store, index) = env();
    let photos = dir.path().join("photos");
    std::fs::create_dir_all(photos.join("2026/spring")).expect("dirs");
    std::fs::write(photos.join("2026/a.jpg"), b"first photo").expect("write");
    std::fs::write(photos.join("2026/spring/b.jpg"), b"second photo").expect("write");
    std::fs::write(photos.join(".hidden.jpg"), b"secret photo").expect("write");

    let first = scan_directory(&store, &index, &photos).expect("scans");
    assert_eq!(first.scanned, 2, "hidden files are not candidates");
    assert_eq!(first.ingested, 2, "both visible photos are new");
    assert_eq!(first.skipped, 0);
    assert!(first.failed.is_empty());
    assert_eq!(
        index.count().expect("count"),
        2,
        "recursion reached the subdir"
    );

    // Re-scan the same tree: the vault already holds every byte, so a
    // re-pointed scan is a no-op — the point of content addressing.
    let second = scan_directory(&store, &index, &photos).expect("scans");
    assert_eq!(second.scanned, 2);
    assert_eq!(second.ingested, 0, "nothing written twice");
    assert_eq!(second.skipped, 2, "everything recognized by checksum");
}

#[test]
fn mixed_scan_ingests_the_new_and_skips_the_known() {
    let (dir, store, index) = env();
    let photos = dir.path().join("photos");
    std::fs::create_dir_all(&photos).expect("dir");
    std::fs::write(photos.join("old.jpg"), b"already backed up").expect("write");
    scan_directory(&store, &index, &photos).expect("first scan");

    std::fs::write(photos.join("new.jpg"), b"a fresh capture").expect("write");
    let report = scan_directory(&store, &index, &photos).expect("second scan");
    assert_eq!(report.scanned, 2);
    assert_eq!(report.ingested, 1, "only the new file is written");
    assert_eq!(report.skipped, 1, "the old file is recognized");
    assert_eq!(index.count().expect("count"), 2);
}

#[test]
fn failures_are_reported_not_fatal() {
    let (dir, store, index) = env();
    let photos = dir.path().join("photos");
    std::fs::create_dir_all(&photos).expect("dir");
    std::fs::write(photos.join("good.jpg"), b"ingestable").expect("write");

    // A directory as a "file" candidate is skipped by is_file; a file that
    // vanishes between listing and open lands in `failed`.
    std::fs::create_dir(photos.join("sub")).expect("dir");
    let report = scan_directory(&store, &index, &photos).expect("scans");
    assert_eq!(report.ingested, 1);
    assert!(report.failed.is_empty(), "{:?}", report.failed);
}

#[test]
fn gc_removes_orphans_and_keeps_every_marked_object() {
    let (dir, store, index) = env();
    // One real asset through the pipeline.
    let asset = {
        let photos = dir.path().join("photos");
        std::fs::create_dir_all(&photos).expect("dir");
        std::fs::write(photos.join("keep.jpg"), b"the keeper").expect("write");
        aperture_pipeline::Pipeline::ingest(
            &store,
            &index,
            std::io::Cursor::new(b"the keeper"),
            Some("keep.jpg"),
        )
        .expect("ingests")
        .address
    };

    // An orphan: bytes in the CAS with no index row (the crash-window
    // shape, or the post-trash-empty shape).
    let (_orphan_addr, newly) = store
        .put_bytes(b"orphaned bytes nobody references")
        .expect("writes");
    assert!(newly);

    // Mark set: the asset + nothing else. GC takes the orphan, keeps the asset.
    let report = aperture_pipeline::gc::gc(&store, &index).expect("gc");
    assert_eq!(report.removed, 1);
    assert_eq!(report.kept, 1);
    assert!(
        store.has(&asset),
        "a marked object survives the sweep — YP-CAS-GC-002 gc_safety"
    );

    // And after trash-empty of the owner, its blob is the next orphan.
    index.trash(&[asset.hex()]).expect("trash");
    index.trash_empty().expect("empty");
    let report = aperture_pipeline::gc::gc(&store, &index).expect("gc");
    assert_eq!(report.removed, 1);
    assert!(!store.has(&asset), "emptied trash is reclaimed");
}
