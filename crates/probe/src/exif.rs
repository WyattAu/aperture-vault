//! EXIF: the TIFF structure embedded in a JPEG APP1 segment.
//!
//! An EXIF block is a TIFF file without a header strip: an endianness mark
//! (`II` little-endian, `MM` big-endian), a magic 42, an offset to IFD0, and
//! from there a tree of Image File Directories. Three directories matter to a
//! vault: IFD0 (camera, orientation, and pointers onward), the Exif sub-IFD
//! (capture time), and the GPS sub-IFD (where).
//!
//! # The trap this module is built around
//!
//! Every offset in an EXIF block is relative to the **TIFF header**, not to the
//! start of the JPEG file or the APP1 payload. A reader that resolves offsets
//! against the wrong base parses *plausible garbage* — strings that look like
//! camera models, coordinates that are small numbers. [`Exif::parse`] takes the
//! TIFF block alone, so the base is unambiguous by construction.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

/// A parsed EXIF block. Absent fields are `None`; parsing never fails, it
/// just finds less.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Exif {
    /// `Make`, whitespace-trimmed.
    pub make: Option<String>,
    /// `Model`, whitespace-trimmed.
    pub model: Option<String>,
    /// `DateTime` in EXIF's `YYYY:MM:DD HH:MM:SS` form, trimmed.
    pub date_time: Option<String>,
    /// `DateTimeOriginal` — when the shutter fired.
    pub date_time_original: Option<String>,
    /// `Orientation` value 1..=8.
    pub orientation: Option<u8>,
    /// `ExifImageWidth` (pixels).
    pub pixel_x: Option<u32>,
    /// `ExifImageHeight` (pixels).
    pub pixel_y: Option<u32>,
    /// Latitude in signed decimal degrees (north positive).
    pub latitude: Option<f64>,
    /// Longitude in signed decimal degrees (east positive).
    pub longitude: Option<f64>,
}

impl Exif {
    /// Parse an EXIF block: the TIFF structure including its 8-byte header.
    ///
    /// Parsing is total — every access is bounds-checked, and a truncated or
    /// malformed structure yields an empty [`Exif`] rather than a partial one.
    /// Callers distinguish "no metadata" from "metadata we failed to read" at
    /// the segment layer, where the APP1 marker tells them a block exists.
    #[must_use]
    pub fn parse(tiff: &[u8]) -> Exif {
        let mut out = Exif::default();
        let little = match (tiff.first(), tiff.get(1)) {
            (Some(b'I'), Some(b'I')) => true,
            (Some(b'M'), Some(b'M')) => false,
            _ => return out,
        };
        if read_u16(tiff, 2, little) != Some(42) {
            return out;
        }
        let Some(ifd0_off) = read_u32(tiff, 4, little) else {
            return out;
        };

        let mut exif_ifd = None;
        let mut gps_ifd = None;
        if let Some(count) = read_u16(tiff, ifd0_off as usize, little) {
            for i in 0..count as usize {
                let Some(e) = entry_at(tiff, ifd0_off, i, little) else {
                    break;
                };
                match e.tag {
                    0x010F => out.make = e.ascii(tiff).map(trim),
                    0x0110 => out.model = e.ascii(tiff).map(trim),
                    0x0112 => out.orientation = e.first_short(tiff).filter(|v| (1..=8).contains(v)),
                    0x0132 => out.date_time = e.ascii(tiff).map(trim),
                    // A sub-IFD pointer is a LONG whose *value* is the offset
                    // — inline, since count 1 fits the 4-byte field. Reading
                    // the field's position instead is how a reader ends up
                    // walking an IFD at offset 66, finding nothing, and
                    // reporting the file as metadata-free.
                    0x8769 => exif_ifd = e.first_long(tiff),
                    0x8825 => gps_ifd = e.first_long(tiff),
                    _ => {}
                }
            }
        }

        if let Some(off) = exif_ifd {
            if let Some(count) = read_u16(tiff, off as usize, little) {
                for i in 0..count as usize {
                    let Some(e) = entry_at(tiff, off, i, little) else {
                        break;
                    };
                    match e.tag {
                        0x9003 => out.date_time_original = e.ascii(tiff).map(trim),
                        0xA002 => out.pixel_x = e.first_long(tiff),
                        0xA003 => out.pixel_y = e.first_long(tiff),
                        _ => {}
                    }
                }
            }
        }

        if let Some((lat, lon)) = gps_pair(tiff, gps_ifd) {
            out.latitude = Some(lat);
            out.longitude = Some(lon);
        }

        out
    }
}

