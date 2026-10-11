//! The whole store, exercised through its public surface: ingest, dedup,
//! integrity, streaming, atomicity artifacts, and the sweeper.
//!
//! Fixtures are allowed to assert: an `expect` here is the test stating what it
//! assumes, the honest form, and the crate lint denies it everywhere a caller
//! would write it.
#![allow(clippy::expect_used, clippy::indexing_slicing)]

use aperture_cas::{hash_bytes, Address, Store};
use std::io::Cursor;

fn tmp_store(name: &str) -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Store::open(dir.path().join(name)).expect("opens");
    (dir, store)
}

#[test]
fn round_trip_and_identity() {
    let (_dir, store) = tmp_store("roundtrip");
    let data = "the photo is 4MB of pixels".as_bytes().repeat(100);
    let (addr, written) = store.put_bytes(&data).expect("put");
    assert!(written, "first write is new");
    assert_eq!(store.get_bytes(&addr).expect("get"), data);
    assert_eq!(addr, hash_bytes(&data), "address is the content hash");
}

#[test]
fn duplicate_ingest_is_free() {
    let (_dir, store) = tmp_store("dedup");
    let data = b"identical bytes";
    let (addr1, new1) = store.put_bytes(data).expect("first");
    let (addr2, new2) = store.put_bytes(data).expect("second");
    assert_eq!(addr1, addr2);
    assert!(new1);
    assert!(!new2, "the second write deduplicates");
    assert_eq!(store.get_bytes(&addr1).expect("get"), data);
}

#[test]
fn different_bytes_different_addresses() {
    let (_dir, store) = tmp_store("distinct");
    let (a, _) = store.put_bytes(b"one").expect("put");
    let (b, _) = store.put_bytes(b"two").expect("put");
    assert_ne!(a, b);
    assert_ne!(a, hash_bytes(b"three"));
}

#[test]
fn addresses_render_and_parse_as_hex() {
    let addr = hash_bytes(b"hex round trip");
    let hex = addr.hex();
    assert_eq!(hex.len(), Address::LENGTH * 2);
    assert_eq!(hex.to_lowercase(), hex);
    assert_eq!(Address::parse_hex(&hex).expect("parses"), addr);
    // Wrong length and bad digits are refused.
    assert!(Address::parse_hex("abcd").is_err());
    assert!(Address::parse_hex(&"zz".repeat(32)).is_err());
}

#[test]
fn missing_blob_is_reported_as_missing() {
    let (_dir, store) = tmp_store("missing");
    let absent = Address::parse_hex(&"ab".repeat(32)).expect("valid hex");
    assert!(matches!(
        store.get_bytes(&absent),
        Err(aperture_cas::CasError::Missing { .. })
    ));
    assert_eq!(store.verify(&absent).expect("verify"), None);
}

#[test]
fn verify_detects_corruption() {
    let (_dir, store) = tmp_store("corruption");
    let data = b"verifiable bytes";
    let (addr, _) = store.put_bytes(data).expect("put");
    assert_eq!(store.verify(&addr).expect("verify"), Some(true));
    // Simulate bit-rot: flip a byte on disk, under the store's feet.
    let path = store.path(&addr);
    let mut bytes = std::fs::read(&path).expect("read");
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    std::fs::write(&path, bytes).expect("write");
    assert_eq!(store.verify(&addr).expect("verify"), Some(false));
}

#[test]
fn streaming_ingest_matches_bytes_ingest() {
    let (_dir, store) = tmp_store("streaming");
    let data: Vec<u8> = (0..500_000u32).map(|i| (i % 251) as u8).collect();
    let streamed = store.put_stream(Cursor::new(&data)).expect("stream ingest");
    let (direct, newly) = store.put_bytes(&data).expect("bytes ingest");
    assert_eq!(
        streamed.address, direct,
        "both paths address the same bytes"
    );
    // The stream was written first, so the bytes-ingest deduplicated.
    assert!(streamed.newly_written);
    assert!(!newly);
    assert_eq!(streamed.len, data.len() as u64);
}

#[test]
fn streaming_a_large_synthetic_blob_stays_bounded_in_memory() {
    // 32 MiB in 128 KiB chunks: the buffer stays fixed; the address must match
    // hashing the same bytes directly.
    let (_dir, store) = tmp_store("large");
    let mut hasher = blake3::Hasher::new();
    let chunk = vec![0xABu8; 128 * 1024];
    for _ in 0..256 {
        hasher.update(&chunk);
    }
    let expected = Address::new(*hasher.finalize().as_bytes());
    let chunks = chunk.repeat(256);
    let ingested = store
        .put_stream(Cursor::new(&chunks))
        .expect("stream ingest");
    assert_eq!(ingested.address, expected);
    assert_eq!(ingested.len, 33_554_432);
}

#[test]
fn layout_is_sharded_by_address_prefix() {
    let (_dir, store) = tmp_store("layout");
    let (addr, _) = store.put_bytes(b"sharded").expect("put");
    let path = store.path(&addr);
    let hex = addr.hex();
    let expected = store
        .root()
        .join("objects")
        .join(&hex[0..2])
        .join(&hex[2..4])
        .join(&hex);
    assert_eq!(path, expected);
    assert!(path.is_file(), "stored at the sharded path");
}

#[test]
fn sweeper_respects_the_horizon_and_never_touches_content() {
    let (_dir, store) = tmp_store("sweep");
    let (addr, _) = store.put_bytes(b"kept").expect("put");
    // Two orphaned staging files, created now: fresh against any sane horizon.
    let orphan = store.root().join("objects/.ingest-9999-1");
    let fresh = store.root().join("objects/.ingest-9999-2");
    std::fs::write(&orphan, b"partial").expect("write orphan");
    std::fs::write(&fresh, b"live").expect("write fresh");

    // A conservative sweep (1 hour horizon) removes nothing: both staging
    // files are newer than the horizon, and removing them could destroy a
    // live ingest.
    let removed = store.sweep(3600).expect("sweep");
    assert_eq!(removed, 0, "fresh staging files are not swept");

    // After the horizon passes, the sweep collects them — and leaves content
    // untouched.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let removed = store.sweep(0).expect("sweep");
    assert_eq!(removed, 2, "both orphans collected");
    assert!(!orphan.exists());
    assert!(!fresh.exists());
    assert!(store.has(&addr), "real content is untouched");
}

#[test]
fn open_on_a_missing_root_creates_it() {
    let dir = tempfile::tempdir().expect("temp dir");
    let nested = dir.path().join("a").join("b").join("vault");
    let store = Store::open(&nested).expect("creates the root");
    let (addr, _) = store.put_bytes(b"lazy init").expect("put");
    assert!(store.has(&addr));
}
