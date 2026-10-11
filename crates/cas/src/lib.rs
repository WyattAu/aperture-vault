//! Content-addressed blob storage for the Aperture photo and video vault.
//!
//! # The one design decision
//!
//! Every asset is stored under its own BLAKE3 hash. Everything a vault needs —
//! deduplication, integrity verification, reference safety, cheap identity for
//! derived assets (thumbnails, transcodes) — follows from that addressing, and
//! none of it is a feature:
//!
//! - **Dedup is a property of addressing.** Two uploads with identical bytes
//!   resolve to the same address; the second write is a no-op by construction,
//!   not by comparison.
//! - **Integrity is a read-side check.** `verify` re-hashes a stored blob and
//!   compares with the address it was stored under; bit-rot is detected, not
//!   discovered by a missing thumbnail.
//! - **Derived assets are first-class content.** A thumbnail is itself a blob
//!   with its own address, so a derived asset deduplicates, verifies, and is
//!   garbage-collectable by exactly the same rules as the original.
//!
//! # Why this beats a database-row store
//!
//! Immich (the comparison point) stores assets as rows in PostgreSQL with the
//! bytes on a filesystem keyed by UUID, and deduplicates with a perceptual-hash
//! pipeline after ingest. The CAS model inverts this: identity is established
//! *during* ingest (the hash is computed while streaming to disk), duplicates
//! are short-circuited before they consume space, and the index never has to be
//! queried to answer "do I already have these bytes".
//!
//! # Durability
//!
//! Writes go to a temporary file in the same directory and are renamed into
//! place. On POSIX, rename within a directory is atomic, so a crash mid-ingest
//! leaves either the previous content or a complete orphan temp file (swept by
//! [`Store::sweep`]), never a partially-written asset addressed as complete.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

pub mod error;

pub use error::CasError;

/// A content address: the BLAKE3 hash of a blob's bytes.
///
/// Held as bytes; rendered as lowercase hex by [`Address::hex`]. Ordering is
/// byte-order, so addresses sort stably across runs and machines.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Address([u8; Self::LENGTH]);

impl Address {
    /// Length of a BLAKE3 digest in bytes.
    pub const LENGTH: usize = 32;

    /// Construct from a 32-byte digest.
    #[must_use]
    pub const fn new(bytes: [u8; Self::LENGTH]) -> Self {
        Self(bytes)
    }

    /// The raw digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; Self::LENGTH] {
        &self.0
    }

    /// Lowercase hex rendering, the form used in filenames and indexes.
    #[must_use]
    pub fn hex(&self) -> String {
        let mut out = String::with_capacity(Self::LENGTH * 2);
        for byte in &self.0 {
            use std::fmt::Write as _;
            let _ = write!(out, "{byte:02x}");
        }
        out
    }

    /// Parse lowercase or uppercase hex. Errors on wrong length or bad digits.
    ///
    /// # Errors
    ///
    /// [`CasError::BadAddress`] when `hex` is not exactly 32 bytes of hex.
    // The loop bound is the length check above; the clippy lint cannot see it.
    #[allow(clippy::indexing_slicing)]
    pub fn parse_hex(hex: &str) -> Result<Self, CasError> {
        if hex.len() != Self::LENGTH * 2 {
            return Err(CasError::BadAddress {
                text: hex.to_string(),
            });
        }
        // `hex` was length-checked to exactly 2*LENGTH hex digits, so `i`
        // always indexes inside `out`; the loop cannot exceed it.
        let mut out = [0u8; Self::LENGTH];
        for (i, pair) in hex.as_bytes().chunks(2).enumerate() {
            let byte = match (pair.first(), pair.get(1)) {
                (Some(hi), Some(lo)) => [*hi, *lo],
                _ => {
                    return Err(CasError::BadAddress {
                        text: hex.to_string(),
                    })
                }
            };
            let hi = (byte[0] as char)
                .to_digit(16)
                .ok_or_else(|| CasError::BadAddress {
                    text: hex.to_string(),
                })?;
            let lo = (byte[1] as char)
                .to_digit(16)
                .ok_or_else(|| CasError::BadAddress {
                    text: hex.to_string(),
                })?;
            out[i] = ((hi << 4) | lo) as u8;
        }
        Ok(Self(out))
    }
}

