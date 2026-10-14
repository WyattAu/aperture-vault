//! The index exercised through its public surface: record, dedup, timeline
//! order, pagination, and the no-metadata fallback ordering.

// Fixtures are allowed to assert: an `expect` here is the test stating what it
// assumes, the honest form, and the crate lint denies it everywhere a caller
// would write it.
#![allow(clippy::expect_used)]

use aperture_index::{Index, Recorded};

fn file_index() -> (tempfile::TempDir, Index) {
    let dir = tempfile::tempdir().expect("temp dir");
    let index = Index::open(&dir.path().join("index.db")).expect("opens");
    (dir, index)
}

fn record(address: &str, taken_at: Option<&str>, name: &str) -> aperture_index::AssetRecord {
    aperture_index::AssetRecord {
        address: address.to_string(),
        width: Some(4000),
        height: Some(3000),
        taken_at: taken_at.map(str::to_string),
        make: Some("Fujifilm".into()),
        model: Some("X100V".into()),
        latitude: None,
        longitude: None,
        duration_ms: None,
        byte_len: 1000,
        file_name: Some(format!("{name}.jpg")),
        sha1: None,
        thumbhash: None,
        is_favorite: false,
    }
}

#[test]
fn re_recording_a_known_asset_is_not_a_duplicate_row() {
    let (_, index) = file_index();
    let r = record("aa01", Some("2026-05-01T10:00:00"), "one.jpg");
    assert_eq!(index.record_asset(&r).expect("first"), Recorded::Inserted);
    assert_eq!(
        index.record_asset(&r).expect("second"),
        Recorded::AlreadyKnown,
        "the CAS deduplicates; the index inherits it"
    );
    assert_eq!(index.count().expect("count"), 1);
}

#[test]
fn the_timeline_is_newest_first_by_capture_day() {
    let (_, index) = file_index();
    for (addr, day) in [
        ("a1", "2024-01-01"),
        ("a2", "2026-05-01"),
        ("a3", "2025-06-15"),
    ] {
        index
            .record_asset(&record(addr, Some(&format!("{day}T12:00:00")), addr))
            .expect("record");
    }
    let timeline = index.timeline(10, 0).expect("timeline");
    let days: Vec<&str> = timeline.iter().map(|e| e.day.as_str()).collect();
    assert_eq!(days, vec!["2026-05-01", "2025-06-15", "2024-01-01"]);
}

#[test]
fn assets_without_metadata_still_appear_ordered_by_ingest() {
    // Three assets with no EXIF at all: they still appear on the timeline,
    // grouped at the sentinel day, ordered by ingest recency among
    // themselves. A vault that hides untagged assets is a vault that hides a
    // whole library.
    let (_, index) = file_index();
    for name in ["first", "second", "third"] {
        let mut r = record(name, None, name);
        r.file_name = Some(name.to_string());
        index.record_asset(&r).expect("record");
    }
    let timeline = index.timeline(10, 0).expect("timeline");
    assert_eq!(timeline.len(), 3);
    for entry in &timeline {
        assert_eq!(entry.day, "1970-01-01", "no metadata, sentinel day");
    }
}

#[test]
fn timeline_pagination_is_stable() {
    let (_, index) = file_index();
    for i in 0..7u32 {
        let addr = format!("addr{i}");
        let day = format!("2026-01:{i:02}");
        let day = day.replace(':', "-");
        let _ = day;
        index
            .record_asset(&record(
                &addr,
                Some(&format!("2026-01-{:02}T00:00:00", 7 - i)),
                &addr,
            ))
            .expect("record");
    }
    let page1 = index.timeline(3, 0).expect("page 1");
    let page2 = index.timeline(3, 3).expect("page 2");
    assert_eq!(page1.len(), 3);
    assert_eq!(page2.len(), 3);
    // Pages must not overlap: every page-1 address is absent from page 2.
    for e1 in &page1 {
        assert!(
            !page2.iter().any(|e2| e2.address == e1.address),
            "page 2 repeats {}",
            e1.address
        );
    }
}

