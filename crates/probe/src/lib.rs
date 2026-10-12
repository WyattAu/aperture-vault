//! Asset metadata probe for the Aperture vault.
//!
//! One call — [`probe`] — turns raw asset bytes into the fields a vault index
//! stores: dimensions, capture time, camera, GPS. Photos are parsed from JPEG
//! EXIF directly; PNG contributes dimensions; video is delegated to the
//! estate's `mp4-core`.
//!
//! # Honesty rules
//!
//! - Parsing is *total*: malformed bytes yield an empty [`AssetMeta`], never a
//!   partial one or an error. A vault listing must not fail because one file
//!   is corrupt.
//! - Nothing is guessed. If the EXIF block is absent, `taken_at` is `None` —
//!   the index falls back to file mtime *at the index layer*, visibly, rather
//!   than the probe fabricating a capture time.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

pub mod exif;
pub mod jpeg;
pub mod png;

use alloc::string::String;

/// Everything a vault index needs about one asset, probed from its bytes.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AssetMeta {
    /// Declared or parsed width in pixels.
    pub width: Option<u32>,
    /// Declared or parsed height in pixels.
    pub height: Option<u32>,
    /// When the photo was taken (EXIF `DateTimeOriginal`, else `DateTime`),
    /// in EXIF's `YYYY:MM:DD HH:MM:SS` local-time form.
    pub taken_at: Option<String>,
    /// Camera manufacturer (EXIF `Make`).
    pub make: Option<String>,
    /// Camera model (EXIF `Model`).
    pub model: Option<String>,
    /// GPS latitude in signed decimal degrees (north/east positive).
    pub latitude: Option<f64>,
    /// GPS longitude in signed decimal degrees (north/east positive).
    pub longitude: Option<f64>,
    /// Orientation as the EXIF 1..8 value, when present and in range.
    pub orientation: Option<u8>,
    /// Video duration in milliseconds (MP4 only).
    pub duration_ms: Option<u64>,
}

/// Probe asset bytes.
#[must_use]
pub fn probe(data: &[u8]) -> AssetMeta {
    if let Some(meta) = jpeg::probe(data) {
        return meta;
    }
    if let Some(meta) = png::probe(data) {
        return meta;
    }
    mp4_probe(data)
}

fn mp4_probe(data: &[u8]) -> AssetMeta {
    let mut meta = AssetMeta::default();
    let Ok(file) = mp4_core::Mp4File::parse(data) else {
        return meta;
    };
    meta.duration_ms = file.duration_ms();
    // The moov box, found by a second top-level walk: the dimensions live in
    // the sample entry, and this keeps the probe's mp4 use symmetric with the
    // windowed reader's.
    if let Ok(boxes) = mp4_core::parse_boxes(data) {
        let moov = boxes.iter().find(|b| b.box_type == *b"moov");
        if let Some(dims) = moov.and_then(mp4_core::read_video_dimensions_in) {
            meta.width = Some(dims.0);
            meta.height = Some(dims.1);
        }
    }
    meta
}
