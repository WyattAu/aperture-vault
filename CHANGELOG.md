- **`aperture-probe`**; asset metadata from bytes: JPEG/EXIF (both TIFF
- **`aperture-index`**; SQLite/WAL asset index keyed by content address:
  `thumbhash`, `is_favorite`, `description`, `trashed_at`; all additive,
- **`aperture-auth`**; salted Argon2 verifiers (estate `salting`),
- **`aperture-pipeline`**; the ingest junction: hash-while-stream → dedup
- **`aperture-server`**; the binary and router: `add-user` and `scan`
  subcommands, and the Immich-compatible API; connect handshake
- Asset ids are content addresses, not UUIDs; clients treat ids as
  flag still states only what is true (ML, OAuth, trash as a stage -
  refused; reverse geocoding; no gazetteer shipped).
  syscall *entry* (~2 µs/op, ~6 ops/ingest); 0.0045% of the path. The
- **`aperture-cas`**; the content-addressed store: BLAKE3 addressing, streaming
  *fresh* staging files; potentially a live ingest; and kept stale ones. The