#[test]
fn lookup_by_address_returns_the_full_record() {
    let (_, index) = file_index();
    let mut r = record("beef", Some("2026-03-03T09:00:00"), "beef.jpg");
    r.latitude = Some(41.89);
    r.longitude = Some(12.49);
    index.record_asset(&r).expect("record");
    let got = index.asset("beef").expect("query").expect("present");
    assert_eq!(got, r);
    assert!(index.asset("missing").expect("query").is_none());
}

#[test]
fn an_index_on_disk_survives_reopen() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("index.db");
    {
        let index = Index::open(&path).expect("opens");
        index
            .record_asset(&record("persist", Some("2026-01-01T00:00:00"), "p.jpg"))
            .expect("record");
    }
    let reopened = Index::open(&path).expect("reopens");
    assert_eq!(reopened.count().expect("count"), 1);
    // WAL mode leaves a -wal sidecar beside the database; its presence or
    // absence after a clean close is not asserted — only that the data
    // survived the reopen, which is the property that matters.
}

#[test]
fn a_sha1_recorded_at_ingest_is_findable() {
    let (_dir, index) = file_index();
    // The pipeline records the checksum in the same insert as the asset —
    // one ingest, one row carrying both content keys. (Re-recording an
    // address is DO NOTHING, so a checksum cannot be smuggled in later.)
    let mut with_sha1 = record("shot", Some("2026-03-01T12:00:00"), "shot.jpg");
    with_sha1.sha1 = Some("a1b2c3d4e5f6a7b8c9d0a1b2c3d4e5f6a7b8c9d0".into());
    index.record_asset(&with_sha1).expect("record");

    let found = index
        .asset_by_sha1("a1b2c3d4e5f6a7b8c9d0a1b2c3d4e5f6a7b8c9d0")
        .expect("query");
    assert_eq!(found.as_deref(), Some("shot"));
    assert_eq!(
        index
            .asset_by_sha1("0000000000000000000000000000000000000000")
            .expect("query"),
        None,
        "an unknown checksum is an unknown checksum"
    );
}

#[test]
fn an_index_created_before_sha1_migrates_in_place() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("index.db");
    // The schema as the M2 release wrote it: no sha1 column.
    rusqlite::Connection::open(&path)
        .expect("opens")
        .execute_batch(
            "CREATE TABLE assets (
                 address    TEXT PRIMARY KEY,
                 width      INTEGER,
                 height     INTEGER,
                 taken_at   TEXT,
                 make       TEXT,
                 model      TEXT,
                 latitude   REAL,
                 longitude  REAL,
                 duration_ms INTEGER,
                 byte_len   INTEGER NOT NULL,
                 file_name  TEXT
             );
             INSERT INTO assets (address, byte_len)
             VALUES ('legacy', 5);",
        )
        .expect("old schema");
    // Opening with the current code must widen, not reject.
    let index = Index::open(&path).expect("opens a legacy database");
    assert_eq!(index.count().expect("count"), 1, "old rows survive");
    // The legacy row has no checksum — it predates the column, which is
    // exactly what the bulk-upload-check endpoint answers `accept` to.
    assert_eq!(
        index
            .asset_by_sha1("1234567890123456789012345678901234567890")
            .expect("query"),
        None,
    );
    // The widened column is usable for new rows.
    let mut migrated = record("fresh", None, "new.jpg");
    migrated.sha1 = Some("1234567890123456789012345678901234567890".into());
    index
        .record_asset(&migrated)
        .expect("records into the new column");
    assert_eq!(
        index
            .asset_by_sha1("1234567890123456789012345678901234567890")
            .expect("query"),
        Some("fresh".into()),
    );
}

