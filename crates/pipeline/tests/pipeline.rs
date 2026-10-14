//! The pipeline exercised end to end: real CAS, real index, synthetic bytes.
//! First ingest writes and indexes; the identical second ingest deduplicates
//! at both layers and runs neither probe nor index write again. And the
//! design claim "the index is a cache, not the system of record" is tested
//! for real: wipe the index, replay the stored blobs, and it comes back.

// Fixtures are allowed to assert: an `expect` here is the test stating what it
// assumes, the honest form, and the workspace lint denies it everywhere a
// caller would write it.
#![allow(clippy::expect_used)]

use aperture_cas::Store;
use aperture_index::Index;
use aperture_pipeline::Pipeline;
use std::io::Cursor;

fn env() -> (tempfile::TempDir, Store, Index) {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Store::open(dir.path().join("vault")).expect("opens");
    let index = Index::open(&dir.path().join("index.db")).expect("opens");
    (dir, store, index)
}

#[test]
fn first_ingest_indexes_second_ingest_deduplicates() {
    let (_dir, store, index) = env();
    let data = b"one photo".repeat(500);

    let first =
        Pipeline::ingest(&store, &index, Cursor::new(&data), Some("a.jpg")).expect("first ingest");
    assert!(first.newly_written);
    assert!(first.indexed);

    let second =
        Pipeline::ingest(&store, &index, Cursor::new(&data), Some("a.jpg")).expect("second ingest");
    assert!(!second.newly_written, "CAS deduplicated");
    assert!(!second.indexed, "index skipped: nothing changed");
    assert_eq!(second.meta, None, "probe skipped on dedup hit");
    assert_eq!(index.count().expect("count"), 1);
}

#[test]
fn the_index_record_carries_what_the_probe_found() {
    // A PNG built byte-level: signature + IHDR with known dimensions.
    let mut data = vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // signature
    ];
    data.extend_from_slice(&13u32.to_be_bytes());
    data.extend_from_slice(b"IHDR");
    data.extend_from_slice(&640u32.to_be_bytes());
    data.extend_from_slice(&480u32.to_be_bytes());
    data.extend_from_slice(&[8, 2, 0, 0, 0]);
    data.extend_from_slice(&[0, 0, 0, 0]); // CRC, unchecked by the probe

    let (_dir, store, index) = env();
    let report =
        Pipeline::ingest(&store, &index, Cursor::new(&data), Some("shot.png")).expect("ingest");
    assert!(report.indexed);

    let record = index
        .asset(&report.address.hex())
        .expect("query")
        .expect("present");
    assert_eq!(record.width, Some(640));
    assert_eq!(record.height, Some(480));
}

#[test]
fn a_failure_below_the_pipeline_leaves_no_partial_state() {
    let (_dir, store, index) = env();
    // A reader that fails immediately: the CAS must clean its staging file,
    // and the index must never learn about an asset that was not stored.
    struct Failing;
    impl std::io::Read for Failing {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("injected failure"))
        }
    }
    let err = Pipeline::ingest(&store, &index, Failing, Some("x.jpg")).expect_err("fails");
    assert!(err.to_string().contains("injected failure"));
    assert_eq!(index.count().expect("count"), 0, "nothing indexed");
}

#[test]
fn the_index_is_rebuildable_from_the_cas_alone() {
    // The design claim, tested for real: wipe the index, replay every stored
    // blob through probe + index, and the timeline is restored. Storage is
    // the truth; the index agrees.
    let (_dir, store, index) = env();
    let addresses: Vec<String> = ["a.jpg", "b.jpg"]
        .iter()
        .map(|name| {
            Pipeline::ingest(&store, &index, Cursor::new(name.as_bytes()), Some(name))
                .expect("ingest")
                .address
                .hex()
        })
        .collect();
    assert_eq!(index.count().expect("count"), 2);

    // "Wipe": a fresh, empty index.
    let wiped = Index::in_memory().expect("opens");
    // Replay: read each blob from the CAS and re-record what the probe finds.
    for hex in &addresses {
        let addr = aperture_cas::Address::parse_hex(hex).expect("valid address");
        let mut file = store.open_read(&addr).expect("stored");
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut bytes).expect("read");
        let meta = aperture_probe::probe(&bytes);
        wiped
            .record_asset(&aperture_index::AssetRecord {
                address: hex.clone(),
                width: meta.width,
                height: meta.height,
                taken_at: meta.taken_at.clone(),
                make: meta.make.clone(),
                model: meta.model.clone(),
                latitude: meta.latitude,
                longitude: meta.longitude,
                duration_ms: meta.duration_ms,
                byte_len: bytes.len() as u64,
                file_name: None,
                sha1: None,
                thumbhash: None,
                is_favorite: false,
            })
            .expect("record");
    }
    assert_eq!(wiped.count().expect("count"), 2, "the index came back");
}

#[test]
fn ingest_records_the_sha1_compat_checksum() {
    let (_dir, store, index) = env();
    let data = b"checksummed bytes".repeat(1000);

    let report =
        Pipeline::ingest(&store, &index, Cursor::new(&data), Some("c.jpg")).expect("ingest");

    // Independent computation: the recorded checksum must be the real SHA-1
    // of the bytes, lowercase hex — the form a client's base64 decodes to.
    use sha1::Digest as _;
    let expected = {
        let mut h = sha1::Sha1::new();
        h.update(&data);
        h.finalize()
    };
    let expected_hex: String = expected.iter().map(|b| format!("{b:02x}")).collect();

    let record = index
        .asset(&report.address.hex())
        .expect("query")
        .expect("recorded");
    assert_eq!(record.sha1.as_deref(), Some(expected_hex.as_str()));
}
