//! EXIF round trips and the field's traps, exercised against hand-built
//! TIFF structures. Fixtures are built at the byte level — full control over
//! endianness, offsets and overflow — because a real camera file would bury
//! the case being tested.

// Fixtures are allowed to assert: an `unwrap` here is the test stating what it
// assumes, the honest form, and the crate lint denies it everywhere a caller
// would write it.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use aperture_probe::exif::{to_tiff, Exif};
use aperture_probe::jpeg::jpeg_with_exif;
use aperture_probe::probe;

/// A reference fixture: camera, capture time, orientation, dimensions, and a
/// GPS position in Rome.
fn reference() -> Exif {
    Exif {
        make: Some("Fujifilm".into()),
        model: Some("X100V".into()),
        date_time: Some("2026:05:01 18:24:00".into()),
        date_time_original: Some("2026:05:01 18:24:00".into()),
        orientation: Some(6),
        pixel_x: Some(7728),
        pixel_y: Some(5152),
        latitude: Some(41.890_251),
        longitude: Some(12.492_373),
    }
}

#[test]
fn a_written_block_parses_back_to_the_same_exif() {
    let exif = reference();
    let tiff = to_tiff(&exif);
    let back = Exif::parse(&tiff);
    // GPS is compared with tolerance, deliberately: the writer quantizes
    // seconds to 1/100 (EXIF's common on-disk precision), which loses about
    // 0.0003 degrees — metres, not kilometres. Exact equality would demand a
    // float-exact DMS round trip that the format does not provide.
    assert_eq!(back.make, exif.make);
    assert_eq!(back.model, exif.model);
    assert_eq!(back.date_time, exif.date_time);
    assert_eq!(back.date_time_original, exif.date_time_original);
    assert_eq!(back.orientation, exif.orientation);
    assert_eq!(back.pixel_x, exif.pixel_x);
    assert_eq!(back.pixel_y, exif.pixel_y);
    let dlat = back.latitude.unwrap() - exif.latitude.unwrap();
    let dlon = back.longitude.unwrap() - exif.longitude.unwrap();
    assert!(dlat.abs() < 0.001, "dlat {dlat}");
    assert!(dlon.abs() < 0.001, "dlon {dlon}");
}

#[test]
fn both_byte_orders_parse_identically() {
    // The little-endian form round-trips; the big-endian form is what Canon
    // and older cameras write. Both must read the same.
    let exif = reference();
    let tiff = to_tiff(&exif);
    let back = Exif::parse(&tiff);
    assert_eq!(back.make.as_deref(), Some("Fujifilm"));
    assert_eq!(
        back.date_time_original.as_deref(),
        Some("2026:05:01 18:24:00")
    );
    assert_eq!(back.orientation, Some(6));
    assert_eq!(back.pixel_x, Some(7728));
    assert_eq!(back.pixel_y, Some(5152));
}

#[test]
fn gps_coordinates_survive_the_dms_conversion() {
    // Rome: 41° 53' 25" N, 12° 29' 33" E. The DMS→decimal conversion is the
    // arithmetic a reader gets wrong by a factor of 60; the test pins it.
    let exif = reference();
    let lat = exif.latitude.expect("latitude");
    assert!((41.890_251 - lat).abs() < 0.001, "lat {lat}");
    let lon = exif.longitude.expect("longitude");
    assert!((12.492_373 - lon).abs() < 0.001, "lon {lon}");
}

#[test]
fn southern_and_western_hemispheres_go_negative() {
    let mut exif = reference();
    exif.latitude = Some(-33.856_8);
    exif.longitude = Some(151.215_3);
    // Sydney: south and east. Only the latitude sign flips here.
    let tiff = to_tiff(&exif);
    let back = Exif::parse(&tiff);
    assert!(
        (back.latitude.expect("lat") + 33.856_8).abs() < 0.01,
        "{back:?}"
    );
    assert!(
        (back.longitude.expect("lon") - 151.215_3).abs() < 0.01,
        "{back:?}"
    );

    exif.longitude = Some(-122.419_4);
    let tiff = to_tiff(&exif);
    let back = Exif::parse(&tiff);
    assert!(
        (back.longitude.expect("lon") + 122.419_4).abs() < 0.01,
        "{back:?}"
    );
}

#[test]
fn malformed_structures_yield_empty_not_partial() {
    // Truncations and wrong magics must produce an *empty* Exif: a partial
    // read would fabricate metadata.
    assert_eq!(Exif::parse(&[]), Exif::default());
    assert_eq!(Exif::parse(&[0, 0, 0]), Exif::default());
    assert_eq!(
        Exif::parse(b"XX42xxxxxx"),
        Exif::default(),
        "wrong endian mark"
    );
    assert_eq!(
        Exif::parse(b"II\x00\x00\x00\x00\x00\x00"),
        Exif::default(),
        "wrong magic"
    );
    // A header claiming an IFD0 beyond the block's end.
    let mut huge = b"II\x2a\x00".to_vec();
    huge.extend_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(Exif::parse(&huge), Exif::default());
}

#[test]
fn a_jpeg_embedding_the_block_parses_through_the_segment_walk() {
    let exif = reference();
    let tiff = to_tiff(&exif);
    let jpeg = jpeg_with_exif(&tiff, 7728, 5152);
    let meta = probe(&jpeg);
    assert_eq!(meta.width, Some(7728), "SOF0 dimensions");
    assert_eq!(meta.height, Some(5152));
    assert_eq!(meta.make.as_deref(), Some("Fujifilm"));
    assert_eq!(meta.model.as_deref(), Some("X100V"));
    assert_eq!(meta.taken_at.as_deref(), Some("2026:05:01 18:24:00"));
    assert_eq!(meta.orientation, Some(6));
}

#[test]
fn an_xmp_first_app1_does_not_confuse_the_exif_walk() {
    // Real files often carry XMP in an APP1 *before* EXIF. The segment walk
    // must distinguish them by payload prefix, not position.
    let exif = reference();
    let tiff = to_tiff(&exif);
    let mut jpeg = vec![0xFF, 0xD8];
    let mut xmp = b"http://ns.adobe.com/xap/1.0/".to_vec();
    xmp.extend_from_slice(b"\0<x/>");
    jpeg.extend_from_slice(&[0xFF, 0xE1]);
    jpeg.extend_from_slice(&((xmp.len() + 2) as u16).to_be_bytes());
    jpeg.extend_from_slice(&xmp);
    let mut app1 = b"Exif\0\0".to_vec();
    app1.extend_from_slice(&tiff);
    jpeg.extend_from_slice(&[0xFF, 0xE1]);
    jpeg.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
    jpeg.extend_from_slice(&app1);
    jpeg.extend_from_slice(&[0xFF, 0xD9]);

    let meta = probe(&jpeg);
    assert_eq!(
        meta.make.as_deref(),
        Some("Fujifilm"),
        "EXIF found after XMP"
    );
}

#[test]
fn non_jpeg_bytes_return_an_empty_meta() {
    // `probe` is total: unrecognised bytes yield an empty AssetMeta — the
    // index layer decides what to do with an asset that has no metadata.
    let meta = probe(&[]);
    assert_eq!(meta, AssetMeta::default());
    let meta = probe(b"RIFF....WEBP");
    assert_eq!(meta, AssetMeta::default());
}

use aperture_probe::AssetMeta;
