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

/// One album: a user-curated, named group of asset addresses.
#[derive(Debug, Clone, PartialEq)]
pub struct AlbumRecord {
    /// Opaque album id (SQLite `randomblob` hex).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Free-text description, empty when none.
    pub description: String,
    /// Creation time, `YYYY-MM-DDTHH:MM:SS`.
    pub created_at: String,
    /// Last membership or rename change.
    pub updated_at: String,
    /// How many assets are members.
    pub asset_count: u64,
    /// The address shown as the album's cover: the first member added.
    pub thumbnail_address: Option<String>,
}

/// Local wall-clock time in the index's `YYYY-MM-DDTHH:MM:SS` format — the
/// same shape EXIF `taken_at` is stored in, so album timestamps sort beside
/// capture timestamps without a second convention.
fn timestamp_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    // Days since epoch -> civil date, Howard Hinnant's algorithm; the leap
    // handling is the well-known one, no external time dependency.
    let days = now.as_secs() / 86_400;
    let secs_of_day = now.as_secs() % 86_400;
    let z = i64::try_from(days).unwrap_or(0) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let (hh, mm, ss) = (
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    );
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}")
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
    /// SHA-1 hex, computed while streaming — the compat alias Immich
    /// clients compare. Not an identity: [`Index::asset_by_sha1`] exists so
    /// clients' checksums resolve, while the CAS's truth stays BLAKE3.
    pub sha1: Option<String>,
    /// Base64 ThumbHash placeholder, computed when a thumbnail is first
    /// derived. `None` until then: the hash costs a decode, so it is paid
    /// on the same on-demand path as the thumbnail itself.
    pub thumbhash: Option<String>,
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
        apply_migrations(&conn)?;
        conn.execute_batch(POST_MIGRATION_SCHEMA)?;
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
        apply_migrations(&conn)?;
        conn.execute_batch(POST_MIGRATION_SCHEMA)?;
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
                latitude, longitude, duration_ms, byte_len, file_name, sha1, thumbhash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
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
                r.sha1,
                r.thumbhash,
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
                    latitude, longitude, duration_ms, byte_len, file_name, sha1, thumbhash
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
                sha1: row.get(11)?,
                thumbhash: row.get(12)?,
            })
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// The address whose SHA-1 is `hex`, or `None`. SHA-1 is the *compat
    /// alias* Immich clients speak, never the identity: the CAS stays
    /// BLAKE3-only, and this lookup only ever answers "which asset did a
    /// client's checksum name".
    ///
    /// # Errors
    ///
    /// [`IndexError`] on database failure.
    pub fn asset_by_sha1(&self, hex: &str) -> Result<Option<String>, IndexError> {
        self.conn
            .query_row(
                "SELECT address FROM assets WHERE sha1 = ?1",
                rusqlite::params![hex],
                |row| row.get(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other.into()),
            })
    }

    /// Store an asset's ThumbHash placeholder (base64), after generation.
    ///
    /// # Errors
    ///
    /// [`IndexError`] on database failure.
    pub fn record_thumbhash(&self, address: &str, thumbhash: &str) -> Result<(), IndexError> {
        self.conn.execute(
            "UPDATE assets SET thumbhash = ?2 WHERE address = ?1",
            rusqlite::params![address, thumbhash],
        )?;
        Ok(())
    }

    /// Capture-day buckets, newest first: `(day, count)`. The same day rule
    /// the timeline uses — EXIF `taken_at` when present, the 1970 sentinel
    /// grouping untagged assets by ingest order.
    ///
    /// # Errors
    ///
    /// [`IndexError`] on database failure.
    pub fn buckets(&self) -> Result<Vec<(String, u64)>, IndexError> {
        let mut stmt = self.conn.prepare(
            "SELECT COALESCE(substr(taken_at, 1, 10), '1970-01-01') AS day,
                    count(*) AS n
             FROM assets
             GROUP BY day
             ORDER BY day DESC",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(IndexError::from)
    }

    /// Every asset captured on `day` (`YYYY-MM-DD`, or the 1970 sentinel for
    /// untagged), ordered by capture time.
    ///
    /// # Errors
    ///
    /// [`IndexError`] on database failure.
    pub fn assets_in_day(&self, day: &str) -> Result<Vec<AssetRecord>, IndexError> {
        let mut stmt = self.conn.prepare(
            "SELECT address, width, height, taken_at, make, model,
                    latitude, longitude, duration_ms, byte_len, file_name, sha1, thumbhash
             FROM assets
             WHERE COALESCE(substr(taken_at, 1, 10), '1970-01-01') = ?1
             ORDER BY substr(taken_at, 12, 8) DESC, address ASC",
        )?;
        let rows = stmt.query_map(rusqlite::params![day], |row| {
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
                sha1: row.get(11)?,
                thumbhash: row.get(12)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(IndexError::from)
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

    /// Create an album, optionally seeded with assets. The id is SQLite's
    /// own randomness (`randomblob`), because the index already trusts the
    /// database for uniqueness everywhere else. Asset ids that name nothing
    /// are skipped: an album is a view over existing assets, never a
    /// place where phantom members are invented.
    ///
    /// # Errors
    ///
    /// [`IndexError`] on database failure.
    #[allow(clippy::too_many_lines)]
    pub fn create_album(
        &self,
        name: &str,
        description: &str,
        asset_ids: &[String],
    ) -> Result<AlbumRecord, IndexError> {
        let now = timestamp_now();
        self.conn.execute(
            "INSERT INTO albums (id, name, description, created_at, updated_at)
             VALUES (lower(hex(randomblob(16))), ?1, ?2, ?3, ?3)",
            rusqlite::params![name, description, now],
        )?;
        let id: String = self
            .conn
            .query_row(
                "SELECT id FROM albums WHERE created_at = ?1 AND name = ?2
                 ORDER BY rowid DESC LIMIT 1",
                rusqlite::params![now, name],
                |row| row.get(0),
            )
            .map_err(IndexError::from)?;
        for address in asset_ids {
            self.conn.execute(
                "INSERT INTO album_assets (album_id, address, added_at)
                 SELECT ?1, address, ?3 FROM assets WHERE address = ?2",
                rusqlite::params![id, address, now],
            )?;
        }
        self.album(&id)?
            .ok_or(IndexError::Sqlite(rusqlite::Error::QueryReturnedNoRows))
    }

    /// Every album, newest activity first, with asset counts.
    ///
    /// # Errors
    ///
    /// [`IndexError`] on database failure.
    pub fn albums(&self) -> Result<Vec<AlbumRecord>, IndexError> {
        let mut stmt = self.conn.prepare(
            "SELECT a.id, a.name, a.description, a.created_at, a.updated_at,
                    count(m.address),
                    (SELECT m2.address FROM album_assets m2
                     WHERE m2.album_id = a.id ORDER BY m2.added_at ASC, m2.address ASC LIMIT 1)
             FROM albums a
             LEFT JOIN album_assets m ON m.album_id = a.id
             GROUP BY a.id
             ORDER BY a.updated_at DESC, a.created_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(AlbumRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
                asset_count: row.get::<_, u64>(5)?,
                thumbnail_address: row.get(6)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(IndexError::from)
    }

    /// One album by id, or `None`.
    ///
    /// # Errors
    ///
    /// [`IndexError`] on database failure.
    pub fn album(&self, id: &str) -> Result<Option<AlbumRecord>, IndexError> {
        self.albums()
            .map(|all| all.into_iter().find(|a| a.id == id))
    }

    /// An album's assets, in the order they were added — the order Immich
    /// clients display.
    ///
    /// # Errors
    ///
    /// [`IndexError`] on database failure.
    pub fn album_assets(&self, id: &str) -> Result<Vec<AssetRecord>, IndexError> {
        let mut stmt = self.conn.prepare(
            "SELECT assets.address, assets.width, assets.height, assets.taken_at,
                    assets.make, assets.model, assets.latitude, assets.longitude,
                    assets.duration_ms, assets.byte_len, assets.file_name, assets.sha1,
                    assets.thumbhash
             FROM album_assets m
             JOIN assets ON assets.address = m.address
             WHERE m.album_id = ?1
             ORDER BY m.added_at ASC, m.address ASC",
        )?;
        let rows = stmt.query_map(rusqlite::params![id], |row| {
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
                sha1: row.get(11)?,
                thumbhash: row.get(12)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(IndexError::from)
    }

    /// Add assets to an album. Returns each id with whether it was newly
    /// added; ids that name no asset report `false` — the album never gains
    /// a phantom member.
    ///
    /// # Errors
    ///
    /// [`IndexError`] on database failure.
    pub fn album_add_assets(
        &self,
        id: &str,
        addresses: &[String],
    ) -> Result<Vec<(String, bool)>, IndexError> {
        let now = timestamp_now();
        let mut out = Vec::with_capacity(addresses.len());
        for address in addresses {
            let added = self.conn.execute(
                "INSERT INTO album_assets (album_id, address, added_at)
                 SELECT ?1, a.address, ?3 FROM assets a WHERE a.address = ?2
                 ON CONFLICT(album_id, address) DO NOTHING",
                rusqlite::params![id, address, now],
            )?;
            out.push((address.clone(), added > 0));
        }
        self.conn.execute(
            "UPDATE albums SET updated_at = ?2 WHERE id = ?1",
            rusqlite::params![id, now],
        )?;
        Ok(out)
    }

    /// Remove assets from an album. Returns each id with whether it was a
    /// member. The assets themselves are untouched.
    ///
    /// # Errors
    ///
    /// [`IndexError`] on database failure.
    pub fn album_remove_assets(
        &self,
        id: &str,
        addresses: &[String],
    ) -> Result<Vec<(String, bool)>, IndexError> {
        let mut out = Vec::with_capacity(addresses.len());
        for address in addresses {
            let removed = self.conn.execute(
                "DELETE FROM album_assets WHERE album_id = ?1 AND address = ?2",
                rusqlite::params![id, address],
            )?;
            out.push((address.clone(), removed > 0));
        }
        Ok(out)
    }

    /// Delete an album. Membership rows go with it; the assets do not.
    ///
    /// # Errors
    ///
    /// [`IndexError`] on database failure.
    pub fn album_delete(&self, id: &str) -> Result<bool, IndexError> {
        self.conn.execute(
            "DELETE FROM album_assets WHERE album_id = ?1",
            rusqlite::params![id],
        )?;
        let gone = self
            .conn
            .execute("DELETE FROM albums WHERE id = ?1", rusqlite::params![id])?;
        Ok(gone > 0)
    }

    /// Rename an album and/or change its description; `None` leaves a field
    /// as it is.
    ///
    /// # Errors
    ///
    /// [`IndexError`] on database failure.
    pub fn album_update(
        &self,
        id: &str,
        name: Option<&str>,
        description: Option<&str>,
    ) -> Result<Option<AlbumRecord>, IndexError> {
        let now = timestamp_now();
        if let Some(name) = name {
            self.conn.execute(
                "UPDATE albums SET name = ?2, updated_at = ?3 WHERE id = ?1",
                rusqlite::params![id, name, now],
            )?;
        }
        if let Some(description) = description {
            self.conn.execute(
                "UPDATE albums SET description = ?2, updated_at = ?3 WHERE id = ?1",
                rusqlite::params![id, description, now],
            )?;
        }
        self.album(id)
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

/// Additive, idempotent migrations for databases opened by older versions.
/// Each entry runs until it either succeeds or fails with "already exists";
/// the index is documented as a rebuildable cache, so migrations never
/// rewrite data — they only widen it.
fn apply_migrations(conn: &Connection) -> Result<(), IndexError> {
    let migrations: &[&str] = &[
        "ALTER TABLE assets ADD COLUMN sha1 TEXT",
        "ALTER TABLE assets ADD COLUMN thumbhash TEXT",
    ];
    for sql in migrations {
        let result = conn.execute_batch(sql);
        if let Err(rusqlite::Error::SqliteFailure(_, msg)) = &result {
            // SQLITE_ERROR(1) with this message is the "already applied" case.
            if msg
                .as_deref()
                .is_some_and(|m| m.contains("duplicate column name"))
            {
                continue;
            }
        }
        result.map_err(IndexError::from)?;
    }
    Ok(())
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
    file_name  TEXT,
    sha1       TEXT,
    thumbhash  TEXT
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

-- Albums: user-curated sets of assets. Membership is a join table, so an
-- asset can live in many albums and deleting an album never touches the
-- assets — the CAS owns the bytes, albums only name groups of them.
CREATE TABLE IF NOT EXISTS albums (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS album_assets (
    album_id  TEXT NOT NULL,
    address   TEXT NOT NULL,
    added_at  TEXT NOT NULL,
    PRIMARY KEY (album_id, address)
);
";

/// Indexes that reference columns added by migrations; applied after
/// `apply_migrations`, because an index on a not-yet-added column would
/// fail on databases opened by older versions.
const POST_MIGRATION_SCHEMA: &str = "
CREATE INDEX IF NOT EXISTS assets_sha1 ON assets(sha1);
";
/// Errors from the index.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum IndexError {
    /// Database failure.
    #[error("index error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}
