//! Thumbnail derivation: decode an original, resize it, cache the result.
//!
//! # Why synchronous, not a job queue
//!
//! A 250-pixel JPEG resize costs tens of milliseconds. A job queue earns its
//! operational weight when work is slow (video transcodes) or must never
//! block an ingest; for still thumbnails the queue would add a worker
//! process, a queue table, and retry semantics to save one request a few
//! dozen milliseconds. Generation is on-demand, and the derived blob is
//! cached in the CAS keyed by `(source, spec)` in the index, so the cost is
//! paid once per asset per size — forever, because content addresses do not
//! rot. Video poster frames are the honest future case for worker-kit.

use crate::PipelineError;
use aperture_cas::{Address, Store};
use aperture_index::Index;
use image::imageops::FilterType;

/// A requested thumbnail size. The two sizes are the ones Immich clients
/// ask for, so the vault generates exactly those and nothing speculative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbSize {
    /// Small grid thumbnail; the longest edge is bounded to 250 px.
    Thumbnail,
    /// Large preview; the longest edge is bounded to 1440 px.
    Preview,
}

impl ThumbSize {
    /// The longest-edge bound, in pixels.
    #[must_use]
    pub fn bound(self) -> u32 {
        match self {
            Self::Thumbnail => 250,
            Self::Preview => 1440,
        }
    }

    /// The index spec key this size caches under.
    #[must_use]
    pub fn spec(self) -> &'static str {
        match self {
            Self::Thumbnail => "thumb:thumbnail",
            Self::Preview => "thumb:preview",
        }
    }

    /// Parse an Immich `?size=` parameter.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "thumbnail" => Some(Self::Thumbnail),
            "preview" => Some(Self::Preview),
            _ => None,
        }
    }
}

/// One generated (or cache-hit) thumbnail.
#[derive(Debug, Clone, PartialEq)]
pub struct Thumbnail {
    /// The content address of the derived bytes.
    pub address: Address,
    /// Whether this came from the cache rather than a fresh decode.
    pub cached: bool,
    /// The encoded JPEG bytes.
    pub bytes: Vec<u8>,
}

/// Generate — or fetch from the cache — a thumbnail of a stored asset.
///
/// The decode happens on the caller's thread; a server wraps this in
/// `spawn_blocking`, a batch tool calls it directly.
///
/// # Errors
///
/// [`PipelineError::NotAnImage`] when the original is not a decodable still
/// image (videos, corrupt bytes) — the caller answers with a "no thumbnail"
/// response, which is true. [`PipelineError::Cas`] and
/// [`PipelineError::Index`] on storage failure.
pub fn thumbnail(
    store: &Store,
    index: &Index,
    source: &Address,
    size: ThumbSize,
) -> Result<Thumbnail, PipelineError> {
    let source_hex = source.hex();
    if let Some(address) = index
        .derived(&source_hex, size.spec())
        .map_err(PipelineError::Index)?
    {
        let address = Address::parse_hex(&address).map_err(PipelineError::Cas)?;
        let bytes = store.get_bytes(&address)?;
        return Ok(Thumbnail {
            address,
            cached: true,
            bytes,
        });
    }

    let original = store.get_bytes(source)?;
    let decoded = image::load_from_memory(&original).map_err(|_| PipelineError::NotAnImage)?;
    // Bound the longest edge, preserving aspect ratio — `resize` already
    // keeps the aspect, the guard declines to "enlarge" small originals.
    let bounded = if decoded.width().max(decoded.height()) > size.bound() {
        decoded.resize(size.bound(), size.bound(), FilterType::Triangle)
    } else {
        decoded
    };
    let mut encoded = Vec::new();
    bounded
        .write_to(
            &mut std::io::Cursor::new(&mut encoded),
            image::ImageFormat::Jpeg,
        )
        .map_err(|_| PipelineError::NotAnImage)?;
    let (address, _new) = store.put_bytes(&encoded)?;
    index
        .record_derived(&source_hex, size.spec(), &address.hex())
        .map_err(PipelineError::Index)?;
    Ok(Thumbnail {
        address,
        cached: false,
        bytes: encoded,
    })
}
