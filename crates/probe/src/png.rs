//! PNG: dimension probe from the IHDR chunk.

use crate::AssetMeta;

/// Probe PNG bytes: signature, IHDR chunk, width and height.
pub fn probe(data: &[u8]) -> Option<AssetMeta> {
    // Signature: 89 50 4E 47 0D 0A 1A 0A.
    const SIG: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    if data.get(..8)? != SIG {
        return None;
    }
    // IHDR: length(4)=13, "IHDR", width(4), height(4).
    if data.get(12..16)? != b"IHDR" {
        return None;
    }
    let width = u32be(data, 16)?;
    let height = u32be(data, 20)?;
    Some(AssetMeta {
        width: Some(width),
        height: Some(height),
        ..AssetMeta::default()
    })
}

fn u32be(data: &[u8], at: usize) -> Option<u32> {
    let b = data.get(at..at.checked_add(4)?)?;
    Some(u32::from_be_bytes(*b.first_chunk::<4>()?))
}