/// Read the GPS pair: latitude and longitude as signed decimal degrees.
///
/// Requires the reference characters (`N`/`S`, `E`/`W`) to be present; a GPS
/// block without references has no sign, and guessing one would fabricate a
/// hemisphere.
fn gps_pair(tiff: &[u8], gps_ifd: Option<u32>) -> Option<(f64, f64)> {
    let lat_ref = gps_ascii(tiff, gps_ifd, 0x0001)?;
    let lat = gps_rational_sum(tiff, gps_ifd, 0x0002)?;
    let lon_ref = gps_ascii(tiff, gps_ifd, 0x0003)?;
    let lon = gps_rational_sum(tiff, gps_ifd, 0x0004)?;
    let lat_signed = match lat_ref.as_str() {
        "N" => lat,
        "S" => -lat,
        _ => return None,
    };
    let lon_signed = match lon_ref.as_str() {
        "E" => lon,
        "W" => -lon,
        _ => return None,
    };
    Some((lat_signed, lon_signed))
}

fn gps_ascii(tiff: &[u8], gps_ifd: Option<u32>, tag: u16) -> Option<String> {
    let e = gps_entry(tiff, gps_ifd, tag)?;
    let off = e.value_offset()? as usize;
    let bytes: Vec<u8> = tiff.get(off..off + e.count as usize)?.to_vec();
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    let text = bytes.get(..end)?;
    Some(String::from_utf8_lossy(text).into_owned())
}

fn gps_rational_sum(tiff: &[u8], gps_ifd: Option<u32>, tag: u16) -> Option<f64> {
    let e = gps_entry(tiff, gps_ifd, tag)?;
    let little = little_for(tiff);
    let off = e.value_offset()? as usize;
    // DMS weights: degrees + minutes/60 + seconds/3600. Summing the rationals
    // bare — the first draft's mistake — returns 118.9 for a coordinate at
    // 41° 53' 25": the degrees and minutes added as if they were seconds.
    let weights = [1.0f64, 1.0 / 60.0, 1.0 / 3600.0];
    let mut sum = 0.0f64;
    for (k, weight) in weights.iter().enumerate() {
        let at = off + k * 8;
        let Some(num) = read_u32(tiff, at, little) else {
            break;
        };
        let Some(den) = read_u32(tiff, at + 4, little) else {
            break;
        };
        if den == 0 {
            return None;
        }
        sum += num as f64 / den as f64 * weight;
    }
    Some(sum)
}

fn gps_entry(tiff: &[u8], gps_ifd: Option<u32>, tag: u16) -> Option<Entry> {
    let off = gps_ifd?;
    let little = little_for(tiff);
    let count = read_u16(tiff, off as usize, little)?;
    for i in 0..count as usize {
        let e = entry_at(tiff, off, i, little)?;
        if e.tag == tag {
            return Some(e);
        }
    }
    None
}

fn trim(s: String) -> String {
    s.trim_end_matches(['\0', ' ']).trim().to_string()
}

/// One 12-byte IFD entry: tag, type, count, and the 4-byte value field —
/// an inline value when it fits, otherwise an offset to the real bytes.
#[derive(Debug, Clone, Copy)]
struct Entry {
    tag: u16,
    typ: u16,
    count: u32,
    field: u32,
    field_at: u32,
}

impl Entry {
    /// The value's absolute offset: the inline field when the value fits,
    /// otherwise the field read as a pointer.
    fn value_offset(&self) -> Option<u32> {
        let bytes = match self.typ {
            1 | 2 | 7 => self.count,
            3 | 9 => self.count * 2,
            4 => self.count * 4,
            5 | 10 => self.count * 8,
            _ => return None,
        };
        if bytes <= 4 {
            Some(self.field_at)
        } else {
            Some(self.field)
        }
    }

