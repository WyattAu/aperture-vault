# Changelog

All notable changes to this project are documented in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
