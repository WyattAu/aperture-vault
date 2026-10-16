//! Thumbnail derivation driven against a real CAS and a real index: the
//! first call decodes and writes, the second call reads the cache, a
//! non-image is refused honestly, and the longest-edge rule is checked on a
//! wide original.

// Fixtures are allowed to assert: an `expect` here is the test stating what it
// assumes, the honest form, and the workspace lint denies it everywhere a
// caller would write it.
#![allow(clippy::expect_used)]

use aperture_cas::{Address, Store};
use aperture_index::Index;
use aperture_pipeline::thumb::{thumbnail, ThumbSize};
use aperture_pipeline::Pipeline;
use image::{imageops::FilterType, DynamicImage, ImageFormat};
use std::io::Cursor;

fn env() -> (tempfile::TempDir, Store, Index) {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Store::open(dir.path().join("vault")).expect("opens");
    let index = Index::open(&dir.path().join("index.db")).expect("opens");
    (dir, store, index)
}

/// A real decodable PNG of the given size, ingested through the pipeline.
fn ingest_png(store: &Store, index: &Index, width: u32, height: u32) -> Address {
    let img = DynamicImage::new_rgb8(width, height);
    let mut png = Vec::new();
    img.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .expect("encodes");
    Pipeline::ingest(store, index, Cursor::new(&png), Some("shot.png"))
        .expect("ingests")
        .address
}

#[test]
fn generates_once_then_reads_the_cache() {
    let (_dir, store, index) = env();
    let source = ingest_png(&store, &index, 400, 300);

    let first = thumbnail(&store, &index, &source, ThumbSize::Thumbnail).expect("generates");
    assert!(!first.cached);
    assert_eq!(
        first.bytes.get(0..2),
        Some(&b"\xFF\xD8"[..]),
        "a real JPEG starts with its magic"
    );
    assert!(
        first.bytes.len() < 400 * 300 * 3,
        "smaller than the original"
    );

    let second = thumbnail(&store, &index, &source, ThumbSize::Thumbnail).expect("cache hit");
    assert!(
        second.cached,
        "the second request is a read, not a re-decode"
    );
    assert_eq!(second.address, first.address);
    assert_eq!(second.bytes, first.bytes);
}

#[test]
fn each_size_caches_under_its_own_spec() {
    let (_dir, store, index) = env();
    let source = ingest_png(&store, &index, 3200, 2000);

    let small = thumbnail(&store, &index, &source, ThumbSize::Thumbnail).expect("generates");
    let large = thumbnail(&store, &index, &source, ThumbSize::Preview).expect("generates");
    assert_ne!(small.address, large.address, "two sizes, two blobs");

    let decoded = image::load_from_memory(&large.bytes).expect("decodes back");
    assert!(
        decoded.width().max(decoded.height()) <= 1440,
        "the preview's longest edge is bounded: {}x{}",
        decoded.width(),
        decoded.height()
    );
    let ratio = f64::from(decoded.width()) / f64::from(decoded.height());
    assert!(
        (ratio - 1.6).abs() < 0.05,
        "aspect preserved: {ratio} vs 1.6"
    );
}

#[test]
fn a_non_image_is_refused_not_imagined() {
    let (_dir, store, index) = env();
    let not_an_image = b"\x00\x01\x02definitely not pixels".repeat(50);
    let source = Pipeline::ingest(&store, &index, Cursor::new(&not_an_image), Some("x.bin"))
        .expect("ingests");

    let outcome = thumbnail(&store, &index, &source.address, ThumbSize::Thumbnail);
    assert!(
        matches!(outcome, Err(aperture_pipeline::PipelineError::NotAnImage)),
        "a non-image has no thumbnail; the error says so"
    );
    assert_eq!(
        index
            .derived(&source.address.hex(), ThumbSize::Thumbnail.spec())
            .expect("query"),
        None,
        "nothing cached for a generation that failed"
    );
}

#[test]
fn small_originals_are_not_upscaled() {
    let (_dir, store, index) = env();
    let source = ingest_png(&store, &index, 100, 50);

    let thumb = thumbnail(&store, &index, &source, ThumbSize::Thumbnail).expect("generates");
    let decoded = image::load_from_memory(&thumb.bytes).expect("decodes back");
    assert_eq!(
        (decoded.width(), decoded.height()),
        (100, 50),
        "a 100px original stays 100px; enlargement would be invention"
    );
}

#[test]
fn unknown_size_strings_do_not_parse() {
    assert_eq!(ThumbSize::parse("thumbnail"), Some(ThumbSize::Thumbnail));
    assert_eq!(ThumbSize::parse("preview"), Some(ThumbSize::Preview));
    assert_eq!(ThumbSize::parse("blurhash"), None);
    assert_eq!(ThumbSize::parse(""), None);
}

#[test]
fn resize_filter_preserves_aspect_for_the_record() {
    // Documents the filter choice: Triangle on a synthetic gradient keeps
    // the resized image's aspect within the bound, the property the grid
    // layout depends on.
    let img = DynamicImage::new_rgb8(2000, 1000);
    let resized = img.resize(250, 250, FilterType::Triangle);
    let ratio = f64::from(resized.width()) / f64::from(resized.height());
    assert!((ratio - 2.0).abs() < 0.05, "2:1 stays 2:1, got {ratio}");
}

#[test]
fn thumbnail_generation_also_yields_the_thumbhash() {
    let (_dir, store, index) = env();
    let source = ingest_png(&store, &index, 300, 200);

    // The hash is recorded by thumbnail generation itself, from the same
    // decode — one read of the original pays for both artifacts.
    let _ = thumbnail(&store, &index, &source, ThumbSize::Thumbnail).expect("generates");
    let record = index
        .asset(&source.hex())
        .expect("query")
        .expect("recorded");
    let b64 = record.thumbhash.expect("placeholder stored");
    assert!(!b64.is_empty());

    // It is a real ThumbHash: decodable back to pixels, byte-for-byte stable
    // across a re-derivation through the standalone path.
    use base64::Engine as _;
    let hash = base64::engine::general_purpose::STANDARD
        .decode(&b64)
        .expect("base64");
    let (w, h, _rgba) = thumbhash::thumb_hash_to_rgba(&hash).expect("decodes");
    assert!(
        (1..=32).contains(&w) && (1..=32).contains(&h),
        "hash dims in range"
    );

    let again = aperture_pipeline::thumb::thumbhash(&store, &index, &source).expect("cached");
    assert_eq!(
        again, b64,
        "the stored placeholder is returned, not recomputed"
    );
}

#[test]
fn thumbhash_of_a_non_image_is_refused() {
    let (_dir, store, index) = env();
    let not_an_image = b"\x00\x01not pixels".repeat(64);
    let report = Pipeline::ingest(&store, &index, Cursor::new(&not_an_image), Some("x.bin"))
        .expect("ingests");
    let outcome = aperture_pipeline::thumb::thumbhash(&store, &index, &report.address);
    assert!(matches!(
        outcome,
        Err(aperture_pipeline::PipelineError::NotAnImage)
    ));
}

#[test]
fn derived_blobs_are_marked_too() {
    let (_dir, store, index) = env();
    let source = ingest_png(&store, &index, 200, 150);
    let _ = thumbnail(&store, &index, &source, ThumbSize::Thumbnail).expect("derives");
    // The derived blob has no assets row; it is referenced through the
    // derived table — the mark set must include it or every thumbnail
    // would die at the first gc.
    let report = aperture_pipeline::gc::gc(&store, &index).expect("gc");
    assert_eq!(report.removed, 0, "derived mappings are reachability roots");
}
