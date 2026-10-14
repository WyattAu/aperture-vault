//! The ingest pipeline: CAS + probe + index composed into one call.
//!
//! This is the junction the server needs, and the reason the three crates
//! exist separately: storage, metadata and index each stand alone, and this
//! module is the only place they touch. The pipeline is deliberately
//! sequential and boring — hash while streaming, dedup short-circuit, probe
//! what survived, index what probed — because the fast path is the one that
//! does less work, not the one that does more in parallel.

use aperture_cas::{Address, Store};
use aperture_probe::AssetMeta;
use sha1::{Digest as _, Sha1};

/// A reader that hashes SHA-1 while the bytes flow through it, so ingest
/// computes both content keys in one pass: BLAKE3 inside the CAS (identity),
/// SHA-1 in the tee (the compat alias Immich clients compare).
struct Sha1Tee<R> {
    inner: R,
    hasher: Sha1,
}

impl<R: std::io::Read> std::io::Read for Sha1Tee<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if let Some(read) = buf.get(..n) {
            self.hasher.update(read);
        }
        Ok(n)
    }
}

impl<R> Sha1Tee<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            hasher: Sha1::new(),
        }
    }

    fn finish_hex(self) -> String {
        hex_encode(&self.hasher.finalize())
    }
}

/// Lowercase hex, the form the index stores and bulk-upload-check compares.
fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(byte & 0xF), 16).unwrap_or('0'));
    }
    out
}

/// What one ingest produced.
#[derive(Debug, Clone, PartialEq)]
pub struct IngestReport {
    /// The content address of the ingested bytes.
    pub address: Address,
    /// Whether the bytes were newly written to the CAS.
    pub newly_written: bool,
    /// Whether the index learned a new asset.
    pub indexed: bool,
    /// The metadata the probe found, if the asset was newly ingested.
    pub meta: Option<AssetMeta>,
}

/// Ingest one asset: CAS write (dedup short-circuit), metadata probe, index
/// record. `file_name` is display metadata and travels with the record.
///
/// On a duplicate upload the probe and index are *skipped, not half-run*:
/// the store already holds the bytes and the index already holds the record,
/// so the report says `newly_written: false, indexed: false` and the caller
/// knows nothing else happened.
///
/// # Errors
///
/// [`PipelineError`] when the CAS write, the probe read or the index write
/// fails. The asset is not half-ingested: the CAS write is atomic, and the
/// index write happens only after a successful probe.
#[derive(Debug, Clone)]
pub struct Pipeline;

impl Pipeline {
    /// Ingest `reader`'s bytes into `store` + `index`.
    pub fn ingest(
        store: &Store,
        index: &aperture_index::Index,
        reader: impl std::io::Read,
        file_name: Option<&str>,
    ) -> Result<IngestReport, PipelineError> {
        let mut tee = Sha1Tee::new(reader);
        let ingested = store.put_stream(&mut tee).map_err(PipelineError::Cas)?;
        let sha1_hex = tee.finish_hex();
        if !ingested.newly_written {
            return Ok(IngestReport {
                address: ingested.address,
                newly_written: false,
                indexed: false,
                meta: None,
            });
        }
        // Probe the stored bytes, not the caller's reader: the bytes are now
        // in the CAS, and re-reading them proves the stored copy is the one
        // described.
        let mut file = store
            .open_read(&ingested.address)
            .map_err(PipelineError::Cas)?;
        let meta = aperture_probe::probe(&{
            use std::io::Read as _;
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).map_err(|e| {
                PipelineError::Cas(aperture_cas::CasError::Io(std::io::Error::new(
                    e.kind(),
                    e.to_string(),
                )))
            })?;
            bytes
        });
        let record = aperture_index::AssetRecord {
            address: ingested.address.hex(),
            width: meta.width,
            height: meta.height,
            taken_at: meta.taken_at.clone(),
            make: meta.make.clone(),
            model: meta.model.clone(),
            latitude: meta.latitude,
            longitude: meta.longitude,
            duration_ms: meta.duration_ms,
            byte_len: ingested.len,
            file_name: file_name.map(str::to_string),
            sha1: Some(sha1_hex),
        };
        index.record_asset(&record).map_err(PipelineError::Index)?;
        Ok(IngestReport {
            address: ingested.address,
            newly_written: true,
            indexed: true,
            meta: Some(meta),
        })
    }
}

/// Why an ingest failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PipelineError {
    /// The CAS layer failed.
    #[error("cas: {0}")]
    Cas(#[from] aperture_cas::CasError),
    /// The index write failed.
    #[error("index: {0}")]
    Index(#[from] aperture_index::IndexError),
    /// The original is not a decodable still image — a video, or corrupt
    /// bytes. A thumbnail of it does not exist.
    #[error("not a decodable still image")]
    NotAnImage,
}

pub mod thumb;
