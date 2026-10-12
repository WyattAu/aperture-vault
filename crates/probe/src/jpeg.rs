//! JPEG: segment walk and EXIF extraction.
//!
//! A JPEG is a sequence of markers, each `FF marker [length] payload...`. The
//! EXIF block lives in an `APP1` segment whose payload starts with
//! `Exif\0\0` followed by the TIFF structure. APP1 segments can also carry
//! XMP (`http://ns.adobe.com/xap/1.0/`) — distinguished by the payload
//! prefix, not by position, so a file with XMP first and EXIF second parses
//! correctly.

use crate::exif::Exif;
use crate::AssetMeta;
use alloc::vec;
use alloc::vec::Vec;

/// Probe JPEG bytes.
#[must_use]
pub fn probe(data: &[u8]) -> Option<AssetMeta> {
    // SOI, then a marker: without both there is no JPEG here.
    if data.first() != Some(&0xFF) || data.get(1) != Some(&0xD8) {
        return None;
    }
    let mut meta = AssetMeta::default();
    let mut exif_seen = false;
    let mut at = 2usize;
    while at + 4 <= data.len() {
        let Some(byte) = data.get(at) else {
            break;
        };
        if *byte != 0xFF {
            // Skip fill bytes between markers (padding runs of 0xFF).
            at += 1;
            continue;
        }
        let Some(marker) = data.get(at + 1).copied() else {
            break;
        };
        match marker {
            // Standalone markers carry no length field.
            0xD8 | 0x01 | 0xD0..=0xD7 => {
                at += 2;
                continue;
            }
            // End of image.
            0xD9 => break,
            _ => {}
        }
        let Some(high) = data.get(at + 2).copied() else {
            break;
        };
        let Some(low) = data.get(at + 3).copied() else {
            break;
        };
        let seg_len = (u16::from(high) << 8) | u16::from(low);
        if seg_len < 2 {
            break;
        }
        let payload_at = at + 4;
        let payload_end = payload_at
            .saturating_add(seg_len as usize - 2)
            .min(data.len());
        match marker {
            // SOF0/1/2/3/9/10: frame header — dimensions.
            0xC0 | 0xC1 | 0xC2 | 0xC3 | 0xC9 | 0xCA => {
                // Payload: precision(1), height(2), width(2).
                if let (Some(h), Some(w)) =
                    (u16be(data, payload_at + 1), u16be(data, payload_at + 3))
                {
                    if meta.height.is_none() {
                        meta.height = Some(u32::from(h));
                    }
                    if meta.width.is_none() {
                        meta.width = Some(u32::from(w));
                    }
                }
            }
            0xE1 => {
                // APP1: "Exif\0\0" prefix marks the TIFF block.
                let is_exif = data.get(payload_at..payload_at + 4) == Some(b"Exif".as_slice())
                    && data.get(payload_at + 4) == Some(&0)
                    && data.get(payload_at + 5) == Some(&0);
                if is_exif && !exif_seen {
                    exif_seen = true;
                    let tiff_at = payload_at + 6;
                    if let Some(tiff) = data.get(tiff_at..payload_end) {
                        let exif = Exif::parse(tiff);
                        meta.taken_at = exif
                            .date_time_original
                            .clone()
                            .or_else(|| exif.date_time.clone());
                        meta.make = exif.make.clone();
                        meta.model = exif.model.clone();
                        meta.latitude = exif.latitude;
                        meta.longitude = exif.longitude;
                        meta.orientation = exif.orientation;
                    }
                }
            }
            _ => {}
        }
        at = payload_end;
    }
    Some(meta)
}

fn u16be(data: &[u8], at: usize) -> Option<u16> {
    let hi = *data.get(at)? as u16;
    let lo = *data.get(at + 1)? as u16;
    Some((hi << 8) | lo)
}

/// Build a minimal JPEG around an EXIF TIFF block: SOI, APP1, SOF0 with the
/// given dimensions, EOI. The fixture the segment-walk tests run against.
#[must_use]
pub fn jpeg_with_exif(tiff: &[u8], width: u16, height: u16) -> Vec<u8> {
    let mut out = vec![0xFF, 0xD8];
    // APP1 payload: "Exif\0\0" + tiff.
    let mut app1 = b"Exif\0\0".to_vec();
    app1.extend_from_slice(tiff);
    let seg_len = app1.len() + 2;
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&(seg_len as u16).to_be_bytes());
    out.extend_from_slice(&app1);
    // SOF0: precision 8, height, width.
    out.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x0B, 0x08]);
    out.extend_from_slice(&height.to_be_bytes());
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&[0x01, 0x01, 0x00]);
    // EOI.
    out.extend_from_slice(&[0xFF, 0xD9]);
    out
}
