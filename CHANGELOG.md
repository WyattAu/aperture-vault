# Changelog

All notable changes to this project are documented in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **`aperture-probe`** — asset metadata from bytes: JPEG/EXIF (both TIFF
  endiannesses, GPS DMS→decimal), PNG IHDR dimensions, MP4 duration and
  dimensions via the estate's `mp4-core`. Total parsing: corrupt bytes
  yield an empty record, never a failed listing.
- **`aperture-index`** — SQLite/WAL asset index keyed by content address:
  timeline newest-first by capture day, sentinel-day grouping for untagged
  assets, stable pagination. Columns since: `sha1` (the compat checksum),
  `thumbhash`, `is_favorite`, `description`, `trashed_at` — all additive,
  all migrated in place (tested against a real M2-era database), plus
  derived-asset, album, album-membership, and share-link tables.
- **`aperture-auth`** — salted Argon2 verifiers (estate `salting`),
  session JWTs, share-link capability tokens (estate `tokenkit`),
  account-enumeration defence (unknown user and wrong password burn the
  same hash and return the same error), minimum-length password policy.
- **`aperture-pipeline`** — the ingest junction: hash-while-stream → dedup
  short-circuit → probe what survived → index what probed. SHA-1 computed
  in the same streaming pass as BLAKE3. Thumbnail derivation (one decode,
  two artifacts: the sized JPEG and the ThumbHash placeholder) and
  directory scan (checksum skip-check, two streaming passes, per-file
  failure isolation).
- **`aperture-server`** — the binary and router: `add-user` and `scan`
  subcommands, and the Immich-compatible API — connect handshake
  (`server/ping|features|config|version`, feature flags stating only what
  is true), login (`username` aliased), session validation, user
  directory, checksum dedup (`assets/bulk-upload-check`, base64 or hex),
  multipart and raw upload (`{id, status}`), originals, thumbnails
  (`?size=thumbnail|preview`), columnar timeline buckets (album-scoped
  too), albums (CRUD + bulk membership), share links (create, list,
  revoke, public `me?key=` access), asset detail, favorite and
  description updates, statistics, media types, memories ("on this
  day"), trash (delete, restore, empty), search (name/kind/favorite),
  map markers, and download archive (STORE-method ZIP, 2 GiB budget).
  Serves on Immich's default port 2283.

### Changed

- Asset ids are content addresses, not UUIDs — clients treat ids as
  opaque strings, and an id that lies about identity would be a defect.
- One upload URL serves both client encodings: a dispatcher branches on
  Content-Type between Immich's multipart `assetData` and the native raw
  bytes + `?filename=`.
- `map` feature flag is true now that map markers exist; every other
  flag still states only what is true (ML, OAuth, trash as a stage —
  refused; reverse geocoding — no gazetteer shipped).

### Refuted

- uring-kit under the CAS: measured, not assumed. A 100 MiB ingest spends
  ~100 ms reading, ~50 ms hashing, ~110 ms writing; io_uring removes
  syscall *entry* (~2 µs/op, ~6 ops/ingest) — 0.0045% of the path. The
  cost is the page-cache copies, and the CAS must copy because BLAKE3
  needs the bytes in userspace. The measurement lives at
  `crates/pipeline/examples/ingest_profile.rs`.

## [0.1.0] - 2026-10-11

### Added

- **`aperture-cas`** — the content-addressed store: BLAKE3 addressing, streaming
  ingest (bounded memory, hashed while written), atomic same-directory rename,
  deduplication by construction, corruption detection (`verify`), sharded object
  layout, and an age-horizon sweeper for orphaned staging files.

### Verified

- Streaming ingest and byte ingest address the same bytes; the second ingest of
  identical bytes is a dedup hit, not a rewrite.
- A flipped byte on disk is reported by `verify` as corruption.
- The sweeper respects its age horizon (fresh staging files are never swept) and
  never touches real content.

### Fixed during development

- The sweeper's staleness comparison was inverted in the first draft: it removed
  *fresh* staging files — potentially a live ingest — and kept stale ones. The
  sweeper test caught it before any release; recorded because the sweeper is the
  one component licensed to delete.
