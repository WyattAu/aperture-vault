//! Perceptual hashing: the DCT-based pHash, for near-duplicate detection
//! without a model download.
//!
//! YP-PIPELINE-PHASH-009: Immich solves semantic duplicates with an ML
//! embedding stack; a photo vault's duplicate class is *perceptual* —
//! the same image re-compressed, resized, re-encoded, watermarked
//! differently — and the DCT-based pHash (Farid's survey baseline)
//! catches that class with a 64-bit hash and a Hamming distance. The
//! transform is the estate's own `dsp-spectral::dct2`: the loop's
//! dogfood outcome, not a reimplementation.

use crate::PipelineError;
use aperture_cas::{Address, Store};
use aperture_index::Index;
use image::imageops::FilterType;

/// The grid pHash downsamples to. The reference implementation works at
/// 32×32 and reads the low-frequency 8×8 block of the DCT.
const GRID: usize = 32;

/// The low-frequency block kept after the transform: 8×8 minus the DC
/// term = 64 hash bits.
const BLOCK: usize = 8;

/// Compute the pHash of `rgba` pixels (row-major, 4 bytes per pixel) at
/// the given width and height, as 16 hex characters (64 bits).
///
/// Grayscale → downsample to [`GRID`]² → separable 2-D DCT-II (the
/// estate's `dsp_spectral::dct2` applied to rows, then columns) →
/// low-frequency [`BLOCK`]² block minus DC → median threshold → 64 bits.
#[must_use]
pub fn hash_rgba(rgba: &[u8], width: usize, height: usize) -> String {
    // Grayscale + downsample to GRID², box-filter style: average every
    // pixel that lands in each grid cell.
    let mut gray = vec![0.0_f64; GRID * GRID];
    let mut counts = vec![0_u64; GRID * GRID];
    // Grid cell (gx, gy) averages every source pixel that maps into it.
    // Chunks of 4 keep the indexing inside the borrow checker's good
    // graces (the workspace denies slicing).
    for (index, pixel) in rgba.chunks_exact(4).enumerate() {
        let x = index % width;
        let y = index / width;
        let gy = core::cmp::min(y * GRID / height, GRID - 1);
        let gx = core::cmp::min(x * GRID / width, GRID - 1);
        let cell = gy * GRID + gx;
        if let [r, g, b, ..] = pixel {
            let (r, g, b) = (f64::from(*r), f64::from(*g), f64::from(*b));
            if let Some(cell) = gray.get_mut(cell) {
                *cell += 0.299 * r + 0.587 * g + 0.114 * b;
            }
            if let Some(count) = counts.get_mut(cell) {
                *count += 1;
            }
        }
    }
    for (cell, count) in gray.iter_mut().zip(counts) {
        if count > 0 {
            *cell /= count as f64;
        }
    }

    // Separable 2-D DCT: the 1-D transform along rows, then along
    // columns. 32×32 makes the naive O(N²) basis sum trivial.
    let mut coefficients: Vec<Vec<f64>> = gray.chunks(GRID).map(dsp_spectral::dct::dct2).collect();
    let transposed: Vec<Vec<f64>> = (0..GRID)
        .map(|c| {
            coefficients
                .iter()
                .filter_map(|row| row.get(c))
                .copied()
                .collect()
        })
        .collect();
    let transformed_columns: Vec<Vec<f64>> = transposed
        .iter()
        .map(|column| dsp_spectral::dct::dct2(column))
        .collect();
    for (r, row) in coefficients.iter_mut().enumerate() {
        for (c, slot) in row.iter_mut().enumerate() {
            if let Some(value) = transformed_columns.get(c).and_then(|column| column.get(r)) {
                *slot = *value;
            }
        }
    }

    // Low-frequency block: rows 0..8, columns 0..8, skipping DC — DC
    // carries average brightness, which a re-crop or a brightness slider
    // must not move.
    let mut low: Vec<f64> = Vec::with_capacity(BLOCK * BLOCK - 1);
    for (r, row) in coefficients.iter().enumerate().take(BLOCK) {
        for (c, value) in row.iter().enumerate().take(BLOCK) {
            if !(r == 0 && c == 0) {
                low.push(*value);
            }
        }
    }
    let mut sorted = low.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let median = sorted.get(sorted.len() / 2).copied().unwrap_or_default();
    to_hex(
        low.iter()
            .enumerate()
            .fold(0_u64, |acc, (i, &v)| acc | (u64::from(v > median) << i)),
    )
}

fn to_hex(value: u64) -> String {
    let mut out = String::with_capacity(16);
    let mut rest = value;
    for _ in 0..16 {
        let nibble = u32::try_from((rest >> 60) & 0xF).unwrap_or(0);
        out.push(char::from_digit(nibble, 16).unwrap_or('0'));
        rest <<= 4;
    }
    out
}

/// The stored (or freshly computed) pHash of a stored asset, as hex.
/// Assets whose thumbnails predate the feature get filled in here on
/// first request.
///
/// # Errors
///
/// [`PipelineError::NotAnImage`] when the original is not a decodable
/// still image; [`PipelineError::Cas`] / [`PipelineError::Index`] on
/// storage failure.
pub fn phash(store: &Store, index: &Index, source: &Address) -> Result<String, PipelineError> {
    let source_hex = source.hex();
    if let Some(existing) = index
        .asset(&source_hex)
        .map_err(PipelineError::Index)?
        .and_then(|record| record.phash)
    {
        return Ok(existing);
    }
    let original = store.get_bytes(source)?;
    let decoded = image::load_from_memory(&original).map_err(|_| PipelineError::NotAnImage)?;
    let small = if decoded.width().max(decoded.height()) > GRID as u32 {
        decoded.resize(GRID as u32, GRID as u32, FilterType::Triangle)
    } else {
        decoded
    };
    let rgba = small.to_rgba8();
    let hash = hash_rgba(
        rgba.as_raw(),
        usize::try_from(rgba.width()).unwrap_or(GRID),
        usize::try_from(rgba.height()).unwrap_or(GRID),
    );
    index
        .record_phash(&source_hex, &hash)
        .map_err(PipelineError::Index)?;
    Ok(hash)
}

/// Hamming distance between two equal-length hex hashes.
#[must_use]
pub fn hamming(a: &str, b: &str) -> u32 {
    let a_bits = u64::from_str_radix(a, 16).unwrap_or(0);
    let b_bits = u64::from_str_radix(b, 16).unwrap_or(0);
    (a_bits ^ b_bits).count_ones()
}

/// Whether two pHashes are near-duplicate candidates at the
/// community-standard threshold: Hamming distance ≤ 10 of 64 bits.
#[must_use]
pub fn near_duplicate(a: &str, b: &str) -> bool {
    hamming(a, b) <= 10
}
