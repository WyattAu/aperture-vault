//! SQLite-backed asset index for the Aperture vault.
//!
//! # Why SQLite, not PostgreSQL
//!
//! Immich requires a database server for an index that is, in practice,
//! queried by one user at a time: timelines, albums, per-asset lookups. An
//! embedded SQLite database in WAL mode serves that workload from the same
//! process, removes two network hops per query, and makes a backup a single
//! file copy. The index is a cache over the CAS, not the system of record —
//! the CAS's objects directory is — so the index can be rebuilt from storage
//! at any time, which is why nothing here is load-bearing for data safety.
//!
//! # The one design decision
//!
//! Assets are keyed by **content address** (BLAKE3 hex). Two ingests of the
//! same bytes cannot produce two rows, because `record_asset` is
//! `INSERT OR IGNORE` on the address primary key — the index inherits the
//! CAS's deduplication instead of re-implementing it.

use rusqlite::Connection;

/// The opened index.
pub struct Index {
    conn: Connection,
}

/// One asset record, as stored and returned.
#[derive(Debug, Clone, PartialEq)]
pub struct AssetRecord {
    /// Content address (BLAKE3 hex), the primary key.
    pub address: String,
    /// Width in pixels, when known.
    pub width: Option<u32>,
    /// Height in pixels, when known.
    pub height: Option<u32>,
    /// Capture time, `YYYY-MM-DDTHH:MM:SS` local, from EXIF when present.
    pub taken_at: Option<String>,
    /// Camera manufacturer.
    pub make: Option<String>,
    /// Camera model.
    pub model: Option<String>,
    /// GPS latitude in signed decimal degrees.
    pub latitude: Option<f64>,
    /// GPS longitude in signed decimal degrees.
    pub longitude: Option<f64>,
    /// Video duration in milliseconds.
    pub duration_ms: Option<u64>,
    /// Blob length in bytes.
    pub byte_len: u64,
    /// The original file name, for display.
    pub file_name: Option<String>,
}

/// One timeline entry: the address, its capture day, and display time.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct TimelineEntry {
    /// Content address.
    pub address: String,
    /// Capture day, `YYYY-MM-DD` — the timeline's grouping key.
    pub day: String,
    /// Capture time-of-day when known, `HH:MM:SS`.
    pub time: Option<String>,
    /// Original file name, for display.
    pub file_name: Option<String>,
}

/// What `record_asset` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recorded {
    /// The asset was new; a row was inserted.
    Inserted,
    /// The address was already present; nothing changed.
    AlreadyKnown,
}

impl Index {
    /// Open (or create) the index at `path`, applying the schema and enabling
    /// WAL mode.
    ///
    /// # Errors
    ///
    /// [`IndexError`] when the database cannot be opened or the schema cannot
    /// be applied.
    pub fn open(path: &std::path::Path) -> Result<Self, IndexError> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    /// An index backed by in-memory storage — for tests and throwaway queries.
    ///
    /// # Errors
    ///
    /// [`IndexError`] when the schema cannot be applied.
    pub fn in_memory() -> Result<Self, IndexError> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    /// Record an asset. Idempotent on the address: re-recording a known
    /// asset is [`Recorded::AlreadyKnown`], not a duplicate row.
    ///
    /// # Errors
    ///
    /// [`IndexError`] on database failure.
    pub fn record_asset(&self, r: &AssetRecord) -> Result<Recorded, IndexError> {
        let changed = self.conn.execute(
            "INSERT INTO assets
               (address, width, height, taken_at, make, model,
                latitude, longitude, duration_ms, byte_len, file_name)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(address) DO NOTHING",
            rusqlite::params![
                r.address,
                r.width,
                r.height,
                r.taken_at,
                r.make,
                r.model,
                r.latitude,
                r.longitude,
                r.duration_ms,
                r.byte_len,
                r.file_name,
            ],
        )?;
        Ok(if changed == 0 {
            Recorded::AlreadyKnown
        } else {
            Recorded::Inserted
        })
    }