    /// The first SHORT of the value field — how EXIF packs a single-count
    /// SHORT into the 4-byte field (left-aligned in big-endian files).
    fn first_short(&self, tiff: &[u8]) -> Option<u8> {
        if self.typ != 3 || self.count == 0 {
            return None;
        }
        let at = self.value_offset()? as usize;
        let b0 = *tiff.get(at)?;
        let b1 = *tiff.get(at + 1)?;
        Some(if little_for(tiff) { b0 } else { b1 })
    }

    /// The first LONG of the value field.
    fn first_long(&self, tiff: &[u8]) -> Option<u32> {
        if self.typ != 4 || self.count == 0 {
            return None;
        }
        let at = self.value_offset()? as usize;
        read_u32(tiff, at, little_for(tiff))
    }

    /// The ASCII value: `count` bytes including the terminating NUL, decoded
    /// lossily and cut at the first NUL.
    fn ascii(&self, tiff: &[u8]) -> Option<String> {
        if self.typ != 2 {
            return None;
        }
        let off = self.value_offset()? as usize;
        let bytes: Vec<u8> = tiff.get(off..off + self.count as usize)?.to_vec();
        let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
        let text = bytes.get(..end)?;
        Some(String::from_utf8_lossy(text).into_owned())
    }
}

fn entry_at(tiff: &[u8], ifd_off: u32, i: usize, little: bool) -> Option<Entry> {
    let i = u32::try_from(i).ok()?;
    let base = ifd_off.checked_add(2)?.checked_add(i.checked_mul(12)?)?;
    let base = usize::try_from(base).ok()?;
    let tag = read_u16(tiff, base, little)?;
    let typ = read_u16(tiff, base + 2, little)?;
    let count = read_u32(tiff, base + 4, little)?;
    let field_at = u32::try_from(base + 8).ok()?;
    let field = read_u32(tiff, field_at as usize, little)?;
    Some(Entry {
        tag,
        typ,
        count,
        field,
        field_at,
    })
}

fn little_for(tiff: &[u8]) -> bool {
    tiff.first() == Some(&b'I')
}

fn read_u16(data: &[u8], at: usize, little: bool) -> Option<u16> {
    let b = data.get(at..at.checked_add(2)?)?;
    Some(if little {
        u16::from_le_bytes(*b.first_chunk::<2>()?)
    } else {
        u16::from_be_bytes(*b.first_chunk::<2>()?)
    })
}

fn read_u32(data: &[u8], at: usize, little: bool) -> Option<u32> {
    let b = data.get(at..at.checked_add(4)?)?;
    Some(if little {
        u32::from_le_bytes(*b.first_chunk::<4>()?)
    } else {
        u32::from_be_bytes(*b.first_chunk::<4>()?)
    })
}