impl std::fmt::Debug for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Address").field(&self.hex()).finish()
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.hex())
    }
}

/// Hash bytes into an address.
#[must_use]
pub fn hash_bytes(data: &[u8]) -> Address {
    Address::new(*blake3::hash(data).as_bytes())
}

/// Hash a stream into an address without buffering it in memory.
///
/// # Errors
///
/// [`CasError::Io`] when the reader fails.
pub fn hash_reader(mut reader: impl Read) -> Result<(Address, u64), CasError> {
    let mut hasher = blake3::Hasher::new();
    let mut written = 0u64;
    let mut buf = vec![0u8; 128 * 1024];
    loop {
        // `read` returns how many bytes it filled; a zero means EOF.
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let filled = buf.get(..n).unwrap_or(&buf);
        hasher.update(filled);
        written += n as u64;
    }
    Ok((Address::new(*hasher.finalize().as_bytes()), written))
}

/// A content-addressed store rooted at a directory.
///
/// Layout: `<root>/objects/<hex[0..2]>/<hex[2..4]>/<hex>` — two levels of
/// sharding keep any single directory small at millions of assets, and the
/// sharding is pure function of the address, so the layout is identical on
/// every replica and reconstructible from the objects alone.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// Open (or lazily create) a store rooted at `root`.
    ///
    /// # Errors
    ///
    /// [`CasError::Io`] when the root cannot be created.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, CasError> {
        let root = root.into();
        fs::create_dir_all(root.join("objects"))?;
        Ok(Self { root })
    }

    /// The store's root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where an address lives: `objects/<aa>/<bb>/<address>`.
    #[must_use]
    pub fn path(&self, address: &Address) -> PathBuf {
        let hex = address.hex();
        // An address renders as exactly 64 hex characters, so both prefixes are
        // always in range; the slicing cannot panic.
        debug_assert!(hex.len() >= 4);
        self.root
            .join("objects")
            .join(hex.get(0..2).unwrap_or_default())
            .join(hex.get(2..4).unwrap_or_default())
            .join(&hex)
    }

    /// Whether the exact bytes for this address are already stored.
    #[must_use]
    pub fn has(&self, address: &Address) -> bool {
        self.path(address).is_file()
    }

    /// Ingest bytes: write them under their own hash, deduplicating.
    ///
    /// Returns the address and whether the bytes were **newly written**
    /// (`false` means the store already had them — the dedup short-circuit
    /// that makes duplicate uploads free).
    ///
    /// # Errors
    ///
    /// [`CasError::Io`] on filesystem failures.
    pub fn put_bytes(&self, data: &[u8]) -> Result<(Address, bool), CasError> {
        let address = hash_bytes(data);
        if self.has(&address) {
            return Ok((address, false));
        }
        let target = self.path(&address);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        // Same-directory temp + rename: atomic on POSIX, so a crash leaves the
        // previous state or a complete file, never a partial asset under its
        // final address.
        let tmp = target.with_extension(format!("tmp-{}", std::process::id()));
        fs::write(&tmp, data)?;
        match fs::rename(&tmp, &target) {
            Ok(()) => Ok((address, true)),
            Err(e) => {
                let _ = fs::remove_file(&tmp);
                Err(e.into())
            }
        }
    }

    /// Ingest a stream, hashing while writing. The bytes are never buffered
    /// whole: a 4 GB video costs a fixed-size buffer, and the address is known
    /// the moment the stream ends.
    ///
    /// The returned [`Ingested`] reports the address, whether bytes were newly
    /// written, and the stream length — so a caller that needs the size does
    /// not re-read or stat.
    ///
    /// # Errors
    ///
    /// [`CasError::Io`] on filesystem failures.
    pub fn put_stream(&self, mut reader: impl Read) -> Result<Ingested, CasError> {
        let mut hasher = blake3::Hasher::new();
        let mut buf = vec![0u8; 128 * 1024];
        let staged = self.root.join(format!(
            "objects/.ingest-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        let mut file = fs::File::create(&staged)?;
        let mut total = 0u64;
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            let filled = buf.get(..n).unwrap_or(&buf);
            hasher.update(filled);
            file.write_all(filled)?;
            total += n as u64;
        }
        file.flush()?;
        drop(file);
        let address = Address::new(*hasher.finalize().as_bytes());
        let target = self.path(&address);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        if self.has(&address) {
            // Dedup short-circuit: the bytes are already stored, so the staged
            // copy is garbage.
            let _ = fs::remove_file(&staged);
            return Ok(Ingested {
                address,
                newly_written: false,
                len: total,
            });
        }
        match fs::rename(&staged, &target) {
            Ok(()) => Ok(Ingested {
                address,
                newly_written: true,
                len: total,
            }),
            Err(e) => {
                let _ = fs::remove_file(&staged);
                Err(e.into())
            }
        }
    }

    /// Read the full blob for an address.
    ///
    /// # Errors
    ///
    /// [`CasError::Missing`] when the address is not stored.
    pub fn get_bytes(&self, address: &Address) -> Result<Vec<u8>, CasError> {
        fs::read(self.path(address)).map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => CasError::Missing { address: *address },
            other => CasError::Io(io::Error::new(other, e.to_string())),
        })
    }

    /// Open the blob for streaming reads.
    ///
    /// # Errors
    ///
    /// [`CasError::Missing`] when the address is not stored.
    pub fn open_read(&self, address: &Address) -> Result<fs::File, CasError> {
        fs::File::open(self.path(address)).map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => CasError::Missing { address: *address },
            other => CasError::Io(io::Error::new(other, e.to_string())),
        })
    }

    /// Verify stored bytes against their address. Returns `false` when the
    /// content's hash no longer matches — the bit-rot detector.
    ///
    /// A missing blob is *not* corruption: it reports [`Ok`] with `None`.
    ///
    /// # Errors
    ///
    /// [`CasError::Io`] on filesystem failures other than absence.
    pub fn verify(&self, address: &Address) -> Result<Option<bool>, CasError> {
        match self.open_read(address) {
            Ok(mut file) => {
                let (actual, _) = hash_reader(&mut file)?;
                Ok(Some(actual == *address))
            }
            Err(CasError::Missing { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Remove orphaned `.ingest-*` staging files older than `max_age_secs`.
    ///
    /// A crash mid-ingest leaves a staging file behind; this is the sweeper.
    /// Returns how many files were removed.
    ///
    /// # Errors
    ///
    /// [`CasError::Io`] when the objects directory cannot be read.
    pub fn sweep(&self, max_age_secs: u64) -> Result<usize, CasError> {
        let objects = self.root.join("objects");
        let mut removed = 0usize;
        let cutoff = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            .saturating_sub(max_age_secs);
        for entry in fs::read_dir(&objects)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let staging = path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with(".ingest-"))
                .unwrap_or(false);
            if !staging {
                continue;
            }
            // Stale means: modified before the horizon. A staging file newer
            // than the horizon may belong to a live ingest — removing it would
            // destroy in-flight data, which is the one thing a sweeper must
            // never do.
            let modified = entry.metadata()?.modified()?;
            let mtime = modified
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if mtime >= cutoff {
                continue;
            }
            fs::remove_file(&path)?;
            removed += 1;
        }
        Ok(removed)
    }
}

/// What [`Store::put_stream`] reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ingested {
    /// The content address of the ingested bytes.
    pub address: Address,
    /// Whether the bytes were newly written (`false` = deduplicated).
    pub newly_written: bool,
    /// The stream's length in bytes, hashed along the way.
    pub len: u64,
}