    /// The timeline, newest first, paginated.
    /// Capture time falls back to a sentinel epoch when EXIF is absent, so
    /// assets without metadata still appear — ordered by *ingest recency*
    /// among themselves, via the rowid tiebreaker.
    ///
    /// # Errors
    ///
    /// [`IndexError`] on database failure.
    pub fn timeline(&self, limit: u32, offset: u32) -> Result<Vec<TimelineEntry>, IndexError> {
        let mut stmt = self.conn.prepare(
            "SELECT address,
                    COALESCE(substr(taken_at, 1, 10), '1970-01-01') AS day,
                    substr(taken_at, 12, 8) AS time,
                    file_name
             FROM assets
             ORDER BY day DESC, time DESC, address ASC
             LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![limit, offset], |row| {
            Ok(TimelineEntry {
                address: row.get(0)?,
                day: row.get(1)?,
                time: row.get(2)?,
                file_name: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(IndexError::from)
    }

    /// One asset by address, or `None`.
    ///
    /// # Errors
    ///
    /// [`IndexError`] on database failure.
    pub fn asset(&self, address: &str) -> Result<Option<AssetRecord>, IndexError> {
        let mut stmt = self.conn.prepare(
            "SELECT address, width, height, taken_at, make, model,
                    latitude, longitude, duration_ms, byte_len, file_name
             FROM assets WHERE address = ?1",
        )?;
        let mut rows = stmt.query_map(rusqlite::params![address], |row| {
            Ok(AssetRecord {
                address: row.get(0)?,
                width: row.get(1)?,
                height: row.get(2)?,
                taken_at: row.get(3)?,
                make: row.get(4)?,
                model: row.get(5)?,
                latitude: row.get(6)?,
                longitude: row.get(7)?,
                duration_ms: row.get(8)?,
                byte_len: row.get(9)?,
                file_name: row.get(10)?,
            })
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// Record a derived asset — the mapping from (source asset, spec) to the
    /// derived blob's address. Re-recording the same pair overwrites, so a
    /// regenerated thumbnail replaces its predecessor atomically.
    ///
    /// # Errors
    ///
    /// [`IndexError`] on database failure.
    pub fn record_derived(
        &self,
        source: &str,
        spec: &str,
        address: &str,
    ) -> Result<(), IndexError> {
        self.conn.execute(
            "INSERT INTO derived (source, spec, address) VALUES (?1, ?2, ?3)
             ON CONFLICT(source, spec) DO UPDATE SET address = excluded.address",
            rusqlite::params![source, spec, address],
        )?;
        Ok(())
    }

    /// The address of a derived asset, when one has been generated.
    ///
    /// # Errors
    ///
    /// [`IndexError`] on database failure.
    pub fn derived(&self, source: &str, spec: &str) -> Result<Option<String>, IndexError> {
        self.conn
            .query_row(
                "SELECT address FROM derived WHERE source = ?1 AND spec = ?2",
                rusqlite::params![source, spec],
                |row| row.get(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other.into()),
            })
    }

    /// How many assets the index knows.
    ///
    /// # Errors
    ///
    /// [`IndexError`] on database failure.
    pub fn count(&self) -> Result<u64, IndexError> {
        self.conn
            .query_row("SELECT count(*) FROM assets", [], |row| row.get(0))
            .map_err(IndexError::from)
    }
}

/// The schema, applied idempotently on open.
const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS assets (
    address    TEXT PRIMARY KEY,
    width      INTEGER,
    height     INTEGER,
    taken_at   TEXT,
    make       TEXT,
    model      TEXT,
    latitude   REAL,
    longitude  REAL,
    duration_ms INTEGER,
    byte_len   INTEGER NOT NULL,
    file_name  TEXT
);
CREATE INDEX IF NOT EXISTS assets_taken_at ON assets(taken_at DESC);

-- Derived assets: thumbnails and future transcodes. The CAS stores every
-- blob by its own content address; this table is the mapping from
-- (source asset, derived spec) to the derived blob's address, so the same
-- thumbnail is generated once and then read from storage forever.
CREATE TABLE IF NOT EXISTS derived (
    source  TEXT NOT NULL,
    spec    TEXT NOT NULL,
    address TEXT NOT NULL,
    PRIMARY KEY (source, spec)
);
";

/// Errors from the index.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum IndexError {
    /// Database failure.
    #[error("index error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}
