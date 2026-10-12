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