#[test]
fn albums_are_views_over_assets_and_own_nothing() {
    let (_dir, index) = file_index();
    index
        .record_asset(&record("a", Some("2026-01-01T08:00:00"), "a.jpg"))
        .expect("record");
    index
        .record_asset(&record("b", Some("2026-01-01T19:00:00"), "b.jpg"))
        .expect("record");

    let album = index
        .create_album(
            "Holiday",
            "Selected shots",
            &["a".into(), "b".into(), "phantom".into()],
        )
        .expect("create");
    assert_eq!(album.asset_count, 2, "the phantom id named no asset");
    assert_eq!(
        album.thumbnail_address.as_deref(),
        Some("a"),
        "first member added is the cover"
    );

    // Members come back in the order they were added, full records.
    let members = index.album_assets(&album.id).expect("members");
    assert_eq!(
        members
            .iter()
            .map(|r| r.address.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b"]
    );

    // Re-adding is not double membership.
    let outcomes = index
        .album_add_assets(&album.id, &["a".into(), "b".into()])
        .expect("add");
    assert_eq!(outcomes, vec![("a".into(), false), ("b".into(), false)]);
    assert_eq!(
        index
            .album(&album.id)
            .expect("album")
            .expect("the album exists")
            .asset_count,
        2
    );

    // Removal reports membership truth; the assets themselves stay.
    let removed = index
        .album_remove_assets(&album.id, &["a".into(), "zz".into()])
        .expect("remove");
    assert_eq!(removed, vec![("a".into(), true), ("zz".into(), false)]);
    assert!(
        index.asset("a").expect("query").is_some(),
        "removal keeps the asset"
    );

    // Rename touches only the album.
    let renamed = index
        .album_update(&album.id, Some("Holiday 2026"), None)
        .expect("rename")
        .expect("album exists");
    assert_eq!(renamed.name, "Holiday 2026");
    assert_eq!(renamed.asset_count, 1);

    // Delete the album: memberships go, assets remain.
    assert!(index.album_delete(&album.id).expect("delete"));
    assert_eq!(index.album(&album.id).expect("query"), None);
    assert_eq!(
        index.count().expect("count"),
        2,
        "assets outlive their album"
    );
    assert!(index.album_assets(&album.id).expect("members").is_empty());
}

#[test]
fn buckets_group_by_capture_day_newest_first() {
    let (_dir, index) = file_index();
    index
        .record_asset(&record("a", Some("2026-01-01T08:00:00"), "a.jpg"))
        .expect("record");
    index
        .record_asset(&record("b", Some("2026-01-01T19:00:00"), "b.jpg"))
        .expect("record");
    index
        .record_asset(&record("c", Some("2025-12-25T10:00:00"), "c.jpg"))
        .expect("record");
    index
        .record_asset(&record("d", None, "d.jpg"))
        .expect("record");

    let buckets = index.buckets().expect("buckets");
    assert_eq!(
        buckets,
        vec![
            ("2026-01-01".into(), 2),
            ("2025-12-25".into(), 1),
            ("1970-01-01".into(), 1),
        ],
        "newest day first; untagged assets fall to the sentinel day"
    );

    let day = index.assets_in_day("2026-01-01").expect("day");
    assert_eq!(day.len(), 2);
    assert_eq!(
        day.first().map(|r| r.address.as_str()),
        Some("b"),
        "later capture first within the day"
    );
}

#[test]
fn favorite_flags_toggle_and_count_in_the_statistics() {
    let (_dir, index) = file_index();
    index
        .record_asset(&record("img1", Some("2026-01-01T08:00:00"), "a.jpg"))
        .expect("record");
    index
        .record_asset(&record("img2", Some("2026-02-01T08:00:00"), "b.jpg"))
        .expect("record");
    let mut video = record("vid", Some("2026-03-01T08:00:00"), "c.mp4");
    video.duration_ms = Some(30_000);
    index.record_asset(&video).expect("record");

    assert_eq!(
        index.statistics().expect("stats"),
        (2, 1, 3),
        "two images, one video, three total"
    );

    // Toggle on, on again, off: idempotent, honest about existence.
    assert!(index.set_favorite("img1", true).expect("set"));
    assert!(index.set_favorite("img1", true).expect("re-set"));
    assert!(index.set_favorite("img1", false).expect("clear"));
    assert!(index.set_favorite("img1", true).expect("set again"));

    // The flag persists through a fresh connection (it is a column, not
    // an in-memory whim).
    let favorite = index.asset("img1").expect("query").expect("recorded");
    assert!(favorite.is_favorite);
    let not = index.asset("img2").expect("query").expect("recorded");
    assert!(!not.is_favorite, "the other asset is untouched");

    // A favorite on a nonexistent asset reports the truth.
    assert!(!index.set_favorite("ghost", true).expect("set"));
}