/// Serialize a minimal TIFF structure from typed fields — the writer the
/// round-trip tests and the ingest sidecar path use.
///
/// Little-endian throughout: Aperture writes one canonical form and accepts
/// both on read. Layout: header(8) | IFD0 | IFD0-heap | ExifIFD |
/// ExifIFD-heap | GpsIFD | GpsIFD-heap, with sub-IFD pointers patched to the
/// layouts above. Values larger than 4 bytes heap directly after their own
/// IFD.
#[must_use]
pub fn to_tiff(exif: &Exif) -> Vec<u8> {
    let ascii = |tag: u16, value: &str| -> (u16, u16, u32, Vec<u8>) {
        let mut bytes = value.as_bytes().to_vec();
        bytes.push(0);
        (tag, 2, bytes.len() as u32, bytes)
    };

    let mut ifd0: Vec<(u16, u16, u32, Vec<u8>)> = Vec::new();
    let mut exif_ifd: Vec<(u16, u16, u32, Vec<u8>)> = Vec::new();
    let mut gps_ifd: Vec<(u16, u16, u32, Vec<u8>)> = Vec::new();

    if let Some(m) = &exif.make {
        ifd0.push(ascii(0x010F, m));
    }
    if let Some(m) = &exif.model {
        ifd0.push(ascii(0x0110, m));
    }
    if let Some(d) = &exif.date_time {
        ifd0.push(ascii(0x0132, d));
    }
    if let Some(o) = exif.orientation {
        ifd0.push((0x0112, 3, 1, vec![o, 0, 0, 0]));
    }

    if let Some(d) = &exif.date_time_original {
        exif_ifd.push(ascii(0x9003, d));
    }
    if let (Some(w), Some(h)) = (exif.pixel_x, exif.pixel_y) {
        exif_ifd.push((0xA002, 4, 1, w.to_le_bytes().to_vec()));
        exif_ifd.push((0xA003, 4, 1, h.to_le_bytes().to_vec()));
    }

    if let (Some(lat), Some(lon)) = (exif.latitude, exif.longitude) {
        let (lat_ref, lat_v) = if lat >= 0.0 {
            (b'N', lat)
        } else {
            (b'S', -lat)
        };
        let (lon_ref, lon_v) = if lon >= 0.0 {
            (b'E', lon)
        } else {
            (b'W', -lon)
        };
        // The magnitude is non-negative here — hemisphere signs are carried by
        // the refs — so integer truncation *is* the floor, and no libm floor
        // is needed in no_std.
        let dms = |v: f64| {
            let deg = v as u32;
            let min_f = (v - deg as f64) * 60.0;
            let min = min_f as u32;
            let sec = (min_f - min as f64) * 60.0;
            ((deg, 1u32), (min, 1u32), ((sec * 100.0) as u32, 100u32))
        };
        gps_ifd.push((0x0001, 2, 2, vec![lat_ref, 0, 0, 0]));
        gps_ifd.push((0x0003, 2, 2, vec![lon_ref, 0, 0, 0]));
        // One RATIONAL count-3 entry per coordinate — how real cameras write
        // it, and how gps_rational_sum reads it. Three count-1 entries would
        // leave the reader summing degrees only.
        let mut rat3 = |tag: u16, v: f64| {
            let ((d_num, d_den), (m_num, m_den), (s_num, s_den)) = dms(v);
            let mut bytes = Vec::with_capacity(24);
            for (num, den) in [(d_num, d_den), (m_num, m_den), (s_num, s_den)] {
                bytes.extend_from_slice(&num.to_le_bytes());
                bytes.extend_from_slice(&den.to_le_bytes());
            }
            gps_ifd.push((tag, 5, 3, bytes));
        };
        rat3(0x0002, lat_v);
        rat3(0x0004, lon_v);
    }

    // The sub-IFD pointers: without them the Exif and GPS IFDs are written
    // but orphaned — a spec defect the round-trip test caught, because the
    // pointer entries were simply never pushed into IFD0. Computed after all
    // sub-IFD fills, deliberately: the flags must reflect what was actually
    // written.
    let has_exif = !exif_ifd.is_empty();
    let has_gps = !gps_ifd.is_empty();
    if has_exif {
        ifd0.push((0x8769, 4, 1, vec![0, 0, 0, 0]));
    }
    if has_gps {
        ifd0.push((0x8825, 4, 1, vec![0, 0, 0, 0]));
    }

    let ifd_len = |n: usize| 2 + n * 12 + 4;
    let ifd_len32 = |n: usize| ifd_len(n) as u32;

    // Sizes first: the sub-IFD pointers need the layouts before emission.
    let ifd0_size = ifd_len(ifd0.len()) as u32;
    let ifd0_heap_size: u32 = ifd0
        .iter()
        .filter(|(_, _, _, v)| v.len() > 4)
        .map(|(_, _, _, v)| v.len() as u32)
        .sum();
    let exif_at = 8 + ifd0_size + ifd0_heap_size;

    let exif_size = ifd_len32(exif_ifd.len());
    let exif_heap_size: u32 = exif_ifd
        .iter()
        .filter(|(_, _, _, v)| v.len() > 4)
        .map(|(_, _, _, v)| v.len() as u32)
        .sum();
    let gps_at = exif_at + exif_size + exif_heap_size;
    let gps_size = ifd_len32(gps_ifd.len());
    let _gps_heap_size: u32 = gps_ifd
        .iter()
        .filter(|(_, _, _, v)| v.len() > 4)
        .map(|(_, _, _, v)| v.len() as u32)
        .sum();

    let mut out = vec![b'I', b'I', 42, 0];
    out.extend_from_slice(&8u32.to_le_bytes());

    let emit = |out: &mut Vec<u8>,
                at: u32,
                entries: &[(u16, u16, u32, Vec<u8>)],
                next: u32,
                heap: &mut Vec<u8>| {
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        let mut heap_at = at + ifd_len(entries.len()) as u32;
        for (tag, typ, count, value) in entries {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&typ.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
            if value.len() <= 4 {
                let mut field = value.clone();
                field.resize(4, 0);
                out.extend_from_slice(&field);
            } else {
                out.extend_from_slice(&heap_at.to_le_bytes());
                heap_at += value.len() as u32;
            }
        }
        out.extend_from_slice(&next.to_le_bytes());
        out.extend_from_slice(heap);
    };

    // IFD0 with its heap inline (it sits between IFD0 and the ExifIFD).
    let mut ifd0_entries: Vec<(u16, u16, u32, Vec<u8>)> = Vec::new();
    let mut ifd0_heap: Vec<u8> = Vec::new();
    {
        let mut heap_at = 8 + ifd0_size;
        for (tag, typ, count, value) in &ifd0 {
            if value.len() <= 4 {
                let mut field = value.clone();
                field.resize(4, 0);
                ifd0_entries.push((*tag, *typ, *count, field));
            } else {
                ifd0_entries.push((*tag, *typ, *count, heap_at.to_le_bytes().to_vec()));
                ifd0_heap.extend_from_slice(value);
                heap_at += value.len() as u32;
            }
        }
    }
    // Patch the sub-IFD pointers now that the layouts are known.
    for (tag, _, _, field) in &mut ifd0_entries {
        match *tag {
            0x8769 => *field = exif_at.to_le_bytes().to_vec(),
            0x8825 => *field = gps_at.to_le_bytes().to_vec(),
            _ => {}
        }
        let _ = field;
    }
    emit(
        &mut out,
        8,
        &ifd0_entries,
        if exif_ifd.is_empty() { 0 } else { exif_at },
        &mut ifd0_heap,
    );

    if !exif_ifd.is_empty() {
        let mut exif_entries: Vec<(u16, u16, u32, Vec<u8>)> = Vec::new();
        let mut exif_heap: Vec<u8> = Vec::new();
        let mut heap_at = exif_at + exif_size;
        for (tag, typ, count, value) in &exif_ifd {
            if value.len() <= 4 {
                let mut field = value.clone();
                field.resize(4, 0);
                exif_entries.push((*tag, *typ, *count, field));
            } else {
                exif_entries.push((*tag, *typ, *count, heap_at.to_le_bytes().to_vec()));
                exif_heap.extend_from_slice(value);
                heap_at += value.len() as u32;
            }
        }
        let next = if gps_ifd.is_empty() { 0 } else { gps_at };
        emit(&mut out, exif_at, &exif_entries, next, &mut exif_heap);
    }

    if !gps_ifd.is_empty() {
        let mut gps_entries: Vec<(u16, u16, u32, Vec<u8>)> = Vec::new();
        let mut gps_heap: Vec<u8> = Vec::new();
        let mut heap_at = gps_at + gps_size;
        for (tag, typ, count, value) in &gps_ifd {
            if value.len() <= 4 {
                let mut field = value.clone();
                field.resize(4, 0);
                gps_entries.push((*tag, *typ, *count, field));
            } else {
                gps_entries.push((*tag, *typ, *count, heap_at.to_le_bytes().to_vec()));
                gps_heap.extend_from_slice(value);
                heap_at += value.len() as u32;
            }
        }
        emit(&mut out, gps_at, &gps_entries, 0, &mut gps_heap);
    }
    out
}
