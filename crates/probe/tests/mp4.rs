//! MP4 probe: duration and dimensions, delegated to the estate's mp4-core.
//! The fixture is assembled from the mp4-core builders so every byte is
//! accounted for.

// Fixtures are allowed to assert: an `expect` here is the test stating what it
// assumes, the honest form, and the crate lint denies it everywhere a caller
// would write it.
#![allow(clippy::expect_used)]

use aperture_probe::probe;
use mp4_core::chapter::{Chapter, ChapterList};
use mp4_core::container::{container, leaf};
use mp4_core::moov::{FileType, MovieHeader};

fn mp4_fixture() -> Vec<u8> {
    let chapters = ChapterList {
        chapters: vec![Chapter::new(0, "Opening"), Chapter::new(600_000, "Later")],
    };
    let moov = container(
        *b"moov",
        vec![
            leaf(*b"mvhd", MovieHeader::to_payload(1000, 3_600_000)).expect("infallible"),
            chapters.to_chpl_box(),
        ],
    );
    // ftyp | mdat | moov — the shape ffmpeg writes.
    let mut data = leaf(*b"ftyp", FileType::audiobook().to_payload())
        .expect("infallible")
        .to_bytes();
    data.extend_from_slice(
        &leaf(*b"mdat", vec![0u8; 2048])
            .expect("infallible")
            .to_bytes(),
    );
    data.extend_from_slice(&moov.to_bytes());
    data
}

#[test]
fn an_mp4_reports_its_duration() {
    let meta = probe(&mp4_fixture());
    assert_eq!(meta.duration_ms, Some(3_600_000));
    assert_eq!(meta.width, None, "the fixture has no video track");
}
